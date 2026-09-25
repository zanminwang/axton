// Generated TypeScript API misuse that must NOT compile. `tsc -p
// integration/generated-api` type-checks this file, and every
// `@ts-expect-error` below has to be the error it names; the Dart twin is
// misuse.dart ([#150](https://github.com/zanminwang/axton/issues/150)).
import type {GeneratedClient, Subscription} from '../client.ts';

export function scopeMisuse(client:GeneratedClient,subscription:Subscription){
 // @ts-expect-error a status snapshot is immutable
 subscription.status.active=false;
 // @ts-expect-error the Scope a handle names is fixed for its lifetime
 subscription.scope='other';
 // @ts-expect-error the first Scope API deliberately omits a get-only accessor
 void client.scopes.get('project:123');
}
