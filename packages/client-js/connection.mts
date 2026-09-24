export type Transport = (
  kind: string,
  body: string,
  signal?: AbortSignal,
) => Promise<string>;
export type ConnectionOptions = {
  onError?: (error: unknown) => void;
  refreshAuth?: () => Promise<void>;
  /** Maximum duration of one direct Action attempt, including token acquisition and authentication refresh. Default 30000 ms. */
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
  if (!Number.isSafeInteger(directTimeoutMs) || directTimeoutMs <= 0)
    throw Error("directTimeoutMs must be a positive finite integer");
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

/** What Rust asks the live lane's host to do ([`LiveAction`] in the client crate). */
export type LiveAction =
  | { type: "open"; epoch: number; subscribe: string }
  | { type: "request"; epoch: number; body: string }
  | { type: "close"; epoch: number; reason: string | null }
  | { type: "wake"; lane: "push" }
  | { type: "report"; reports: ReportDetails[] }
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
export type LiveCommand = (
  event: Record<string, unknown>,
) => Promise<LiveAction[]>;
export type LiveNetwork = {
  push: Transport;
  open(subscribe: string, signal: AbortSignal, on: SocketEvents): void;
};
/** How the live lane hears from one socket. Frames arrive one at a time, in order. */
export type SocketEvents = {
  message(text: string): Promise<void>;
  /** The frame buffer overflowed; frames were dropped. */
  overflow(): Promise<void>;
  /** The socket ended on its own; not called for a socket the signal aborted. */
  closed(error: unknown): void;
};
export type LiveLane = Connection & {
  /** Abandon the current session's socket and request now; Rust learns of it on the next event. */
  cancel(): void;
};
type Session = { epoch: number; abort: AbortController; ended: boolean };
/**
 * Host loop of the live lane. Rust owns the session: which channels, when to
 * catch up, what a page means, when to retry. This loop feeds it events and
 * executes its actions with sockets, HTTP, timers and the credential refresh.
 */
export async function startLiveLane(
  command: LiveCommand,
  network: LiveNetwork,
  options: ConnectionOptions,
  wakePush: () => void,
): Promise<LiveLane> {
  let stopped = false;
  let session: Session | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const clearTimer = () => {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  };
  const abandon = (current: Session) => {
    current.ended = true;
    current.abort.abort();
  };
  const dispatch = async (event: Record<string, unknown>): Promise<void> => {
    if (stopped && event.event !== "stop") return;
    clearTimer();
    let actions: LiveAction[];
    try {
      actions = await command(event);
    } catch (error) {
      if (stopped) return;
      options.onError?.(error);
      // A command that failed on an event of the session ends that session.
      const epoch = event.epoch;
      if (
        typeof epoch === "number" &&
        event.event !== "closed" &&
        session?.epoch === epoch
      ) {
        const current = session;
        session = undefined;
        abandon(current);
        await dispatch({ event: "closed", epoch });
      }
      return;
    }
    for (const action of actions) execute(action);
  };
  const fail = async (current: Session, error: unknown) => {
    if (current.ended || stopped) return;
    abandon(current);
    if (session === current) session = undefined;
    options.onError?.(error);
    if ((error as { status?: number })?.status === 401 && options.refreshAuth) {
      try {
        await options.refreshAuth();
      } catch (refreshError) {
        options.onError?.(refreshError);
      }
    }
    await dispatch({ event: "closed", epoch: current.epoch });
  };
  const execute = (action: LiveAction) => {
    switch (action.type) {
      case "open": {
        const current: Session = {
          epoch: action.epoch,
          abort: new AbortController(),
          ended: false,
        };
        session = current;
        network.open(action.subscribe, current.abort.signal, {
          message: (text) =>
            dispatch({ event: "message", epoch: current.epoch, body: text }),
          overflow: () => dispatch({ event: "overflow", epoch: current.epoch }),
          closed: (error) => void fail(current, error),
        });
        return;
      }
      case "request": {
        const current = session;
        if (!current || current.epoch !== action.epoch || current.ended) return;
        network.push("pull", action.body, current.abort.signal).then(
          (text) =>
            dispatch({ event: "catchUp", epoch: current.epoch, body: text }),
          (error) => fail(current, error),
        );
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
      case "wait":
        clearTimer();
        timer = setTimeout(() => {
          timer = undefined;
          void dispatch({ event: "next" });
        }, action.millis);
        return;
    }
  };
  await dispatch({ event: "start" });
  return {
    cancel() {
      if (session) abandon(session);
    },
    async pause() {
      if (stopped) return;
      if (session) abandon(session);
      await dispatch({ event: "pause" });
    },
    async resume() {
      if (stopped) return;
      await dispatch({ event: "resume" });
    },
    async wake() {
      if (stopped) return;
      await dispatch({ event: "wake" });
      // A session the host abandoned that Rust still holds (the change it was
      // abandoned for did not commit, or changed nothing) is reported closed.
      if (session?.ended) {
        const current = session;
        session = undefined;
        await dispatch({ event: "closed", epoch: current.epoch });
      }
    },
    async close() {
      if (stopped) return;
      stopped = true;
      if (session) abandon(session);
      session = undefined;
      clearTimer();
      await dispatch({ event: "stop" });
    },
  };
}
