import type {Handlers,Loaders,EntryV1} from './backend.ts';
import type {GeneratedClient} from './client.ts';
import type {Transaction as RawTransaction} from '../../packages/client-js/index.mts';
import {CreateEntry,EditEntry,RemoveEntries,decodeEntry,encodeEntry,EntryModel,EntryLiveModel,GeneratedTransaction,Mutate,type Entry,type ReadPort,type LivePort,type WritePort,type MutationName,type SyncState} from './generated.ts';
const row:Entry={id:'123e4567-e89b-42d3-a456-426614174000',title:'hello',note:null,at:new Date('2026-01-01T00:00:00Z'),tags:['x'],status:'active'};
function check(v:unknown,m:string){if(!v)throw Error(m)}
const create=CreateEntry({entry:row});
check(!('id' in (create.operations[0] as {values:object}).values),'identity leaked into state');
check(JSON.stringify(decodeEntry(encodeEntry(row)))===JSON.stringify(row),'source conversion');
const patch=EditEntry({entry:{identity:{id:row.id},values:{note:null}}});
check(JSON.stringify((patch.operations[0] as {values:object}).values)==='{"note":null}','presence semantics');
check(RemoveEntries({entries:[]}).operations.length===0,'optional/list');
const reads:ReadPort={async read(){return encodeEntry(row)},async querySpec(){return [encodeEntry(row)]},async related(){return null},async referencing(){return []}};
if(false){
 const entries=new EntryModel(reads);
 // @ts-expect-error lists cannot be query predicates
 entries.query({where:{tags:[]}});
 // @ts-expect-error enum ordering is not defined
 entries.query({orderBy:[{field:'status',direction:'ascending'}]});
 // @ts-expect-error date filter must be a Date
 entries.query({where:{at:'2026-01-01'}});
 const live:LivePort={...reads,async direct(){},watch(){return ()=>{}},async syncState(){return {pending:[],rejections:[]}}};
 new EntryLiveModel(live).watch({},(rows)=>rows[0]?.at.getTime());
 // A record's sync state is typed by model: the identity is the model's, pending names are the schema's mutations.
 const state:Promise<SyncState>=new EntryLiveModel(live).syncState({id:row.id});
 void state.then(s=>{const name:MutationName=s.pending[0]!.name;const diverged:boolean|undefined=s.pending[0]!.diverged;void name;void diverged;});
 // @ts-expect-error syncState takes the model's identity
 new EntryLiveModel(live).syncState({title:'x'});
 // @ts-expect-error a pending name is one of the schema's mutations
 const unknown:SyncState['pending'][number]={ordinal:1,name:'NoSuchMutation',phase:'queued',prerequisites:[]};
 // Mutations run outside a transaction through any port that can enqueue one.
 const direct:Promise<number>=new Mutate({async mutate(){return 1}}).editEntry({entry:{identity:{id:row.id},values:{note:null}}});
 const writes:WritePort={...reads,async direct(){}};
 // @ts-expect-error watch is not available inside a transaction
 new GeneratedTransaction(writes).models.entry.watch({},()=>{});
 // @ts-expect-error named mutations are unavailable in local transactions
 new GeneratedTransaction(writes).mutate;
 // @ts-expect-error actions are unavailable in local transactions
 new GeneratedTransaction(writes).actions;
 const rawTx={} as RawTransaction;
 // @ts-expect-error raw transactions cannot enqueue named mutations
 rawTx.mutate;
 // @ts-expect-error raw transactions have no action namespace
 rawTx.actions;

 // @ts-expect-error identity is immutable in patch
 EditEntry({entry:{identity:{id:row.id},values:{id:'bad'}}});
 // @ts-expect-error mutation forbids tags
 EditEntry({entry:{identity:{id:row.id},values:{tags:[]}}});
 // @ts-expect-error nonnullable title
 EditEntry({entry:{identity:{id:row.id},values:{title:null}}});
 // @ts-expect-error enum typo
 const bad:Entry={...row,status:'typo'};

 type Tx={rows:Map<string,object>};
 const shorthand:Handlers<Tx>['addBook']=async({input,tx,changes,publish})=>{tx.rows.set(input.book.id,input.book);changes.add(input.book);publish({channel:'c'})};
 const grouped:Handlers<Tx>['editEntry']={
  async v1({input,publish}){publish({channel:'c',records:[input.target]})},
  // A record added after the publication call still joins the default publication; explicit records may name an empty set.
  async v2({input,changes,publish}){publish({channel:'c'});changes.add(input.entry);publish({channel:'audit',records:[]});changes.records.map(r=>r.model)},
  // @ts-expect-error v3 is not a retained version of EditEntry
  async v3(){},
 };
 // @ts-expect-error a mutation with two retained versions cannot register a bare function
 const bare:Handlers<Tx>['editEntry']=async()=>{};
 // @ts-expect-error every retained version must be registered
 const partial:Handlers<Tx>['editEntry']={v2:async({input,publish})=>{publish({channel:'c',records:[input.entry]})}};
 // @ts-expect-error handlers publish through `publish`; there is no notify and no return value
 const legacy:Handlers<Tx>['addBook']=async({notify})=>{notify({channel:'c',records:[]})};
 // @ts-expect-error a handler has no return value to select a channel with
 const returned:Handlers<Tx>['addBook']=async()=>({channel:'c'});
 // @ts-expect-error loaders receive no channel
 const channelled:Loaders<Tx>['book']=async({ids,channel})=>ids.map(id=>({...id,title:String(channel)}));

 // Loaders follow the same shape; a retained older contract has its own record type.
 const v1Row:EntryV1={id:row.id,title:'old',note:null,at:row.at,status:'active'};
 const versionedLoaders:Loaders<Tx>['entry']={
  async v1({ids}){return ids.map(()=>v1Row)},
  async v2({ids}){return ids.map(()=>row)},
  // @ts-expect-error v3 is not a retained version of Entry
  async v3(){return []},
 };
 const shorthandLoader:Loaders<Tx>['book']=async({ids})=>ids.map(id=>({...id,title:'t'}));
 // @ts-expect-error a model with two retained versions cannot register a bare function
 const bareLoader:Loaders<Tx>['entry']=async()=>[];
 // @ts-expect-error every retained model version must be registered
 const partialLoader:Loaders<Tx>['entry']={v2:async({ids})=>ids.map(()=>row)};
 // @ts-expect-error a v1 loader cannot return a value outside the v1 contract
 const wrongEnum:EntryV1={...v1Row,status:'typo'};
 // @ts-expect-error the v1 contract has no tags
 const extra:EntryV1={...v1Row,tags:[]};
}
const tx=new GeneratedTransaction({...reads,async direct(op){check(JSON.stringify(op)===JSON.stringify({model:'Entry',op:'delete',identity:{id:row.id}}),'local write');}});
async function main(){check((await tx.models.entry.get({id:row.id}))?.at instanceof Date,'read decode');check((await tx.models.entry.query()).length===1,'query facade');check(await new Mutate({async mutate(m){check(JSON.stringify(m)===JSON.stringify(create),'forwarding');return 1}}).createEntry({entry:row})===1,'mutate facade');await tx.models.entry.delete({id:row.id});}
main();
// The generated client carries the schema check and the rebuild call.
type Rebuilt=Awaited<ReturnType<GeneratedClient['rebuild']>>;
type SchemaCheck=Awaited<ReturnType<GeneratedClient['syncState']>>['schema'];
const rebuildShape=(report:Rebuilt,state:SchemaCheck):[number,number,boolean]=>[report.leftPending,report.leftDirect,state.rebuilt];
void rebuildShape;
function misuse(app:GeneratedClient){
 // @ts-expect-error rebuild takes an options object
 void app.rebuild(true);
}
void misuse;
