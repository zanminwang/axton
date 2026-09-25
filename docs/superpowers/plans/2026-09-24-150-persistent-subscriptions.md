# Persistent subscriptions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement durable offline Scope subscriptions, stable SDK handles and one committed synchronization origin per subscription identity.

**Architecture:** SQLite owns intent and progress; the Rust live controller owns initialization and session decisions. SDK objects observe committed state and register local commands. Implement this before #151.

**Tech Stack:** Rust core/client, SQLite, shared native bindings, TypeScript/React Native, Dart, generated APIs, existing WebSocket transport.

## Global Constraints

- Review draft; implement only after the public API in the [spec](../specs/2026-09-24-150-persistent-subscriptions-design.md) is approved.
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
- [ ] Add the spec's table constraints and `axton_client.next_subscription`. Allocate and insert in one transaction; duplicate subscribe reads the existing row without incrementing generation. Replace cursor upsert with update-only operations fenced by subscription identity. Preserve desired Scope names but allocate fresh identities when the explicit incompatible-layout rebuild path resets metadata.
- [ ] Run `cargo test -p axton-client -p axton-sqlite --locked`; fix actual regressions in cursor callers and rebuild tests. Commit the ledger change with its tests.

## Task 2: Initialize from acknowledged head, then resume

**Files:** Modify `crates/client/src/{live,transport,downlink,lib,subscriptions}.rs`, `crates/sqlite/tests/{live,downlink,subscriptions}.rs`; extend `crates/server/tests/live.rs` only for server race evidence.

**Interfaces:** Add `Client::initialize_subscriptions(&mut self, expected: &BTreeMap<String, u64>, heads: &BTreeMap<String, u64>) -> Result<()>`. `LiveSession` holds the expected identity map for its existing session epoch. Normal pulls consume initialized states only; desired socket membership consumes all rows.

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

- [ ] Derive connection status from Rust transport events, not a second SDK network state machine. Close observers and process-local handles without deleting SQLite rows. Reject other operations on closed handles; repeated old-handle unsubscribe is harmless.
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
- [ ] Document the behavioral change from default historical sync to first-handshake origin, offline registration, persistent lifetime, error/status meanings, and #151 as the explicit historical loading operation.
- [ ] Run `bash integration/e2e/run.sh`, then `bash scripts/test.sh` for the completed runtime change. Record environment-related exclusions explicitly; do not treat host tests as device verification.
- [ ] Run `git diff --check`, check changed relative links/examples, and review A2/D6/R1–R4 against the final code. Commit, publish the implementation PR with `Closes #150`, attach it to the task, and update issue evidence. #151 starts only from the accepted #150 contract.
