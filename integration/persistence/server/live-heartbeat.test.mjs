import test from "node:test";
import assert from "node:assert/strict";
import { once } from "node:events";
import { createServer, connect } from "node:net";
import { setImmediate as turn } from "node:timers/promises";
import { WebSocket, WebSocketServer } from "ws";
import { createBackend } from "../../../packages/backend/server/index.mts";

const gate = () => {
  let resolve;
  const promise = new Promise((done) => {
    resolve = done;
  });
  return { promise, resolve };
};

// Real HTTP/WebSocket transport; the native controller is inert so these tests
// isolate transport liveness rather than database or authority decisions.
async function fixture(t, { autoPong = true, idleProxy = false, held } = {}) {
  const accepted = gate();
  const diagnostics = [];
  const state = { subscribed: 0, released: 0, clearedTimers: [] };
  t.mock.timers.enable({ apis: ["setInterval", "setTimeout", "Date"], now: 0 });
  const clear = globalThis.clearInterval;
  const clearMock = t.mock.method(globalThis, "clearInterval", (timer) => {
    if (timer !== undefined) state.clearedTimers.push(timer);
    return clear(timer);
  });
  const upgrade = WebSocketServer.prototype.handleUpgrade;
  t.mock.method(WebSocketServer.prototype, "handleUpgrade", function (...args) {
    const callback = args.pop();
    return upgrade.call(this, ...args, (socket) => {
      accepted.resolve(socket);
      callback(socket);
    });
  });
  const backend = createBackend({
    config: { schema: { enums: [], models: [], actions: [] }, mutations: [] },
    native: {
      validateConfig() {},
      serverMaterializationId05: () => "fixture",
      async negotiateLive(_config, _owner, request) {
        state.subscribed++;
        if (held) {
          held.entered.resolve();
          await held.release.promise;
        }
        const subscribe = JSON.parse(request);
        return JSON.stringify({
          handle: 1,
          actions: [
            {
              type: "send",
              frame: JSON.stringify({ ...subscribe, head: 0 }),
            },
          ],
        });
      },
      liveEvent: () => "[]",
      liveClose() {
        state.released++;
      },
    },
    database: {
      transaction: async (body) => body({}),
      persistence: () => ({
        call: async () => {
          throw new Error("unexpected persistence");
        },
      }),
    },
    protocol5: { projectionGeneration: "1", authorizeStream: () => true },
    authenticate: () => "fixture",
    loaders: {},
    onError: (error) => diagnostics.push(error),
  });
  const listener = await backend.listen({ port: 0 });
  const connections = new Set();
  let proxy;
  let url = listener.url.replace("http:", "ws:") + "/sync/live";
  if (idleProxy) {
    proxy = createServer((client) => {
      const upstream = connect(new URL(listener.url).port, "127.0.0.1");
      connections.add(client);
      connections.add(upstream);
      let idle;
      const activity = () => {
        clearTimeout(idle);
        idle = setTimeout(() => {
          client.destroy();
          upstream.destroy();
        }, 35_000);
      };
      client.on("data", activity);
      upstream.on("data", activity);
      client.on("error", () => {});
      upstream.on("error", () => {});
      client.on("close", () => {
        clearTimeout(idle);
        connections.delete(client);
        upstream.destroy();
      });
      upstream.on("close", () => {
        clearTimeout(idle);
        connections.delete(upstream);
        client.destroy();
      });
      client.pipe(upstream);
      upstream.pipe(client);
      activity();
    });
    proxy.listen(0, "127.0.0.1");
    await once(proxy, "listening");
    url = `ws://127.0.0.1:${proxy.address().port}/sync/live`;
  }
  const socket = new WebSocket(url, { autoPong });
  t.after(async () => {
    try {
      held?.release.resolve();
      const gone =
        socket.readyState === WebSocket.CLOSED
          ? undefined
          : once(socket, "close");
      socket.terminate();
      for (const connection of connections) connection.destroy();
      await listener.close();
      await gone;
      if (proxy) await new Promise((resolve) => proxy.close(resolve));
      assert.deepEqual(diagnostics, []);
    } finally {
      // Restore the observer before resetting the timer implementation it wraps.
      clearMock.mock.restore();
      t.mock.timers.reset();
    }
  });
  await once(socket, "open");
  const serverSocket = await accepted.promise;
  const ready = once(socket, "message");
  socket.send(
    JSON.stringify({ protocol: 5, storeId: "fixture", stream: "User:fixture" }),
  );
  if (held) await held.entered.promise;
  else await ready;
  return { socket, serverSocket, listener, state };
}

test(
  "host control heartbeats keep an idle proxy connection alive without extra subscribe frames",
  { timeout: 5000 },
  async (t) => {
    const f = await fixture(t, { idleProxy: true });
    for (let cycle = 0; cycle < 4; cycle++) {
      const pong = once(f.serverSocket, "pong", {
        signal: AbortSignal.timeout(500),
      });
      t.mock.timers.tick(30_000);
      await assert.doesNotReject(
        pong,
        "host must receive a Pong before the proxy idle window",
      );
      assert.equal(f.socket.readyState, WebSocket.OPEN);
    }
    assert.equal(
      f.state.subscribed,
      1,
      "control frames must not reach the application protocol",
    );
  },
);

test(
  "host terminates a peer that stops answering control pings",
  { timeout: 5000 },
  async (t) => {
    const f = await fixture(t, { autoPong: false });
    const ping = once(f.socket, "ping", { signal: AbortSignal.timeout(500) });
    t.mock.timers.tick(30_000);
    await assert.doesNotReject(
      ping,
      "host must probe the peer before reaping it",
    );
    const closed = once(f.socket, "close");
    t.mock.timers.tick(30_000);
    const [code] = await closed;
    assert.equal(code, 1006);
    await turn();
    assert.equal(f.state.released, 1);
  },
);

test(
  "listener close releases its heartbeat timer",
  { timeout: 5000 },
  async (t) => {
    const f = await fixture(t);
    const pong = once(f.serverSocket, "pong", {
      signal: AbortSignal.timeout(500),
    });
    t.mock.timers.tick(30_000);
    await assert.doesNotReject(pong);
    assert.equal(f.state.clearedTimers.length, 0);
    await f.listener.close();
    assert.equal(f.state.clearedTimers.length, 1);
    assert.equal(f.state.released, 1);
  },
);

test(
  "listener close stops heartbeat while still draining admitted negotiation",
  { timeout: 5000 },
  async (t) => {
    const held = { entered: gate(), release: gate() };
    const f = await fixture(t, { held });
    const pong = once(f.serverSocket, "pong", {
      signal: AbortSignal.timeout(500),
    });
    t.mock.timers.tick(30_000);
    await assert.doesNotReject(pong);
    let closed = false;
    const gone = once(f.socket, "close");
    const closing = f.listener.close().then(() => {
      closed = true;
    });
    await gone;
    await turn();
    assert.equal(f.state.clearedTimers.length, 1);
    assert.equal(
      closed,
      false,
      "transport close must not release admitted work",
    );
    held.release.resolve();
    await closing;
    assert.equal(f.state.released, 1);
  },
);
