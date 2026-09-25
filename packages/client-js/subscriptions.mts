/**
 * Subscription handles: the identity a registration keeps, the status it
 * publishes and the observers watching it
 * ([#150](https://github.com/zanminwang/axton/issues/150)). A handle owns none
 * of the synchronization: Rust decides what is delivered and when, and this
 * file only projects what it committed and what its lane is doing onto one
 * immutable snapshot per subscription.
 */

/** One stored subscription, as the native Scope commands answer it. A boundary that is not committed yet is `null`; zero is a delivery position. */
export type SubscriptionState = {
  scope: string;
  subscriptionId: number;
  startingCursor: number | null;
  cursor: number | null;
};
export type SubscriptionStatus = Readonly<{
  /** Whether this handle still names a live registration. */
  active: boolean;
  /** `ready` once a durable starting boundary exists; not a statement about the connection. */
  initialization: "pending" | "ready";
  /** `live` means the session delivers normally, not that all history is loaded. */
  connection: "offline" | "connecting" | "catching-up" | "live" | "stopped";
}>;
export interface Subscription {
  readonly scope: string;
  readonly status: SubscriptionStatus;
  /** Deliver the current snapshot at once, then every change, until the returned function is called. */
  watch(listener: (status: SubscriptionStatus) => void): () => void;
  unsubscribe(): Promise<void>;
}
/** Work attempted through a handle that is closed: unsubscribed, or stopped with its client. */
export const subscriptionClosed = () =>
  Object.assign(Error("subscription.closed"), {
    code: "subscription.closed" as const,
  });

/**
 * What the downlink lane tells the registry. It is transport state the lane
 * already has, not a second sync state machine: which session is open, whether
 * its handshake covered a Scope, how many catch-up requests are out, and which
 * Scopes a commit moved.
 */
export type DownlinkSignal =
  /** The socket session of this epoch opened, or ended; an epoch that is not the current one is ignored. */
  | { lane: "opened" | "ended"; epoch: number }
  | { lane: "paused" | "resumed" | "stopped" }
  | { lane: "requests"; outstanding: number }
  | { lane: "acknowledged" | "changed"; scopes: string[] };

/** The native commands and host services the registry needs; the runtime owns the serialized command path. */
export type SubscriptionCommands = {
  subscribe(scope: string): Promise<SubscriptionState>;
  state(scope: string): Promise<SubscriptionState | null>;
  /** Remove exactly the registration this identity names; `true` when a row went. */
  remove(scope: string, subscriptionId: number): Promise<boolean>;
  /** Remove whatever registration a Scope name has, in one command, so calls for one Scope keep their order. */
  removeScope(scope: string): Promise<void>;
  /** A committed membership change: wake the lanes, as every commit does. */
  committed(): void;
  /** Report an observer's exception the way the host reports an uncaught one. */
  report(error: unknown): void;
};

/** The lane state every handle's `connection` is projected from. */
type Lane = {
  attached: boolean;
  paused: boolean;
  /** The epoch of the open socket session, if one is open. */
  session: number | undefined;
  outstanding: number;
  /**
   * The Scopes the open session's handshake covered. A removal drops its Scope,
   * because the acknowledgement belonged to the registration that went: a
   * registration created after it has never been acknowledged and is
   * `connecting` until a session subscribes it.
   */
  acknowledged: Set<string>;
};

class Handle implements Subscription {
  readonly scope: string;
  readonly subscriptionId: number;
  #lane: Lane;
  #initialization: "pending" | "ready";
  /** This handle committed its own removal: further removals are a no-op. */
  #removed = false;
  /** The handle was stopped with its client: its status is readable, its work is not. */
  #stopped = false;
  #listeners = new Set<(status: SubscriptionStatus) => void>();
  #report: (error: unknown) => void;
  #remove: () => Promise<void>;
  #snapshot: SubscriptionStatus;
  constructor(
    state: SubscriptionState,
    lane: Lane,
    report: (error: unknown) => void,
    remove: () => Promise<void>,
  ) {
    this.scope = state.scope;
    this.subscriptionId = state.subscriptionId;
    this.#lane = lane;
    this.#initialization = state.startingCursor === null ? "pending" : "ready";
    this.#report = report;
    this.#remove = remove;
    this.#snapshot = this.#project();
  }
  get status(): SubscriptionStatus {
    return this.#snapshot;
  }
  get closed(): boolean {
    return this.#removed || this.#stopped;
  }
  /** The one place a status comes from: what is committed for this subscription and what its lane is doing. */
  #project(): SubscriptionStatus {
    const lane = this.#lane;
    const connection = this.closed
      ? "stopped"
      : !lane.attached || lane.paused
        ? "offline"
        : lane.session === undefined
          ? "connecting"
          : lane.outstanding > 0
            ? "catching-up"
            : lane.acknowledged.has(this.scope)
              ? "live"
              : "connecting";
    return Object.freeze({
      active: !this.closed,
      initialization: this.#initialization,
      connection,
    });
  }
  /** Publish a new snapshot when anything changed; an observer's exception never reaches the caller. */
  refresh(): void {
    const next = this.#project();
    const previous = this.#snapshot;
    if (
      next.active === previous.active &&
      next.initialization === previous.initialization &&
      next.connection === previous.connection
    )
      return;
    this.#snapshot = next;
    this.#deliver(next);
    // A closed handle has no changes left after that last snapshot.
    if (this.closed) this.#listeners.clear();
  }
  #deliver(status: SubscriptionStatus): void {
    for (const listener of [...this.#listeners])
      try {
        listener(status);
      } catch (error) {
        this.#report(error);
      }
  }
  /** The committed state of this identity; another identity's state is not this handle's, and a closed handle takes none. */
  apply(state: SubscriptionState | null): void {
    if (this.closed) return;
    if (!state || state.subscriptionId !== this.subscriptionId) return;
    this.#initialization = state.startingCursor === null ? "pending" : "ready";
    this.refresh();
  }
  /** Removed durably through this handle, or stopped with the client. */
  close(reason: "removed" | "stopped"): void {
    if (this.closed) return;
    if (reason === "removed") this.#removed = true;
    else this.#stopped = true;
    this.refresh();
    this.#listeners.clear();
  }
  watch(listener: (status: SubscriptionStatus) => void): () => void {
    // A closed handle has no changes left: it delivers its stopped snapshot and
    // is done.
    if (this.closed) {
      this.#deliverOne(listener, this.#snapshot);
      return () => {};
    }
    this.#listeners.add(listener);
    this.#deliverOne(listener, this.#snapshot);
    return () => {
      this.#listeners.delete(listener);
    };
  }
  #deliverOne(
    listener: (status: SubscriptionStatus) => void,
    status: SubscriptionStatus,
  ): void {
    try {
      listener(status);
    } catch (error) {
      this.#report(error);
    }
  }
  async unsubscribe(): Promise<void> {
    if (this.#removed) return;
    if (this.#stopped) throw subscriptionClosed();
    await this.#remove();
  }
}

/**
 * The registry: one handle per persistent subscription identity, the lane
 * projection they share, and the committed status they publish.
 */
export class Subscriptions {
  #commands: SubscriptionCommands;
  #handles = new Map<number, Handle>();
  /** The client closed: a read still in flight answers to nobody. */
  #closed = false;
  #lane: Lane = {
    attached: false,
    paused: false,
    session: undefined,
    outstanding: 0,
    acknowledged: new Set(),
  };
  constructor(commands: SubscriptionCommands) {
    this.#commands = commands;
  }
  /**
   * Register durable intent and answer with the handle of the identity that
   * commit belongs to. Concurrent calls run through the same serialized command
   * path, read the same identity and share one cached handle.
   */
  async subscribe(scope: string): Promise<Subscription> {
    const state = await this.#commands.subscribe(scope);
    const existing = this.#handles.get(state.subscriptionId);
    if (existing) {
      existing.apply(state);
      this.#commands.committed();
      return existing;
    }
    const handle = new Handle(
      state,
      this.#lane,
      (error) => this.#commands.report(error),
      () => this.#removeIdentity(state.scope, state.subscriptionId),
    );
    this.#handles.set(state.subscriptionId, handle);
    // The lane learns of committed membership from Rust; it is only woken here.
    this.#commands.committed();
    return handle;
  }
  /**
   * Remove whatever registration a Scope name has - the Scope-named form the
   * generated `channels` facade keeps - and close the handle it had. One
   * command, so a removal and a registration of the same Scope commit in the
   * order they were called in.
   */
  async unsubscribeScope(scope: string): Promise<void> {
    await this.#commands.removeScope(scope);
    for (const handle of [...this.#handles.values()])
      if (handle.scope === scope) {
        this.#handles.delete(handle.subscriptionId);
        handle.close("removed");
      }
    this.#forget(scope);
    this.#commands.committed();
  }
  async #removeIdentity(scope: string, subscriptionId: number): Promise<void> {
    const removed = await this.#commands.remove(scope, subscriptionId);
    const handle = this.#handles.get(subscriptionId);
    if (handle) {
      this.#handles.delete(subscriptionId);
      handle.close("removed");
    }
    this.#forget(scope);
    if (removed) this.#commands.committed();
  }
  /**
   * The open session's handshake covered the registration that just went, not
   * the one a later subscribe creates: forget the Scope, so a recreated
   * subscription is `connecting` until a session of its own acknowledges it.
   */
  #forget(scope: string): void {
    this.#lane.acknowledged.delete(scope);
    this.#publish();
  }
  /** The lane is running for this client: until then, and once it closes, every subscription is offline. */
  attach(): void {
    this.#lane.attached = true;
    this.#lane.paused = false;
    this.#endSession();
  }
  detach(): void {
    this.#lane.attached = false;
    this.#endSession();
  }
  signal(signal: DownlinkSignal): void {
    switch (signal.lane) {
      case "opened":
        this.#lane.session = signal.epoch;
        this.#lane.outstanding = 0;
        this.#lane.acknowledged.clear();
        break;
      case "ended":
        // Whatever an abandoned session still reports belongs to no lane state.
        if (this.#lane.session !== signal.epoch) return;
        this.#endSession();
        return;
      case "paused":
        this.#lane.paused = true;
        this.#endSession();
        return;
      case "resumed":
        this.#lane.paused = false;
        break;
      case "stopped":
        this.#lane.attached = false;
        this.#endSession();
        return;
      case "requests":
        this.#lane.outstanding = signal.outstanding;
        break;
      case "acknowledged":
        for (const scope of signal.scopes) this.#lane.acknowledged.add(scope);
        break;
      case "changed":
        // A commit moved these Scopes: re-read what it committed for them.
        for (const scope of signal.scopes) this.#reload(scope);
        break;
    }
    this.#publish();
  }
  #endSession(): void {
    this.#lane.session = undefined;
    this.#lane.outstanding = 0;
    this.#lane.acknowledged.clear();
    this.#publish();
  }
  #publish(): void {
    for (const handle of [...this.#handles.values()]) handle.refresh();
  }
  /** Read the committed state of a Scope and hand it to the handle it belongs to. */
  #reload(scope: string): void {
    const handles = [...this.#handles.values()].filter(
      (handle) => handle.scope === scope && !handle.closed,
    );
    if (handles.length === 0) return;
    this.#commands.state(scope).then(
      (state) => {
        for (const handle of handles) handle.apply(state);
      },
      (error) => {
        if (!this.#closed) this.#commands.report(error);
      },
    );
  }
  /** The client closed: every handle stops and its observers are cancelled; no subscription is removed. */
  close(): void {
    this.#closed = true;
    const handles = [...this.#handles.values()];
    this.#handles.clear();
    this.#lane.attached = false;
    this.#endSession();
    for (const handle of handles) handle.close("stopped");
  }
}
