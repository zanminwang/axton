// Generated TypeScript API misuse that must NOT compile. `tsc -p
// integration/generated-api` type-checks this file, and every
// `@ts-expect-error` below has to be the error it names; the Dart twin is
// misuse.dart ([#150](https://github.com/zanminwang/axton/issues/150)).
import {GeneratedClient, type GeneratedTransaction, Draft, DraftCreate, Entry, ApplicationTransaction, PublishEntryOutput} from '../client.ts';

export function boundSurfaceMisuse(client:GeneratedClient,tx:GeneratedTransaction){
 // @ts-expect-error exactly one Stream is selected at bound open
 void client.streams;
 // @ts-expect-error callbacks cannot mutate Store enrollment
 void tx.streams;
 // @ts-expect-error anonymous remote writes are retired
 void client.mutate;
 // @ts-expect-error queued Query lane is retired
 void client.queries.enqueue;
 // @ts-expect-error split Mutation direct lane is retired
 void client.mutations.call;
 // @ts-expect-error path alone cannot establish binding
 void GeneratedClient.open({path:'only.sqlite'});
}

// Creation defaults (#27): only a create input may omit defaulted fields.
export function createMisuse(){
 // @ts-expect-error a field without a creation default is still required
 const missing:DraftCreate={};
 // @ts-expect-error the complete record keeps every field required
 const incomplete:Draft={body:'x',mood:'calm',created:new Date(),note:null,memo:null};
 // @ts-expect-error an omitted field is left out, not set to undefined
 const undef:DraftCreate={memo:null,body:undefined};
 const ok:DraftCreate={memo:null};
 return [missing,incomplete,undef,ok];
}
// A schema without Loads generates no `loads` facade (#173).
export function loadMisuse(client:GeneratedClient){
 // @ts-expect-error no Load is declared, so there is no loads facade
 return client.loads;
}

// Model Fetch ([#153](https://github.com/zanminwang/axton/issues/153)): the
// generated identity and a boolean `store` are the whole input, and Fetch is
// absent from local transactions.
export async function fetchMisuse(client:GeneratedClient,tx:GeneratedTransaction,row:Entry,at:Date){
 // @ts-expect-error a composite identity needs every component
 await client.fetch.placement({shelf:'s'});
 // @ts-expect-error a DateTime identity component is a Date
 await client.fetch.placement({shelf:'s',at:'2026-01-01'});
 // @ts-expect-error a UUID identity is a string
 await client.fetch.entry({id:1});
 // @ts-expect-error an identity literal names only identity fields
 await client.fetch.entry({id:row.id,title:row.title});
 // @ts-expect-error there is no refresh option either
 await client.fetch.entry({id:row.id},{refresh:true});
 // @ts-expect-error storage is a boolean
 await client.fetch.entry({id:row.id},{store:'false'});
 // @ts-expect-error a Model the schema does not declare has no Fetch method
 await client.fetch.unknown({id:row.id});
 // @ts-expect-error Fetch is unavailable within a local transaction
 void tx.fetch;
 // @ts-expect-error the transaction port cannot fetch
 void tx.transaction.fetchModel;
 // The result is the snapshot or null, never a live handle.
 const result=await client.fetch.placement({shelf:'s',at});
 // @ts-expect-error the result may be null
 void result.label;
}

// A named Mutation takes its typed input; a caller reads first in the same
// transaction instead of passing a callback.
export async function transactionMisuse(client:GeneratedClient,tx:ApplicationTransaction,row:Entry){
 const id=row.id;
 // @ts-expect-error there is no direct route inside a transaction
 await tx.mutations.call.rename({id,title:'t'});
 // @ts-expect-error Queries are unavailable inside a transaction
 void tx.queries;
 // @ts-expect-error Fetch is unavailable inside a transaction
 void tx.fetch;
 // @ts-expect-error a Mutation keeps its declared args
 await tx.mutations.rename({id,title:1});
 // @ts-expect-error `local` is an option, never a business arg
 await tx.mutations.rename({id,title:'t',local:async()=>{}});
 // @ts-expect-error a store selection names only store-eligible outputs
 await tx.mutations.publishEntry({entry:row,composition:id},{store:{missing:false}});
 // @ts-expect-error the Call observes the declared output
 const wrong:Promise<import('../client.ts').Call<string>>=tx.mutations.publishEntry({entry:row,composition:id});
 // @ts-expect-error legacy local option is retired
 await client.mutations.rename({id,title:'t'},{local:async()=>{}});
 // @ts-expect-error split direct Mutation lane is retired
 await client.mutations.call.rename({id,title:'t'},{local:async()=>{}});
 // @ts-expect-error the Mutation input callback is retired
 await tx.mutations.publishEntry(async()=>({entry:row,composition:id}));
 // @ts-expect-error the Mutation input callback is retired on the client too
 await client.mutations.rename(()=>({id,title:'t'}));
 // @ts-expect-error there is no companion context to pass one to
 void import('../client.ts').then(module=>module.CompanionContext);
 const call:PublishEntryOutput|undefined=(await (await tx.mutations.publishEntry({entry:row,composition:id})).wait()).result;
 return [wrong,call];
}

async function retiredScopeAliases(client:GeneratedClient,tx:GeneratedTransaction):Promise<void>{
 // @ts-expect-error the generated client exposes scopes only
 void client.channels;
 // @ts-expect-error the generated transaction exposes scopes only
 void tx.channels;
 // @ts-expect-error no enrollment facade in local transactions
 void tx.streams;
}
void retiredScopeAliases;

// Field inheritance keeps complete records separate from defaultable inputs.
import type {DraftFields, DraftIdentity} from '../generated.ts';
const defaultableDraft = {memo:null} as DraftCreate;
// @ts-expect-error a create input may omit shared fields with defaults
const fieldsFromCreate:DraftFields = defaultableDraft;
// @ts-expect-error abstract field types have no concrete identity helper
const identityFromFields:DraftIdentity = fieldsFromCreate.identity;
void identityFromFields;

function abstractModelCrudMisuse(client:GeneratedClient) {
 // @ts-expect-error abstract fields have no local Model/table accessor
 client.models.draftFields;
}
void abstractModelCrudMisuse;
