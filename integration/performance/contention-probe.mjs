// Bounded real-PostgreSQL experiment, not a latency benchmark or production fix.
// Observe the blocked waiter before releasing its writer; no timing threshold.
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {randomBytes} from 'node:crypto';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {Pool} from 'pg';
import * as SQL from '../../packages/backend/postgres/src/sql.mts';
import {poolLifecycle} from './support/postgres.mjs';

const url=new URL(process.env.DATABASE_URL);
assert.equal(url.hostname,'127.0.0.1','disposable localhost PostgreSQL only');
const pool=new Pool({connectionString:url.href,max:4});
const errors=[];
const closePool=poolLifecycle(pool,e=>errors.push(String(e)));
const admin=await pool.connect();
const output={schemaVersion:1,kind:'snapshot-contention-probe',
  commit:execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim(),
  dirty:execFileSync('git',['status','--porcelain'],{encoding:'utf8'}).trim().length>0,
  postgres:(await admin.query('SELECT version() AS version')).rows[0].version,
  cases:[],errors};
const lock='LOCK TABLE axton_publication_fence IN SHARE ROW EXCLUSIVE MODE';
async function blocked(pid) {
  for(let i=0;i<400;i++) {
    const row=(await admin.query('SELECT wait_event_type FROM pg_stat_activity WHERE pid=$1',[pid])).rows[0];
    if(row?.wait_event_type==='Lock')return;
    await new Promise(resolve=>setTimeout(resolve,5));
  }
  assert.fail('waiter did not reach a database lock');
}
async function claim(client,id) {
  await client.query(SQL.V05_STORE_INSERT,[id,'benchmark','Benchmark:entries']);
  await client.query(SQL.V05_STORE_LOCK,[id]);
}
async function experiment(mode) {
  const schema='contention_'+randomBytes(6).toString('hex');
  const writer=await pool.connect(),waiter=await pool.connect();
  const sample={mode,writer:'not-started',waiter:'not-started',blocked:false};
  output.cases.push(sample);
  let pending;
  try {
    await admin.query(`CREATE SCHEMA ${schema}`);
    for(const client of [admin,writer,waiter])await client.query(`SET search_path TO ${schema}`);
    await admin.query(await readFile(new URL('../../packages/backend/postgres/migration.sql',import.meta.url),'utf8'));
    await admin.query("INSERT INTO axton_store(id,principal,stream) VALUES('a','benchmark','Benchmark:entries'),('b','benchmark','Benchmark:entries')");
    await admin.query('CREATE TABLE business_counter(value integer NOT NULL); INSERT INTO business_counter VALUES(0)');
    for(const client of [writer,waiter]) {
      await client.query("SET statement_timeout='5s'");
      await client.query('BEGIN ISOLATION LEVEL SERIALIZABLE');
    }
    if(mode==='lock-before-snapshot')await writer.query(lock);
    await claim(writer,'a');
    await writer.query(SQL.PUBLICATION_FENCE);
    await writer.query('UPDATE business_counter SET value=value+1');
    sample.writer='holding-fence';
    let phase='start';
    pending=(async()=>{
      try {
        if(mode==='snapshot-before-lock') {
          phase='freeze-snapshot';
          sample.staleRead=(await waiter.query('SELECT value FROM business_counter')).rows[0].value;
        }
        if(mode!=='current-row-fence') {phase='table-lock';await waiter.query(lock);}
        phase='claim-store';await claim(waiter,'b');
        phase='publication-fence';await waiter.query(SQL.PUBLICATION_FENCE);
        phase='read-business';
        sample.waiterRead=(await waiter.query('SELECT value FROM business_counter')).rows[0].value;
        await waiter.query('UPDATE business_counter SET value=value+1');
        phase='commit';await waiter.query('COMMIT');sample.waiter='committed';
      } catch(error) {
        sample.waiter='aborted';sample.error={phase,code:error.code,message:error.message};
        await waiter.query('ROLLBACK');
      }
    })();
    await blocked(waiter.processID);sample.blocked=true;
    // A normal reader remains available while a publishing transaction holds
    // either fence. It sees only the last committed business value.
    sample.concurrentRead=(await admin.query('SELECT value FROM business_counter')).rows[0].value;
    assert.equal(sample.concurrentRead,0);
    await writer.query('COMMIT');sample.writer='committed';
    await pending;
    sample.finalValue=(await admin.query('SELECT value FROM business_counter')).rows[0].value;
    if(mode==='lock-before-snapshot') {
      assert.equal(sample.waiter,'committed');
      assert.equal(sample.waiterRead,1,'waiter snapshot includes the preceding committed publication');
      assert.equal(sample.finalValue,2);
    } else {
      assert.equal(sample.waiter,'aborted');assert.equal(sample.error.code,'40001');
      assert.equal(sample.finalValue,1,'aborted transaction leaves no partial business work');
    }
    sample.verified=true;
  } finally {
    for(const client of [writer,waiter])await client.query('ROLLBACK').catch(()=>{});
    await pending;
    for(const client of [writer,waiter])client.release();
    await admin.query('SET search_path TO public');
    await admin.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);
  }
}
try {
  for(const mode of ['current-row-fence','lock-before-snapshot','snapshot-before-lock'])await experiment(mode);
  output.status='passed';
} catch(error) {output.status='failed';output.error=String(error);process.exitCode=1;}
finally {
  admin.release();await closePool();
  if(errors.length){output.status='failed';process.exitCode=1;}
  await mkdir('artifacts/performance',{recursive:true});
  await writeFile('artifacts/performance/contention-probe.json',JSON.stringify(output,null,2)+'\n');
}
console.log(JSON.stringify(output,null,2));
