import test from 'node:test';
import assert from 'node:assert/strict';
import {Transaction,strictJson} from '../../../packages/client-js/transaction.mts';
test('raw transactions do not expose named mutation enqueue',()=>{
 const tx=new Transaction(async()=>{});
 assert.equal('mutate' in tx,false);
});
test('forgotten calls drain before rollback and cannot escape callback lifetime',async()=>{
 const calls=[];let release;const gate=new Promise(r=>release=r);const tx=new Transaction(async r=>{if(r.op==='direct')await gate;calls.push(r.op);});
 void tx.direct({});const ending=tx.finish();assert.deepEqual(calls,[]);release();await assert.rejects(ending,/unawaited/);assert.deepEqual(calls,['direct']);await assert.rejects(tx.direct({}),/closed/);
});
test('parallel awaited commands preserve invocation order',async()=>{
 const calls=[];const tx=new Transaction(async r=>{await new Promise(r=>setTimeout(r,2));calls.push(r.operation.n);});await Promise.all([tx.direct({n:1}),tx.direct({n:2})]);await tx.finish();assert.deepEqual(calls,[1,2]);
});
test('caught failed operation poisons outer transaction, savepoint confines failure',async()=>{
 const calls=[];const tx=new Transaction(async r=>{calls.push(r.op);if(r.op==='direct')throw Error('bad');});await tx.direct({}).catch(()=>{});await assert.rejects(tx.finish(),/bad/);
 const tx2=new Transaction(async r=>{if(r.op==='direct')throw Error('bad');});await tx2.savepoint(()=>tx2.direct({})).catch(()=>{});await tx2.finish();
});
test('overlapping savepoints fail without popping each others stack',async()=>{
 const calls=[];let release;const gate=new Promise(r=>release=r);const tx=new Transaction(async r=>{calls.push(r.op);});const first=tx.savepoint(async()=>{await gate;});await new Promise(r=>setImmediate(r));await assert.rejects(tx.savepoint(async()=>{}),/overlapping/);release();await assert.rejects(first,/overlapping/);await assert.rejects(tx.finish(),/overlapping/);assert.deepEqual(calls,['savepoint']);
});
test('nonfinite JSON is rejected instead of becoming a nullable clear',()=>{assert.throws(()=>strictJson({value:NaN}),/finite/);assert.throws(()=>strictJson({value:Infinity}),/finite/);assert.equal(strictJson({value:null}),'\{"value":null\}');});
test('unawaited nested scope poisons transaction without releasing another scope',async()=>{
 const calls=[];let release;const gate=new Promise(r=>release=r);const tx=new Transaction(async r=>{calls.push(r.op);});let child;
 await assert.rejects(tx.savepoint(async()=>{await tx.direct({});child=tx.savepoint(async()=>{await gate;throw Error('late child');}).catch(()=>{});}),/unawaited nested/);
 release();await child;await assert.rejects(tx.finish(),/unawaited nested/);assert.deepEqual(calls,['savepoint','direct','savepoint']);
});
test('late savepoint callback after transaction closes cannot issue stack commands',async()=>{
 const calls=[];let release;const gate=new Promise(r=>release=r);const tx=new Transaction(async r=>{calls.push(r.op);});const scope=tx.savepoint(async()=>{await gate;});await new Promise(r=>setImmediate(r));await assert.rejects(tx.finish(),/unawaited/);release();await assert.rejects(scope,/closed/);assert.deepEqual(calls,['savepoint']);
});
