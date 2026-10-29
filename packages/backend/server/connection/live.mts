import { STATUS_CODES, type IncomingMessage, type Server } from "node:http";
import type { Duplex } from "node:stream";
import { WebSocketServer, WebSocket } from "ws";
import { EngineError } from "../bindings/native.mts";
import {
  ADMISSION_HEADER,
  type Admission,
  type Refusal,
} from "./admission.mts";

/** What the executor reports to the Rust `Subscriptions` controller. */
export type LiveEvent =
  | { type: "committed"; stream: string }
  | { type: "pulled"; page: string }
  | { type: "closed" };
/** What the controller asks the executor to do, in order. */
export type LiveAction =
  | { type: "pullV05"; request: string }
  | { type: "listen"; stream: string }
  | { type: "send"; frame: string };
/**
 * The live executor's seams: the Rust `Subscriptions` controller behind
 * `negotiateLive`, `liveEvent` and `liveClose`, plus the database pull and the
 * commit hub it asks the executor to use.
 */
export interface LiveBackend {
  pull05Live?(owner: string, request: string): Promise<string>;
  pull?(owner: string, request: string): Promise<string>;
  negotiateLive(
    owner: string,
    request: Uint8Array | string,
  ): Promise<{ handle: number; actions: LiveAction[] }>;
  liveEvent(handle: number, event: LiveEvent): LiveAction[];
  liveClose(handle: number): void;
  onCommitted(stream: string, wake: () => void): () => void;
}

export function attachLive(
  server: Server,
  options: {
    backend: LiveBackend;
    admit: Admission;
    maxPayloadBytes?: number;
    onError?: (error: unknown) => void;
  },
) {
  const sockets = new WebSocketServer({
    noServer: true,
    maxPayload: options.maxPayloadBytes ?? 1_048_576,
  });
  const answered = new WeakSet<WebSocket>();
  let heartbeat: ReturnType<typeof setInterval> | undefined;
  server.once("listening", () => {
    heartbeat = setInterval(() => {
      for (const socket of sockets.clients) {
        if (socket.readyState !== WebSocket.OPEN) continue;
        if (!answered.delete(socket)) {
          socket.terminate();
          continue;
        }
        socket.ping();
      }
    }, 30_000);
  });
  let closing = false;
  const sessions = new Set<Promise<void>>();
  const upgrades = new Set<Promise<void>>();
  const refuse = (socket: Duplex, status: number) => {
    socket.end(
      `HTTP/1.1 ${status} ${status === 401 ? "Unauthorized" : "Error"}\r\nConnection: close\r\n\r\n`,
    );
  };
  const refuseAdmission = (socket: Duplex, { status, body }: Refusal) => {
    socket.end(
      `HTTP/1.1 ${status} ${STATUS_CODES[status] ?? "Error"}\r\n` +
        `Content-Type: application/json; charset=utf-8\r\n` +
        `Content-Length: ${Buffer.byteLength(body)}\r\n` +
        `Cache-Control: no-store\r\n${ADMISSION_HEADER}: refused\r\n` +
        `Connection: close\r\n\r\n${body}`,
    );
  };
  const upgrade = (request: IncomingMessage, socket: Duplex, head: Buffer) => {
    const pending = (async () => {
      if (request.url?.split("?")[0] !== "/sync/live") return;
      if (closing) {
        refuse(socket, 503);
        return;
      }
      let owner: string | null;
      try {
        const admission = await options.admit(request);
        if (closing || socket.destroyed) {
          refuse(socket, 503);
          return;
        }
        if (admission.refusal) {
          refuseAdmission(socket, admission.refusal);
          return;
        }
        owner = admission.owner;
      } catch (error) {
        options.onError?.(error);
        refuse(socket, 500);
        return;
      }
      if (!owner?.trim()) {
        refuse(socket, 401);
        return;
      }
      sockets.handleUpgrade(request, socket, head, (connection) => {
        answered.add(connection);
        const pong = () => answered.add(connection);
        connection.on("pong", pong);
        connection.once("close", () => connection.off("pong", pong));
        const session = serveLive(
          connection,
          owner!,
          options.backend,
          options.onError,
        );
        sessions.add(session);
        void session.then(
          () => sessions.delete(session),
          () => sessions.delete(session),
        );
      });
    })();
    upgrades.add(pending);
    void pending.then(
      () => upgrades.delete(pending),
      () => upgrades.delete(pending),
    );
  };
  server.on("upgrade", upgrade);
  return {
    close: async () => {
      if (closing) return;
      closing = true;
      clearInterval(heartbeat);
      const owned = [...upgrades, ...sessions];
      server.off("upgrade", upgrade);
      for (const socket of sockets.clients) socket.close(1001, "closing");
      await new Promise<void>((resolve) => sockets.close(() => resolve()));
      const drained = await Promise.allSettled(owned);
      const failure = drained.find((result) => result.status === "rejected");
      if (failure?.status === "rejected") throw failure.reason;
    },
  };
}

/**
 * Executes the Rust controller's actions for one socket. Every sync decision
 * (what to pull, when, what to send) is the controller's; this only carries
 * events in and performs actions out. The controller keeps at most one pull
 * outstanding per session; it covers every stream with a pending commit.
 */
async function serveLive(
  connection: WebSocket,
  owner: string,
  backend: LiveBackend,
  onError?: (error: unknown) => void,
): Promise<void> {
  const cleanups: (() => void)[] = [];
  const pulls = new Set<Promise<void>>();
  const trackPull = (pull: Promise<void>) => {
    pulls.add(pull);
    void pull.then(
      () => pulls.delete(pull),
      () => pulls.delete(pull),
    );
  };
  let settled = false;
  let handshakeReject: ((error: Error) => void) | undefined;
  const transportError = (error: Error) => {
    handshakeReject?.(error);
  };
  connection.on("error", transportError);
  cleanups.push(() => connection.off("error", transportError));
  let handle: number | undefined;
  let released = false;
  const open = () => connection.readyState === WebSocket.OPEN;
  const fail = (error: unknown) => {
    onError?.(error);
    if (open()) connection.close(1011, "server");
  };
  const dispatch = (event: LiveEvent) => {
    if (handle === undefined || released) return;
    let actions: LiveAction[];
    try {
      actions = backend.liveEvent(handle, event);
    } catch (error) {
      fail(error);
      return;
    }
    execute(actions);
  };
  const execute = (actions: LiveAction[]) => {
    for (const action of actions) {
      if (action.type === "listen") {
        const { stream } = action;
        cleanups.push(
          backend.onCommitted(stream, () =>
            dispatch({ type: "committed", stream }),
          ),
        );
      } else if (action.type === "send") {
        if (open()) connection.send(action.frame);
      } else if (action.type === "pullV05") {
        if (!backend.pull05Live) {
          fail(new Error("v05 live carrier unavailable"));
          return;
        }
        trackPull(
          backend
            .pull05Live(owner, action.request)
            .then((page) => dispatch({ type: "pulled", page }), fail),
        );
      }
    }
  };
  const closed = () => dispatch({ type: "closed" });
  try {
    const first = await new Promise<Buffer>((resolve, reject) => {
      const message = (data: Buffer) => {
        if (settled) {
          connection.close(1002, "subscribe is the only client frame");
          return;
        }
        settled = true;
        resolve(Buffer.from(data));
      };
      const handshakeClosed = () => reject(new Error("live handshake closed"));
      handshakeReject = reject;
      connection.on("message", message);
      connection.once("close", handshakeClosed);
      cleanups.push(
        () => connection.off("message", message),
        () => connection.off("close", handshakeClosed),
      );
    });
    const opened = await backend.negotiateLive(owner, first);
    handshakeReject = undefined;
    handle = opened.handle;
    connection.once("close", closed);
    connection.once("error", closed);
    cleanups.push(
      () => connection.off("close", closed),
      () => connection.off("error", closed),
    );
    if (!open()) return;
    execute(opened.actions);
    await new Promise<void>((resolve) => {
      connection.once("close", () => resolve());
      connection.once("error", () => resolve());
    });
  } catch (error) {
    // A malformed subscribe or a refused read-contract declaration is the
    // client's fault: closed as a protocol violation, not reported as a failure.
    const refused =
      error instanceof EngineError &&
      (error.code === "protocol.unsupported" ||
        error.code === "request.invalid" ||
        error.code === "model_version_unsupported");
    if (open())
      connection.close(
        refused ? 1002 : 1011,
        refused ? (error as EngineError).code : "request.invalid",
      );
    if (!refused) onError?.(error);
  } finally {
    if (handle !== undefined) {
      closed();
      released = true;
      backend.liveClose(handle);
    }
    for (const cleanup of cleanups) cleanup();
    // Socket closure stops new work; already admitted database pulls still own
    // their transaction and must finish before the listener releases its host.
    await Promise.all(pulls);
  }
}
