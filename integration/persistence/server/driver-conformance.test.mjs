// Every PostgreSQL shim (`pg`, `prisma`, `drizzle`) must give AXTON the same
// persistence behavior through the two-method driver. The assertions read the
// database through a separate `pg` pool so they do not depend on the tool
// under test; business writes inside handlers go through `driver.query`, which
// is what makes the same suite run unchanged against every shim.
import test,{before,after} from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {Pool} from 'pg';
import {drizzle as drizzleOrm} from 'drizzle-orm/node-postgres';
import {createBackend} from '../../../packages/server/index.mts';
import {pg,prisma,drizzle,answer,pgDriver} from '../../../packages/postgres/index.mts';
const require=createRequire(import.meta.url);
const {PrismaClient}=require('../../bindings/node/generated/client');
const url=process.env.DATABASE_URL;
const check=new Pool({connectionString:url});
const q=async(sql,params=[])=>(await check.query(sql,params)).rows;
const schema={enums:[],models:[{name:'Task',identity:['id'],fields:[{name:'id',type:{kind:'scalar',name:'string'},nullable:false},{name:'title',type:{kind:'scalar',name:'string'},nullable:false}]}]};
const config={schema,mutations:[{name:'edit',version:1,slots:[{name:'task',model:'Task',operation:'update',cardinality:'single',allowedPatchFields:['title']}]}]};
const authenticate=async req=>req.headers.authorization==='Bearer alice'?'alice':null;
const key=id=>JSON.stringify({id});
before(async()=>{for(const sql of (await readFile(new URL('../../../packages/postgres/migration.sql',import.meta.url),'utf8')).split(';').map(x=>x.trim()).filter(Boolean))await q(sql);await q('CREATE TABLE IF NOT EXISTS conformance_task(id text PRIMARY KEY,title text NOT NULL)');});
after(()=>check.end());

test('pg: a connection whose ROLLBACK fails is released as broken, a healthy one is released for reuse',async()=>{
 const releases=[];
 const fakePool=(failRollback)=>({connect:async()=>({
  query:async sql=>{if(sql==='ROLLBACK'&&failRollback)throw new Error('connection lost');return {rows:[]};},
  release:arg=>releases.push(arg),
 })});
 await assert.rejects(()=>pgDriver(fakePool(true)).transaction(async()=>{throw new Error('body failed');}),/body failed/);
 assert.ok(releases.at(-1) instanceof Error,'the broken connection is discarded');
 await assert.rejects(()=>pgDriver(fakePool(false)).transaction(async()=>{throw new Error('body failed');}),/body failed/);
 assert.equal(releases.at(-1),undefined,'a rolled-back connection goes back to the pool');
 assert.equal(await pgDriver(fakePool(false)).transaction(async()=>7),7);
 assert.equal(releases.at(-1),undefined);
});

const shims=[];
{const pool=new Pool({connectionString:url});shims.push({name:'pg',database:pg(pool),close:()=>pool.end()});}
{const client=new PrismaClient();shims.push({name:'prisma',database:prisma(client),close:()=>client.$disconnect()});}
{const pool=new Pool({connectionString:url});shims.push({name:'drizzle',database:drizzle(drizzleOrm(pool)),close:()=>pool.end()});}

for(const shim of shims){
 const {database}=shim;const {driver}=database;
 const inTx=body=>driver.transaction(tx=>body(tx,(sql,params=[])=>driver.query(tx,sql,params),r=>answer(driver,tx,r)));
 const p=name=>`${shim.name}-${name}`;
 test(`[${shim.name}] a push writes business rows and AXTON metadata in one transaction and a pull reads them back`,async()=>{
  const backend=createBackend({config,database,authenticate,handlers:{async edit({input,tx,publish}){await driver.query(tx,'INSERT INTO conformance_task(id,title) VALUES($1,$2) ON CONFLICT(id) DO UPDATE SET title=$2',[input.task.identity.id,input.task.patch.title]);publish({channel:p('shared')});}},loaders:{async task({ids,tx}){const rows=await driver.query(tx,'SELECT id,title FROM conformance_task WHERE id = ANY($1)',[ids.map(i=>i.id)]);return ids.map(i=>{const r=rows.find(r=>r.id===i.id);return r?{title:r.title}:null;});}}});
  const receipt=JSON.parse(await backend.push('alice',JSON.stringify({clientId:p('c'),batchSequence:1,models:{Task:1},mutations:[{ordinal:1,name:'edit',operations:[{model:'Task',op:'update',identity:{id:p('t')},values:{title:'typed'}}]}]})));
  assert.deepEqual(receipt.records,[{identity:{id:p('t')},model:'Task',stamp:1,state:{title:'typed'}}]);
  assert.deepEqual(await q('SELECT title FROM conformance_task WHERE id=$1',[p('t')]),[{title:'typed'}]);
  assert.equal(Number((await q('SELECT sequence FROM axton_client WHERE client_id=$1',[p('c')]))[0].sequence),1);
  const page=JSON.parse(await backend.pull('alice',JSON.stringify({cursors:{[p('shared')]:0},models:{Task:1}})));
  assert.equal(page.changes.length,1);assert.deepEqual(page.changes[0].state,{title:'typed'});assert.equal(page.changes[0].stamp,1);
  assert.equal(await backend.push('alice',JSON.stringify({clientId:p('c'),batchSequence:1,models:{Task:1},mutations:[{ordinal:1,name:'edit',operations:[]}]})),JSON.stringify(receipt),'a retry answers from the stored receipt');
 });
 test(`[${shim.name}] claim creates and locks the client row; saveReceipt refuses another owner; head of an unknown channel is 0`,async()=>{
  const claimed=await inTx((tx,_,a)=>a({op:'claim',owner:'alice',clientId:p('claim')}));
  assert.deepEqual(claimed,{clientId:p('claim'),owner:'alice',sequence:0,receipt:null});
  await assert.rejects(()=>inTx((tx,_,a)=>a({op:'saveReceipt',owner:'bob',clientId:p('claim'),sequence:1,receipt:'{}'})),/Receipt owner mismatch/);
  await inTx((tx,_,a)=>a({op:'saveReceipt',owner:'alice',clientId:p('claim'),sequence:2**40,receipt:'{"big":true}'}));
  const again=await inTx((tx,_,a)=>a({op:'claim',owner:'alice',clientId:p('claim')}));
  assert.deepEqual(again,{clientId:p('claim'),owner:'alice',sequence:2**40,receipt:'{"big":true}'},'bigint counters round-trip beyond 32 bits');
  assert.equal(await inTx((tx,_,a)=>a({op:'head',channel:p('nowhere')})),0);
 });
 test(`[${shim.name}] advanceStamp increments without a channel; ensureStamp initialises once and keeps an advanced stamp`,async()=>{
  const ref={model:'Task',identityKey:key(p('stamp'))};
  assert.deepEqual(await inTx(async(tx,_,a)=>[await a({op:'advanceStamp',...ref}),await a({op:'advanceStamp',...ref})]),[1,2]);
  assert.deepEqual(await q('SELECT stamp::int AS stamp FROM axton_record WHERE identity_key=$1',[ref.identityKey]),[{stamp:2}]);
  assert.deepEqual(await q('SELECT * FROM axton_invalidation WHERE identity_key=$1',[ref.identityKey]),[]);
  const race={model:'Task',identityKey:key(p('race'))};
  const ensure=()=>inTx((tx,_,a)=>a({op:'ensureStamp',...race}));
  assert.deepEqual(await Promise.all([ensure(),ensure(),ensure()]),[1,1,1],'concurrent first publications agree on 1 (serialization retries)');
  assert.equal((await q('SELECT * FROM axton_record WHERE identity_key=$1',[race.identityKey])).length,1);
  await inTx((tx,_,a)=>a({op:'advanceStamp',...race}));
  assert.equal(await ensure(),2);
 });
 test(`[${shim.name}] publish allocates only the channel cursor at the record's current stamp and refuses a missing or stale stamp`,async()=>{
  const id=p('pub');const ref={model:'Task',identity:{id},identityKey:key(id)};
  await assert.rejects(()=>inTx((tx,_,a)=>a({op:'publish',channel:p('ch'),...ref,stamp:1})),/Record metadata missing/);
  const published=await inTx(async(tx,_,a)=>{const stamp=await a({op:'ensureStamp',model:'Task',identityKey:ref.identityKey});return a({op:'publish',channel:p('ch'),...ref,stamp});});
  assert.deepEqual(published,{cursor:1,stamp:1});
  await assert.rejects(()=>inTx((tx,_,a)=>a({op:'publish',channel:p('ch'),...ref,stamp:5})),/names stamp 5 .* is at stamp 1/);
  assert.deepEqual(await q('SELECT channel,cursor::int AS cursor,stamp::int AS stamp,identity FROM axton_invalidation WHERE identity_key=$1',[ref.identityKey]),[{channel:p('ch'),cursor:1,stamp:1,identity:{id}}]);
  assert.equal(await inTx((tx,_,a)=>a({op:'head',channel:p('ch')})),1);
 });
 test(`[${shim.name}] a thrown body rolls back a first initialisation together with its publication`,async()=>{
  const id=p('undone');
  await assert.rejects(()=>inTx(async(tx,_,a)=>{const stamp=await a({op:'ensureStamp',model:'Task',identityKey:key(id)});await a({op:'publish',channel:p('undone'),model:'Task',identity:{id},identityKey:key(id),stamp});throw new Error('cancel');}),/cancel/);
  assert.deepEqual(await q('SELECT * FROM axton_record WHERE identity_key=$1',[key(id)]),[]);
  assert.deepEqual(await q('SELECT * FROM axton_invalidation WHERE identity_key=$1',[key(id)]),[]);
  assert.deepEqual(await q('SELECT * FROM axton_channel WHERE channel=$1',[p('undone')]),[]);
 });
 test(`[${shim.name}] scan pairs the invalidation cursor with the current record stamp and reports missing metadata`,async()=>{
  const id=p('scan');
  await inTx(async(tx,_,a)=>{const stamp=await a({op:'ensureStamp',model:'Task',identityKey:key(id)});await a({op:'publish',channel:p('scan'),model:'Task',identity:{id},identityKey:key(id),stamp});await a({op:'advanceStamp',model:'Task',identityKey:key(id)});});
  const rows=await inTx((tx,_,a)=>a({op:'scan',channel:p('scan'),after:0,limit:50}));
  assert.deepEqual(rows,[{channel:p('scan'),cursor:1,model:'Task',identityKey:key(id),identity:{id},stamp:2}]);
  await q('DELETE FROM axton_record WHERE identity_key=$1',[key(id)]);
  await assert.rejects(()=>inTx((tx,_,a)=>a({op:'scan',channel:p('scan'),after:0,limit:50})),/Record metadata missing/);
 });
 test(`[${shim.name}] savepoints isolate one mutation's writes and the transaction continues after a rollback`,async()=>{
  const id=p('sp');
  await inTx(async(tx,query,a)=>{
   await query('INSERT INTO conformance_task(id,title) VALUES($1,$2)',[id+'-kept','kept']);
   await a({op:'savepoint',ordinal:1});
   await query('INSERT INTO conformance_task(id,title) VALUES($1,$2)',[id+'-undone','undone']);
   await assert.rejects(()=>query('INSERT INTO conformance_task(id,title) VALUES($1,$2)',[id+'-undone','duplicate']));
   await a({op:'rollback',ordinal:1});
   await a({op:'release',ordinal:1});
   await query('INSERT INTO conformance_task(id,title) VALUES($1,$2)',[id+'-after','after']);
  });
  assert.deepEqual((await q('SELECT id FROM conformance_task WHERE id LIKE $1 ORDER BY id',[id+'-%'])).map(r=>r.id),[id+'-after',id+'-kept']);
  await assert.rejects(()=>inTx((tx,_,a)=>a({op:'savepoint',ordinal:0})),/Invalid savepoint ordinal/);
 });
 test(`[${shim.name}] a serialization conflict retries the whole body and commits once; with no retries it is reported`,async()=>{
  const channel=p('serial');
  await q("INSERT INTO axton_channel(channel,head) VALUES($1,0) ON CONFLICT(channel) DO UPDATE SET head=0",[channel]);
  let bodies=0;let entered,release;const inside=new Promise(r=>{entered=r;});const gate=new Promise(r=>{release=r;});
  const first=driver.transaction(async tx=>{bodies++;const [{head}]=await driver.query(tx,'SELECT head FROM axton_channel WHERE channel=$1',[channel]);if(bodies===1){entered();await gate;}await driver.query(tx,'UPDATE axton_channel SET head=head+1 WHERE channel=$1',[channel]);return Number(head);});
  await inside;await q('UPDATE axton_channel SET head=head+10 WHERE channel=$1',[channel]);release();
  assert.equal(await first,10);assert.equal(bodies,2);
  assert.equal(Number((await q('SELECT head FROM axton_channel WHERE channel=$1',[channel]))[0].head),11);
 });
 test(`[${shim.name}] close`,async()=>{await shim.close();});
}

test('the pg driver retries only serialization failures, a bounded number of times',async()=>{
 const attempts=[];const failing=codes=>({async connect(){return {async query(sql){if(sql==='COMMIT'){const code=codes.shift();if(code){attempts.push(code);throw Object.assign(new Error(code),{code});}}return {rows:[]};},release(){}};}});
 assert.equal(await pgDriver(failing(['40001','40P01'])).transaction(async()=>'body'),'body');assert.deepEqual(attempts,['40001','40P01']);
 await assert.rejects(()=>pgDriver(failing(['40001','40001','40001','40001'])).transaction(async()=>{}),error=>error.code==='40001');
 await assert.rejects(()=>pgDriver(failing(['23505'])).transaction(async()=>{}),error=>error.code==='23505');
 await assert.rejects(()=>pgDriver(failing(['40001','40001']),{retries:1}).transaction(async()=>{}),error=>error.code==='40001');
});
