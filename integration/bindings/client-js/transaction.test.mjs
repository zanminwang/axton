import test from 'node:test';
import assert from 'node:assert/strict';
import { AsyncLocalStorage } from 'node:async_hooks';
import {Transaction,strictJson} from '../../../packages/frontend/client-js/api/transaction.mts';
import { Client } from '../../../packages/frontend/client-js/index.mts';
import { openStore } from './store-fixture.mjs';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
test('public callback context is disabled only after settlement, including rejection and finish failure',async()=>{
 const originalRun=AsyncLocalStorage.prototype.run;
 const originalDisable=AsyncLocalStorage.prototype.disable;
 const ran=[];const disabled=[];
 AsyncLocalStorage.prototype.run=function(...args){ran.push(this);return originalRun.apply(this,args);};
 AsyncLocalStorage.prototype.disable=function(...args){disabled.push(this);return originalDisable.apply(this,args);};
 try {
  for(const outcome of ['success','rejection','finish failure']){
   const tx=new Transaction(async()=>{throw Error('finish failed');});
   let release,entered;const gate=new Promise(resolve=>{release=resolve;});const started=new Promise(resolve=>{entered=resolve;});
   const callback=tx.runCallback(async()=>{assert.equal(tx.inCallback(),true);entered();await gate;assert.equal(tx.inCallback(),true);if(outcome==='rejection')throw Error('callback failed');return 'done';});
   await started;
   const context=ran.at(-1);
   assert.ok(context);
   assert.equal(tx.inCallback(),false);
   assert.equal(disabled.includes(context),false);
   release();
   if(outcome==='rejection')await assert.rejects(callback,/callback failed/);
   else assert.equal(await callback,'done');
   assert.equal(disabled.filter(store=>store===context).length,1);
   if(outcome==='finish failure'){
    await assert.rejects(tx.read('Entry',{id:'e'}),/finish failed/);
    await assert.rejects(tx.finish(),/finish failed/);
   }else await tx.finish();
   assert.equal(disabled.filter(store=>store===context).length,1);
  }
 }finally{
  AsyncLocalStorage.prototype.run=originalRun;
  AsyncLocalStorage.prototype.disable=originalDisable;
 }
});
test('raw transactions do not expose named mutation enqueue',()=>{
 const tx=new Transaction(async()=>{});
 assert.equal('mutate' in tx,false);
});
test('forgotten calls drain before rollback and cannot escape callback lifetime',async()=>{
 const calls=[];let release;const gate=new Promise(r=>release=r);const tx=new Transaction(async r=>{if(r.kind==='read')await gate;calls.push(r.kind);});
 void tx.read('Entry',{id:'e'});const ending=tx.finish();assert.deepEqual(calls,[]);release();await assert.rejects(ending,/unawaited/);assert.deepEqual(calls,['read']);await assert.rejects(tx.read('Entry',{id:'e'}),/closed/);
});
test('parallel awaited commands preserve invocation order',async()=>{
 const calls=[];const tx=new Transaction(async r=>{await new Promise(r=>setTimeout(r,2));calls.push(r.key.identity.n);});await Promise.all([tx.read('Entry',{n:1}),tx.read('Entry',{n:2})]);await tx.finish();assert.deepEqual(calls,[1,2]);
});
test('caught failed operation poisons outer transaction, savepoint confines failure',async()=>{
 const calls=[];const tx=new Transaction(async r=>{calls.push(r.kind);if(r.kind==='read')throw Error('bad');});await tx.read('Entry',{id:'e'}).catch(()=>{});await assert.rejects(tx.finish(),/bad/);
 const tx2=new Transaction(async r=>{if(r.kind==='read')throw Error('bad');});await tx2.savepoint(()=>tx2.read('Entry',{id:'e'})).catch(()=>{});await tx2.finish();
});
test('overlapping savepoints fail without popping each others stack',async()=>{
 const calls=[];let release;const gate=new Promise(r=>release=r);const tx=new Transaction(async r=>{calls.push(r.kind);});const first=tx.savepoint(async()=>{await gate;});await new Promise(r=>setImmediate(r));await assert.rejects(tx.savepoint(async()=>{}),/overlapping/);release();await assert.rejects(first,/overlapping/);await assert.rejects(tx.finish(),/overlapping/);assert.deepEqual(calls,['savepoint']);
});
test('nonfinite JSON is rejected instead of becoming a nullable clear',()=>{assert.throws(()=>strictJson({value:NaN}),/finite/);assert.throws(()=>strictJson({value:Infinity}),/finite/);assert.equal(strictJson({value:null}),'\{"value":null\}');});
test('unawaited nested scope poisons transaction without releasing another scope',async()=>{
 const calls=[];let release;const gate=new Promise(r=>release=r);const tx=new Transaction(async r=>{calls.push(r.kind);});let child;
 await assert.rejects(tx.savepoint(async()=>{await tx.read('Entry',{id:'e'});child=tx.savepoint(async()=>{await gate;throw Error('late child');}).catch(()=>{});}),/unawaited nested/);
 release();await child;await assert.rejects(tx.finish(),/unawaited nested/);assert.deepEqual(calls,['savepoint','read','savepoint']);
});
test('late savepoint callback after transaction closes cannot issue stack commands',async()=>{
 const calls=[];let release;const gate=new Promise(r=>release=r);const tx=new Transaction(async r=>{calls.push(r.kind);});const scope=tx.savepoint(async()=>{await gate;});await new Promise(r=>setImmediate(r));await assert.rejects(tx.finish(),/unawaited/);release();await assert.rejects(scope,/closed/);assert.deepEqual(calls,['savepoint']);
});
test('a callback is not Mutation input: it is refused before reaching the runtime',async()=>{
 const sent=[];
 const tx=new Transaction(async command=>{sent.push(command.kind);return null;},{submit:(command)=>{sent.push(command.kind);return Promise.resolve('call');}});
 await assert.rejects(tx.submitMutation('Publish',1,async()=>({id:'p'}),value=>value),{code:'action.invalid'});
 assert.equal(await tx.submitMutation('Ping',1,{},value=>value),'call');
 await tx.read('Entry',{id:'e'});
 await tx.finish();
 assert.deepEqual(sent,['submitMutation','read']);
});

// Mutations in a transaction through the native runtime (shared with React Native).
import { mutationTests } from './mutations-harness.mjs';
mutationTests(test, Transaction, { savepoints: true });
test('a transaction reads a row and submits a Mutation built from it: the commit keeps both, a rollback discards the Call',async()=>{
 const schema=JSON.parse(await readFile(new URL('../../../fixtures/schemas/entry.json',import.meta.url),'utf8'));
 schema.actions=[{name:'Create',version:1,kind:'mutation',inputs:[{kind:'model',name:'entry',model:'Entry',operation:'create',cardinality:'single',allowedFields:['text','note']}],outputs:[]}];
 const dir=await mkdtemp(join(tmpdir(),'axton-transaction-'));
 const client=await openStore(Client,{path:join(dir,'client.sqlite'),schema});
 const create=(port,id,text)=>port.submitMutation('Create',1,{entry:{id,text,note:null}},value=>value);
 try {
  await create(client,'draft','local');
  const copy=async tx=>{
   const draft=await tx.read('Entry',{id:'draft'});
   const call=await create(tx,'copy',draft.text);
   assert.equal((await tx.read('Entry',{id:'copy'})).text,'local','the transaction sees its own Call');
   return call;
  };
  let rolledBack;
  await assert.rejects(client.transaction(async tx=>{rolledBack=await copy(tx);throw Error('boom');}),/boom/);
  await assert.rejects(rolledBack.wait(),{code:'transaction_rolled_back'});
  assert.equal(await client.read('Entry',{id:'copy'}),null);
  assert.equal((await client.syncState()).pending,1,'only the seed is queued');
  const committed=await client.transaction(copy);
  assert.equal(committed.provisional,false);
  assert.equal((await client.read('Entry',{id:'copy'})).text,'local');
  assert.equal((await client.syncState()).pending,2);
 } finally { await client.close(); await rm(dir,{recursive:true,force:true}); }
});
