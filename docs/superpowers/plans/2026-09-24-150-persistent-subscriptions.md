# Persistent subscriptions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement durable offline Scope subscriptions, stable SDK handles and one committed synchronization origin per subscription identity.

**Architecture:** SQLite owns intent and progress; a dedicated Rust Downlink worker owns initialization, page processing and recovery. The live-session component owns only socket-session mechanics. SDK objects observe committed state and register local commands. Implement this before #151.

**Tech Stack:** Rust core/client, SQLite, shared native bindings, TypeScript/React Native, Dart, generated APIs, existing WebSocket transport.

## Global Constraints

- Reviewed implementation baseline: follow the [spec](../specs/2026-09-24-150-persistent-subscriptions-design.md) and [cloud handoff](2026-09-24-150-151-cloud-handoff.md). User approval covers implementation and merge after verification; no additional routine approval gate is needed.
- Work in an isolated `codex/` worktree. Keep repository documentation in English.
- Null initialization is distinct from cursor zero. Counters are safe integers, 0..2^53-1.
- Subscribe waits for local commit, never for network. Reconnect retains committed progress.
- Unsubscribe retains Models, stamps, before images and pending Actions.
- #144 owns incremental membership changes on a healthy socket. #152 owns comprehensive terminology cleanup. No Bootstrap implementation in this issue.
- Use the existing testing strategy and host prerequisites; record executed tests separately from inspected code.

## Task 1: Durable subscription identity and nullable origin

**Files:** Create `crates/client/src/subscriptions.rs`, `crates/sqlite/tests/subscriptions.rs`; modify `crates/client/src/{ddl,ledger,lib,mutate}.rs`, `crates/sqlite/tests/{ddl,rebuild}.rs`.

**Interfaces:** Produce `SubscriptionState { scope: String, subscription_id: u64, starting_cursor: Option<u64>, cursor: Option<u64> }` and Client methods `ensure_subscription(&mut self, &str) -> Result<SubscriptionState>`, `subscription_state(&mut self, &str) -> Result<Option<SubscriptionState>>`, `remove_subscription(&mut self, &str, u64) -> Result<()>`. Use internal transaction-aware helpers from existing `set_channel`; do not nest writer transactions.

- [ ] Add SQLite cases for duplicate registration, reopen, unsubscribe/recreate, rollback, safe-counter exhaustion, and malformed one-null/one-non-null cursor pairs. Use this contract assertion sequence with the existing SQLite test fixture's client:

```rust
let a = client.ensure_subscription("project:123")?;
let b = client.ensure_subscription("project:123")?;
assert_eq!(a.subscription_id, b.subscription_id);
assert_eq!(a.starting_cursor, None);
assert_eq!(a.cursor, None);
client.remove_subscription("project:123", a.subscription_id)?;
let c = client.ensure_subscription("project:123")?;
assert_ne!(a.subscription_id, c.subscription_id);
client.remove_subscription("project:123", a.subscription_id)?;
assert!(client.subscription_state("project:123")?.is_some());
```

- [ ] Run `cargo test -p axton-sqlite --test subscriptions --locked`; confirm the missing behavior fails before implementing.
- [ ] Add the spec's table constraints and `axton_client.next_subscription`. Carry the allocator forward when rebuilding a current-format replica, and invalidate old handles/controllers on replacement. Allocate and insert in one transaction; duplicate subscribe reads the existing row without incrementing generation. Replace cursor upsert with update-only operations fenced by subscription identity. Preserve desired Scope names but allocate fresh identities when the explicit incompatible-layout rebuild path resets metadata.
- [ ] Run `cargo test -p axton-client -p axton-sqlite --locked`; fix actual regressions in cursor callers and rebuild tests. Commit the ledger change with its tests.

## Task 1A: Extract the Downlink worker from LiveSession

**Files:** Create `crates/client/src/downlink_worker.rs`, `crates/sqlite/tests/downlink_worker.rs`; modify `crates/client/src/{lib,live,transport}.rs`, `bindings/common/src/lib.rs`, `packages/client-js/{connection,runtime}.mts`, `packages/dart/lib/src/{connection,client}.dart` and existing live/connection integration tests.

**Interfaces:** `DownlinkWorker` is the long-lived Rust owner of the inbound queue and downlink scheduling. Introduce typed `DownlinkEvent` for socket messages/close/overflow, HTTP completion/failure and committed-work wake; typed `DownlinkAction` for socket open/close, HTTP request, retry wait, reports and post-commit status. Reuse existing wire bodies and policy types. Native `downlink` commands expose enqueue/control and bounded `next` pumping; SDK hosts provide `startDownlinkLane` / Dart `DownlinkLane` and a wake-generation loop. `LiveSession` remains a subordinate socket-session component and no longer calls Client page-application or pull-building methods. Preserve existing outward client connection controls.

- [ ] Characterize current behavior with tests before moving it: buffered gap repair, overlapping/covered frames, overflow, reconnect, stale socket epoch, independent Uplink progress, pause/close and lost-wake prevention. Run `cargo test -p axton-sqlite --test live --locked` plus existing SDK connection tests for a baseline.
- [ ] Add tests separating enqueue from pump: enqueue a valid page and assert no Model/cursor commit yet; pump and assert data/progress commit together. Replace the socket and assert the worker remains active; keep an HTTP request pending and prove further control events still execute.
- [ ] Move queue consumption, Client application, catch-up requests and scheduling decisions to `DownlinkWorker`. Keep transport callbacks as producers and all sync policy in Rust:

```text
callback(socket or HTTP event): enqueue tagged event; wake host loop
host loop:
    next = Rust worker's bounded pump
    execute returned socket/HTTP actions asynchronously
    yield between page commits
    wait only if idle and wake generation is unchanged
```

- [ ] Preserve the existing 64-live-frame recovery bound. Keep lifecycle/acknowledgment and HTTP completion events deliverable when the stream buffer overflows; coalesce wakes. A held gap page cannot block processing its repair response. Scope request correlation by worker/session/subscription identity as appropriate; never silently discard a completion because a live queue was full.
- [ ] Run `cargo test -p axton-client -p axton-sqlite -p axton-binding --locked`; rebuild bindings and run JS/RN live and connection suites plus Dart equivalents. Existing wire/recovery behavior must pass before changing initialization defaults. Commit this extraction separately within the #150 PR for review.

## Task 2: Initialize from acknowledged head, then resume

**Files:** Modify `crates/client/src/{downlink_worker,live,transport,downlink,lib,subscriptions}.rs`, `crates/sqlite/tests/{live,downlink,subscriptions}.rs`; extend `crates/server/tests/live.rs` only for server race evidence.

**Interfaces:** Add `Client::initialize_subscriptions(&mut self, expected: &BTreeMap<String, u64>, heads: &BTreeMap<String, u64>) -> Result<()>`. `DownlinkWorker` holds the expected identity map for the subordinate socket session epoch. Normal pulls consume initialized states only; desired socket membership consumes all rows.

- [ ] Add deterministic event cases: offline subscribe; acknowledgment at zero; fresh acknowledgment at 100; reconnect at 120 from saved 100; acknowledgment after unsubscribe/recreate; transaction rollback; duplicate registration without an extra Open/Close action.
- [ ] Run `cargo test -p axton-sqlite --test live --locked`; confirm the fresh subscription currently requests historical data and the new assertion fails.
- [ ] Implement initialization before applying buffered live frames, using this transaction rule:

```text
for each acknowledged scope:
    reject malformed or unexpected acknowledgment
    if current subscription ID != session's captured ID: discard stale work
    if origin is NULL: set origin = cursor = acknowledged head
    else if head < cursor: report server-state fault without modifying progress
    else: retain cursor and schedule catch-up when cursor < head
commit before publishing status or processing subsequent frames
```

- [ ] Add mixed-set coverage: existing A at 80, new B with NULL cursors, absent C; acknowledgment A=100/B=200 catches up A, initializes B and never requests C. Keep server listener-then-drain coverage. Test 120-to-125 direct application versus a 124-to-125 gap at local 120, and no automatic request for only-uninitialized rows.
- [ ] Keep generation-driven reconnection for genuine desired-set changes; do not introduce #144's new wire protocol. Ensure a late normal page cannot upsert an unsubscribed row.
- [ ] Run `cargo test -p axton-client -p axton-sqlite -p axton-server --locked`. Commit controller and race tests.

## Task 3: Native commands and committed status observation

**Files:** Modify `bindings/common/src/lib.rs`, `packages/client-js/{runtime,events,connection}.mts`, `packages/dart/lib/src/{client,connection,live}.dart`; create `packages/client-js/subscriptions.mts` and `packages/dart/lib/src/subscriptions.dart`.

**Interfaces:** Native `scopeSubscribe {scope}` returns SubscriptionState; `scopeState {scope}` returns state/null; `scopeUnsubscribe {scope,subscriptionId}` removes only a matching row. The status and public handle interfaces are exactly those in spec section 2. Existing transaction `channel` commands delegate to the same intent implementation.

- [ ] Extend existing native/host tests to reject malformed IDs, verify null cursor serialization, and distinguish close from unsubscribe. Snapshot output must not coerce NULL into zero.
- [ ] Add JS/Dart tests asserting that observer exceptions occur after commit and do not reject subscribe, replay a write or reconnect. Test one initial observer snapshot and no updates after cancellation.
- [ ] Remove pre-write live cancellation from SDK subscribe/unsubscribe. Implement the handle cache by persistent identity under the existing serialized command path:

```text
subscribe(scope):
    state = await local scopeSubscribe command
    handle = cached handle for state.subscription_id, or create one
    publish committed snapshot; signal existing work wake
    return handle
```

- [ ] Derive connection status from the Rust Downlink worker and subordinate socket-session events, not a second SDK network state machine. Close observers and process-local handles without deleting SQLite rows. Reject other operations on closed handles; repeated old-handle unsubscribe is harmless.
- [ ] After `bash scripts/build.sh`, run `node --test integration/bindings/client-js/*.test.mjs` and `node --test integration/bindings/client-react-native/*.test.mjs`. Run Dart analysis/tests with the native library configured per `docs/engineering/testing/running.md`. Commit only after relevant host tests pass.

## Task 4: Generated Scope facade and frontend conformance

**Files:** Modify `crates/compiler/src/emit.rs`, `integration/generated-api/{client.ts,test.ts,generated_test.dart}`, generated fixtures from that suite; add `integration/bindings/client-js/subscriptions.test.mjs`, `integration/bindings/client-react-native/subscriptions.test.mjs`, `packages/dart/test/subscriptions_test.dart`. Update public runtime exports where the new handle types are consumed.

**Interfaces:** Generated `client.scopes.subscribe(scope)` returns the runtime Subscription. Existing `channels` spelling delegates to this same implementation during #150; it must not retain cursor-zero behavior.

- [ ] Add generated type checks for `scope`, `status`, `watch`, and `unsubscribe`; add negative checks for mutating readonly status. Use the generated client's established construction fixture for:

```ts
const [a, b] = await Promise.all([
  client.scopes.subscribe("project:123"),
  client.scopes.subscribe("project:123"),
]);
assert.equal(a, b);
assert.equal(a.status.initialization, "pending");
await a.unsubscribe();
const c = await client.scopes.subscribe("project:123");
await a.unsubscribe();
assert.equal(c.status.active, true);
```

- [ ] Generate `Scopes` and typed exports for TS/Dart; reuse the existing shared JS runtime for React Native, without introducing platform-specific subscription semantics. Regenerate affected fixtures using repository scripts, not manual edits to generated bodies.
- [ ] Run `bash integration/generated-api/verify.sh`, `npm run typecheck`, and the new host tests. Commit generated API and conformance changes.

## Task 5: End-to-end behavior and owning documentation

**Files:** Extend `integration/e2e/run.sh` and its existing client/server fixtures; update `docs/engineering/architecture/{client/frontend-interface.md,client/storage/store.md,client/storage/reconciliation.md,client/connection/controller/live-session.md,protocol/subscriptions.md,sdks/typed-api/client.md}` and `docs/engineering/guarantees.md`.

- [ ] Add an assembled scenario: publish old Todo; subscribe at head S; confirm old Todo is not automatically loaded; publish a newer Todo; confirm it arrives; disconnect/publish/reconnect; confirm catch-up retains S and fills the gap. Reopen the local database and repeat without resetting initialization.
- [ ] Audit old tests/examples that assumed subscribe implied a historical download. Update their expected from-now behavior or establish subscriptions before publishing their fixtures, without suppressing reconnect/gap assertions or adding a legacy zero-cursor backdoor. Keep #150 independently green; #151 adds explicit Bootstrap to whole-history examples.
- [ ] Document the behavioral change from default historical sync to first-handshake origin, offline registration, persistent lifetime, error/status meanings, and #151 as the explicit historical loading operation.
- [ ] Run `bash integration/e2e/run.sh`, then `bash scripts/test.sh` for the completed runtime change. Record environment-related exclusions explicitly; do not treat host tests as device verification.
- [ ] Run `git diff --check`, check changed relative links/examples, and review A2/D6/R1–R4 against the final code. Commit, publish the implementation PR with `Closes #150`, attach it to the task, and update issue evidence. Review the final diff, resolve correctness findings, wait for required CI on the final commit, and merge without another user confirmation. #151 starts from main after this merge. Respect branch protection; do not force or administratively bypass checks.
