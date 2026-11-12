import {mkdtemp,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createRequire} from 'node:module';
import {GeneratedClient,schema} from './client.ts';
import {createClient} from '../../packages/frontend/client-js/api/runtime.mts';
import {Transaction} from '../../packages/frontend/client-js/api/transaction.mts';
import {streamServer} from '../bindings/client-js/store-fixture.mjs';
const directory=await mkdtemp(join(tmpdir(),'generated-native-'));
const connection={url:'http://127.0.0.1:1',token:'offline'};
const id='123e4567-e89b-42d3-a456-426614174000';
const entryState=(title:string,at:string)=>({title,note:null as string|null,at,tags:[],status:'active'});
// Seeds: rows the fixture's Stream delivers, at Bootstrap and later live.
const server=streamServer([
 {model:'Entry',identity:{id},state:entryState('native','2026-01-01T00:00:00.000Z')},
 {model:'Book',identity:{id:'b'},state:{title:'Book'}},
 {model:'Comment',identity:{id:'c'},state:{bookId:'b',text:'Comment'}},
]);
const native=createRequire(import.meta.url)('../../bindings/node/axton-node.node');
const Fixture=createClient(native,Transaction,server.network);
// The generated facade over a client of the fixture's network.
const Generated=GeneratedClient as unknown as new (client:unknown)=>GeneratedClient;
const client=new Generated(await Fixture.open({path:join(directory,'state.sqlite'),schema,stream:'User:viewer',connection}));
const delivered=async(rows:{model:string;identity:object;state:object}[])=>{
 const cursor=await server.send(rows);
 const deadline=Date.now()+5000;
 while((await client.syncState()).cursors['User:viewer']!==cursor){
  if(Date.now()>deadline)throw Error(`delivery ${cursor} timed out`);
  await new Promise(resolve=>setTimeout(resolve,5));
 }
};
try {
 await client.bootstrap();
 const row=await client.models.entry.get({id});
 if(row?.note!==null||row.title!=='native'||!(row.at instanceof Date))throw Error('native roundtrip');
 if((await client.models.entry.query()).length!==1)throw Error('native query');
 const filtered=await client.models.entry.query({where:{at:new Date('2026-01-01T01:00:00+01:00'),note:null},orderBy:[{field:'title',direction:'descending'}],limit:1});
 if(filtered.length!==1)throw Error('typed query normalization');
 if((await client.models.comment.book({id:'c'}))?.id!=='b')throw Error('forward relation');
 if((await client.models.book.comments({id:'b'})).length!==1)throw Error('inverse relation');
 // Read-only SQL over several Models through the generated client (#184):
 // a commit to either joined Model re-emits the join.
 const joined:unknown[]=[];const stopJoined=client.watchSql('SELECT b.title AS title, c.text AS text FROM Book b JOIN Comment c ON c.bookId = b.id ORDER BY c.id',[],rows=>joined.push(rows.map(r=>`${r.title}:${r.text}`)));
 await new Promise(r=>setTimeout(r,20));
 await delivered([{model:'Comment',identity:{id:'c2'},state:{bookId:'b',text:'Second'}}]);
 await new Promise(r=>setTimeout(r,20));
 await delivered([{model:'Book',identity:{id:'b'},state:{title:'Renamed'}}]);
 await new Promise(r=>setTimeout(r,20));stopJoined();
 if(JSON.stringify(joined)!==JSON.stringify([['Book:Comment'],['Book:Comment','Book:Second'],['Renamed:Comment','Renamed:Second']]))throw Error(`watchSql ${JSON.stringify(joined)}`);

 // Each emission owns its mutable objects, arrays and Dates; unchanged rows
 // are decoded only in the SDK cache, then copied for each application snapshot.
 const snapshots: import('./generated.ts').Entry[][]=[];
 const errors: unknown[]=[];
 const stopEntries=client.models.entry.watch({},rows=>snapshots.push(rows),error=>errors.push(error));
 const until=async(predicate:()=>boolean)=>{
  const deadline=Date.now()+5000;
  while(!predicate()){
   if(Date.now()>deadline)throw Error('typed watch snapshot timed out');
   await new Promise(resolve=>setTimeout(resolve,5));
  }
 };
 try {
  await until(()=>snapshots.length===1);
  const first=snapshots[0]!;
  const callerRow=first.find(entry=>entry.id===id)!;
  callerRow.title='application';callerRow.tags.push('application');callerRow.at.setUTCFullYear(1999);
  first.reverse();first.pop();
  await delivered([{model:'Entry',identity:{id:'123e4567-e89b-42d3-a456-426614174001'},state:entryState('other row','2026-01-02T00:00:00.000Z')}]);
  await until(()=>snapshots.length===2);
  const retained=snapshots[1]!;
  const unchanged=retained.find(entry=>entry.id===id)!;
  if(unchanged.title!=='native'||unchanged.tags.length!==0||unchanged.at.getUTCFullYear()!==2026)
   throw Error('application mutation corrupted unchanged watch cache');
  await delivered([{model:'Entry',identity:{id},state:entryState('new native','2026-01-01T00:00:00.000Z')}]);
  await until(()=>snapshots.length===3);
  if(unchanged.title!=='native'||snapshots[2]!.find(entry=>entry.id===id)!.title!=='new native')
   throw Error('later patch changed an earlier typed watch result');
  if(errors.length)throw errors[0];
 }finally{stopEntries();}
 // Paused, so the Mutation stays queued instead of uploading.
 await client.connection?.pause();
 const pendingId='123e4567-e89b-42d3-a456-426614174002';
 const call=await client.mutations.publishEntry({entry:{id:pendingId,title:'pending',note:null,at:new Date(),tags:[],status:'active'},composition:id});
 const state=await client.models.entry.syncState({id:pendingId});
 if(!state.pending.some(p=>p.name==='PublishEntry'&&p.phase==='queued'))throw Error('typed record sync state');
 if((await client.syncState()).pending<1||client.clientId==='')throw Error('client sync state');
 if(call.status!=='pending')throw Error('named Call remains pending without backend settlement');
}finally{await client.close();await rm(directory,{recursive:true,force:true})}

// Failed setup must not retain physical ownership of the requested Store.
const {strict:assert}=await import('node:assert');
const failedDirectory=await mkdtemp(join(tmpdir(),'generated-failed-open-'));
let reopened:GeneratedClient|undefined;
try{
 const path=join(failedDirectory,'state.sqlite');
 await assert.rejects(GeneratedClient.open({path,stream:'User:viewer',connection:{...connection,url:'http://['}}),/Invalid URL/);
 reopened=await GeneratedClient.open({path,stream:'User:viewer',connection});
 assert.equal((await reopened.syncState()).pending,0);
}finally{await reopened?.close();await rm(failedDirectory,{recursive:true,force:true});}
