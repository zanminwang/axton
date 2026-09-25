import test from "node:test";
import assert from "node:assert/strict";
import {
  startConnection,
  startDownlinkLane,
} from "../../../packages/client-js/connection.mts";
test("direct attempt is bounded even when transport ignores abort", async () => {
  const connection = await startConnection(
    async () => ({ type: "idle" }),
    async () => {},
    async () => new Promise(() => {}),
    { directTimeoutMs: 15 },
  );
  try {
    await assert.rejects(
      connection.requestAction("same bytes"),
      (error) =>
        error.code === "action.execution_unknown" &&
        error.execution === "unknown",
    );
  } finally {
    await connection.close();
  }
});
test("direct timeout rejects values outside the JavaScript timer range", async () => {
  await assert.rejects(
    startConnection(async () => ({ type: "idle" }), async () => {}, async () => "ok", { directTimeoutMs: 2_147_483_648 }),
    /directTimeoutMs/,
  );
});
test("direct request completes while durable delivery is blocked", async () => {
  let entered;
  const blocked = new Promise(resolve => { entered = resolve; });
  const connection = await startConnection(async event => event === 'next' ? { type: 'sync' } : undefined, async request => { entered(); await request('push', 'queued'); }, async kind => kind === 'push' ? new Promise(() => {}) : 'direct-result');
  try {
    await blocked;
    assert.equal(await connection.requestAction('direct'), 'direct-result');
  } finally { await connection.close(); }
});
test("direct auth retry keeps the same body and close bounds a hanging refresh", async () => {
  const seen = [];
  const connection = await startConnection(
    async () => ({ type: "idle" }),
    async () => {},
    async (kind, body) => {
      seen.push([kind, body]);
      if (seen.length === 1)
        throw Object.assign(Error("expired"), { status: 401 });
      return "ok";
    },
    { refreshAuth: async () => {}, directTimeoutMs: 50 },
  );
  assert.equal(await connection.requestAction("frozen"), "ok");
  assert.deepEqual(seen, [
    ["action", "frozen"],
    ["action", "frozen"],
  ]);
  await connection.close();
  await assert.rejects(
    connection.requestAction("new"),
    (error) => error.code === "action.unavailable",
  );
  const hanging = await startConnection(
    async () => ({ type: "idle" }),
    async () => {},
    async () => {
      throw Object.assign(Error("expired"), { status: 401 });
    },
    { refreshAuth: async () => new Promise(() => {}), directTimeoutMs: 15 },
  );
  try {
    await assert.rejects(
      hanging.requestAction("frozen"),
      (error) => error.code === "action.execution_unknown",
    );
  } finally {
    await hanging.close();
  }
});
test("raw direct Action fails immediately without an active connection", async () => {
  const { Client } = await import("../../../packages/client-js/index.mts");
  const { mkdtemp, rm } = await import("node:fs/promises");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const directory = await mkdtemp(join(tmpdir(), "axton-direct-unavailable-"));
  const client = await Client.open({
    path: join(directory, "db"),
    schema: {
      enums: [],
      models: [],
      actions: [{ name: "Ping", version: 1, inputs: [], outputs: [] }],
    },
  });
  try {
    await assert.rejects(
      client.callAction("Ping", 1, {}),
      (error) =>
        error.code === "action.unavailable" && error.execution === "unknown",
    );
    assert.equal((await client.syncState()).pending, 0);
  } finally {
    await client.close();
    await rm(directory, { recursive: true, force: true });
  }
});
test("raw Action discard and rebuild deliver terminal call identities", async () => {
  const { Client } = await import("../../../packages/client-js/index.mts");
  const { mkdtemp, rm, readFile } = await import("node:fs/promises");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const directory = await mkdtemp(join(tmpdir(), "axton-action-discard-"));
  const schema = JSON.parse(await readFile(new URL("../../../fixtures/schemas/entry.json", import.meta.url), "utf8"));
  schema.actions = [{ name: "Ping", version: 1, inputs: [], outputs: [] }];
  const breaking = structuredClone(schema);
  breaking.models[0].fields.push({ name: "due", nullable: false, type: { kind: "scalar", name: "string" } });
  try {
    for (const frozen of [false, true]) {
      const path = join(directory, frozen ? "frozen" : "unsent");
      const original = await Client.open({ path, schema });
      const delivered = [];
      original.onActionCompletion(value => delivered.push(value));
      const dropped = await original.submitAction("Ping", 1, {});
      await original.drop(dropped.ordinal);
      assert.equal(delivered[0].callId, dropped.callId);
      assert.equal(delivered[0].outcome.code, "dropped");
      const pending = await original.submitAction("Ping", 1, {});
      if (frozen) await original.freeze();
      await original.close();
      const reopened = await Client.open({ path, schema: breaking });
      const abandoned = [];
      reopened.onActionCompletion(value => abandoned.push(value));
      try {
        const report = await reopened.rebuild({ discardPending: true });
        assert.deepEqual(report.abandonedCalls, [{ callId: pending.callId, frozen }]);
        assert.equal(abandoned[0].callId, pending.callId);
        assert.equal(abandoned[0].outcome.code, "abandoned");
        assert.equal(abandoned[0].outcome.execution, frozen ? "unknown" : "rejected");
      } finally { await reopened.close(); }
    }
  } finally { await rm(directory, { recursive: true, force: true }); }
});
test("a waiting direct network response leaves local reads free and cannot apply after close", async () => {
  const { createClient } = await import("../../../packages/client-js/runtime.mts");
  const { Transaction } = await import("../../../packages/client-js/transaction.mts");
  const { createRequire } = await import("node:module");
  const { mkdtemp, rm } = await import("node:fs/promises");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const native = createRequire(import.meta.url)("../../../bindings/node/axton-node.node");
  const directory = await mkdtemp(join(tmpdir(), "axton-direct-race-"));
  let release, entered;
  const waiting = new Promise(resolve => { release = resolve; });
  const begun = new Promise(resolve => { entered = resolve; });
  const Client = createClient(native, Transaction, () => ({
    push: (kind, body) => { if (kind === 'action') { entered(body); return waiting; } return Promise.reject(Error('unexpected background request')); },
    open() {},
  }));
  const client = await Client.open({ path: join(directory, 'db'), schema: { enums: [], models: [], actions: [{ name: 'Ping', version: 1, inputs: [], outputs: [] }] } });
  let connection;
  const observed = [];
  try {
    connection = await client.connect({ url: 'http://unused', token: 'token' });
    client.onActionCompletion(value => observed.push(value));
    const call = client.callAction('Ping', 1, {});
    const failure = assert.rejects(call, error => error.code === 'action.unavailable' || error.code === 'action.execution_unknown');
    const body = JSON.parse(await begun);
    assert.equal((await Promise.race([client.syncState(), new Promise((_, reject) => setTimeout(() => reject(Error('local queue blocked')), 100))])).pending, 0);
    await connection.close();
    await failure;
    release(JSON.stringify({ completion: { callId: body.call.callId, outcome: { status: 'succeeded', result: null } }, records: [] }));
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(observed, []);
    assert.equal((await client.syncState()).pending, 0);
  } finally { await connection?.close(); await client.close(); await rm(directory, { recursive: true, force: true }); }
});
test("wake arriving during idle decision cannot be lost", async () => {
  let release;
  const gate = new Promise((r) => (release = r));
  let calls = 0;
  const connection = await startConnection(
    async (event) => {
      if (event === "next") {
        if (++calls === 1) await gate;
        return { type: "idle" };
      }
    },
    async () => {},
    async () => "",
  );
  await connection.wake();
  release();
  await new Promise((r) => setTimeout(r, 10));
  assert.ok(calls >= 2);
  await connection.close();
});
test("close aborts an uncooperative network promise and prevents late completion", async () => {
  const events = [];
  let entered;
  const begun = new Promise((r) => (entered = r));
  const connection = await startConnection(
    async (event) => {
      events.push(event);
      return { type: "sync" };
    },
    async (request) => {
      entered();
      await request("push", "body");
    },
    async () => new Promise(() => {}),
  );
  await begun;
  await connection.close();
  await new Promise((r) => setImmediate(r));
  assert.ok(events.includes("stop"));
  assert.ok(!events.includes("success"));
  assert.ok(!events.includes("failure"));
});

test("closed connection controls cannot affect a replacement driver", async () => {
  const events = [];
  const connection = await startConnection(
    async (event) => {
      events.push(event);
      return { type: "idle" };
    },
    async () => {},
    async () => "",
  );
  await connection.close();
  const ended = events.length;
  await connection.pause();
  await connection.resume();
  await connection.wake();
  await connection.close();
  assert.equal(events.length, ended);
});
test("closing an old client connection twice preserves ownership of the replacement", async () => {
  const { Client } = await import("../../../packages/client-js/index.mts");
  const { mkdtemp, rm, readFile } = await import("node:fs/promises");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const directory = await mkdtemp(join(tmpdir(), "axton-connection-"));
  const schema = JSON.parse(
    await readFile(
      new URL("../../../fixtures/schemas/entry.json", import.meta.url),
      "utf8",
    ),
  );
  const client = await Client.open({
    path: join(directory, "client.sqlite"),
    schema,
  });
  let second;
  try {
    const first = await client.connect({url:"http://127.0.0.1:1",token:"secret"});
    await first.close();
    second = await client.connect({url:"http://127.0.0.1:1",token:"secret"});
    await first.close();
    await assert.rejects(
      client.connect({url:"http://127.0.0.1:1",token:"secret"}),
      /already active/,
    );
  } finally {
    await second?.close();
    await client.close();
    await rm(directory, { recursive: true, force: true });
  }
});

test("client close waits for in-flight connection setup and remains idempotent", async () => {
  const { Client } = await import("../../../packages/client-js/index.mts");
  const { mkdtemp, rm, readFile } = await import("node:fs/promises");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const directory = await mkdtemp(join(tmpdir(), "axton-connection-close-"));
  const schema = JSON.parse(
    await readFile(
      new URL("../../../fixtures/schemas/entry.json", import.meta.url),
      "utf8",
    ),
  );
  const client = await Client.open({
    path: join(directory, "client.sqlite"),
    schema,
  });
  const errors = [];
  try {
    const starting = client.connect(
      {url:"http://127.0.0.1:1",token:"secret"},
      { onError: (error) => errors.push(error) },
    );
    await Promise.all([starting, client.close()]);
    await new Promise((r) => setImmediate(r));
    assert.deepEqual(errors, []);
    await client.close();
    await (await starting).close();
    await assert.rejects(
      client.connect({url:"http://127.0.0.1:1",token:"secret"}),
      /closed/,
    );
  } finally {
    await client.close().catch(() => {});
    await rm(directory, { recursive: true, force: true });
  }
});

const settled = () => new Promise((r) => setTimeout(r, 10));
/** A downlink lane over a scripted worker: every event is recorded, `next` answers the queued actions. */
const downlinkLane = (answers, network = {}, options = {}) => {
  const events = [];
  const lane = startDownlinkLane(
    async (event) => {
      events.push(event);
      return event.event === "next" ? (answers.shift() ?? []) : [];
    },
    { push: async () => "{}", open() {}, ...network },
    options,
    () => {},
  );
  return { events, lane, pumps: () => events.filter((e) => e.event === "next").length };
};
test("downlink wake arriving during the idle decision cannot be lost", async () => {
  let release;
  const gate = new Promise((r) => (release = r));
  let pumps = 0;
  const events = [];
  const lane = await startDownlinkLane(
    async (event) => {
      events.push(event.event);
      if (event.event !== "next") return [];
      if (++pumps === 1) await gate;
      return [];
    },
    { push: async () => "{}", open() {} },
    {},
    () => {},
  );
  await lane.wake();
  release();
  await settled();
  assert.ok(pumps >= 2, `the wake was lost after ${pumps} pumps`);
  assert.deepEqual(events.slice(0, 2), ["start", "next"]);
  await lane.close();
  assert.ok(events.includes("stop"));
});
test("downlink socket events only enqueue; the pump drives the socket", async () => {
  let frames;
  const { events, lane, pumps } = downlinkLane(
    [[{ type: "open", epoch: 1, subscribe: '{"type":"subscribe"}' }]],
    { open: (subscribe, signal, on) => (frames = on) },
  );
  const downlink = await lane;
  await settled();
  assert.ok(frames, "the pump opened the socket");
  const idle = pumps();
  await settled();
  assert.equal(pumps(), idle, "an idle worker is not polled");
  await frames.message("page");
  await frames.overflow();
  await settled();
  assert.deepEqual(
    events.filter((e) => e.event !== "next"),
    [
      { event: "start" },
      { event: "message", epoch: 1, body: "page" },
      { event: "overflow", epoch: 1 },
    ],
    "a callback hands the frame over and decides nothing",
  );
  assert.ok(pumps() > idle, "each enqueue woke the pump");
  await downlink.close();
});
test("a downlink catch-up is answered by its own request id", async () => {
  const pulls = [];
  let answer;
  const { events, lane } = downlinkLane(
    [
      [{ type: "open", epoch: 1, subscribe: "{}" }],
      [{ type: "request", request: 7, body: '{"cursors":{}}' }],
    ],
    {
      push: (kind, body) => {
        pulls.push({ kind, body });
        return new Promise((resolve, reject) => (answer = { resolve, reject }));
      },
      open() {},
    },
  );
  const downlink = await lane;
  await settled();
  assert.deepEqual(pulls, [{ kind: "pull", body: '{"cursors":{}}' }]);
  answer.resolve('{"cursors":{}}');
  await settled();
  assert.deepEqual(events.find((e) => e.event === "response"), {
    event: "response",
    request: 7,
    body: '{"cursors":{}}',
  });
  await downlink.close();
});
test("a failed downlink catch-up reports the failure by request id", async () => {
  const reported = [];
  let answer;
  const { events, lane } = downlinkLane(
    [
      [{ type: "open", epoch: 1, subscribe: "{}" }],
      [{ type: "request", request: 3, body: '{"cursors":{}}' }],
    ],
    {
      push: () => new Promise((resolve, reject) => (answer = { resolve, reject })),
      open() {},
    },
    { onError: (error) => reported.push(error) },
  );
  const downlink = await lane;
  await settled();
  answer.reject(Error("pull failed: 500"));
  await settled();
  assert.deepEqual(events.find((e) => e.event === "failed"), {
    event: "failed",
    request: 3,
    reason: "pull failed: 500",
  });
  assert.match(reported[0].message, /pull failed: 500/);
  await downlink.close();
});
