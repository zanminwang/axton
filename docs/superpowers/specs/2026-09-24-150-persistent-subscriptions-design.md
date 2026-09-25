# Persistent Scope subscriptions and first initialization

Status: reviewed for implementation handoff; the user authorized a separate cloud agent to implement, verify and merge #150 followed by #151. No implementation is claimed. Issue: [#150](https://github.com/zanminwang/axton/issues/150). Companion: [whole-Scope bootstrap](2026-09-24-151-scope-bootstrap-design.md).

## 1. Goal and boundary

Existing subscriptions and delivery cursors are already durable. This issue changes first-subscription initialization from cursor zero to the negotiated server head, adds a stable observable SDK handle, and removes unnecessary SDK-driven socket cancellation. It does not introduce persistence from scratch.

A subscription is a durable local request to receive a Scope's changes. Its SDK object is a handle, not the owner of a socket or of the subscription's lifetime. Subscribe works offline, repeated calls reuse the same subscription, and restart resumes committed progress.

The user approved the following boundary: a Scope retains existing publication semantics. A record must have been published to the delivery range, and the canonical Loader controls its visibility. Current membership and move-out rules remain #140. A Scope does not own local records. This work does not add query-driven sync, relation loading, per-Model loading strategies or data deletion on unsubscribe.

## 2. Public API

```ts
const subscription = await client.scopes.subscribe("project:123");
const same = await client.scopes.subscribe("project:123");
assert(subscription === same);

const stopWatching = subscription.watch(status => render(status));
console.log(subscription.status);
await subscription.unsubscribe();
stopWatching();
```

```ts
type SubscriptionStatus = Readonly<{
  active: boolean;
  initialization: "pending" | "ready";
  connection: "offline" | "connecting" | "catching-up" | "live" | "stopped";
}>;

interface Subscription {
  readonly scope: string;
  readonly status: SubscriptionStatus;
  watch(listener: (status: SubscriptionStatus) => void): () => void;
  unsubscribe(): Promise<void>;
}
```

Dart exposes `Future<Subscription> scopes.subscribe(String scope)`, `String get scope`, `SubscriptionStatus get status`, `Stream<SubscriptionStatus> watch()`, and `Future<void> unsubscribe()`. Use generated immutable status value types/enums rather than untyped maps. Watching delivers the current snapshot then changes; cancellation removes the observer, not the persistent subscription.

`subscribe` resolves after the local transaction commits. It does not await authentication, connection or server acknowledgment. Concurrent SDK calls for the same active Scope are serialized/coalesced so they obtain one cached handle keyed by persistent subscription identity. A local failure rejects without publishing a new handle. Missing network configuration is an offline condition; callers use the existing client connection setup to supply transport/authentication.

The existing generated `channels` facade delegates to the same runtime during this delivery milestone; the new `scopes` facade is authoritative for new examples. Do not keep a second cursor-zero subscription algorithm behind the old spelling. #152 owns removal of old public names and backend/wire vocabulary changes. Internally `channel` names and wire fields remain unchanged here. A get-only accessor is deliberately omitted from this first API.

An explicit unsubscribe commits local removal, then marks the old handle inactive/stopped. Repeating it on the same closed handle is a no-op; it must not delete a newly created subscription with the same Scope name. Other work through an old handle fails with `subscription.closed`. Client close marks handles stopped and cancels observers but does not delete subscriptions. SDK observer exceptions are reported through the host's uncaught-error mechanism after commit, never converted to transport failure or transaction rollback.

## 3. Persistent state

Extend `axton_subscription`, retaining its existing `channel` key until #152:

```sql
channel          TEXT PRIMARY KEY,
subscription_id  INTEGER NOT NULL UNIQUE,
starting_cursor  INTEGER,
cursor           INTEGER,
CHECK ((starting_cursor IS NULL AND cursor IS NULL) OR
       (starting_cursor IS NOT NULL AND cursor IS NOT NULL AND
        starting_cursor >= 0 AND cursor >= starting_cursor))
```

All counters obey the existing 0..2^53-1 constraint. Add a monotonically allocated `next_subscription` counter to `axton_client`; allocation and row insertion share the local transaction. IDs are not recycled after unsubscribe. Carry the next-ID counter forward through a replica rebuild when available, invalidate all old handles and in-flight controllers on replica replacement, and never treat an equal numeric ID in a replaced replica as the same handle. They fence stale handles, acknowledgments and requests, including re-creation at the same Scope name. They are client-local metadata and need not travel to the backend.

No row means unsubscribed. A row with both cursor fields NULL means durable intent exists but its first boundary is not yet committed. Zero is a valid initialized position and must never stand for uninitialized. Repeated subscribe uses insert-if-absent, not the existing upsert that can overwrite a cursor.

Only the first initialization transition writes both cursor fields together. Subsequent normal page application advances `cursor` only. `starting_cursor` stays fixed for that subscription identity. Bootstrap adds separate same-row fields in #151.

This is a pre-release metadata contract change, not a backward compatibility project. Incompatible stored layouts use the existing explicit replica-rebuild path; the implementation must preserve requested Scope names, allocate fresh subscription identities, reset initialization and invalidate bootstrap coverage. Do not silently claim old loading completeness across a rebuild. A normal reopen of the new layout preserves all progress.

## 4. Engine ownership and state transitions

### Local registration

The frontend command commits durable intent and emits the existing post-commit work/subscription wake. A rolled-back transaction emits no subscription update. The SDK no longer cancels the live session before attempting the write. The Rust live controller decides whether committed membership actually changed.

The current one-subscribe-frame-per-socket protocol may still reconnect on a genuine membership change in #150. #144 separately replaces that behavior with updates on a healthy socket. Identical subscribe calls cause neither generation changes nor reconnects. Both issues must use this same ledger and epoch fencing.

### First remote initialization

1. When configured connectivity is available, the controller opens a session for desired Scopes. It snapshots their persistent subscription IDs with the local session generation.
2. Existing server negotiation captures heads and establishes replay/listener coverage. The listener-then-initial-drain behavior must cover changes racing the head read.
3. Validate the acknowledgment against the current session and desired set before writing anything.
4. In one local transaction, initialize only still-matching rows whose `starting_cursor` is NULL: set `starting_cursor = cursor = acknowledgedHead`.
5. Existing initialized rows keep their cursor. Compare their cursor to the acknowledged head and use ordinary catch-up for any gap.
6. Publish status after commit; then apply/buffer normal stream pages using the existing gap-recovery rules.

Before the initialization transaction commits, no subsequent stream page may advance that row. A crash before commit leaves NULL and retries first initialization; no promise about an earlier offline/requested wall-clock time was made. A crash after commit resumes from the committed cursor. Acknowledgments from old sessions or subscription identities are discarded.

An unknown Scope with server head zero can initialize at zero under existing Loader/publication behavior. No new Scope authorization layer is inferred; authentication and canonical Loader visibility remain the current contract.

### Reconnect

Every new connection negotiates a session; this is not a new synchronization origin. For a saved cursor 100 and new head 120, fetch/apply the gap instead of replacing 100 with 120. An unexpected head below committed progress is a reported protocol/server-state fault, not permission to rewind or silently reinitialize.

### Mixed subscriptions and delta recovery

One socket carries only the locally desired Scope set. If A has cursor 80, B has an uninitialized row, and C has no row, send A/B only. An acknowledgment of A=100 and B=200 preserves A=80 and schedules catch-up, while atomically initializing B at 200. C participates in neither automatic pulls nor live delivery. A manual delta request with no initialized subscriptions produces no work; never coerce NULL into zero.

Retain the server initial drain after listener registration: it scans from the acknowledged heads to catch publications racing head capture and listener setup. Retain client automatic delta recovery on reconnect, stream gaps and overflow. Only implicit historical loading for a new subscription is removed. Local cursor 120 accepts a page spanning 120 to 125 directly; a page spanning 124 to 125 is buffered while a pull from 120 fills the gap. Numeric record cursors need not be contiguous because invalidations compact. With no desired Scopes the live lane is idle; Action delivery remains independent.

### Local transactions and status

Existing transaction-scoped subscription intent operations remain local-only; returning SDK handles is a standalone API behavior after commit. Status is derived from persisted initialization plus the Engine's current transport state. `live` means the session is delivering normally, not that all historical records are loaded. `ready` initialization means a durable starting boundary exists, not that the socket is currently connected.

## 5. Core interfaces

Put the new ledger behavior in `crates/client/src/subscriptions.rs`, leaving record stamp storage in `ledger.rs`:

```rust
pub struct SubscriptionState {
    pub scope: String,
    pub subscription_id: u64,
    pub starting_cursor: Option<u64>,
    pub cursor: Option<u64>,
}
// Methods on Client; operations own a local transaction.
pub fn ensure_subscription(&mut self, scope: &str) -> Result<SubscriptionState>;
pub fn subscription_state(&mut self, scope: &str) -> Result<Option<SubscriptionState>>;
pub fn remove_subscription(&mut self, scope: &str, subscription_id: u64) -> Result<()>;
```

The initialization helper takes the session's expected Scope-to-subscription-ID map and acknowledged heads, validates them, and atomically initializes matching NULL rows. Normal request construction includes only initialized rows; uninitialized desired rows still appear in the WebSocket subscription set. Cursor advancement becomes an update of an existing matching row, never an insert that resurrects an unsubscribed Scope.

Add native commands `scopeSubscribe`, `scopeState`, and `scopeUnsubscribe`, carrying `{scope}` and, for unsubscribe, `{subscriptionId}`. Responses carry the serializable persistent state. The existing transaction `channel` command calls the same intent primitives. The runtime handle factory owns only identity/observers; synchronization decisions stay in Rust.

## 6. Verification and delivery

Required scenarios: offline registration; idempotent/concurrent calls; committed versus rolled-back registration; head zero; crash/reopen before and after first initialization; 100-to-120 reconnect; stale acknowledgment after unsubscribe/resubscribe; identical calls without cancellation; Scope addition without resetting unchanged cursors; callback failure after commit; client close versus unsubscribe; old-handle unsubscribe after replacement; no network configuration.

Use SQLite tests for atomic state/reopen, Rust live-controller and server live tests for event ordering, real socket integration for handshake/gap behavior, and TypeScript/React Native host/Dart tests for handle identity and observation. Do not claim mobile-device behavior from host tests. Update the frontend, storage, live controller, protocol and guarantee documentation, explicitly identifying the first-subscription default change.

Implement #150 before #151. The frontend surface above is the reviewed implementation baseline; do not request another approval for routine implementation details. #144, #152, #153, #154, #139 and #140 retain their separate responsibilities.
