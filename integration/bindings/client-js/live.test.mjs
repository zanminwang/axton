import test from 'node:test';
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { WebSocketServer } from '../../../packages/server/node_modules/ws/wrapper.mjs';
import * as runtime from '../../../packages/client-js/index.mts';
import {createServerConnection} from '../../../packages/client-js/live.mts';

const timeout = (p) => Promise.race([p, new Promise((_, reject) => { const t = setTimeout(() => reject(Error('timeout')), 3000); t.unref(); })]);

const subscribe = JSON.stringify({type:'subscribe',channels:['scope']});
// The acknowledgement carries every channel's head; `heads` is the fake server's state.
const ack = (sub, heads={}) => JSON.stringify({type:'subscribed',cursors:Object.fromEntries(sub.channels.map(c=>[c,heads[c]??0]))});
const handlers = (over = {}) => ({ message: async () => {}, overflow: async () => {}, closed: () => {}, ...over });

test('internal socket sends the subscribe frame, delivers frames in order, and cancellation ends the socket', async () => {
  assert.equal(typeof createServerConnection, 'function');
  const server = new WebSocketServer({port:0});
  await once(server,'listening');
  const abort = new AbortController();
  const frames = [];
  const connected = once(server,'connection');
  const live = createServerConnection({url:`http://127.0.0.1:${server.address().port}`,token:'secret'});
  live.open(subscribe, abort.signal, handlers({ message: async text => { frames.push(JSON.parse(text)); } }));
  try {
    const [socket, request] = await timeout(connected);
    assert.equal(request.headers.authorization,'Bearer secret');
    const [message] = await timeout(once(socket,'message'));
    assert.deepEqual(JSON.parse(message),{type:'subscribe',channels:['scope']});
    const closed = once(socket,'close');
    socket.send(ack(JSON.parse(message)));
    socket.send(JSON.stringify({cursors:{scope:{from:12,to:13,head:13}},changes:[]}));
    await timeout(new Promise(resolve => { const check = () => frames.length === 2 ? resolve() : setImmediate(check); check(); }));
    assert.equal(frames[0].type,'subscribed', 'the transport does not interpret frames');
    assert.equal(frames[1].cursors.scope.to,13);
    abort.abort();
    await timeout(closed);
  } finally { abort.abort(); for (const s of server.clients) s.terminate(); await new Promise(r => server.close(r)); }
});

test('live transport cancellation does not wait for a stalled token', async () => {
  const abort = new AbortController();
  let closed = 0;
  const live = createServerConnection({url:'http://127.0.0.1:1',token:()=>new Promise(()=>{})});
  live.open(subscribe, abort.signal, handlers({ closed: () => { closed++; } }));
  abort.abort();
  await new Promise(r => setTimeout(r, 20));
  assert.equal(closed, 0, 'an aborted socket is not reported as closed');
});

test('invalid WebSocket credentials reject the session rather than leaking a rejected task', async () => {
 const live = createServerConnection({url:'http://127.0.0.1:1',token:'invalid\nheader'});
 const failure = new Promise(resolve => live.open(subscribe, new AbortController().signal, handlers({ closed: resolve })));
 assert.match(String((await timeout(failure)).message), /header|character/i);
});

import { createServer } from 'node:http';
import { mkdtemp, rm, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
async function openClient() {
 const dir = await mkdtemp(join(tmpdir(),'axton-live-'));
 const schema = JSON.parse(await readFile(new URL('../../../fixtures/schemas/entry.json',import.meta.url),'utf8'));
 const client = await runtime.Client.open({path:join(dir,'client.sqlite'),schema});
 return {client, async close() { await client.close(); await rm(dir,{recursive:true,force:true}); }};
}
// `stamp` defaults to the cursor; a page for a record the client already holds at that
// stamp must carry a newer one, because retained content is compared by stamp alone.
const page = (text, cursor=0, stamp=cursor+1, head=cursor+1) => ({cursors:{scope:{from:cursor,to:cursor+1,head}},changes:[{model:'Entry',identity:{id:'live'},stamp,state:{text,note:null}}]});
// An HTTP answer that moves every requested channel to `head` with no changes.
const emptyPage=(b,head)=>({cursors:Object.fromEntries(Object.entries(b.cursors).map(([c,n])=>[c,{from:n,to:Math.max(n,head?.[c]??n),head:Math.max(n,head?.[c]??n)}])),changes:[]});
// A fake push receipt in the wire shape the client accepts: it answers the batch it
// was asked (clientId and batchSequence echoed) and carries the authoritative state
// of every record the batch's wire operations target, once per record, at a stamp
// the fake server hands out monotonically. The "server" normalizes text by trimming
// it, so a test can tell the receipt's content from the client's prediction.
function receiptFor(body, stamps) {
 const records=new Map();
 for (const mutation of body.mutations) for (const op of mutation.operations ?? []) {
  const state=op.op==='delete'?null:{text:String(op.values?.text ?? '').trim(),note:op.values?.note ?? null};
  records.set(`${op.model}\0${JSON.stringify(op.identity)}`,{model:op.model,identity:op.identity,stamp:++stamps.next,state});
 }
 return {clientId:body.clientId,batchSequence:body.batchSequence,rejections:[],records:[...records.values()]};
}
async function until(predicate) {
 const deadline=Date.now()+5000;
 while(Date.now()<deadline) { if(await predicate()) return; await new Promise(r=>setTimeout(r,5)); }
 throw Error('condition timed out');
}

test('client replaces subscriptions from saved cursors and guards queued obsolete pages', async()=>{
 const fixture=await openClient(); const {client}=fixture; const errors=[];
 const pulls=[];let recovered=false;const heads={scope:0};
 const http=createServer(async(req,res)=>{const chunks=[];for await(const c of req)chunks.push(c);const body=JSON.parse(Buffer.concat(chunks));pulls.push(body);res.end(JSON.stringify(recovered?{cursors:{scope:{from:body.cursors.scope,to:11,head:11}},changes:[page('recovered',10).changes[0]]}:emptyPage(body,heads)));});
 await new Promise(r=>http.listen(0,'127.0.0.1',r));
 const server=new WebSocketServer({server:http});
 const sockets=[]; const handshakes=[];
 server.on('connection',s=>{sockets.push(s);s.on('message',m=>{const sub=JSON.parse(m);handshakes.push(sub);s.send(ack(sub,heads));});});
 try {
  const connection=await client.connect({url:`http://127.0.0.1:${http.address().port}`,token:'secret'},{onError:e=>errors.push(e)});
  await client.subscribe('scope'); await until(()=>handshakes.length===1);
  heads.scope=1;sockets[0].send(JSON.stringify(page('first')));
  await until(async()=>(await client.read('Entry',{id:'live'}))?.text==='first');
  assert.equal(pulls.length,0,'at the head: the acknowledgement starts no catch-up');
  const gate=Promise.withResolvers(),entered=Promise.withResolvers();
  const tx=client.transaction(async()=>{entered.resolve();await gate.promise;});await entered.promise;
  sockets[0].send(JSON.stringify(page('stale',1)));
  const removed=client.unsubscribe('scope');const restored=client.subscribe('scope');
  gate.resolve();await tx;await removed;await restored;
  await until(()=>handshakes.length>=2);
  assert.equal((await client.read('Entry',{id:'live'})).text,'first','unsubscribing retains the downloaded record; the queued obsolete page is dropped, not applied');
  // The resubscribed channel is behind the head it is told: one pull from 0.
  await until(()=>pulls.length>=1);assert.deepEqual(pulls.at(-1).cursors,{scope:0});await until(async()=>(await client.syncState()).cursors.scope===1);
  // The record is retained at stamp 1: the fresh page needs a newer stamp to replace it.
  heads.scope=2;sockets.at(-1).send(JSON.stringify(page('fresh',1,3)));
  await until(async()=>(await client.read('Entry',{id:'live'}))?.text==='fresh');
  await connection.pause();await until(()=>server.clients.size===0);
  await connection.resume();await until(()=>handshakes.length>=3);
  await new Promise(r=>setTimeout(r,50));assert.equal(pulls.length,1,'heads equal to the cursors: no catch-up on reconnect');
  assert.equal(errors.length,0);
  recovered=true;sockets.at(-1).send(JSON.stringify(page('gap',10)));await until(async()=>(await client.read('Entry',{id:'live'}))?.text==='recovered');assert.equal(errors.length,0);
  await connection.close();await until(()=>server.clients.size===0);
 } finally { await fixture.close(); for(const s of server.clients)s.terminate();await new Promise(r=>server.close(r));await new Promise(r=>http.close(r)); }
});

test('client retries upgrade authentication and survives failed refresh',async()=>{
 const fixture=await openClient();const {client}=fixture;const errors=[];
 const server=createServer(async(req,res)=>{const chunks=[];for await(const c of req)chunks.push(c);const b=JSON.parse(Buffer.concat(chunks));res.end(JSON.stringify(emptyPage(b)));});const ws=new WebSocketServer({noServer:true});
 let token='expired', refreshes=0, accepted=0;
 server.on('upgrade',(req,socket,head)=>{if(req.headers.authorization!=='Bearer valid'){socket.end('HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n');return;}ws.handleUpgrade(req,socket,head,s=>{accepted++;s.on('message',m=>s.send(ack(JSON.parse(m))));});});
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 try {
  await client.subscribe('scope');
  await client.connect({url:`http://127.0.0.1:${server.address().port}`,token:()=>token},{onError:e=>errors.push(e),refreshAuth:async()=>{if(++refreshes===1)throw Error('refresh failed');token='valid';}});
  await until(()=>accepted===1);
  assert.equal(refreshes,2);assert.ok(errors.some(e=>e.message==='refresh failed'));
 } finally {await fixture.close();for(const s of ws.clients)s.terminate();await new Promise(r=>ws.close(r));await new Promise(r=>server.close(r));}
});

test('close cancels opening handshake and ignores its eventual server response',async()=>{
 const fixture=await openClient();const server=createServer();const entered=Promise.withResolvers();let pending;
 server.on('upgrade',(_req,socket)=>{pending=socket;socket.resume();socket.on('end',()=>socket.destroy());entered.resolve();});
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 try {
  await fixture.client.subscribe('scope');
  const connection=await fixture.client.connect({url:`http://127.0.0.1:${server.address().port}`,token:'secret'});
  await timeout(entered.promise);const closed=once(pending,'close');
  await timeout(connection.close());await timeout(closed);
 }finally{await fixture.close();pending?.destroy();await new Promise(r=>server.close(r));}
});

test('client close abandons a stalled live token and never opens after it resolves',async()=>{
 const fixture=await openClient();let requests=0;const token=Promise.withResolvers();const called=Promise.withResolvers();
 const server=new WebSocketServer({port:0});await once(server,'listening');server.on('connection',()=>requests++);
 try {
  await fixture.client.subscribe('scope');
  await fixture.client.connect({url:`http://127.0.0.1:${server.address().port}`,token:()=>{called.resolve();return token.promise;}});
  await timeout(called.promise);await timeout(fixture.client.close());token.resolve('late');
  await new Promise(r=>setImmediate(r));assert.equal(requests,0);
 }finally{await fixture.close();for(const s of server.clients)s.terminate();await new Promise(r=>server.close(r));}
});

test('unified connection acknowledges listeners then catches up through HTTP before live delivery', async()=>{
 const fixture=await openClient();const events=[];let pulls=0;
 const server=createServer(async(req,res)=>{
  const chunks=[];for await(const chunk of req)chunks.push(chunk);
  const body=JSON.parse(Buffer.concat(chunks));
  assert.equal(req.url,'/sync/pull');assert.ok(events.includes('ack'));
  pulls++;events.push(`pull:${body.cursors.scope}`);
  res.setHeader('content-type','application/json');res.end(JSON.stringify(page('caught up')));
 });
 const ws=new WebSocketServer({server});let socket;
 ws.on('connection',s=>{socket=s;s.on('message',m=>{events.push('ack');s.send(ack(JSON.parse(m),{scope:1}));});});
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 try {
  await fixture.client.subscribe('scope');
  const options={url:`http://127.0.0.1:${server.address().port}`,token:'secret'};
  const connection=await fixture.client.connect(options);
  await until(()=>events.includes('ack'));
  await new Promise(r=>setTimeout(r,100));
  assert.equal(pulls,1,'a head beyond the cursor in the acknowledgement starts one HTTP catch-up');
  await until(async()=>(await fixture.client.read('Entry',{id:'live'}))?.text==='caught up');
  socket.send(JSON.stringify(page('continuous',1)));
  await until(async()=>(await fixture.client.read('Entry',{id:'live'}))?.text==='continuous');
  await new Promise(r=>setTimeout(r,60));assert.equal(pulls,1,'ordinary live updates do not poll HTTP');
  await connection.close();
 } finally {await fixture.close();for(const s of ws.clients)s.terminate();await new Promise(r=>ws.close(r));await new Promise(r=>server.close(r));}
});

async function syncFixture(onPull, heads={}) {
 const requests=[];const sockets=[];const stamps={next:0};
 const server=createServer(async(req,res)=>{
  const chunks=[];for await(const c of req)chunks.push(c);const body=JSON.parse(Buffer.concat(chunks));requests.push({url:req.url,body});
  if(req.url==='/sync/mutations')res.end(JSON.stringify(receiptFor(body,stamps)));
  else await onPull(body,res,requests.filter(r=>r.url==='/sync/pull').length);
 });
 const ws=new WebSocketServer({server});ws.on('connection',s=>{sockets.push(s);s.on('message',m=>s.send(ack(JSON.parse(m),heads)));});
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 return {requests,sockets,heads,config:{url:`http://127.0.0.1:${server.address().port}`,token:'secret'},async close(){for(const s of ws.clients)s.terminate();await new Promise(r=>ws.close(r));await new Promise(r=>server.close(r));}};
}
test('late HTTP catch-up after unsubscribe and resubscribe cannot resurrect the obsolete generation',async()=>{
 const fixture=await openClient();const network=await syncFixture((b,res,n)=>res.end(JSON.stringify(page(n===1?'obsolete':'fresh'))),{scope:1});
 const original=globalThis.fetch;const entered=Promise.withResolvers(),gate=Promise.withResolvers();let first=true;
 globalThis.fetch=async(...args)=>{const response=await original(...args);if(first){first=false;entered.resolve();await gate.promise;}return response;};
 try {
  await fixture.client.subscribe('scope');await fixture.client.connect(network.config);
  await timeout(entered.promise);await fixture.client.unsubscribe('scope');await fixture.client.subscribe('scope');
  await until(async()=>(await fixture.client.read('Entry',{id:'live'}))?.text==='fresh');
  gate.resolve();await new Promise(r=>setTimeout(r,30));assert.equal((await fixture.client.read('Entry',{id:'live'})).text,'fresh');
 }finally{gate.resolve();globalThis.fetch=original;await fixture.close();await network.close();}
});

test('pause cancels held catch-up and a late HTTP token cannot start a request',async()=>{
 const fixture=await openClient();const network=await syncFixture((b,res)=>res.end(JSON.stringify(emptyPage(b,{scope:1}))),{scope:1});
 const called=Promise.withResolvers(),token=Promise.withResolvers();let calls=0;
 try{
  await fixture.client.subscribe('scope');const connection=await fixture.client.connect({...network.config,token:()=>++calls===1?'secret':(called.resolve(),token.promise)});
  await timeout(called.promise);await timeout(connection.pause());token.resolve('late');await new Promise(r=>setTimeout(r,30));
  assert.equal(network.requests.length,0);assert.equal((await fixture.client.syncState()).cursors.scope??0,0);
  await connection.resume();await until(()=>network.requests.length===1);
 }finally{token.resolve('late');await fixture.close();await network.close();}
});

test('one incoming page path covers duplicates, applies overlap directly and recovers genuine gaps',async()=>{
 const fixture=await openClient();let head=1;
 const network=await syncFixture((b,res)=>res.end(JSON.stringify({cursors:{scope:{from:b.cursors.scope,to:head,head}},changes:[page(`HTTP ${head}`,head-1).changes[0]]})),{scope:1});
 try{
  await fixture.client.subscribe('scope');await fixture.client.connect(network.config);
  await until(async()=>(await fixture.client.syncState()).cursors.scope===1);
  network.sockets[0].send(JSON.stringify(page('duplicate')));await new Promise(r=>setTimeout(r,20));assert.equal(network.requests.length,1);
  head=2;network.sockets[0].send(JSON.stringify({cursors:{scope:{from:0,to:2,head:2}},changes:[page('overlap',1).changes[0]]}));
  await until(async()=>(await fixture.client.syncState()).cursors.scope===2);assert.equal(network.requests.length,1,'overlap must not issue another HTTP pull');assert.equal((await fixture.client.read('Entry',{id:'live'})).text,'overlap');
  head=4;network.sockets[0].send(JSON.stringify(page('gap',3)));
  await until(async()=>(await fixture.client.syncState()).cursors.scope===4);assert.deepEqual(network.requests.at(-1).body.cursors,{scope:2});assert.equal((await fixture.client.read('Entry',{id:'live'})).text,'HTTP 4');
 }finally{await fixture.close();await network.close();}
});

test('HTTP catch-up failures surface and retry without treating the failure as an empty page',async()=>{
 const fixture=await openClient();const errors=[];
 const network=await syncFixture((b,res,n)=>{if(n===1){res.statusCode=503;res.end('unavailable');}else res.end(JSON.stringify(page('retried')));},{scope:1});
 try{
  await fixture.client.subscribe('scope');await fixture.client.connect(network.config,{onError:e=>errors.push(e)});
  await until(async()=>(await fixture.client.read('Entry',{id:'live'}))?.text==='retried');
  assert.equal(errors.length,1);assert.equal(errors[0].status,503);assert.deepEqual(network.requests[1].body.cursors,{scope:0});
 }finally{await fixture.close();await network.close();}
});

test('a reusable server config isolates cancellation and no-channel clients only push',async()=>{
 const a=await openClient(),b=await openClient();const network=await syncFixture((body,res)=>res.end(JSON.stringify(page('shared'))),{scope:1});
 try{
  const ca=await a.client.connect(network.config);await b.client.subscribe('scope');const cb=await b.client.connect(network.config);
  await until(async()=>(await b.client.read('Entry',{id:'live'}))?.text==='shared');assert.equal(network.sockets.length,1);
  await a.client.mutate({name:'Create',operations:[{model:'Entry',op:'create',identity:{id:'local'},values:{text:'  push without channels  ',note:null}}]});
  await until(async()=>(await a.client.syncState()).pending===0);assert.equal(network.requests.filter(r=>r.url==='/sync/mutations').length,1);assert.equal(network.sockets.length,1);
  assert.equal((await a.client.read('Entry',{id:'local'})).text,'push without channels','the receipt alone completes the batch and the row shows the server-returned state; no channel is involved');
  assert.deepEqual(network.requests.find(r=>r.url==='/sync/mutations').body.models,{Entry:1},'the push declares the read contracts its receipt is served at');
  await ca.close();network.sockets[0].send(JSON.stringify(page('still connected',1)));
  await until(async()=>(await b.client.read('Entry',{id:'live'}))?.text==='still connected');await cb.close();
 }finally{await a.close();await b.close();await network.close();}
});

test('removed public modes fail clearly instead of silently opening local-only',async()=>{
 assert.equal(runtime.websocketTransport,undefined);assert.equal(runtime.httpTransport,undefined);assert.equal(runtime.Client.prototype.connectLive,undefined);assert.equal(runtime.Client.prototype.sync,undefined);
 const fixture=await openClient();try{await assert.rejects(fixture.client.connect(async()=>''),/requires server/);}finally{await fixture.close();}
});

test('subscription invalidation cancels pending authentication before the exclusive queue drains',async()=>{
 const fixture=await openClient();const network=await syncFixture((b,res)=>res.end(JSON.stringify(emptyPage(b))));
 const token=Promise.withResolvers(),called=Promise.withResolvers(),gate=Promise.withResolvers(),entered=Promise.withResolvers();
 try{
  await fixture.client.subscribe('scope');await fixture.client.connect({...network.config,token:()=>{called.resolve();return token.promise;}});
  await timeout(called.promise);
  const transaction=fixture.client.transaction(async()=>{entered.resolve();await gate.promise;});await entered.promise;
  const unsubscribe=fixture.client.unsubscribe('scope');token.resolve('late');await new Promise(r=>setTimeout(r,30));
  assert.equal(network.sockets.length,0,'invalidated authentication must not open a socket while unsubscribe awaits SQLite');
  gate.resolve();await transaction;await unsubscribe;
 }finally{gate.resolve();token.resolve('late');await fixture.close();await network.close();}
});

test('bounded receive overflow preserves in-flight HTTP progress and recovers the latest head',async()=>{
 const fixture=await openClient();let head=1;const entered=Promise.withResolvers(),gate=Promise.withResolvers();
 const network=await syncFixture(async(b,res,n)=>{const response={cursors:{scope:{from:b.cursors.scope,to:head,head}},changes:[page(`head ${head}`,head-1).changes[0]]};if(n===1){entered.resolve();await gate.promise;}res.end(JSON.stringify(response));},{scope:1});
 try{
  await fixture.client.subscribe('scope');await fixture.client.connect(network.config);await timeout(entered.promise);
  const socket=network.sockets[0];socket._socket.cork();for(let cursor=1;cursor<=200;cursor++)socket.send(JSON.stringify(page(`live ${cursor}`,cursor)));socket._socket.uncork();
  head=201;gate.resolve();await until(async()=>(await fixture.client.syncState()).cursors.scope===201);
  assert.equal(network.sockets.length,1,'overflow must not restart and starve HTTP catch-up');
  assert.ok(network.requests.length<=4,'bounded queue coalesces recovery work');
 }finally{gate.resolve();await fixture.close();await network.close();}
});

test('push completes from its receipt while the WebSocket upgrade is refused; HTTP catch-up runs only once the upgrade is allowed',async()=>{
 const fixture=await openClient();const {client}=fixture;const errors=[];
 let allowUpgrades=false,upgradeAttempts=0,pushes=0,pulls=0;const stamps={next:0};
 const server=createServer(async(req,res)=>{const chunks=[];for await(const c of req)chunks.push(c);const body=JSON.parse(Buffer.concat(chunks));
  if(req.url==='/sync/mutations'){pushes++;res.end(JSON.stringify(receiptFor(body,stamps)));return;}
  // The catch-up page carries a stamp newer than the receipt's, so it is authority that updates the row.
  pulls++;const caught=page('from catch-up',body.cursors.scope);res.end(JSON.stringify({...caught,changes:[{...caught.changes[0],stamp:stamps.next+1}]}));});
 const ws=new WebSocketServer({noServer:true});
 server.on('upgrade',(req,socket,head)=>{upgradeAttempts++;if(!allowUpgrades){socket.end('HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n');return;}ws.handleUpgrade(req,socket,head,s=>{s.on('message',m=>s.send(ack(JSON.parse(m),{scope:1})));});});
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 try{
  await client.transaction(tx=>tx.direct({model:'Entry',op:'create',identity:{id:'live'},values:{text:'local'}}));
  await client.subscribe('scope');
  await client.mutate({name:'Edit',operations:[{model:'Entry',op:'update',identity:{id:'live'},values:{text:'  edited offline  '}}]});
  assert.equal((await client.read('Entry',{id:'live'})).text,'  edited offline  ','the local prediction is visible before the push');
  await client.connect({url:`http://127.0.0.1:${server.address().port}`,token:'secret'},{onError:e=>errors.push(e)});
  await until(()=>pushes===1&&upgradeAttempts>=2);
  assert.equal(pulls,0,'no HTTP catch-up runs without an acknowledged WebSocket: there is no polling fallback');
  await until(async()=>(await client.syncState()).pending===0);
  assert.equal(pulls,0,'the batch completed from its receipt alone: no page was delivered');
  assert.equal((await client.read('Entry',{id:'live'})).text,'edited offline','the row shows the server-returned state as soon as the response is applied');
  assert.ok(errors.some(e=>/live failed: 503/.test(String(e.message))),`upgrade refusals reach onError: ${errors.map(e=>e.message)}`);
  allowUpgrades=true;
  await until(async()=>(await client.read('Entry',{id:'live'})).text==='from catch-up');
  assert.equal(pushes,1,'the receipt was not re-requested');
  assert.ok(pulls>=1,'catch-up ran over HTTP once the upgrade was acknowledged');
  assert.equal((await client.syncState()).pending,0);
 }finally{await fixture.close();for(const s of ws.clients)s.terminate();await new Promise(r=>ws.close(r));await new Promise(r=>server.close(r));}
});
test('a 401 on both lanes at once shares one refreshAuth; both lanes recover with the new token',async()=>{
 const fixture=await openClient();const {client}=fixture;const errors=[];
 let token='expired',refreshes=0,unauthorized=0,pushes=0,accepted=0,release;const gate=new Promise(r=>{release=r;});const stamps={next:0};
 const server=createServer(async(req,res)=>{const chunks=[];for await(const c of req)chunks.push(c);const body=JSON.parse(Buffer.concat(chunks));
  if(req.headers.authorization!=='Bearer valid'){unauthorized++;res.statusCode=401;res.end();return;}
  // The receipt names no channel: the push completes on its own, whatever the live lane is doing.
  if(req.url==='/sync/mutations'){pushes++;res.end(JSON.stringify(receiptFor(body,stamps)));return;}
  res.end(JSON.stringify(emptyPage(body)));});
 const ws=new WebSocketServer({noServer:true});
 server.on('upgrade',(req,socket,head)=>{if(req.headers.authorization!=='Bearer valid'){unauthorized++;socket.end('HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n');return;}ws.handleUpgrade(req,socket,head,s=>{accepted++;s.on('message',m=>s.send(ack(JSON.parse(m))));});});
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 try{
  await client.transaction(tx=>tx.direct({model:'Entry',op:'create',identity:{id:'live'},values:{text:'local'}}));
  await client.subscribe('scope');
  await client.mutate({name:'Edit',operations:[{model:'Entry',op:'update',identity:{id:'live'},values:{text:'edited offline'}}]});
  await client.connect({url:`http://127.0.0.1:${server.address().port}`,token:()=>token},{onError:e=>errors.push(e),refreshAuth:async()=>{refreshes++;await gate;token='valid';}});
  await until(()=>unauthorized>=2);
  await new Promise(r=>setTimeout(r,100));
  assert.equal(unauthorized,2,'each lane was refused once and neither retried while the refresh was pending');
  assert.equal(refreshes,1,'the second lane joined the pending refresh instead of starting another');
  release();
  await until(()=>accepted>=1&&pushes>=1);
  await until(async()=>(await client.syncState()).pending===0);
  assert.equal((await client.read('Entry',{id:'live'})).text,'edited offline','the receipt completed the batch without any page');
  assert.equal(refreshes,1,'no further refresh once the token is valid');
  assert.equal(unauthorized,2);
 }finally{await fixture.close();for(const s of ws.clients)s.terminate();await new Promise(r=>ws.close(r));await new Promise(r=>server.close(r));}
});
test('a socket the server closes is reconnected after the backoff, resubscribed, and streaming resumes',async()=>{
 const fixture=await openClient();const {client}=fixture;const errors=[];const t0=Date.now();const upgrades=[];const subscribes=[];const sockets=[];
 const server=createServer(async(req,res)=>{const chunks=[];for await(const c of req)chunks.push(c);const body=JSON.parse(Buffer.concat(chunks));res.end(JSON.stringify(emptyPage(body)));});
 const ws=new WebSocketServer({noServer:true});
 server.on('upgrade',(req,socket,head)=>{upgrades.push(Date.now()-t0);ws.handleUpgrade(req,socket,head,s=>{sockets.push(s);s.on('message',m=>{subscribes.push(JSON.parse(m));s.send(ack(JSON.parse(m)));});});});
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 try{
  await client.transaction(tx=>tx.direct({model:'Entry',op:'create',identity:{id:'live'},values:{text:'local'}}));
  await client.subscribe('scope');
  await client.connect({url:`http://127.0.0.1:${server.address().port}`,token:'secret'},{onError:e=>errors.push(e)});
  await until(()=>subscribes.length===1);
  const closedAt=Date.now()-t0;sockets[0].close(1001,'closing');
  await until(()=>upgrades.length===2);
  const waited=upgrades[1]-closedAt;
  assert.ok(waited>=180,`the reconnect waited ${waited} ms; the first retry is due 250 ms later, minus 20% jitter`);
  assert.ok(errors.some(e=>/live disconnected: 1001/.test(String(e.message))),`the close reaches onError: ${errors.map(e=>e.message)}`);
  await until(()=>subscribes.length===2);
  assert.deepEqual(subscribes[1],{type:'subscribe',channels:['scope'],models:{Entry:1}},'the new socket subscribes again without an application event, declaring its read contracts');
  sockets[1].send(JSON.stringify(page('after reconnect',0)));
  await until(async()=>(await client.read('Entry',{id:'live'}))?.text==='after reconnect');
  assert.equal(upgrades.length,2,'one reconnect; no busy loop');
 }finally{await fixture.close();for(const s of ws.clients)s.terminate();await new Promise(r=>ws.close(r));await new Promise(r=>server.close(r));}
});

test('an owner-mismatch refusal reaches onError and leaves the batch frozen for a resend',async()=>{
 const fixture=await openClient();const {client}=fixture;const errors=[];const bodies=[];
 const server=createServer(async(req,res)=>{const chunks=[];for await(const c of req)chunks.push(c);const body=JSON.parse(Buffer.concat(chunks));
  bodies.push(body);res.statusCode=403;res.setHeader('content-type','application/json');res.end(JSON.stringify({code:'client.owner_mismatch'}));});
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 try{
  await client.mutate({name:'Create',operations:[{model:'Entry',op:'create',identity:{id:'live'},values:{text:'local',note:null}}]});
  assert.equal((await client.syncState()).pending,1);
  await client.connect({url:`http://127.0.0.1:${server.address().port}`,token:'secret'},{onError:e=>errors.push(e)});
  await until(()=>bodies.length>=2,'the frozen batch is resent after the refusal');
  assert.ok(errors.some(e=>/client\.owner_mismatch/.test(String(e.message))),`the refusal's code reaches onError: ${errors.map(e=>e.message)}`);
  assert.equal((await client.syncState()).pending,1,'the refused batch stays pending, not dropped or completed');
  assert.deepEqual(bodies[1],bodies[0],'the same request body is resent on the next cycle');
 }finally{await fixture.close();await new Promise(r=>server.close(r));}
});

test('what a page cannot apply reaches onError as an AxtonReport: read failures, skipped changes and divergence',async()=>{
 const fixture=await openClient();const {client}=fixture;const errors=[];
 const network=await syncFixture((b,res)=>res.end(JSON.stringify(emptyPage(b))));
 // The push lane is refused so the local edit stays queued while authority lands beneath it.
 const original=network.requests;
 try{
  await client.subscribe('scope');await client.connect(network.config,{onError:e=>errors.push(e)});
  await until(()=>network.sockets.length===1);
  network.sockets[0].send(JSON.stringify(page('first')));
  await until(async()=>(await client.read('Entry',{id:'live'}))?.text==='first');
  network.sockets[0].send(JSON.stringify({cursors:{scope:{from:1,to:3,head:3}},changes:[
   {model:'Entry',identity:{id:'live'},stamp:9,error:'loader.failed'},
   {model:'Entry',identity:{id:'bad'},stamp:2,state:{text:5,note:null}},
  ]}));
  await until(()=>errors.length===2);
  assert.ok(errors.every(e=>e instanceof runtime.AxtonReport));
  assert.equal(errors[0].kind,'readFailed');assert.equal(errors[0].code,'loader.failed');assert.deepEqual(errors[0].identity,{id:'live'});assert.equal(errors[0].stamp,9);
  assert.equal(errors[1].kind,'skipped');assert.deepEqual(errors[1].identity,{id:'bad'});
  assert.equal((await client.read('Entry',{id:'live'})).text,'first','a read failure keeps the local content');
  assert.equal((await client.syncState()).cursors.scope,3,'the page still moved the cursor');
  assert.match(errors[0].message,/readFailed: Entry .* stamp 9 \(loader.failed\)/);
 }finally{await fixture.close();await network.close();assert.equal(original,network.requests);}
});

test('a queued edit whose replay fails over new authority is reported as diverged and still sent',async()=>{
 const fixture=await openClient();const {client}=fixture;const errors=[];let allowPush=false;const stamps={next:10};
 const server=createServer(async(req,res)=>{const chunks=[];for await(const c of req)chunks.push(c);const body=JSON.parse(Buffer.concat(chunks));
  if(req.url==='/sync/mutations'){if(!allowPush){res.statusCode=503;res.end('later');return;}res.end(JSON.stringify(receiptFor(body,stamps)));return;}
  res.end(JSON.stringify(emptyPage(body)));});
 const ws=new WebSocketServer({server});const sockets=[];ws.on('connection',s=>{sockets.push(s);s.on('message',m=>s.send(ack(JSON.parse(m))));});
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 try{
  await client.subscribe('scope');await client.connect({url:`http://127.0.0.1:${server.address().port}`,token:'secret'},{onError:e=>errors.push(e)});
  await until(()=>sockets.length===1);
  sockets[0].send(JSON.stringify(page('first')));
  await until(async()=>(await client.read('Entry',{id:'live'}))?.text==='first');
  await client.mutate({name:'Edit',operations:[{model:'Entry',op:'update',identity:{id:'live'},values:{text:'edited offline'}}]});
  assert.equal((await client.read('Entry',{id:'live'})).text,'edited offline');
  // The server deleted the record: the update cannot replay over nothing.
  sockets[0].send(JSON.stringify({cursors:{scope:{from:1,to:2,head:2}},changes:[{model:'Entry',identity:{id:'live'},stamp:2,state:null}]}));
  await until(()=>errors.some(e=>e instanceof runtime.AxtonReport));
  const diverged=errors.find(e=>e instanceof runtime.AxtonReport);
  assert.equal(diverged.kind,'diverged');assert.equal(typeof diverged.ordinal,'number');assert.deepEqual(diverged.identity,{id:'live'});
  assert.equal(await client.read('Entry',{id:'live'}),null,"the server's row (a deletion) is visible");
  const status=await client.syncState();assert.equal(status.pending,1,'the mutation is still queued');
  // The push lane retries after its backoff; the receipt completes the diverged mutation.
  allowPush=true;await until(async()=>(await client.syncState()).pending===0);
  assert.equal((await client.read('Entry',{id:'live'})).text,'edited offline','the diverged edit was sent and completed from its receipt');
 }finally{await fixture.close();for(const s of ws.clients)s.terminate();await new Promise(r=>ws.close(r));await new Promise(r=>server.close(r));}
});

test('a receipt record the client cannot apply reaches onError and the batch still completes',async()=>{
 const fixture=await openClient();const {client}=fixture;const errors=[];const stamps={next:10};let pushes=0;
 const server=createServer(async(req,res)=>{const chunks=[];for await(const c of req)chunks.push(c);const body=JSON.parse(Buffer.concat(chunks));
  if(req.url==='/sync/mutations'){pushes++;const receipt=receiptFor(body,stamps);receipt.records[0].state={text:5,note:null};res.end(JSON.stringify(receipt));return;}
  res.end(JSON.stringify(emptyPage(body)));});
 const ws=new WebSocketServer({server});ws.on('connection',s=>s.on('message',m=>s.send(ack(JSON.parse(m)))));
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 try{
  await client.connect({url:`http://127.0.0.1:${server.address().port}`,token:'secret'},{onError:e=>errors.push(e)});
  await client.mutate({name:'Create',operations:[{model:'Entry',op:'create',identity:{id:'odd'},values:{text:'local',note:null}}]});
  await until(async()=>(await client.syncState()).pending===0);
  const skipped=errors.find(e=>e instanceof runtime.AxtonReport);
  assert.ok(skipped,`a report reached onError: ${errors}`);
  assert.equal(skipped.kind,'skipped');assert.deepEqual(skipped.identity,{id:'odd'});
  assert.equal(skipped.detail.batch,1);assert.equal(typeof skipped.detail.error,'string');
  assert.equal(pushes,1,'the batch completed; the receipt was not re-requested');
 }finally{await fixture.close();for(const s of ws.clients)s.terminate();await new Promise(r=>ws.close(r));await new Promise(r=>server.close(r));}
});
