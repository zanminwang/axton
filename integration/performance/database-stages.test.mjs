import test from "node:test";
import assert from "node:assert/strict";
import { Diagnostics } from "../../packages/backend/server/api/diagnostics.mts";
import { pg } from "../../packages/backend/postgres/index.mts";
import * as SQL from "../../packages/backend/postgres/src/sql.mts";

test("database phase spans retain results, source parents and SQL privacy", async () => {
  const records = [],
    commands = [];
  const diagnostics = new Diagnostics({ record: (r) => records.push(r) });
  const tx = {
    async query(sql) {
      commands.push(sql);
      return {
        rows: sql === SQL.CHECK_LAYOUT ? [{ legacy: false }] : [{ head: "7" }],
      };
    },
    release() {},
  };
  const db = pg({
    async connect() {
      return tx;
    },
  });
  await db.initialize();
  const result = await diagnostics.span("operation", {}, () =>
    db.transaction(
      (retained) =>
        db
          .persistence(retained, diagnostics)
          .call({ op: "head", stream: "private-stream" }),
      diagnostics,
    ),
  );
  assert.equal(result, 7);
  for (const name of [
    "postgres.acquire",
    "postgres.transaction.body",
    "postgres.query",
  ])
    assert.equal(records.filter((r) => r.name === name).length, 1, name);
  assert.deepEqual(
    records
      .filter((r) => ["postgres.begin", "postgres.commit"].includes(r.name))
      .map((r) => r.attributes.command),
    ["BEGIN", "COMMIT"],
  );
  const query = records.find((r) => r.name === "postgres.query"),
    body = records.find((r) => r.name === "postgres.transaction.body");
  assert.equal(query.parentSpanId, body.spanId);
  assert.equal(JSON.stringify(records).includes("private-stream"), false);
  assert.equal(JSON.stringify(records).includes(SQL.HEAD), false);
  assert.equal(commands.filter((s) => s === SQL.CHECK_LAYOUT).length, 1);
});

test("database phase diagnostics preserve rollback and a throwing receiver cannot replace failures", async () => {
  const failure = new Error("business failure"),
    commands = [];
  const db = pg({
    async connect() {
      return {
        async query(sql) {
          commands.push(sql);
          return { rows: [{ legacy: false }] };
        },
        release() {},
      };
    },
  });
  await db.initialize();
  const diagnostics = new Diagnostics({
    record() {
      throw new Error("export failed");
    },
  });
  await assert.rejects(
    db.transaction(async () => {
      throw failure;
    }, diagnostics),
    (e) => e === failure,
  );
  assert.equal(commands.at(-1), "ROLLBACK");
});
