import test from "node:test";
import assert from "node:assert/strict";
import { createBackend } from "../../../packages/backend/server/index.mts";
import { persistence } from "../../../packages/backend/postgres/src/persistence.mts";
import * as SQL from "../../../packages/backend/postgres/src/sql.mts";

const backend = (database) =>
  createBackend({
    config: { schema: { enums: [], models: [], actions: [] } },
    native: {
      validateConfig() {},
      handshake05: async () => "handshake",
      settleExternal05: async () => "{}",
      negotiateLive: async () => '{"handle":1,"actions":[]}',
      liveEvent: () => "[]",
      liveClose() {},
    },
    database,
    authenticate: () => "viewer",
    loaders: {},
  });

test("listen waits for database readiness; concurrent API work shares initialization", async (t) => {
  let release;
  const pending = new Promise((resolve) => {
    release = resolve;
  });
  let checks = 0,
    transactions = 0;
  const app = backend({
    async initialize() {
      checks++;
      await pending;
    },
    async transaction(body) {
      transactions++;
      return body({});
    },
    persistence() {
      return {
        call: async () => {
          throw new Error("no host work expected");
        },
      };
    },
  });
  const listening = app.listen({ port: 0 });
  const handshake = app.handshake("viewer", "{}");
  const ready = app.ready();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(checks, 1);
  assert.equal(transactions, 0, "no request transaction before startup check");
  release();
  const server = await listening;
  t.after(() => server.close());
  await ready;
  assert.equal(await handshake, "handshake");
  await app.ready();
  await app.handshake("viewer", "{}");
  assert.equal(checks, 1);
  assert.equal(transactions, 2);
});

test("failed readiness rejects listen before binding and direct business work; a later attempt can retry", async (t) => {
  const failure = new Error("installed legacy framework layout");
  let legacy = true,
    checks = 0,
    bodies = 0;
  const app = backend({
    async initialize() {
      checks++;
      if (legacy) throw failure;
    },
    async transaction(body) {
      return body({});
    },
    persistence() {
      return {
        call: async () => {
          throw new Error("no host work expected");
        },
      };
    },
  });
  await assert.rejects(
    app.listen({ port: 0, host: "invalid:host" }),
    (e) => e === failure,
  );
  await assert.rejects(
    app.transaction(async () => {
      bodies++;
    }),
    (e) => e === failure,
  );
  assert.equal(bodies, 0);
  legacy = false;
  const server = await app.listen({ port: 0 });
  t.after(() => server.close());
  await app.transaction(async () => {
    bodies++;
  });
  assert.equal(bodies, 1);
  assert.equal(checks, 3);
});

test("caller-owned publish checks readiness using its retained transaction before the body", async () => {
  const tx = {};
  const calls = [];
  const app = backend({
    async initialize(retained) {
      assert.strictEqual(retained, tx);
      calls.push("check");
    },
    async transaction() {
      throw new Error("must not acquire another pool connection");
    },
    persistence(retained) {
      assert.strictEqual(retained, tx);
      return { call: async () => null };
    },
  });
  const wake = await app.publish(tx, async () => {
    calls.push("body");
  });
  wake();
  await app.ready();
  assert.deepEqual(calls, ["check", "body"]);
});

test("custom databases without an initialization hook keep their existing startup contract", async () => {
  let transactions = 0;
  const app = backend({
    async transaction(body) {
      transactions++;
      return body({});
    },
    persistence: () => ({ call: async () => null }),
  });
  await app.ready();
  assert.equal(transactions, 0);
  assert.equal(await app.handshake("viewer", "{}"), "handshake");
  assert.equal(transactions, 1);
});

function adapter({ legacy = false } = {}) {
  const statements = [],
    tx = {};
  let transactions = 0;
  const driver = {
    async transaction(body) {
      transactions++;
      return body(tx);
    },
    async query(retained, sql) {
      assert.strictEqual(retained, tx);
      statements.push(sql);
      if (sql === SQL.CHECK_LAYOUT) return [{ legacy }];
      if (sql === SQL.HEAD) return [{ head: "7" }];
      throw new Error(`unexpected SQL: ${sql}`);
    },
  };
  return {
    db: persistence(driver),
    tx,
    statements,
    transactions: () => transactions,
    setLegacy(value) {
      legacy = value;
    },
  };
}

test("PostgreSQL startup checks once and 100 later host operations issue no catalog checks", async () => {
  const f = adapter();
  await Promise.all([f.db.initialize(), f.db.initialize()]);
  assert.equal(f.transactions(), 1);
  for (let i = 0; i < 100; i++)
    assert.equal(
      await f.db.transaction((tx) =>
        f.db.persistence(tx).call({ op: "head", stream: "S" }),
      ),
      7,
    );
  assert.equal(
    f.statements.filter((sql) => sql === SQL.CHECK_LAYOUT).length,
    1,
  );
  assert.equal(f.statements.filter((sql) => sql === SQL.HEAD).length, 100);
});

test("PostgreSQL initialization failures preserve the error and retry; adapters do not share successful validation", async () => {
  const a = adapter(),
    b = adapter({ legacy: true });
  await a.db.initialize();
  await assert.rejects(b.db.initialize(), /legacy framework layout/);
  assert.deepEqual(b.statements, [SQL.CHECK_LAYOUT]);
  await assert.rejects(
    b.db.persistence(b.tx).call({ op: "head", stream: "S" }),
    /legacy framework layout/,
  );
  assert.equal(
    b.statements.includes(SQL.HEAD),
    false,
    "refusal precedes persistence work",
  );
  b.setLegacy(false);
  await b.db.initialize();
  assert.equal(b.statements.length, 3);
  assert.equal(a.statements.length, 1);
});

test("direct persistence fallback checks in the supplied transaction without opening another", async () => {
  const f = adapter();
  const p = f.db.persistence(f.tx);
  assert.deepEqual(
    await Promise.all([
      p.call({ op: "head", stream: "S" }),
      p.call({ op: "head", stream: "S" }),
    ]),
    [7, 7],
  );
  assert.equal(f.transactions(), 0);
  assert.deepEqual(f.statements, [SQL.CHECK_LAYOUT, SQL.HEAD, SQL.HEAD]);
});
