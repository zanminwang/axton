import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {Pool} from 'pg';
import {createBackend, devAuth} from './generated/backend.ts';
import {pg} from '../../../packages/backend/postgres/index.mts';
import {poolLifecycle} from '../support/postgres.mjs';

export const stream = 'Benchmark:entries';
export const textBytes = 512;
export const fixtureBatchSize = 100;
export const text = 'x'.repeat(textBytes);
const identity = (prefix, i) => `${prefix}-${String(i).padStart(8, '0')}`;

// Same generated SDK, backend, PostgreSQL adapter and native host as v05-sdk.
// A new database per sample keeps fixture work outside all timing intervals.
export async function startFixture(connectionString, count, diagnostics) {
  const pool = new Pool({connectionString});
  const executions = new Map(), errors = [];
  const closePool=poolLifecycle(pool,error=>errors.push(String(error)));
  let listener;
  try {
    await pool.query(await readFile(new URL('../../../packages/backend/postgres/migration.sql', import.meta.url), 'utf8'));
    await pool.query('CREATE TABLE performance_entry(id text PRIMARY KEY, text text NOT NULL)');
    await pool.query("INSERT INTO performance_entry SELECT 'seed-' || lpad(i::text,8,'0'), $2 FROM generate_series(0,$1::int-1) i", [count, text]);
    const backend = createBackend({
      native: createRequire(import.meta.url)('../../../bindings/node/axton-node.node'),
      database: pg(pool), authenticate: devAuth(), diagnostics,
      protocol5: {authorizeStream: (principal, name) => principal === 'benchmark' && name === stream},
      mutations: {createEntry: async ({ctx,args}) => {
        executions.set(args.entry.id, (executions.get(args.entry.id) ?? 0) + 1);
        assert.equal(ctx.userId, 'benchmark');
        await ctx.tx.query('INSERT INTO performance_entry VALUES($1,$2) ON CONFLICT(id) DO NOTHING', [args.entry.id,args.entry.text]);
        ctx.stream.track.entry(args.entry.id);
        return {entry:{id:args.entry.id}};
      }},
      loaders: {entry: async ({tx,ids,userId}) => {
        assert.equal(userId, 'benchmark');
        const rows = (await tx.query('SELECT id,text FROM performance_entry WHERE id=ANY($1)', [ids.map(id=>id.id)])).rows;
        const found = new Map(rows.map(row=>[row.id,row]));
        return ids.map(id=>found.get(id.id) ?? null);
      }},
      onError: error => errors.push(String(error)),
    });
    // Independent publication groups preserve incremental first-visible
    // measurement instead of forcing all N records into one atomic cursor.
    for (let first=0; first<count; first+=fixtureBatchSize) {
      await backend.transaction(async ({streams}) => {
        streams([stream]).track.entry(Array.from({length:Math.min(fixtureBatchSize,count-first)},(_,i)=>identity('seed',first+i)));
      });
    }
    listener = await backend.listen({port:0});
    return {
      url:listener.url, errors,
      async verify(writes) {
        const rows = (await pool.query('SELECT id,text FROM performance_entry ORDER BY id')).rows;
        const expected = [...Array.from({length:count},(_,i)=>identity('seed',i)),...Array.from({length:writes},(_,i)=>identity('live',i))].sort();
        assert.deepEqual(rows.map(row=>row.id), expected);
        assert.ok(rows.every(row=>row.text===text), 'server fixture content');
        assert.equal(executions.size, writes, 'handler ran for each live identity');
        // Serializable retries may re-run a handler before commit. Verify
        // its identity set and retain attempts rather than forbidding retries.
        for (const n of executions.values()) assert.ok(n>=1);
        assert.deepEqual(errors, [], 'no backend defects');
        return {rows:rows.length,writes:executions.size,handlerAttempts:Object.fromEntries(executions)};
      },
      async close() { try { await listener.close(); } finally { await closePool(); } assert.deepEqual(errors,[],'backend/connection teardown errors'); },
    };
  } catch(error) {
    try { await listener?.close(); } finally { await closePool(); }
    throw error;
  }
}
