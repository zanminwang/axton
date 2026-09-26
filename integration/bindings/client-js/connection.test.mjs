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
/** Poll until `condition` holds; a real native client answers on its own schedule. */
const eventually = async (condition, what) => {
  const deadline = Date.now() + 5000;
  while (!condition()) {
    if (Date.now() > deadline) assert.fail(`${what} timed out`);
    await new Promise((r) => setTimeout(r, 5));
  }
};
/** A downlink lane over a scripted worker: every event is recorded, `next` answers the queued actions. */
const downlinkLane = (answers, network = {}, options = {}, report) => {
  const events = [];
  const lane = startDownlinkLane(
    async (event) => {
      events.push(event);
      return event.event === "next" ? (answers.shift() ?? []) : [];
    },
    { push: async () => "{}", open() {}, ...network },
    options,
    () => {},
    report,
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
      [{ type: "request", request: 7, body: '{"cursors":{}}', bootstrap: false }],
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
      [{ type: "request", request: 3, body: '{"cursors":{}}', bootstrap: false }],
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
    status: null,
  });
  assert.match(reported[0].message, /pull failed: 500/);
  await downlink.close();
});

/**
 * A bootstrap page is fetched from the same route as a catch-up, but it belongs
 * to the lane and not to a socket: no session has to be open, its failure ends
 * none, and the status travels so Rust can tell a refusal from a transport
 * failure ([#151](https://github.com/zanminwang/axton/issues/151)).
 */
test("a downlink bootstrap page runs without a session and reports its status", async () => {
  const pulls = [];
  const signals = [];
  let answer;
  const reported = [];
  const body = '{"mode":"bootstrap","channel":"a","models":{},"after":0,"until":7}';
  const { events, lane } = downlinkLane(
    [
      [{ type: "request", request: 9, body, bootstrap: true }],
      [],
      [{ type: "request", request: 10, body, bootstrap: true }],
      [
        {
          type: "bootstrap",
          scope: "a",
          subscriptionId: 1,
          state: "loading",
          run: 1,
          cursor: 40,
          barrier: null,
          error: null,
        },
      ],
    ],
    {
      push: (kind, pushed, signal) => {
        pulls.push({ kind, body: pushed });
        signals.push(signal);
        return new Promise((resolve, reject) => (answer = { resolve, reject }));
      },
      open() {
        throw Error("a bootstrap page opens no socket");
      },
    },
    { onError: (error) => reported.push(error) },
  );
  const downlink = await lane;
  await settled();
  assert.deepEqual(pulls, [{ kind: "pull", body }], "the same endpoint");
  answer.resolve('{"mode":"bootstrap","channel":"a","from":0,"to":40,"until":7,"head":9,"records":[]}');
  await settled();
  assert.equal(
    events.find((e) => e.event === "response").request,
    9,
    "answered by its own id",
  );
  // The next one fails with a status: the worker, not the host, decides what a
  // refusal means, and no session was ended by it.
  answer.reject(Object.assign(Error("pull failed: 400"), { status: 400 }));
  await settled();
  assert.deepEqual(events.find((e) => e.event === "failed"), {
    event: "failed",
    request: 10,
    reason: "pull failed: 400",
    status: 400,
  });
  assert.ok(!events.some((e) => e.event === "closed"), "no session was ended");
  assert.match(reported[0].message, /pull failed: 400/);
  await downlink.close();
  assert.ok(
    signals.every((signal) => signal.aborted),
    "closing the lane abandons the pages in flight",
  );
});

/**
 * A stored Bootstrap row the worker cannot decode reaches `onError` once, as an
 * `Error` naming the channel and the bounded reason; it is no status
 * transition, so the subscription status projection hears nothing
 * ([#163](https://github.com/zanminwang/axton/issues/163)).
 */
test("a downlink ledger issue reaches onError without a status transition", async () => {
  const reported = [];
  const signals = [];
  const message = "the stored Bootstrap row cannot be decoded: expected an unsigned integer";
  const { lane } = downlinkLane(
    [[{ type: "ledgerIssue", channel: "a", message }]],
    {},
    { onError: (error) => reported.push(error) },
    (signal) => signals.push(signal),
  );
  const downlink = await lane;
  await settled();
  assert.equal(reported.length, 1, "one error for one issue");
  assert.ok(reported[0] instanceof Error);
  assert.equal(reported[0].message, `bootstrap ledger a: ${message}`);
  assert.deepEqual(signals, [], "no status transition");
  await downlink.close();
});

/**
 * A page the lane abandoned itself is not the application's failure: `pause`
 * aborts it silently, the worker still hears `failed` so it can clear its slot,
 * and `resume` fetches again on a fresh cancellation
 * ([#151](https://github.com/zanminwang/axton/issues/151)).
 */
test("pausing the downlink lane abandons its bootstrap page without reporting it", async () => {
  const reported = [];
  const body = '{"mode":"bootstrap","channel":"a","models":{},"after":0,"until":7}';
  const script = [[{ type: "request", request: 4, body, bootstrap: true }], []];
  let pulls = 0;
  const { events, lane } = downlinkLane(
    script,
    {
      push: (kind, pushed, signal) => {
        pulls++;
        return new Promise((resolve, reject) => {
          signal.addEventListener("abort", () => reject(Error("connection_closed")), {
            once: true,
          });
        });
      },
      open() {
        throw Error("a bootstrap page opens no socket");
      },
    },
    { onError: (error) => reported.push(error) },
  );
  const downlink = await lane;
  await settled();
  assert.equal(pulls, 1, "the page went out");
  await downlink.pause();
  await settled();
  assert.deepEqual(reported, [], "its own cancellation is not an application failure");
  assert.deepEqual(events.find((e) => e.event === "failed"), {
    event: "failed",
    request: 4,
    reason: "connection_closed",
    status: null,
  });
  // Resume fetches again, on a cancellation of its own: the pause does not
  // reach the next page.
  script.push([{ type: "request", request: 5, body, bootstrap: true }]);
  await downlink.resume();
  await settled();
  assert.equal(pulls, 2, "the resumed page went out");
  assert.equal(
    events.filter((e) => e.event === "failed").length,
    1,
    "the old pause abandoned nothing of the resumed page",
  );
  await downlink.close();
});

/**
 * After a replica rebuild the worker's first pump answers `reset` before any new
 * session: the host abandons the old socket and every page in flight, catch-up
 * and bootstrap alike, as a local abort - no `close` that could reach the new
 * socket, nothing reported to the application - and later pages ride a fresh
 * cancellation ([#162](https://github.com/zanminwang/axton/issues/162)).
 */
test("a downlink reset abandons the old socket and every page before the new session opens", async () => {
  const reported = [];
  const signals = [];
  const sockets = [];
  const pulls = [];
  const body = '{"mode":"bootstrap","channel":"a","models":{},"after":0,"until":7}';
  const script = [
    [{ type: "open", epoch: 1, subscribe: "{}" }],
    [
      { type: "request", request: 2, body: '{"cursors":{}}', bootstrap: false },
      { type: "request", request: 3, body, bootstrap: true },
    ],
    [],
  ];
  const { events, lane } = downlinkLane(
    script,
    {
      // An uncooperative transport: a page answers only when the test says so,
      // however long after its cancellation.
      push: (kind, pushed, signal) => {
        const pull = { body: pushed, signal };
        pulls.push(pull);
        return new Promise((resolve, reject) => Object.assign(pull, { resolve, reject }));
      },
      open: (subscribe, signal, on) => sockets.push({ subscribe, signal, on }),
    },
    { onError: (error) => reported.push(error) },
    (signal) => signals.push(signal),
  );
  const downlink = await lane;
  await settled();
  assert.equal(sockets.length, 1, "the old session opened");
  assert.equal(pulls.length, 2, "a catch-up and a bootstrap page are in flight");
  script.push([
    { type: "reset" },
    { type: "open", epoch: 4, subscribe: "{}" },
    { type: "request", request: 5, body, bootstrap: true },
  ]);
  await downlink.wake();
  await settled();
  assert.ok(sockets[0].signal.aborted, "the old socket is abandoned");
  assert.ok(pulls[0].signal.aborted, "the old catch-up is abandoned");
  assert.ok(pulls[1].signal.aborted, "the old bootstrap page is abandoned");
  assert.equal(sockets.length, 2, "the new session opened after the reset");
  assert.equal(sockets[1].signal.aborted, false, "the reset never reaches the new socket");
  assert.equal(pulls[2].signal.aborted, false, "a later page rides a fresh cancellation");
  // Old callbacks racing the abort: none is the application's failure, and none
  // ends the new session.
  sockets[0].on.closed(Error("late close"));
  pulls[0].reject(Error("late catch-up failure"));
  pulls[1].reject(Error("late bootstrap failure"));
  await settled();
  assert.deepEqual(reported, [], "a local abort is not an application failure");
  assert.ok(
    !events.some((e) => e.event === "closed"),
    "the reset replaces a close: the worker already forgot the old session",
  );
  assert.equal(sockets[1].signal.aborted, false);
  const ended = signals.findIndex((s) => s.lane === "ended" && s.epoch === 1);
  const opened = signals.findIndex((s) => s.lane === "opened" && s.epoch === 4);
  assert.ok(ended >= 0 && ended < opened, "the old session ends before the new one opens");
  await downlink.close();
});

/**
 * A rebuild resets the worker behind a connected lane that is asleep with no
 * timer; the SDK wakes it once the native rebuild answered, so the carried
 * Channel is subscribed again without another `connect` or `start`
 * ([#162](https://github.com/zanminwang/axton/issues/162)).
 */
test("a rebuild wakes the sleeping downlink lane without another start", async () => {
  const { createClient } = await import("../../../packages/client-js/runtime.mts");
  const { Transaction } = await import("../../../packages/client-js/transaction.mts");
  const { createRequire } = await import("node:module");
  const { mkdtemp, rm, readFile } = await import("node:fs/promises");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const native = createRequire(import.meta.url)("../../../bindings/node/axton-node.node");
  const directory = await mkdtemp(join(tmpdir(), "axton-rebuild-wake-"));
  const path = join(directory, "client.sqlite");
  const schema = JSON.parse(
    await readFile(new URL("../../../fixtures/schemas/entry.json", import.meta.url), "utf8"),
  );
  const breaking = structuredClone(schema);
  breaking.models[0].fields.push({ name: "due", nullable: false, type: { kind: "scalar", name: "string" } });
  /** Every lane command and rebuild, with what the downlink worker answered. */
  const log = [];
  const sockets = [];
  const Client = createClient(
    {
      async clientCall(request) {
        const { op, event } = JSON.parse(request);
        const answer = await native.clientCall(request);
        if (op === "downlink")
          log.push({ op, event, actions: JSON.parse(answer).value });
        else if (op === "connection" || op === "rebuild") log.push({ op, event });
        return answer;
      },
    },
    Transaction,
    () => ({
      // The unsent mutation's push never answers; closing abandons it.
      push: (kind, body, signal) =>
        new Promise((_, reject) =>
          signal?.addEventListener("abort", () => reject(Error("connection_closed")), { once: true }),
        ),
      open: (subscribe, signal, on) =>
        sockets.push({ subscribe: JSON.parse(subscribe), signal, on }),
    }),
  );
  let client;
  let connection;
  try {
    client = await Client.open({ path, schema });
    await client.subscribe("scope");
    // Unsent work keeps the incompatible file open, so the rebuild happens
    // with this client - and its lane - already connected.
    await client.mutate({ name: "Create", operations: [{ model: "Entry", op: "create", identity: { id: "e" }, values: { text: "A", note: null } }] });
    await client.close();
    client = await Client.open({ path, schema: breaking });
    const reported = [];
    connection = await client.connect(
      { url: "http://unused", token: "token" },
      { onError: (error) => reported.push(error) },
    );
    // The lane opened its socket and is asleep with no timer: nothing but a
    // wake pumps it again.
    const lastPump = () => log.filter((entry) => entry.op === "downlink").at(-1);
    await eventually(
      () => sockets.length === 1 && lastPump()?.event === "next" && lastPump().actions.length === 0,
      "the lane opened its socket and went idle",
    );
    const asleep = log.length;
    await settled();
    assert.equal(log.length, asleep, "the lane sleeps until woken");
    // A refused rebuild reset nothing, so it wakes nothing.
    await assert.rejects(client.rebuild(), /unsent/);
    await settled();
    assert.ok(
      !log.slice(asleep).some((entry) => entry.op === "downlink"),
      "no lane command follows a refused rebuild",
    );
    log.splice(asleep);
    await client.rebuild({ discardPending: true });
    const woken = () =>
      log.slice(asleep).find((entry) => entry.op === "downlink" && entry.event === "next");
    await eventually(() => woken() && sockets.length === 2, "the rebuild woke the lane");
    const after = log.slice(asleep);
    assert.equal(after[0].op, "rebuild", "no lane command answers before the native rebuild");
    const pump = woken();
    assert.equal(pump.actions[0].type, "reset", "the old I/O is abandoned first");
    assert.equal(pump.actions[1]?.type, "open", "the carried Channel is subscribed again");
    assert.ok(pump.actions[1].epoch > 1, "under a fresh epoch");
    assert.ok(
      !after.some((entry) => entry.event === "start"),
      "no lane was started again",
    );
    assert.ok(sockets[0].signal.aborted, "the old socket is abandoned");
    assert.equal(sockets.length, 2);
    assert.deepEqual(sockets[1].subscribe.channels, ["scope"]);
    assert.equal(sockets[1].signal.aborted, false, "the new socket stays open");
    // The old socket still delivers a page for the carried Channel after the
    // rebuild: its epoch names no current session, so the fresh replica never
    // sees it.
    const before = await client.syncState();
    await sockets[0].on.message(
      JSON.stringify({
        cursors: { scope: { from: 0, to: 9, head: 9 } },
        changes: [
          {
            model: "Entry",
            identity: { id: "late" },
            stamp: 9,
            state: { text: "late", note: null, due: "now" },
          },
        ],
      }),
    );
    await settled();
    const later = await client.syncState();
    assert.deepEqual(later.cursors, before.cursors, "the stale page moved no cursor");
    assert.deepEqual(later.channels, before.channels);
    assert.equal(sockets.length, 2, "the stale page ended no session");
    assert.equal(sockets[1].signal.aborted, false);
    assert.deepEqual(reported, []);
  } finally {
    await connection?.close();
    await client?.close();
    await rm(directory, { recursive: true, force: true });
  }
});
