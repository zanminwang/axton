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
/**
 * What the durable load of this registration's published history is doing
 * ([#151](https://github.com/zanminwang/axton/issues/151)).
 * `waiting-for-initialization` is a requested run with no starting boundary to
 * bound its interval yet, `catching-up` a loaded interval whose completion
 * barrier ordinary delivery has not reached, and `complete` says the initial
 * publication coverage was processed - not that a snapshot was taken, nor that
 * the Scope is currently fresh.
 */
export type BootstrapPhase =
  | "not-requested"
  | "waiting-for-initialization"
  | "loading"
  | "catching-up"
  | "complete"
  | "failed";
export type BootstrapStatus = Readonly<{
  phase: BootstrapPhase;
  error: null | Readonly<{ code: string; message: string }>;
}>;
export type SubscriptionStatus = Readonly<{
  /** Whether this handle still names a live registration. */
  active: boolean;
  /** `ready` once a durable starting boundary exists; not a statement about the connection. */
  initialization: "pending" | "ready";
  /** `live` means the session delivers normally, not that all history is loaded. */
  connection: "offline" | "connecting" | "catching-up" | "live" | "stopped";
  /** The durable load of this Scope's published history, as it was last committed. */
  bootstrap: BootstrapStatus;
}>;
export interface Subscription {
  readonly scope: string;
  readonly status: SubscriptionStatus;
  /** Deliver the current snapshot at once, then every change, until the returned function is called. */
  watch(listener: (status: SubscriptionStatus) => void): () => void;
  /**
   * Prepare this Scope's published history. The registration is submitted when
   * the call is made, whether or not the returned Promise is awaited; the
   * Promise resolves only after the completion transaction commits. Calls
   * during one active run share it, a call after a valid completion resolves
   * locally - offline too - and a call after a terminal failure explicitly
   * retries the saved run.
   */
  bootstrap(): Promise<void>;
  unsubscribe(): Promise<void>;
}
/** Work attempted through a handle that is closed: unsubscribed, or stopped with its client. */
export const subscriptionClosed = () =>
  Object.assign(Error("subscription.closed"), {
    code: "subscription.closed" as const,
  });
/** This process stopped waiting because its client closed; the durable task is untouched. */
const clientClosed = () =>
  Object.assign(Error("client_closed"), { code: "client_closed" as const });
/** The stored failure of a run, as the waiters of that run are rejected with it. */
const bootstrapFailed = (error: { code: string; message: string }) =>
  Object.assign(Error(error.message), { code: error.code });

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
  | { lane: "acknowledged" | "changed"; scopes: string[] }
  /** A bootstrap run of this registration changed, and the change is committed. */
  | { lane: "bootstrap"; run: BootstrapRun };

/**
 * One registration's durable load, as the worker announces it after every
 * committed transition ([#151](https://github.com/zanminwang/axton/issues/151)).
 * `cursor` is how far the historical interval has been loaded and `barrier` the
 * delivery position completion waits for, fixed by the final historical page.
 */
export type BootstrapRun = {
  scope: string;
  subscriptionId: number;
  state:
    | "not_requested"
    | "requested"
    | "loading"
    | "catching_up"
    | "complete"
    | "failed";
  run: number;
  cursor: number;
  barrier: number | null;
  error: null | {
    code: string;
    message: string;
    records: {
      model: string;
      identity: Record<string, unknown>;
      stamp: number;
      code: string;
    }[];
  };
};

/** How far a run has got, and the public phase that stored state projects to. */
const PHASES: Record<
  BootstrapRun["state"],
  { rank: number; phase: BootstrapPhase }
> = {
  not_requested: { rank: 0, phase: "not-requested" },
  // The worker writes `loading` on the first applied page, so a requested run
  // whose interval is already bounded is loading as far as the caller is
  // concerned; one without a starting boundary is waiting for #150.
  requested: { rank: 1, phase: "loading" },
  loading: { rank: 2, phase: "loading" },
  catching_up: { rank: 3, phase: "catching-up" },
  complete: { rank: 4, phase: "complete" },
  failed: { rank: 4, phase: "failed" },
};

/** The native commands and host services the registry needs; the runtime owns the serialized command path. */
export type SubscriptionCommands = {
  subscribe(scope: string): Promise<SubscriptionState>;
  state(scope: string): Promise<SubscriptionState | null>;
  /** Register or explicitly retry the durable load of this identity, and answer its stored run. */
  requestBootstrap(
    scope: string,
    subscriptionId: number,
  ): Promise<BootstrapRun>;
  /** The stored run of this identity, read through the same serialized path. */
  bootstrapState(scope: string, subscriptionId: number): Promise<BootstrapRun>;
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

/** The load commands of one identity, bound by the registry, and the wake a commit owes the lanes. */
type BootstrapLoad = {
  /** Register the run, or explicitly retry a failed one; answers what is stored. */
  request(): Promise<BootstrapRun>;
  read(): Promise<BootstrapRun>;
  committed(): void;
};
/** One caller of `bootstrap()`, attached to the run the command answered with. */
type Waiter = {
  run: number;
  resolve: () => void;
  reject: (error: unknown) => void;
};
const NOT_REQUESTED: BootstrapStatus = Object.freeze({
  phase: "not-requested" as const,
  error: null,
});

class Handle implements Subscription {
  readonly scope: string;
  readonly subscriptionId: number;
  #lane: Lane;
  #initialization: "pending" | "ready";
  #load: BootstrapLoad;
  /** The last committed run this handle has seen; phases never move backwards. */
  #run: BootstrapRun | undefined;
  #waiters: Waiter[] = [];
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
    load: BootstrapLoad,
  ) {
    this.scope = state.scope;
    this.subscriptionId = state.subscriptionId;
    this.#lane = lane;
    this.#initialization = state.startingCursor === null ? "pending" : "ready";
    this.#report = report;
    this.#remove = remove;
    this.#load = load;
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
      bootstrap: this.#bootstrap(),
    });
  }
  /**
   * What the stored run projects to. A `requested` run whose interval has no
   * origin to bound it is waiting for #150 initialization rather than loading,
   * and the stored record reports are not part of the public failure.
   */
  #bootstrap(): BootstrapStatus {
    const run = this.#run;
    if (!run) return NOT_REQUESTED;
    return Object.freeze({
      phase:
        run.state === "requested" && this.#initialization === "pending"
          ? ("waiting-for-initialization" as const)
          : PHASES[run.state].phase,
      error: run.error
        ? Object.freeze({ code: run.error.code, message: run.error.message })
        : null,
    });
  }
  /** Publish a new snapshot when anything changed; an observer's exception never reaches the caller. */
  refresh(): void {
    const next = this.#project();
    const previous = this.#snapshot;
    if (
      next.active === previous.active &&
      next.initialization === previous.initialization &&
      next.connection === previous.connection &&
      next.bootstrap.phase === previous.bootstrap.phase &&
      next.bootstrap.error?.code === previous.bootstrap.error?.code &&
      next.bootstrap.error?.message === previous.bootstrap.error?.message
    )
      return;
    this.#snapshot = next;
    this.#deliver(next);
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
  /**
   * Read what is committed for this identity's load. A handle taken after a
   * restart names a task that may already be running or finished, and no
   * further transition has to commit for its status to be true.
   */
  observe(): void {
    this.#load.read().then(
      (run) => this.applyBootstrap(run),
      (error) => {
        if (!this.closed) this.#report(error);
      },
    );
  }
  /**
   * One committed transition of this identity's load. The waiters of that run
   * are settled by it whatever the status already shows, so a re-read that
   * raced ahead of a lane signal cannot swallow an earlier run's outcome; the
   * status itself never moves backwards.
   */
  applyBootstrap(run: BootstrapRun): void {
    if (this.closed) return;
    if (run.subscriptionId !== this.subscriptionId) return;
    if (run.state === "complete") this.#settle(run.run, undefined);
    else if (run.state === "failed")
      this.#settle(
        run.run,
        bootstrapFailed(
          // A failed run always carries its stored failure; the ledger refuses
          // any other pairing.
          run.error ?? {
            code: "bootstrap.failed",
            message: `the bootstrap of ${this.scope} failed`,
          },
        ),
      );
    const known = this.#run;
    if (
      known &&
      (run.run < known.run ||
        (run.run === known.run &&
          PHASES[run.state].rank < PHASES[known.state].rank))
    )
      return;
    this.#run = run;
    this.refresh();
  }
  #settle(run: number, error: unknown): void {
    const settled = this.#waiters.filter((waiter) => waiter.run === run);
    if (settled.length === 0) return;
    this.#waiters = this.#waiters.filter((waiter) => waiter.run !== run);
    for (const waiter of settled)
      if (error === undefined) waiter.resolve();
      else waiter.reject(error);
  }
  bootstrap(): Promise<void> {
    if (this.closed) return Promise.reject(subscriptionClosed());
    // Eager: the registration is submitted when the call is made, not when the
    // returned Promise is awaited.
    return this.#register(this.#load.request());
  }
  async #register(submitted: Promise<BootstrapRun>): Promise<void> {
    let run: BootstrapRun;
    try {
      run = await submitted;
    } catch (error) {
      throw this.closed ? this.#closedError() : error;
    }
    // The commit wakes the lanes the way a membership change does; without it
    // the registered run waits for the next commit or reconnection.
    this.#load.committed();
    if (this.closed) throw this.#closedError();
    const waiting = new Promise<void>((resolve, reject) => {
      this.#waiters.push({ run: run.run, resolve, reject });
    });
    this.applyBootstrap(run);
    // A transition that committed between the command and this waiter would
    // otherwise be missed: re-read the stored run through the same serialized
    // path and apply it.
    if (this.#waiters.length > 0) this.observe();
    return waiting;
  }
  #closedError(): Error {
    return this.#stopped ? clientClosed() : subscriptionClosed();
  }
  /** Removed durably through this handle, or stopped with the client. */
  close(reason: "removed" | "stopped"): void {
    if (this.closed) return;
    if (reason === "removed") this.#removed = true;
    else this.#stopped = true;
    // A removal took the epoch's load state with the row; a client close leaves
    // the durable task exactly where it was. Either way this process stops
    // waiting for it.
    const waiting = this.#waiters;
    this.#waiters = [];
    for (const waiter of waiting)
      waiter.reject(
        reason === "removed" ? subscriptionClosed() : clientClosed(),
      );
    this.refresh();
    // A closed handle has no changes left after that last snapshot.
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
      {
        request: () =>
          this.#commands.requestBootstrap(state.scope, state.subscriptionId),
        read: () =>
          this.#commands.bootstrapState(state.scope, state.subscriptionId),
        committed: () => this.#commands.committed(),
      },
    );
    this.#handles.set(state.subscriptionId, handle);
    // A task of this identity may already be running from before this handle:
    // read what is committed for it, so its status needs no new transition.
    handle.observe();
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
    // Nothing went: another registration is this Scope's current one, and the
    // acknowledgement it may hold is not this handle's to forget.
    if (removed) {
      this.#forget(scope);
      this.#commands.committed();
    }
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
  /**
   * The replica was replaced: the ledger this client reads now holds fresh
   * identities, so every handle names a registration of the file that was left
   * behind. They close the way an unsubscribed handle does - `watch` completes
   * and a later `unsubscribe` is a harmless no-op - and the lane forgets its
   * acknowledgements, because none of them belong to an identity that still
   * exists.
   */
  rebuilt(): void {
    const handles = [...this.#handles.values()];
    this.#handles.clear();
    this.#lane.acknowledged.clear();
    for (const handle of handles) handle.close("removed");
  }
  /** The lane is running for this client: until then, and once it closes, every subscription is offline. */
  attach(): void {
    this.#lane.attached = true;
    this.#lane.paused = false;
    this.#publish();
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
      case "bootstrap":
        // A committed load transition. Another epoch's run belongs to no handle
        // this registry still holds.
        this.#handles
          .get(signal.run.subscriptionId)
          ?.applyBootstrap(signal.run);
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
