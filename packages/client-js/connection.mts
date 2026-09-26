import type { BootstrapRun, DownlinkSignal } from "./subscriptions.mts";
export type Transport = (
  kind: string,
  body: string,
  signal?: AbortSignal,
) => Promise<string>;
export type ConnectionOptions = {
  onError?: (error: unknown) => void;
  refreshAuth?: () => Promise<void>;
  /** Maximum duration of one direct Action attempt, including token acquisition and authentication refresh. Integer 1..2147483647 ms; default 30000 ms. */
  directTimeoutMs?: number;
};
export type Connection = {
  pause(): Promise<void>;
  resume(): Promise<void>;
  wake(): Promise<void>;
  close(): Promise<void>;
};
export type DirectConnection = Connection & {
  requestAction(body: string): Promise<string>;
};
export const directFailure = (code: string) =>
  Object.assign(Error(code), { code, execution: "unknown" as const });
/** Host timers/network only. Rust decides when work/retries are eligible. */
export async function startConnection(
  control: (event: string) => Promise<any>,
  sync: (transport: Transport) => Promise<void>,
  transport: Transport,
  options: ConnectionOptions = {},
): Promise<DirectConnection> {
  const directTimeoutMs = options.directTimeoutMs ?? 30_000;
  if (
    !Number.isSafeInteger(directTimeoutMs) ||
    directTimeoutMs <= 0 ||
    directTimeoutMs > 2_147_483_647
  )
    throw Error("directTimeoutMs must be an integer from 1 to 2147483647");
  let epoch = 0;
  let stopped = false;
  let paused = false;
  let active: Promise<void> | undefined;
  let awaken: (() => void) | undefined;
  let abort = new AbortController();
  const directAttempts = new Set<AbortController>();
  const stopDirect = () => {
    for (const attempt of directAttempts) attempt.abort();
    directAttempts.clear();
  };
  const notify = () => {
    epoch++;
    awaken?.();
    awaken = undefined;
  };
  const wait = (millis?: number) =>
    new Promise<void>((resolve) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      awaken = () => {
        if (timer !== undefined) clearTimeout(timer);
        resolve();
      };
      if (millis !== undefined)
        timer = setTimeout(() => {
          awaken = undefined;
          resolve();
        }, millis);
    });
  const request: Transport = async (kind, body) => {
    if (stopped || paused || abort.signal.aborted)
      throw Error("connection_paused_or_closed");
    const signal = abort.signal;
    return new Promise<string>((resolve, reject) => {
      const cancel = () => reject(Error("connection_closed"));
      signal.addEventListener("abort", cancel, { once: true });
      Promise.resolve()
        .then(() => transport(kind, body, signal))
        .then(resolve, reject)
        .finally(() => signal.removeEventListener("abort", cancel));
    });
  };
  await control("start");
  const loop = async () => {
    while (!stopped) {
      const observed = epoch;
      const action = await control("next");
      if (stopped) return;
      if (action.type === "sync") {
        try {
          abort = new AbortController();
          active = sync(request);
          await active;
          if (!stopped) await control("success");
        } catch (error) {
          if (stopped) return;
          if (paused) {
            await control("success");
            continue;
          }
          options.onError?.(error);
          if (
            (error as { status?: number })?.status === 401 &&
            options.refreshAuth
          ) {
            try {
              await options.refreshAuth();
            } catch (refreshError) {
              options.onError?.(refreshError);
            }
          }
          if (!stopped) await control("failure");
        } finally {
          active = undefined;
        }
      } else {
        if (observed !== epoch) continue;
        await wait(action.type === "wait" ? action.millis : undefined);
      }
    }
  };
  void loop().catch((error) => {
    if (!stopped) options.onError?.(error);
  });
  return {
    async requestAction(body) {
      if (stopped) throw directFailure("action.unavailable");
      const attempt = new AbortController();
      directAttempts.add(attempt);
      let timer: ReturnType<typeof setTimeout> | undefined;
      try {
        const terminated = new Promise<never>((_, reject) => {
          const cancel = () =>
            reject(
              directFailure(
                stopped ? "action.unavailable" : "action.execution_unknown",
              ),
            );
          attempt.signal.addEventListener("abort", cancel, { once: true });
          timer = setTimeout(() => attempt.abort(), directTimeoutMs);
        });
        const send = async () => {
          try {
            return await transport("action", body, attempt.signal);
          } catch (error) {
            if (
              (error as { status?: number })?.status !== 401 ||
              !options.refreshAuth
            )
              throw error;
            await options.refreshAuth();
            if (attempt.signal.aborted)
              throw directFailure("action.execution_unknown");
            return transport("action", body, attempt.signal);
          }
        };
        return await Promise.race([send(), terminated]);
      } finally {
        if (timer !== undefined) clearTimeout(timer);
        directAttempts.delete(attempt);
        attempt.abort();
      }
    },
    async pause() {
      if (stopped) return;
      paused = true;
      abort.abort();
      await control("pause");
      await active?.catch(() => {});
      notify();
    },
    async resume() {
      if (stopped) return;
      paused = false;
      await control("resume");
      notify();
    },
    async wake() {
      if (stopped) return;
      await control("wake");
      notify();
    },
    async close() {
      if (stopped) return;
      stopped = true;
      stopDirect();
      abort.abort();
      notify();
      await control("stop");
    },
  };
}

/** What Rust asks the downlink lane's host to do ([`DownlinkAction`] in the client crate). */
export type DownlinkAction =
  | { type: "open"; epoch: number; subscribe: string }
  | { type: "close"; epoch: number; reason: string | null }
  /**
   * The replica was rebuilt and the worker forgot everything it had in flight:
   * abandon the socket and every page, catch-up and bootstrap alike. It comes
   * first in its batch and replaces a `close`, so it never reaches the session
   * the batch opens next ([#162](https://github.com/zanminwang/axton/issues/162)).
   */
  | { type: "reset" }
  /**
   * `POST /sync/pull`. An ordinary catch-up belongs to the open session, so the
   * session's cancellation abandons it and its failure ends the session. A
   * `bootstrap` page belongs to the lane: it outlives the session, its failure
   * ends none, and only `pause`, `reset` and `close` abandon it
   * ([#151](https://github.com/zanminwang/axton/issues/151)).
   */
  | { type: "request"; request: number; body: string; bootstrap: boolean }
  /** A committed bootstrap transition; the registry projects it onto the subscription's status. */
  | ({ type: "bootstrap" } & BootstrapRun)
  | { type: "wake"; lane: "push" }
  | { type: "report"; reports: ReportDetails[] }
  | { type: "changed"; scopes: string[] }
  | { type: "acknowledged"; scopes: string[] }
  | { type: "wait"; millis: number };
/** Why one record or one queued mutation could not be applied as delivered. */
export type ReportKind = "readFailed" | "skipped" | "conflict" | "diverged";
export type ReportDetails = {
  kind: ReportKind;
  model: string;
  identity: Record<string, unknown>;
  stamp: number;
  /** `readFailed`: the server's code (`loader.failed`, or the refusal code). */
  code?: string;
  /** `diverged`: the queued mutation whose replay failed; it is still sent. */
  ordinal?: number;
  detail?: unknown;
};
/**
 * A delivery the client could not apply, handed to `onError`. The client stays
 * consistent: a `readFailed` or `skipped` record keeps its local content and
 * stamp, a `conflict` keeps the local content, a `diverged` mutation shows the
 * server's row and is still sent.
 */
export class AxtonReport extends Error {
  readonly kind: ReportKind;
  readonly model: string;
  readonly identity: Record<string, unknown>;
  readonly stamp: number;
  readonly code: string | undefined;
  readonly ordinal: number | undefined;
  readonly detail: unknown;
  constructor(report: ReportDetails) {
    super(
      `${report.kind}: ${report.model} ${JSON.stringify(report.identity)} at stamp ${report.stamp}` +
        (report.code ? ` (${report.code})` : "") +
        (report.ordinal !== undefined ? ` (mutation ${report.ordinal})` : ""),
    );
    this.name = "AxtonReport";
    this.kind = report.kind;
    this.model = report.model;
    this.identity = report.identity;
    this.stamp = report.stamp;
    this.code = report.code;
    this.ordinal = report.ordinal;
    this.detail = report.detail;
  }
}
export type DownlinkCommand = (
  event: Record<string, unknown>,
) => Promise<DownlinkAction[]>;
export type DownlinkNetwork = {
  push: Transport;
  open(subscribe: string, signal: AbortSignal, on: SocketEvents): void;
};
/** How the downlink lane hears from one socket. Frames arrive one at a time, in order. */
export type SocketEvents = {
  message(text: string): Promise<void>;
  /** The frame buffer overflowed; frames were dropped. */
  overflow(): Promise<void>;
  /** The socket ended on its own; not called for a socket the signal aborted. */
  closed(error: unknown): void;
};
/** The lane offers the ordinary controls only: no caller abandons its socket, the worker decides that. */
export type DownlinkLane = Connection;
type Session = { epoch: number; abort: AbortController; ended: boolean };
/**
 * Host loop of the downlink lane. Rust owns delivery: which channels, when to
 * catch up, what a page means, when to commit and when to retry. A socket or
 * HTTP callback only enqueues what arrived and wakes this loop; the loop asks
 * Rust to pump and executes what it answers with sockets, HTTP, timers and the
 * credential refresh. Enqueueing answers with no actions, so no page is applied
 * inside a callback.
 */
export async function startDownlinkLane(
  command: DownlinkCommand,
  network: DownlinkNetwork,
  options: ConnectionOptions,
  wakePush: () => void,
  /** Transport state for the subscription status projection ([subscriptions.mts](subscriptions.mts)); it makes no decision here. */
  report: (signal: DownlinkSignal) => void = () => {},
): Promise<DownlinkLane> {
  let stopped = false;
  let session: Session | undefined;
  /**
   * What abandons the historical pages in flight. They belong to the lane, not
   * to a socket, so only `pause`, `reset` and `close` abandon them and a
   * replaced socket leaves them alone ([#151](https://github.com/zanminwang/axton/issues/151)).
   */
  let loading = new AbortController();
  /** Catch-up requests of the open session that have not answered yet. */
  let outstanding = 0;
  // Every enqueue bumps this and the loop re-checks it before sleeping, so a
  // wake between the idle decision and the sleep is never lost.
  let generation = 0;
  let awaken: (() => void) | undefined;
  const notify = () => {
    generation++;
    awaken?.();
    awaken = undefined;
  };
  const wait = (millis?: number) =>
    new Promise<void>((resolve) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      awaken = () => {
        if (timer !== undefined) clearTimeout(timer);
        resolve();
      };
      if (millis !== undefined)
        timer = setTimeout(() => {
          awaken = undefined;
          resolve();
        }, millis);
    });
  const abandon = (current: Session) => {
    current.ended = true;
    current.abort.abort();
    // Signals name their session, so reporting one that is already gone is
    // harmless and no path that ends a socket can forget it.
    report({ lane: "ended", epoch: current.epoch });
  };
  /** Hand Rust one event and wake the loop; Rust answers with no actions. */
  const enqueue = async (event: Record<string, unknown>): Promise<void> => {
    if (stopped && event.event !== "stop") return;
    try {
      await command(event);
    } catch (error) {
      if (!stopped) options.onError?.(error);
    }
    notify();
  };
  /** The HTTP status a transport error carried, for Rust to tell a refusal from a transport failure. */
  const statusOf = (error: unknown) =>
    (error as { status?: number })?.status ?? null;
  /** A 401 the application can clear: refresh once, reporting a failed refresh. */
  const refresh = async (error: unknown) => {
    if (statusOf(error) !== 401 || !options.refreshAuth) return;
    try {
      await options.refreshAuth();
    } catch (refreshError) {
      options.onError?.(refreshError);
    }
  };
  /** A socket or request that failed: report it, refresh once, tell Rust. */
  const fail = async (
    current: Session,
    error: unknown,
    event: Record<string, unknown>,
  ) => {
    if (current.ended || stopped) return;
    abandon(current);
    if (session === current) session = undefined;
    options.onError?.(error);
    await refresh(error);
    await enqueue(event);
  };
  /**
   * One historical page. It rides on no session: its failure ends none, and the
   * worker decides from the status whether the server refused it or the
   * transport did.
   */
  const load = (request: number, body: string) => {
    const signal = loading.signal;
    network.push("pull", body, signal).then(
      (text) => enqueue({ event: "response", request, body: text }),
      async (error) => {
        if (stopped) return;
        // A page this lane abandoned itself - `pause` - is not the
        // application's failure, as an abandoned session's request is not; the
        // worker is still told, so it clears its slot and asks again on
        // `resume`.
        if (!signal.aborted) {
          options.onError?.(error);
          await refresh(error);
        }
        await enqueue({
          event: "failed",
          request,
          reason: String((error as { message?: string })?.message ?? error),
          status: signal.aborted ? null : statusOf(error),
        });
      },
    );
  };
  const execute = (action: DownlinkAction) => {
    switch (action.type) {
      case "open": {
        const current: Session = {
          epoch: action.epoch,
          abort: new AbortController(),
          ended: false,
        };
        session = current;
        outstanding = 0;
        report({ lane: "opened", epoch: current.epoch });
        network.open(action.subscribe, current.abort.signal, {
          message: (text) =>
            enqueue({ event: "message", epoch: current.epoch, body: text }),
          overflow: () => enqueue({ event: "overflow", epoch: current.epoch }),
          closed: (error) =>
            void fail(current, error, {
              event: "closed",
              epoch: current.epoch,
            }),
        });
        return;
      }
      case "request": {
        if (action.bootstrap) return load(action.request, action.body);
        const current = session;
        if (!current || current.ended) return;
        report({ lane: "requests", outstanding: ++outstanding });
        const settled = () => {
          if (session === current)
            report({ lane: "requests", outstanding: --outstanding });
        };
        network.push("pull", action.body, current.abort.signal).then(
          (text) => {
            settled();
            return enqueue({
              event: "response",
              request: action.request,
              body: text,
            });
          },
          (error) => {
            settled();
            return fail(current, error, {
              event: "failed",
              request: action.request,
              reason: String((error as { message?: string })?.message ?? error),
              status: statusOf(error),
            });
          },
        );
        return;
      }
      case "reset": {
        // A local abort, as `pause` is: whatever the old I/O still answers is
        // the application's failure no more, and the worker ignores it by epoch
        // and request id.
        if (session) abandon(session);
        session = undefined;
        outstanding = 0;
        loading.abort();
        loading = new AbortController();
        return;
      }
      case "close": {
        if (session?.epoch === action.epoch) {
          abandon(session);
          session = undefined;
        }
        if (action.reason !== null) options.onError?.(Error(action.reason));
        return;
      }
      case "wake":
        wakePush();
        return;
      case "report":
        for (const report of action.reports)
          options.onError?.(new AxtonReport(report));
        return;
      // The Scopes a commit moved and the set the handshake covered: the
      // subscription status projection reads both.
      case "changed":
      case "acknowledged":
        report({ lane: action.type, scopes: action.scopes });
        return;
      // A committed bootstrap transition: transport state to project, never a
      // decision to make here.
      case "bootstrap": {
        const { type, ...run } = action;
        report({ lane: "bootstrap", run });
        return;
      }
      // The loop sleeps for it; nothing to execute.
      case "wait":
        return;
    }
  };
  const loop = async () => {
    while (!stopped) {
      const observed = generation;
      let actions: DownlinkAction[];
      try {
        actions = await command({ event: "next" });
      } catch (error) {
        if (stopped) return;
        options.onError?.(error);
        // A pump that failed on the open session ends it; with none open there
        // is nothing to retry until something new arrives.
        const current = session;
        if (current) {
          session = undefined;
          abandon(current);
          await enqueue({ event: "closed", epoch: current.epoch });
        } else if (observed === generation) await wait();
        continue;
      }
      if (stopped) return;
      let millis: number | undefined;
      for (const action of actions) {
        if (action.type === "wait") millis = action.millis;
        execute(action);
      }
      // A socket the host abandoned that Rust still holds (the change it was
      // abandoned for did not commit, or changed nothing) is reported closed.
      if (session?.ended) {
        const current = session;
        session = undefined;
        await enqueue({ event: "closed", epoch: current.epoch });
        continue;
      }
      // Actions mean the worker made progress: pump again, yielding first, so a
      // commit never waits on a timer.
      if (millis === undefined && actions.length > 0) continue;
      if (observed !== generation) continue;
      await wait(millis);
    }
  };
  await enqueue({ event: "start" });
  void loop().catch((error) => {
    if (!stopped) options.onError?.(error);
  });
  return {
    async pause() {
      if (stopped) return;
      if (session) abandon(session);
      loading.abort();
      report({ lane: "paused" });
      await enqueue({ event: "pause" });
    },
    async resume() {
      if (stopped) return;
      loading = new AbortController();
      report({ lane: "resumed" });
      await enqueue({ event: "resume" });
    },
    async wake() {
      if (stopped) return;
      await enqueue({ event: "wake" });
    },
    async close() {
      if (stopped) return;
      stopped = true;
      if (session) abandon(session);
      session = undefined;
      loading.abort();
      report({ lane: "stopped" });
      await enqueue({ event: "stop" });
      notify();
    },
  };
}
