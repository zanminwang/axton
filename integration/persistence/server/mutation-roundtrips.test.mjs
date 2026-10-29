// Real PostgreSQL guards, atomic state transitions and query budgets. The
// initialized adapter is measured; startup checks and BEGIN/COMMIT are separate.
import test, { after } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { Pool } from "pg";
import { persistence } from "../../../packages/backend/postgres/src/persistence.mts";
import { pgDriver } from "../../../packages/backend/postgres/src/pg.mts";
import * as SQL from "../../../packages/backend/postgres/src/sql.mts";

const connectionString = process.env.DATABASE_URL;
const admin = new Pool({ connectionString });
const ddl = await readFile(
  new URL("../../../packages/backend/postgres/migration.sql", import.meta.url),
  "utf8",
);
after(() => admin.end());
let serial = 0;
async function fixture(body) {
  const schema = `roundtrips_${process.pid}_${++serial}`;
  await admin.query(`CREATE SCHEMA ${schema}`);
  const pool = new Pool({
    connectionString,
    options: `-c search_path=${schema}`,
  });
  try {
    await pool.query(ddl);
    const base = pgDriver(pool, { retries: 10 });
    const statements = [];
    let afterQuery = async () => {};
    const database = persistence({
      ...base,
      query: async (tx, sql, params) => {
        statements.push(sql);
        const rows = await base.query(tx, sql, params);
        await afterQuery(sql, rows);
        return rows;
      },
    });
    await database.initialize();
    statements.length = 0;
    const call = (tx, op, fields) =>
      database
        .persistence(tx)
        .call({ op: "protocol05", request: { op, ...fields } });
    const run = (op, fields) =>
      database.transaction((tx) => call(tx, op, fields));
    await body({
      pool,
      database,
      statements,
      call,
      run,
      onQuery: (hook) => {
        afterQuery = hook;
      },
    });
  } finally {
    await pool.end();
    await admin.query(`DROP SCHEMA ${schema} CASCADE`);
  }
}
const claim = { storeId: "s", principal: "alice", stream: "User:alice" };
const begin = (batchId = 1, count = 1) => ({
  storeId: "s",
  batchId,
  digest: "a".repeat(64),
  count,
});
const result = (mutationId = 1) => ({
  mutationId,
  outcome: { kind: "rejected", code: "test.refused", message: null },
});
const save = (
  ordinal = 0,
  count = 1,
  mutationId = ordinal + 1,
  batchId = 1,
) => ({
  storeId: "s",
  batchId,
  ordinal,
  count,
  result: result(mutationId),
});
const state = async (pool) => ({
  stores: (await pool.query("SELECT * FROM axton_store ORDER BY id")).rows,
  results: (
    await pool.query(
      "SELECT * FROM axton_mutation_result ORDER BY store_id,batch_id,ordinal",
    )
  ).rows,
});

test("existing Store claims use one locking query and retain the immutable binding", () =>
  fixture(async ({ run, statements, pool }) => {
    const initial = await run("claimStore", claim);
    assert.equal(
      statements.length,
      3,
      "first claim keeps insert/conflict recovery",
    );
    statements.length = 0;
    const existing = await run("claimStore", {
      ...claim,
      principal: "mallory",
      stream: "User:mallory",
    });
    assert.deepEqual(
      existing,
      initial,
      "the executor must check the retained owner/Stream",
    );
    assert.deepEqual(statements, [SQL.V05_STORE_LOCK]);
    assert.equal((await state(pool)).stores.length, 1);
  }));

test(
  "concurrent first claims converge on one immutable binding after Serializable retry",
  { timeout: 15000 },
  () =>
    fixture(async ({ run, pool, onQuery }) => {
      let missing = 0,
        release;
      const bothMissing = new Promise((resolve) => {
        release = resolve;
      });
      onQuery(async (sql, rows) => {
        if (sql === SQL.V05_STORE_LOCK && rows.length === 0 && missing < 2) {
          if (++missing === 2) release();
          await bothMissing;
        }
      });
      const claims = await Promise.all([
        run("claimStore", claim),
        run("claimStore", { ...claim, principal: "bob", stream: "User:bob" }),
      ]);
      assert.equal(
        missing,
        2,
        "both transactions establish a snapshot before creation",
      );
      assert.deepEqual(claims[0], claims[1]);
      const rows = (await state(pool)).stores;
      assert.equal(rows.length, 1);
      assert.equal(rows[0].principal, claims[0].principal);
      assert.equal(rows[0].stream, claims[0].stream);
    }),
);

test("batch admission and result persistence each use one query, including final promotion", () =>
  fixture(async ({ run, statements, pool }) => {
    await run("claimStore", claim);
    statements.length = 0;
    await run("beginBatch", begin(1, 2));
    assert.equal(statements.length, 1);
    statements.length = 0;
    await run("saveResult", save(0, 2));
    assert.equal(statements.length, 1);
    let snapshot = await state(pool);
    assert.equal(snapshot.stores[0].progress, "1");
    assert.equal(snapshot.stores[0].last_processed_batch_id, "0");
    assert.deepEqual(snapshot.results[0].result, result(1));
    statements.length = 0;
    await run("saveResult", save(1, 2));
    assert.equal(statements.length, 1);
    snapshot = await state(pool);
    assert.equal(snapshot.stores[0].progress, "0");
    assert.equal(snapshot.stores[0].last_processed_batch_id, "1");
    assert.equal(snapshot.stores[0].current_digest, null);
    assert.equal(snapshot.stores[0].current_count, null);
    assert.equal(snapshot.stores[0].last_digest, begin().digest);
    assert.equal(snapshot.stores[0].last_count, "2");
    assert.deepEqual(
      snapshot.results.map((row) => row.result),
      [result(1), result(2)],
    );
    statements.length = 0;
    await run("beginBatch", begin(2));
    assert.equal(statements.length, 1);
    assert.deepEqual(
      (await state(pool)).results,
      [],
      "next batch prunes older outcomes",
    );
  }));

test("invalid batch admission neither prunes saved outcomes nor changes progress", () =>
  fixture(async ({ run, pool }) => {
    await run("claimStore", claim);
    await run("beginBatch", begin());
    await run("saveResult", save());
    const before = await state(pool);
    await assert.rejects(
      run("beginBatch", begin(3)),
      /Batch admission state changed/,
    );
    assert.deepEqual(await state(pool), before);
  }));

test("invalid ordinal/count does not save an outcome even before caller rollback", () =>
  fixture(async ({ run, pool, database, call }) => {
    await run("claimStore", claim);
    await run("beginBatch", begin(1, 2));
    const before = await state(pool);
    await database.transaction(async (tx) => {
      await assert.rejects(
        call(tx, "saveResult", save(1, 2)),
        /Batch progress state changed/,
      );
      await assert.rejects(
        call(tx, "saveResult", save(0, 3)),
        /Batch progress state changed/,
      );
      assert.deepEqual(
        (await tx.query("SELECT * FROM axton_mutation_result")).rows,
        [],
      );
    });
    assert.deepEqual(await state(pool), before);
  }));

test("result INSERT failure rolls back progress to the caller's savepoint", () =>
  fixture(async ({ run, pool, database, call }) => {
    await run("claimStore", claim);
    await run("beginBatch", begin(1, 2));
    await run("saveResult", save(0, 2));
    const before = await state(pool);
    await database.transaction(async (tx) => {
      await tx.query("SAVEPOINT caller");
      await assert.rejects(call(tx, "saveResult", save(1, 2, 1)), {
        code: "23505",
      });
      await tx.query("ROLLBACK TO SAVEPOINT caller");
      assert.equal(
        (await tx.query("SELECT progress FROM axton_store")).rows[0].progress,
        "1",
      );
    });
    assert.deepEqual(await state(pool), before);
  }));

test("pruning failure rolls back admission and preserves the preceding batch", () =>
  fixture(async ({ run, pool }) => {
    await run("claimStore", claim);
    await run("beginBatch", begin());
    await run("saveResult", save());
    const before = await state(pool);
    await pool.query(`
      CREATE FUNCTION refuse_prune() RETURNS trigger LANGUAGE plpgsql AS $$
      BEGIN RAISE EXCEPTION 'prune failed'; END $$;
      CREATE TRIGGER refuse_prune BEFORE DELETE ON axton_mutation_result
      FOR EACH ROW EXECUTE FUNCTION refuse_prune()`);
    await assert.rejects(run("beginBatch", begin(2)), /prune failed/);
    assert.deepEqual(await state(pool), before);
  }));

test("outer transaction rollback discards business writes, result and progress together", () =>
  fixture(async ({ run, pool, database, call }) => {
    await pool.query("CREATE TABLE business(id text PRIMARY KEY)");
    await run("claimStore", claim);
    await run("beginBatch", begin());
    const before = await state(pool);
    await assert.rejects(
      database.transaction(async (tx) => {
        await tx.query("INSERT INTO business VALUES('x')");
        await call(tx, "saveResult", save());
        throw new Error("abort after result");
      }),
      /abort after result/,
    );
    assert.deepEqual(await state(pool), before);
    assert.deepEqual((await pool.query("SELECT * FROM business")).rows, []);
  }));

const saveGroup = (ordinal, results, count = results.length) => ({
  storeId: "s",
  batchId: 1,
  ordinal,
  count,
  digest: begin().digest,
  results,
});
test("group outcomes use one guarded write and promote the complete Batch atomically", () =>
  fixture(async ({ run, statements, pool }) => {
    await run("claimStore", claim);
    await run("beginBatch", begin(1, 3));
    statements.length = 0;
    await run("saveResults", saveGroup(0, [result(1), result(2)], 3));
    assert.deepEqual(statements, [SQL.V05_RESULTS_SAVE]);
    let snapshot = await state(pool);
    assert.equal(snapshot.stores[0].progress, "2");
    assert.equal(snapshot.results.length, 2);
    statements.length = 0;
    await run("saveResults", saveGroup(2, [result(3)], 3));
    assert.deepEqual(statements, [SQL.V05_RESULTS_SAVE]);
    snapshot = await state(pool);
    assert.equal(snapshot.stores[0].progress, "0");
    assert.equal(snapshot.stores[0].last_processed_batch_id, "1");
    assert.equal(snapshot.stores[0].current_digest, null);
    assert.equal(snapshot.stores[0].last_digest, begin().digest);
    assert.deepEqual(
      snapshot.results.map((x) => x.result),
      [result(1), result(2), result(3)],
    );
  }));
test("group progress or digest mismatch writes no partial outcomes", () =>
  fixture(async ({ run, pool }) => {
    await run("claimStore", claim);
    await run("beginBatch", begin(1, 3));
    const before = await state(pool);
    for (const request of [
      saveGroup(1, [result(2)], 3),
      { ...saveGroup(0, [result(1)], 3), digest: "b".repeat(64) },
    ]) {
      await assert.rejects(
        run("saveResults", request),
        /Batch progress state changed/,
      );
      assert.deepEqual(await state(pool), before);
    }
    await assert.rejects(
      run("saveResults", saveGroup(0, [], 3)),
      /Invalid result group size/,
    );
  }));
test("group result insertion failure rolls back every outcome and progress", () =>
  fixture(async ({ run, pool, database, call }) => {
    await run("claimStore", claim);
    await run("beginBatch", begin(1, 3));
    const before = await state(pool);
    await database.transaction(async (tx) => {
      await tx.query("SAVEPOINT caller");
      await assert.rejects(
        call(tx, "saveResults", saveGroup(0, [result(1), result(1)], 3)),
        { code: "23505" },
      );
      await tx.query("ROLLBACK TO SAVEPOINT caller");
      assert.equal(
        (await tx.query("SELECT progress FROM axton_store")).rows[0].progress,
        "0",
      );
      assert.equal(
        (await tx.query("SELECT * FROM axton_mutation_result")).rows.length,
        0,
      );
    });
    assert.deepEqual(await state(pool), before);
  }));
