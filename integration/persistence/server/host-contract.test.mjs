// Replays every request in fixtures/protocol/host-operations.json through the
// real TypeScript host with a fake persistence, and checks the answers against
// the same fixture crates/server/tests/host_contract.rs round-trips in Rust.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {createBackend,MutationRejected} from '../../../packages/server/index.mts';
import {HOST_OPERATIONS} from '../../../packages/server/host-contract.mts';
import {answer,persistence} from '../../../packages/postgres/index.mts';
const require=createRequire(import.meta.url);
const native=require('../../../bindings/node/axton-node.node');
const fixture=JSON.parse(await readFile(new URL('../../../fixtures/protocol/host-operations.json',import.meta.url),'utf8'));
const entry=op=>fixture.operations.find(o=>o.op===op);
const response=(op,variant)=>{
 const found=entry(op).responses.find(r=>variant?r.variant===variant:true);
 assert.ok(found,`${op}/${variant??'first'} is in the fixture`);
 return found.value;
};

const schema={enums:[],actions:[{name:'Send',version:1,inputs:[],outputs:[{name:'message',kind:'value',type:{kind:'scalar',name:'string'},cardinality:'single',source:'handlerValue'}]}],models:[{name:'Task',identity:['id'],fields:[
 {name:'id',type:{kind:'scalar',name:'string'},nullable:false},
 {name:'title',type:{kind:'scalar',name:'string'},nullable:false}]}]};
const config={schema,mutations:[{name:'edit',version:1,slots:[
 {name:'task',model:'Task',operation:'update',cardinality:'single',allowedPatchFields:['title']}]}]};

/** Answers the persistence half of the contract from the fixture. */
const fakePersistence=seen=>({
 async call(request){
  seen.push(request);
  switch(request.op){
   case 'claim':return response('claim','claimed');
   case 'claimCall':return response('claimCall','fresh');
   case 'saveReceipt':case 'saveCall':case 'savepoint':case 'rollback':case 'release':return null;
   case 'head':return response('head','cursor');
   case 'scan':return response('scan','rows');
   case 'advanceStamp':return response('advanceStamp','stamped');
   case 'ensureStamp':return response('ensureStamp','stamped');
   case 'publish':return response('publish','published');
   default:throw new Error(`fake persistence reached ${request.op}`);
  }
 },
});

/**
 * Drives the host callback with the fixture requests instead of a real push.
 * Returns [op, response] for every replayed request, plus what the persistence saw.
 */
async function replay(requests,{reject=false,fail=false,onError}={}){
 const seen=[],answers=[],handled=[],loaded=[];
 const backend=createBackend({
  config,
  native:{
   validateConfig:c=>native.validateConfig(c),
   processPush:async(_config,_owner,_request,callback)=>{
    for(const request of requests)answers.push([request.op,JSON.parse(await callback(JSON.stringify(request)))]);
    return '{"batchSequence":1,"clientId":"alice","records":[],"rejections":[]}';
   },
   processPull:async()=>'{}',
   settleExternal:async()=>'[]',
   negotiateLive:async()=>'{}',
   pullLive:async()=>'{}',
  },
  database:{transaction:body=>body({}),persistence:()=>fakePersistence(seen)},
  authenticate:()=>'alice',
  onError,
  // The seeded change set (the update slot's t-1) plus one addition, published
  // by default to one channel and explicitly to another: the fixture's settlement.
  handlers:{async send(){return {message:'sent'};},async edit({input,changes,publish}){
   handled.push(input);
   changes.add({model:'Task',identity:{id:'t-2'}});
   publish({channel:'shared'});
   publish({channel:'other',records:[input.task]});
   if(reject)throw new MutationRejected('task.refused');
   if(fail)throw new Error('boom');
  }},
  loaders:{async task(call){loaded.push(call);if(fail)throw new Error('boom');return response('load','rows');}},
 });
 await backend.push('alice','{}');
 return {answers,seen,handled,loaded};
}

test('the fixture and the TypeScript union cover the same operations',()=>{
 assert.deepEqual([...fixture.operations.map(o=>o.op)].sort(),[...HOST_OPERATIONS].sort());
 assert.equal(new Set(fixture.operations.map(o=>o.op)).size,fixture.operations.length);
});

test('every fixture request replays through the TypeScript host to the fixture answer',async()=>{
 const requests=fixture.operations.map(o=>o.request);
 const {answers,seen,handled,loaded}=await replay(requests);
 assert.deepEqual(answers.map(([op])=>op),HOST_OPERATIONS);
 const expected={
  claim:response('claim','claimed'),saveReceipt:null,claimCall:response('claimCall','fresh'),saveCall:null,head:response('head','cursor'),
  scan:response('scan','rows'),savepoint:null,rollback:null,release:null,
  handle:response('handle','settled'),handleAction:response('handleAction','settled'),load:response('load','rows'),
  advanceStamp:response('advanceStamp','stamped'),ensureStamp:response('ensureStamp','stamped'),publish:response('publish','published'),
 };
 assert.equal(Object.keys(expected).length,HOST_OPERATIONS.length,'every operation has an expected answer');
 for(const [op,answer] of answers)assert.deepEqual(answer,expected[op],`${op} answer`);
 // handle and load reach application code; everything else reaches persistence,
 // savepoint/rollback/release included - they are bookkept *and* forwarded.
 assert.deepEqual(seen.map(r=>r.op),HOST_OPERATIONS.filter(op=>op!=='handle'&&op!=='handleAction'&&op!=='load'));
 assert.equal(handled.length,1);
 assert.deepEqual(handled[0].task.patch,entry('handle').request.arguments.task.patch);
 assert.equal(loaded.length,1);assert.deepEqual(loaded[0].ids,entry('load').request.identities);assert.equal(loaded[0].userId,entry('load').request.owner);
 assert.equal('channel' in loaded[0],false,'loads name no channel');
});

test('the same handle request settles as the fixture rejection when the handler refuses',async()=>{
 const {answers}=await replay([entry('handle').request],{reject:true});
 assert.deepEqual(answers,[['handle',response('handle','rejected')]]);
});

test('a thrown handler error answers as a failure and reaches onError',async()=>{
 const errors=[];
 const {answers}=await replay([entry('handle').request],{fail:true,onError:e=>errors.push(e)});
 assert.equal(answers.length,1);assert.equal(answers[0][0],'handle');
 assert.equal(typeof answers[0][1].error,'string');
 assert.equal(errors.length,1);assert.equal(errors[0].message,'boom');
});

test('a thrown loader error answers as a failure and reaches onError',async()=>{
 const errors=[];
 const {answers}=await replay([entry('load').request],{fail:true,onError:e=>errors.push(e)});
 assert.equal(answers.length,1);assert.equal(answers[0][0],'load');
 assert.equal(typeof answers[0][1].error,'string');
 assert.equal(errors.length,1);assert.equal(errors[0].message,'boom');
});

test('the PostgreSQL persistence answers the persistence half through a two-method driver and refuses application operations',async()=>{
 const driver={transaction:body=>body('tx'),query:async(tx,sql)=>sql.startsWith('SELECT head')?[{head:6}]:[]};
 const bound=persistence(driver).persistence('tx');
 assert.equal(await bound.call(entry('head').request),response('head','cursor'));
 assert.equal(await bound.call(entry('savepoint').request),null);
 assert.equal(await answer(driver,'tx',entry('head').request),6);
 for(const op of ['handle','handleAction','load'])
  await assert.rejects(()=>bound.call(entry(op).request),/Unsupported persistence operation/);
 await assert.rejects(()=>bound.call({op:'vacuum'}),/Unsupported persistence operation vacuum/);
});
