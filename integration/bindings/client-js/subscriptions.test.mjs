// Subscription handles: identity, the committed status they publish, their
// observers, and what closing one means
// ([#150](https://github.com/zanminwang/axton/issues/150)).
import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtemp, rm, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { WebSocketServer } from '../../../packages/server/node_modules/ws/wrapper.mjs';
import * as runtime from '../../../packages/client-js/index.mts';

async function openClient() {
 const dir = await mkdtemp(join(tmpdir(),'axton-subscriptions-'));
 const schema = JSON.parse(await readFile(new URL('../../../fixtures/schemas/entry.json',import.meta.url),'utf8'));
 const client = await runtime.Client.open({path:join(dir,'client.sqlite'),schema});
 return {client, async close() { await client.close(); await rm(dir,{recursive:true,force:true}); }};
}
async function until(predicate,what='condition') {
 const deadline=Date.now()+5000;
 while(Date.now()<deadline) { if(await predicate()) return; await new Promise(r=>setTimeout(r,5)); }
 throw Error(`${what} timed out`);
}
const ack = (sub, heads={}) => JSON.stringify({type:'subscribed',cursors:Object.fromEntries(sub.channels.map(c=>[c,heads[c]??0]))});
const page = (text, cursor=0, stamp=cursor+1) => ({cursors:{scope:{from:cursor,to:cursor+1,head:cursor+1}},changes:[{model:'Entry',identity:{id:'live'},stamp,state:{text,note:null}}]});
/** A fake server whose handshake acknowledges `heads` and whose pull answers when `hold` resolves. */
async function fakeServer(heads={scope:0}) {
 const sockets=[];const handshakes=[];const pulls=[];let hold=Promise.resolve();let answer=(body)=>({cursors:{scope:{from:body.cursors.scope,to:heads.scope,head:heads.scope}},changes:[]});
 const http=createServer(async(req,res)=>{
  const chunks=[];for await(const c of req)chunks.push(c);const body=JSON.parse(Buffer.concat(chunks));
  pulls.push(body);await hold;res.end(JSON.stringify(answer(body)));
 });
 await new Promise(r=>http.listen(0,'127.0.0.1',r));
 const ws=new WebSocketServer({server:http});
 ws.on('connection',s=>{sockets.push(s);s.on('message',m=>{const sub=JSON.parse(m);handshakes.push(sub);s.send(ack(sub,heads));});});
 return {sockets,handshakes,pulls,heads,
  set hold(promise){hold=promise;},set answer(fn){answer=fn;},
  config:{url:`http://127.0.0.1:${http.address().port}`,token:'secret'},
  async close(){for(const s of ws.clients)s.terminate();await new Promise(r=>ws.close(r));await new Promise(r=>http.close(r));}};
}

test('one handle per subscription identity: concurrent and repeated calls coalesce', async()=>{
 const fixture=await openClient();const {client}=fixture;
 try {
  const [first,second]=await Promise.all([client.subscribe('scope'),client.subscribe('scope')]);
  assert.equal(first,second,'concurrent calls obtain one cached handle');
  assert.equal(await client.scopes.subscribe('scope'),first,'the scopes facade shares the cache');
  assert.equal(first.scope,'scope');
  assert.deepEqual({...first.status},{active:true,initialization:'pending',connection:'offline'},
   'registered offline: durable intent with no boundary and no transport');
  assert.deepEqual((await client.syncState()).channels,['scope']);
  assert.deepEqual((await client.syncState()).cursors,{},'an uninitialized subscription has no cursor at all');
  const other=await client.subscribe('other');
  assert.notEqual(other,first,'another Scope is another subscription');
 } finally { await fixture.close(); }
});

test('watch delivers the current snapshot, then changes, and nothing after cancellation', async()=>{
 const fixture=await openClient();const {client}=fixture;
 try {
  const subscription=await client.subscribe('scope');
  const seen=[];
  const stop=subscription.watch(status=>seen.push(status));
  assert.deepEqual(seen.map(s=>s.connection),['offline'],'the current snapshot arrives at once');
  assert.ok(Object.isFrozen(seen[0]),'a status snapshot is immutable');
  await subscription.unsubscribe();
  assert.deepEqual(seen.map(s=>[s.active,s.connection]),[[true,'offline'],[false,'stopped']]);
  stop();
  const replacement=await client.subscribe('scope');
  assert.notEqual(replacement,subscription,'a new registration is a new handle');
  await replacement.unsubscribe();
  assert.equal(seen.length,2,'a cancelled observer hears nothing more');
  // The closed handle still reads its status, and watching it delivers the
  // stopped snapshot once.
  const late=[];subscription.watch(status=>late.push(status))();
  assert.deepEqual(late.map(s=>s.connection),['stopped']);
  assert.equal(subscription.status.active,false);
 } finally { await fixture.close(); }
});

test('an observer exception is reported after the commit and changes nothing', async()=>{
 const fixture=await openClient();const {client}=fixture;
 const previous=globalThis.reportError;const reported=[];
 globalThis.reportError=error=>reported.push(error);
 const network=await fakeServer({scope:0});
 try {
  const subscription=await client.subscribe('scope');
  subscription.watch(()=>{throw Error('observer failed');});
  assert.equal(reported.length,1,'the first snapshot already reached the failing observer');
  const connection=await client.connect(network.config);
  await until(()=>network.handshakes.length===1,'the handshake');
  await until(async()=>(await client.syncState()).cursors.scope===0,'the committed boundary');
  assert.ok(reported.length>=2,`the observer failed again after the commit: ${reported.length}`);
  assert.ok(reported.every(e=>e.message==='observer failed'));
  assert.equal((await client.syncState()).channels.length,1,'nothing was rolled back');
  // A failing observer is not a transport failure: the session it fired in is
  // still the one streaming.
  network.sockets[0].send(JSON.stringify(page('streamed')));
  await until(async()=>(await client.read('Entry',{id:'live'}))?.text==='streamed','the streamed page');
  assert.equal(network.handshakes.length,1,'no reconnect followed the observer failure');
  const again=await client.subscribe('scope');
  assert.equal(again,subscription,'subscribe resolved with the same handle, never rejected');
  await connection.close();
  // The client closes while the observer is still installed: its last snapshot
  // fails too, and that failure is reported like the others.
  await fixture.close();
  assert.ok(reported.length>=3,'the stopped snapshot reached the failing observer');
 } finally { await fixture.close();globalThis.reportError=previous;await network.close(); }
});

test('status follows the committed boundary and the lane: offline, connecting, catching up, live', async()=>{
 const fixture=await openClient();const {client}=fixture;
 const network=await fakeServer({scope:0});
 try {
  const subscription=await client.subscribe('scope');
  const seen=[];subscription.watch(status=>seen.push(status.connection));
  assert.equal(subscription.status.initialization,'pending');
  const connection=await client.connect(network.config);
  await until(()=>subscription.status.connection==='live'&&subscription.status.initialization==='ready',
   'a live subscription with a committed boundary');
  assert.ok(seen.includes('connecting'),`the lane was seen connecting: ${seen}`);
  // The server publishes while the socket is closed: the next handshake
  // acknowledges a head above the committed cursor and one HTTP catch-up runs.
  await connection.pause();
  await until(()=>subscription.status.connection==='offline','a paused lane');
  network.heads.scope=1;
  const gate=Promise.withResolvers();network.hold=gate.promise;
  network.answer=body=>({cursors:{scope:{from:body.cursors.scope,to:1,head:1}},changes:page('caught up').changes});
  await connection.resume();
  await until(()=>subscription.status.connection==='catching-up','a catching-up subscription');
  assert.equal(subscription.status.initialization,'ready','the boundary stays committed while catching up');
  gate.resolve();
  await until(async()=>(await client.read('Entry',{id:'live'}))?.text==='caught up','the catch-up page');
  await until(()=>subscription.status.connection==='live','live again after the catch-up');
  assert.deepEqual(network.pulls.map(p=>p.cursors),[{scope:0}],'one pull, from the committed cursor');
  await connection.close();
  assert.deepEqual({...subscription.status},{active:true,initialization:'ready',connection:'offline'},
   'a closed connection leaves the subscription registered and offline');
 } finally { await fixture.close();await network.close(); }
});

test('a recreated subscription is connecting until its own handshake acknowledges it', async()=>{
 const fixture=await openClient();const {client}=fixture;
 const network=await fakeServer({scope:0});
 try {
  const first=await client.subscribe('scope');
  const connection=await client.connect(network.config);
  await until(()=>first.status.connection==='live'&&first.status.initialization==='ready','a live subscription');
  await first.unsubscribe();
  const second=await client.subscribe('scope');
  assert.notEqual(second,first,'a new registration is a new handle');
  assert.deepEqual({...second.status},{active:true,initialization:'pending',connection:'connecting'},
   'the open session never subscribed this registration: it is not live and has no boundary');
  // The worker replaces the socket for the new membership; that handshake is
  // this subscription's own.
  await until(()=>network.handshakes.length>=2,'the socket the new membership needs');
  await until(()=>second.status.connection==='live'&&second.status.initialization==='ready',
   'the new subscription goes live on its own acknowledgement');
  assert.deepEqual({...first.status},{active:false,initialization:'ready',connection:'stopped'},
   'the handle it replaced stays stopped');
  await connection.close();
 } finally { await fixture.close();await network.close(); }
});

test('unsubscribe removes one registration; an old handle cannot remove its replacement', async()=>{
 const fixture=await openClient();const {client}=fixture;
 try {
  const first=await client.subscribe('scope');
  await first.unsubscribe();
  assert.deepEqual({...first.status},{active:false,initialization:'pending',connection:'stopped'});
  assert.deepEqual((await client.syncState()).channels,[],'the registration is gone');
  await first.unsubscribe();
  assert.deepEqual((await client.syncState()).channels,[],'repeating it on a closed handle is a no-op');
  const second=await client.subscribe('scope');
  assert.notEqual(second,first);
  await first.unsubscribe();
  assert.deepEqual((await client.syncState()).channels,['scope'],
   'an old handle must not delete the subscription that replaced it');
  assert.equal(second.status.active,true);
  // The Scope-named form removes whatever is registered and closes its handle.
  await client.unsubscribe('scope');
  assert.deepEqual((await client.syncState()).channels,[]);
  assert.deepEqual({...second.status},{active:false,initialization:'pending',connection:'stopped'});
 } finally { await fixture.close(); }
});

test('closing the client stops handles and deletes nothing; work through them fails closed', async()=>{
 const dir=await mkdtemp(join(tmpdir(),'axton-subscriptions-'));
 const schema=JSON.parse(await readFile(new URL('../../../fixtures/schemas/entry.json',import.meta.url),'utf8'));
 const path=join(dir,'client.sqlite');
 try {
  const client=await runtime.Client.open({path,schema});
  const subscription=await client.subscribe('scope');
  const seen=[];subscription.watch(status=>seen.push(status));
  await client.close();
  assert.deepEqual({...subscription.status},{active:false,initialization:'pending',connection:'stopped'});
  assert.deepEqual(seen.map(s=>s.connection),['offline','stopped'],'observers hear the stop, then are cancelled');
  const failed=await subscription.unsubscribe().then(()=>null,error=>error);
  assert.equal(failed?.code,'subscription.closed','a stopped handle cannot commit work');
  const reopened=await runtime.Client.open({path,schema});
  try {
   assert.deepEqual((await reopened.syncState()).channels,['scope'],'closing the client deleted nothing');
   const restored=await reopened.subscribe('scope');
   assert.equal(restored.status.active,true);
  } finally { await reopened.close(); }
 } finally { await rm(dir,{recursive:true,force:true}); }
});
