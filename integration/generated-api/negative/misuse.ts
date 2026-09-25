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
 // The load status is part of that immutable snapshot, and this milestone
 // introduces no task-cancel or forced-refresh API
 // ([#151](https://github.com/zanminwang/axton/issues/151)).
 // @ts-expect-error a load status is immutable too
 subscription.status.bootstrap.phase='complete';
 // @ts-expect-error a registered task cannot be cancelled
 void subscription.bootstrap.cancel();
 // @ts-expect-error there is no forced refresh
 void subscription.refresh();
}
