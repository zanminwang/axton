import test from "node:test";
import assert from "node:assert/strict";
import { createClient } from "./api/runtime.mts";
import { ModelWatchReceiver } from "./api/model-watch.mts";

const complete = (requestId, value) => ({
  type: "taskCompleted",
  requestId,
  ok: true,
  value,
});
const full = (version, rows) => ({
  kind: "watch",
  protocol: "splice-v1",
  version,
  rows,
});
const patch = (baseVersion, splices) => ({
  kind: "watch",
  protocol: "splice-v1",
  baseVersion,
  version: baseVersion + 1,
  splices,
});
const splice = (index, deleteCount, rows = []) => ({
  index,
  deleteCount,
  rows,
});
const turn = () => new Promise((resolve) => setImmediate(resolve));

async function fixture({ initial, delayed = false } = {}) {
  let wake;
  const events = [],
    commands = [],
    registrations = [];
  let issued = 0;
  const publish = (...batch) => {
    events.push(...batch);
    queueMicrotask(() => wake());
  };
  const changed = (observerId, snapshot) => ({
    type: "observerChanged",
    observerId,
    snapshot,
  });
  const carrier = {
    runtimeOpen(text, notify) {
      wake = notify;
      publish(complete(JSON.parse(text).requestId, { clientId: "fake" }));
      return "1";
    },
    runtimeSubmit(_runtime, text) {
      const envelope = JSON.parse(text),
        command = envelope.command;
      if (envelope.type === "close") {
        publish(
          ...registrations.map((r) =>
            changed(r.observerId, { kind: "watch", closed: true }),
          ),
          { type: "runtimeClosed" },
        );
        return;
      }
      commands.push(command);
      if (command.kind === "watch" || command.kind === "watchSql") {
        const registered = {
          observerId: String(++issued),
          requestId: envelope.requestId,
        };
        registrations.push(registered);
        if (!delayed)
          publish(
            complete(envelope.requestId, { observerId: registered.observerId }),
            ...(initial ? [changed(registered.observerId, initial)] : []),
          );
      } else publish(complete(envelope.requestId));
    },
    runtimeDrain() {
      return JSON.stringify(events.splice(0));
    },
    runtimeDetach() {},
  };
  const Client = createClient(carrier, class {}, () => {
    throw Error("unexpected connection");
  });
  const client = await Client.open({
    path: "unused",
    schema: {},
    stream: "User:u",
  });
  return {
    client,
    commands,
    registrations,
    publish,
    emit: (snapshot, id = registrations.at(-1).observerId) =>
      publish(changed(id, snapshot)),
    register: (snapshot) => {
      const r = registrations.at(-1);
      publish(
        complete(r.requestId, { observerId: r.observerId }),
        changed(r.observerId, snapshot),
      );
    },
  };
}

test("Model watch opts into splices and retains isolated raw snapshots", async (t) => {
  const f = await fixture({
    initial: full(1, [
      { id: "a", nested: { items: [{ value: "canonical" }] } },
      { id: "b" },
      { id: "c" },
    ]),
  });
  t.after(() => f.client.close());
  const seen = [],
    errors = [];
  f.client.watch(
    "Entry",
    { active: true },
    (rows) => seen.push(rows),
    (error) => errors.push(error),
  );
  await turn();
  assert.deepEqual(f.commands[0], {
    kind: "watch",
    model: "Entry",
    spec: { filter: { active: true }, delivery: "splice-v1" },
  });
  const first = seen[0];
  first[0].nested.items[0].value = "application mutation";
  first.push({ id: "extra" });
  f.emit(
    patch(1, [
      splice(1, 1, [{ id: "d" }]),
      splice(2, 0, [{ id: "e" }]),
      splice(3, 1),
    ]),
  );
  await turn();
  assert.deepEqual(seen[1], [
    { id: "a", nested: { items: [{ value: "canonical" }] } },
    { id: "d" },
    { id: "e" },
  ]);
  assert.deepEqual(
    first.map((r) => r.id),
    ["a", "b", "c", "extra"],
  );
  assert.deepEqual(errors, []);
});

test("typed watch decodes only inserted rows and copies retained mutable values", async (t) => {
  const f = await fixture({
    initial: full(1, [
      { id: "a", at: "2026-01-01", tags: ["canonical"] },
      { id: "b", at: "2026-02-01", tags: [] },
    ]),
  });
  t.after(() => f.client.close());
  assert.equal(typeof f.client.watchDecoded, "function");
  let decoded = 0;
  const seen = [],
    errors = [];
  f.client.watchDecoded(
    "Entry",
    {},
    (row) => {
      decoded++;
      return { ...row, at: new Date(row.at) };
    },
    (value) => ({ ...value, at: new Date(value.at), tags: [...value.tags] }),
    (rows) => seen.push(rows),
    (error) => errors.push(error),
  );
  await turn();
  const first = seen[0];
  first[0].at.setUTCFullYear(1999);
  first[0].tags.push("mutated");
  f.emit(
    patch(1, [
      splice(1, 1, [{ id: "c", at: "2026-03-01", tags: [] }]),
      splice(2, 0, [{ id: "d", at: "2026-04-01", tags: [] }]),
    ]),
  );
  await turn();
  assert.equal(decoded, 4);
  assert.equal(seen[1][0].at.toISOString(), "2026-01-01T00:00:00.000Z");
  assert.deepEqual(seen[1][0].tags, ["canonical"]);
  assert.equal(first.length, 2);
  assert.equal(first[1].id, "b");
  assert.deepEqual(errors, []);
});

test("invalid patches are atomic, request recovery once, ignore patches and accept newer reset", async (t) => {
  const f = await fixture({ initial: full(1, [{ id: "a" }, { id: "b" }]) });
  t.after(() => f.client.close());
  const seen = [],
    errors = [];
  f.client.watch(
    "Entry",
    {},
    (rows) => seen.push(rows),
    (error) => errors.push(error),
  );
  await turn();
  f.emit(
    patch(1, [
      splice(0, 1, [{ id: "partial" }]),
      splice(8, 0, [{ id: "invalid" }]),
    ]),
  );
  await turn();
  assert.equal(errors.length, 1);
  assert.equal(seen.length, 1);
  assert.deepEqual(
    f.commands.filter((c) => c.kind === "watchSnapshot"),
    [{ kind: "watchSnapshot", observerId: "1" }],
  );
  f.emit(patch(1, [splice(0, 1, [{ id: "ignored" }])]));
  await turn();
  assert.equal(errors.length, 1);
  assert.equal(seen.length, 1);
  f.emit(full(5, [{ id: "a" }, { id: "b" }]));
  f.emit(patch(5, [splice(1, 1, [{ id: "recovered" }])]));
  await turn();
  assert.deepEqual(
    seen.map((rows) => rows.map((r) => r.id)),
    [
      ["a", "b"],
      ["a", "b"],
      ["a", "recovered"],
    ],
  );
});

for (const [name, message] of [
  ["patch before full", patch(1, [])],
  ["wrong base", patch(2, [])],
  ["version gap", { ...patch(1, []), version: 3 }],
  ["unsafe version", full(Number.MAX_SAFE_INTEGER + 1, [])],
  ["nonpositive version", full(0, [])],
  ["fractional version", full(1.5, [])],
  ["unknown protocol", { ...full(2, []), protocol: "future" }],
  ["negative index", patch(1, [splice(-1, 0)])],
  ["fractional index", patch(1, [splice(0.5, 0)])],
  ["negative delete count", patch(1, [splice(0, -1)])],
  ["excess delete count", patch(1, [splice(0, 2)])],
  ["invalid row", patch(1, [splice(0, 1, [null])])],
  ["ambiguous message", { ...full(2, []), splices: [] }],
])
  test(`Model watch recovers from ${name}`, async (t) => {
    const f = await fixture({
      initial:
        name === "patch before full"
          ? undefined
          : full(1, [{ id: "canonical" }]),
    });
    t.after(() => f.client.close());
    const seen = [],
      errors = [];
    f.client.watch(
      "Entry",
      {},
      (rows) => seen.push(rows),
      (e) => errors.push(e),
    );
    await turn();
    f.emit(message);
    await turn();
    assert.equal(errors.length, 1);
    assert.equal(
      f.commands.filter((c) => c.kind === "watchSnapshot").length,
      1,
    );
    f.emit(full(3, [{ id: "reset" }]));
    await turn();
    assert.deepEqual(seen.at(-1), [{ id: "reset" }]);
  });

test("decode failures do not publish a partial patch and recover from a full snapshot", async (t) => {
  const f = await fixture({ initial: full(1, [{ id: "a" }, { id: "b" }]) });
  t.after(() => f.client.close());
  assert.equal(typeof f.client.watchDecoded, "function");
  const seen = [],
    errors = [];
  f.client.watchDecoded(
    "Entry",
    {},
    (row) => {
      if (row.id === "bad") throw Error("decode failed");
      return row.id;
    },
    (value) => value,
    (rows) => seen.push(rows),
    (e) => errors.push(e),
  );
  await turn();
  f.emit(
    patch(1, [
      splice(0, 1, [{ id: "partial" }]),
      splice(1, 1, [{ id: "bad" }]),
    ]),
  );
  await turn();
  assert.equal(errors[0].message, "decode failed");
  assert.deepEqual(seen, [["a", "b"]]);
  f.emit(full(3, [{ id: "a" }, { id: "b" }]));
  f.emit(patch(3, [splice(1, 1, [{ id: "c" }])]));
  await turn();
  assert.deepEqual(seen.at(-1), ["a", "c"]);
});

test("listener failures never request snapshots and legacy full messages remain supported", async (t) => {
  const f = await fixture({ initial: { kind: "watch", rows: [{ id: "a" }] } });
  t.after(() => f.client.close());
  const seen = [],
    errors = [];
  f.client.watch(
    "Entry",
    {},
    (rows) => {
      seen.push(rows);
      if (seen.length === 1) throw Error("listener failed");
    },
    (e) => errors.push(e),
  );
  await turn();
  f.emit({ kind: "watch", rows: [{ id: "b" }] });
  f.emit(full(1, [{ id: "c" }]));
  f.emit(patch(1, [splice(0, 1, [{ id: "d" }])]));
  await turn();
  assert.deepEqual(
    seen.map((rows) => rows[0].id),
    ["a", "b", "c", "d"],
  );
  assert.equal(errors[0].message, "listener failed");
  assert.equal(f.commands.filter((c) => c.kind === "watchSnapshot").length, 0);
});

test("stop before registration and terminal close suppress delivery and recovery", async (t) => {
  const f = await fixture({ delayed: true });
  t.after(() => f.client.close());
  const seen = [],
    errors = [];
  const stop = f.client.watch(
    "Entry",
    {},
    (rows) => seen.push(rows),
    (e) => errors.push(e),
  );
  stop();
  f.register(full(1, [{ id: "late" }]));
  await turn();
  assert.deepEqual(seen, []);
  assert.equal(f.commands.filter((c) => c.kind === "unwatch").length, 1);
  f.client.watch(
    "Entry",
    {},
    (rows) => seen.push(rows),
    (e) => errors.push(e),
  );
  f.register(full(1, [{ id: "a" }]));
  await turn();
  f.emit({ kind: "watch", closed: true });
  await turn();
  f.emit(patch(9, []));
  await turn();
  assert.deepEqual(seen, [[{ id: "a" }]]);
  assert.deepEqual(errors, []);
  assert.equal(f.commands.filter((c) => c.kind === "watchSnapshot").length, 0);
});

test("watchSql keeps legacy full snapshot delivery", async (t) => {
  const f = await fixture({ initial: { kind: "watch", rows: [{ n: 1 }] } });
  t.after(() => f.client.close());
  const seen = [];
  f.client.watchSql("SELECT count(*) n FROM Entry", [], (rows) =>
    seen.push(rows),
  );
  await turn();
  f.emit({ kind: "watch", rows: [{ n: 2 }] });
  await turn();
  assert.deepEqual(f.commands[0], {
    kind: "watchSql",
    sql: "SELECT count(*) n FROM Entry",
    parameters: [],
  });
  assert.deepEqual(seen, [[{ n: 1 }], [{ n: 2 }]]);
});

for (const failing of ["decode", "copy"])
  test(`persistent ${failing} failure reports recovery failure without a snapshot loop`, async (t) => {
    const f = await fixture({ initial: full(1, [{ id: "a" }]) });
    t.after(() => f.client.close());
    let broken = true;
    const seen = [],
      errors = [];
    f.client.watchDecoded(
      "Entry",
      {},
      (row) => {
        if (broken && failing === "decode") throw Error("decode failed");
        return row.id;
      },
      (value) => {
        if (broken && failing === "copy") throw Error("copy failed");
        return value;
      },
      (rows) => seen.push(rows),
      (e) => errors.push(e),
    );
    await turn();
    f.emit(full(2, [{ id: "a" }]));
    f.emit(patch(2, [splice(0, 1, [{ id: "ignored" }])]));
    await turn();
    assert.equal(errors.length, 2);
    assert.deepEqual(seen, []);
    assert.equal(
      f.commands.filter((c) => c.kind === "watchSnapshot").length,
      1,
    );
    broken = false;
    f.emit(full(3, [{ id: "reset" }]));
    f.emit(patch(3, [splice(0, 1, [{ id: "resumed" }])]));
    await turn();
    assert.deepEqual(seen, [["reset"], ["resumed"]]);
  });

test("receiver rejection retains canonical cache and version before recovery starts", () => {
  const receiver = new ModelWatchReceiver(
    (row) => {
      if (row.id === "bad") throw Error("decode failed");
      return row.id;
    },
    (value) => value,
  );
  const first = receiver.receive(full(1, [{ id: "a" }, { id: "b" }]));
  assert.throws(() =>
    receiver.receive(
      patch(1, [splice(0, 1, [{ id: "partial" }]), splice(5, 0)]),
    ),
  );
  assert.throws(() =>
    receiver.receive(
      patch(1, [
        splice(0, 1, [{ id: "partial" }]),
        splice(1, 1, [{ id: "bad" }]),
      ]),
    ),
  );
  assert.throws(() => receiver.receive(full(1, [{ id: "stale" }])));
  assert.deepEqual(receiver.receive(patch(1, [splice(1, 1, [{ id: "c" }])])), [
    "a",
    "c",
  ]);
  assert.deepEqual(first, ["a", "b"]);
});

test("receiver replays multirow inserts without JS call argument limits", () => {
  let decoded = 0;
  const receiver = new ModelWatchReceiver(
    (row) => {
      decoded++;
      return row.id;
    },
    (value) => value,
  );
  receiver.receive(full(1, [{ id: "start" }, { id: "end" }]));
  const inserted = Array.from({ length: 150000 }, (_, index) => ({
    id: String(index),
  }));
  const delivered = receiver.receive(
    patch(1, [splice(1, 0, inserted), splice(150001, 1)]),
  );
  assert.equal(delivered.length, 150001);
  assert.equal(delivered[0], "start");
  assert.equal(delivered.at(-1), "149999");
  assert.equal(decoded, 150002);
});
