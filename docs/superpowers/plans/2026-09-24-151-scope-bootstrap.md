# Subscription-bound Bootstrap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Load the historical part of a Scope through a durable, awaitable Engine task, jointly with its ongoing subscription.

**Architecture:** Bootstrap advances its own numeric cursor B from zero to the subscription's fixed starting cursor S. Normal delivery owns positions after S. On the final historical page, a fixed server head H becomes the completion barrier; complete after ongoing cursor L reaches H. Reuse existing scans, Loaders, stamps and `/sync/pull`.

**Tech Stack:** Rust protocol/server/client controllers, SQLite, existing PostgreSQL persistence, native bindings, TS/React Native and Dart.

## Global Constraints

- Review draft based on the [spec](../specs/2026-09-24-151-scope-bootstrap-design.md); requires #150's accepted persistent-subscription implementation.
- Do not introduce `first_cursor`, stable identity pagination, a generic task table, or a second sync engine.
- Publication-based delivery remains the boundary; current membership is #140.
- Bootstrap never changes the subscription origin or normal cursor. Preserve existing Loader visibility, D7/D8 errors and authority stamp rules.
- Persist work before network execution. One bootstrap HTTP request at a time across Scopes; rotate after each page. Push/live work remains independent.
- All counters use 0..2^53-1; each page loads at most the existing 50 records.
- Completion is processed delivery coverage, conditional on successful authoritative reads; it is not a globally frozen snapshot or a claim that reported live read failures succeeded.

## Task 1: Bounded pull protocol and server execution

**Files:** Modify `crates/core/src/protocol.rs`, `crates/core/tests/contracts.rs`, `crates/server/src/lib.rs`, `crates/server/tests/runtime.rs`, `packages/server/index.mts`; create `crates/server/src/loading.rs`, `crates/server/tests/bootstrap.rs`, protocol JSON fixtures beside existing pull fixtures.

**Interfaces:** Add serializable `BootstrapRequest` and `BootstrapPage` exactly as spec section 4. Add `process_bootstrap(config: &Config, owner: &str, bytes: &[u8], host: &impl Host) -> Result<String>`. Factor grouped record resolution into a crate-private function accepting Config, owner, declared versions and canonical `(RecordKey, stamp)` entries and returning `Vec<AuthorityRecord>`; normal pull calls the same helper.

- [ ] Write decoder cases for unknown mode, missing/unsafe counters and malformed channel. Add server scenarios with S=100 and scan cursors `[40,120]` (load only 40, terminal to=100), exactly 50 rows ending below S (nonterminal), a row exactly at S, empty scan, and after=S.
- [ ] Run `cargo test -p axton-core -p axton-server --locked`; confirm new protocol/behavior assertions fail before adding implementation.
- [ ] Implement the bounded scan using existing host operations:

```text
head = read current head in the request transaction
validate 0 <= after <= until <= head
rows = scan(channel, after, 50)
validate existing cursor order, head bounds and canonical identities
historical = rows whose cursor <= until
to = until if rows are short, reach until, or cross until
     otherwise last historical cursor
records = resolve historical identities using shared Loader helper
return {mode: bootstrap, channel, from: after, to, until, head, records}
```

- [ ] Dispatch bootstrap mode at the existing authenticated `/sync/pull` route, inside its repeatable-read wrapper. Reject unknown modes before normal pull decode. Keep normal pull's JSON and behavior unchanged. Do not modify host Scan or PostgreSQL schema.
- [ ] Add PostgreSQL-backed bounded-request cases to `integration/persistence/server/runtime.test.mjs`, including republish during successive pages and content/stamp consistency. Run `bash integration/persistence/server/run.sh` and the Rust suites. Commit protocol/server changes.

## Task 2: Durable Bootstrap registration and atomic page application

**Files:** Create `crates/client/src/bootstrap.rs`, `crates/sqlite/tests/bootstrap.rs`; modify `crates/client/src/{ddl,lib,subscriptions,authority}.rs` and `crates/sqlite/tests/rebuild.rs`.

**Interfaces:** Define `BootstrapState` with spec fields `state`, `run`, `cursor`, `barrier`, `error`. Add `Client::request_bootstrap(&mut self, scope: &str, subscription_id: u64) -> Result<BootstrapState>`, `Client::bootstrap_state(&mut self, scope: &str, subscription_id: u64) -> Result<BootstrapState>`, and `Client::apply_bootstrap_page(&mut self, scope: &str, subscription_id: u64, run: u64, expected_after: u64, page: &BootstrapPage) -> Result<ApplyReport>`. The controller owns an additional in-memory request ID; ledger methods fence persistent identities/run/progress.

- [ ] Test registration offline before initialization, duplicate active calls, valid completed state, explicit failure retry, unsubscribe/recreate, and reopen. Test old authority against newer live update/deletion and pending optimism using existing SQLite fixtures.
- [ ] Run `cargo test -p axton-sqlite --test bootstrap --locked`; confirm missing-state/application behavior fails.
- [ ] Add bootstrap fields to the same subscription row. Registration changes `not_requested` to `requested`, increments run for explicit failed-run retries, and retains B on retry. Initialized S is read from #150; no duplicated origin column.
- [ ] Apply a validated page via `Engine::apply_records`, bypassing normal delivery cursor gates. Use this committed transition:

```text
if identity/run/from no longer match: ignore stale response
if any record report failed:
    commit successful authority, persist bounded page error, mark failed
    keep B unchanged; reject this run's waiters after commit
else:
    commit authority and B = page.to
    if B == S: persist H = page.head; mark catching_up
    if B == S and L >= H: mark complete
never write starting_cursor or normal cursor
```

- [ ] Validate `from <= to <= S <= head`, echoed request markers and progress before authority writes. Distinguish protocol-invalid responses (no partial progress) from attributable record failures. Cap error storage to the page's 50 reports and bounded messages, without storing arbitrary server payloads.
- [ ] Exercise rollback injection immediately before commit and reopen immediately after commit; assert data/progress stay atomic. Run `cargo test -p axton-client -p axton-sqlite --locked`. Commit ledger/application changes.

## Task 3: Engine scheduling, transport and fixed completion barrier

**Files:** Create `crates/client/src/bootstrap_controller.rs`; modify `crates/client/src/{lib,connection,live}.rs`, `bindings/common/src/lib.rs`, `packages/client-js/connection.mts`, `packages/dart/lib/src/connection.dart`; extend `crates/sqlite/tests/bootstrap.rs` and existing host connection tests.

**Interfaces:** Define `BootstrapController`, typed events `Wake`, `Response {request_id, body}`, `Failure {request_id}`, `Closed`; actions `Request {request_id, body}`, `RetryAt {time}`, `Changed {scope, subscription_id, run}`. Reuse existing time/backoff/transport conventions. Native registration command is `scopeBootstrap {scope,subscriptionId}`; status command is `scopeBootstrapState` with the same identifiers.

- [ ] Test no request before initialization; independent push/live progress while a bootstrap response is held; round-robin page fairness; stale response after retry/close/reopen; transient backoff; no network configuration.
- [ ] Implement one in-flight bootstrap request, with native-owned correlation IDs. Persisted requested/loading state resumes without another frontend invocation. Release SQLite transactions before HTTP and between pages.
- [ ] Test the handoff: S=100, historical B reaches 100 with final H=130, L=120; remain catching_up. Advance L to 130 through normal delivery; mark complete locally. Publish to 150 meanwhile; retain target 130.

```text
on committed normal-delivery progress:
    for pending barriers on affected subscriptions:
        if B == S and L >= H: commit complete, then emit Changed
on reopen:
    resume requested/loading tasks
    re-evaluate persisted catching_up barriers before issuing more I/O
```

- [ ] If L<H, wake ordinary catch-up; do not fabricate cursor progress or wait exclusively for a future WebSocket message. Retry transport errors without an overall task timeout. Propagate terminal request/protocol failures as failed runs with explicit retry.
- [ ] Run Rust client/SQLite suites and JS/RN connection suites after `bash scripts/build.sh`. Commit scheduler/transport wiring.

## Task 4: Eager SDK method, shared waiters and status

**Files:** Modify #150's `packages/client-js/subscriptions.mts`, `packages/dart/lib/src/subscriptions.dart`, runtime exports and `crates/compiler/src/emit.rs` if generated exports require updates; add `integration/bindings/client-js/bootstrap.test.mjs`, `integration/bindings/client-react-native/bootstrap.test.mjs`, `packages/dart/test/bootstrap_test.dart`; extend generated API fixtures.

**Interfaces:** `Subscription.bootstrap(): Promise<void>` / `Future<void>` plus the exact `status.bootstrap` value in the spec. Waiters are keyed by subscription identity and run; observation uses committed native events.

- [ ] Test starting without awaiting, concurrent calls sharing work, restart continuation, already-complete calls offline, old-handle rejection, and earlier failed waiters staying failed across rapid retry.

```ts
const subscription = await client.scopes.subscribe("project:123");
const first = subscription.bootstrap();
const second = subscription.bootstrap();
// Test transport holds the response: both remain pending, one task registered.
await Promise.all([first, second]);
assert.equal(subscription.status.bootstrap.phase, "complete");
```

- [ ] Submit the local command eagerly. Attach waiters with a serialized status reread so completion between registration and listener installation cannot be missed. Cancel process-local waiters on client close; leave durable tasks intact. Observer callback exceptions use the existing uncaught-error path.
- [ ] On unsubscribe reject waiters with `subscription.closed`; on client close use `client_closed`; on page failure reject with the stored code/message. Background usage handles rejections explicitly. Expose no extra task-cancel or forced-refresh method.
- [ ] Run generated API verification, typecheck, new JS/RN host tests and Dart analysis/tests with native-library configuration from the test guide. Commit SDK work and fixtures.

## Task 5: Race coverage, assembled scenario and documentation

**Files:** Extend `crates/sim/src/host.rs` and relevant simulation tests only as required for new pull dispatch; add the assembled scenario to existing `integration/e2e` fixtures; update `docs/engineering/architecture/{protocol/pull.md,server/engine/pull.md,client/engine/pull.md,client/storage/store.md,client/connection/controller/scheduling.md,sdks/typed-api/client.md}`, `docs/engineering/guarantees.md`, and relevant frontend examples.

- [ ] Add the critical moving-record scenario: publish at 40, subscribe at S=100, republish at 120 before history scans that record; historical scan omits it, normal delivery supplies it, and Bootstrap waits for final H. Also reverse arrival order and deliver a newer tombstone first.
- [ ] Exercise exact/empty pages, repeated new publications, overlapping Scopes, local Action progress, offline start and process restart. Show that history terminates while publication continues, and that all successful authority converges once writes stop.
- [ ] Test D7 explicitly: a live read failure reports an error while L advances; Bootstrap completion does not rewrite it as success. Separately fail a historical Loader: preserve successful page records, reject the run, retry the unchanged B, and then complete.
- [ ] Document Bootstrap+Subscription joint coverage, eager background invocation, completion barrier, retained-record assumptions and error limits. Do not present `await bootstrap()` as a fresh snapshot or unconditional proof every record successfully loaded.
- [ ] Run `bash integration/e2e/run.sh`, the simulation suite, and `bash scripts/test.sh`. Run `git diff --check` and changed-document link/example checks. Performance scales belong to #12; make no new capacity claim from this suite.
- [ ] Review against every spec section, commit, publish the implementation PR with `Closes #151`, attach it to the task, and write issue evidence including actual test limits. Do not merge unreviewed runtime changes merely because the design branch is approved.
