// Generated TypeScript API misuse that must NOT compile. `tsc -p
// integration/generated-api` type-checks this file, and every
// `@ts-expect-error` below has to be the error it names; the Dart twin is
// misuse.dart ([#150](https://github.com/zanminwang/axton/issues/150)).
import type {GeneratedClient, Subscription, Draft, DraftCreate, AddDraftArgs} from '../client.ts';

export function scopeMisuse(client:GeneratedClient,subscription:Subscription){
 // @ts-expect-error a status snapshot is immutable
 subscription.status.active=false;
 // @ts-expect-error the Scope a handle names is fixed for its lifetime
 subscription.scope='other';
 // @ts-expect-error the first Scope API deliberately omits a get-only accessor
 void client.scopes.get('project:123');
}

// Creation defaults (#27): only a create input may omit defaulted fields.
export function createMisuse(){
 // @ts-expect-error a field without a creation default is still required
 const missing:DraftCreate={};
 // @ts-expect-error the complete record keeps every field required
 const incomplete:Draft={body:'x',mood:'calm',created:new Date(),note:null,memo:null};
 // @ts-expect-error an omitted field is left out, not set to undefined
 const undef:DraftCreate={memo:null,body:undefined};
 const ok:AddDraftArgs={draft:{memo:null}};
 return [missing,incomplete,undef,ok];
}
