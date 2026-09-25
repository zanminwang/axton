# Action @ephemeral outputs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add explicit Model-output `@ephemeral` without changing Action invocation/result types, required mutation reconciliation or call replay.

**Architecture:** Annotate the Action output descriptor, separate result reads from authority contributors in the existing server executor, and keep clients applying the unchanged response `records` envelope. Ordinary outputs and required mutation readback contribute authority; ephemeral-only outputs contribute result values. Materialization-policy edits alone do not require an Action version bump.

**Tech Stack:** Rust compiler/core/server/client, SQLite, PostgreSQL host adapters, generated TypeScript and Dart, JS/React Native native hosts.

## Global Constraints

- Follow the [spec](../specs/2026-09-25-116-ephemeral-outputs-design.md). This document is a plan, not implementation evidence.
- Work in an isolated `codex/` checkout. Preserve concurrent #150/#151 changes; do not alter Downlink scheduling, subscription tables or Bootstrap protocols.
- `@ephemeral` is a bare annotation on explicit Model outputs only: single, nullable single, non-null lists. No input/declaration/scalar/Delete-identity support.
- Preserve both `actions.name` and `actions.call.name`, handler identity objects, result shapes, read-version requirements and all shared authority-applier interfaces.
- Same-version changes to this policy are allowed. Actual incompatible input/output/read-contract rules remain unchanged. Persisted completed responses are immutable and replayed as saved.
- No local Model row/stamp change solely from an ephemeral output. Required mutation authority and ordinary outputs always retain their contribution, even for the same identity.
- No fetch API, retention system, tool adapter, migration project, new response envelope or client result lifetime policy.
- Keep docs in English. Use tests from the repository strategy and prerequisites in `docs/engineering/testing/running.md`; do not claim unexecuted checks or mobile-device coverage.

## Task 1: Syntax, typed policy and compatible Action history

**Files:** Modify `crates/compiler/src/{parse,validate,generate,history}.rs`, `crates/core/src/actions.rs`; extend `crates/compiler/tests/{parse,compiler,history}.rs`, `crates/core/tests/{contracts,compatibility}.rs`.

**Interfaces:** Add `ephemeral: bool` to validated `ActionOutput` and `ActionOutputDescriptor`; default missing to false and omit false on serialization. Semantic validation accepts true only for a Model with HandlerIdentity source. Do not store an unvalidated boolean only in the generic metadata map.

- [ ] Add compile/descriptor tests using the existing `compile` helper:

```rust
let source = "model Todo { id String title String @@id(id) } action Search(query String) { todos Todo[] @ephemeral }";
let compiled = axton_compiler::compile(source).unwrap();
assert_eq!(compiled["schema"]["actions"][0]["outputs"][0]["ephemeral"], true);
```

  Add separate cases for single/nullable output, missing=false, explicit false descriptor, non-boolean descriptor, duplicate annotation, annotation arguments, scalar output and implicit-output misuse. Reuse existing negative-diagnostic assertions to check useful source locations.
- [ ] Run `cargo test -p axton-compiler -p axton-core --locked`; confirm annotation acceptance currently fails before implementation.
- [ ] Reuse parsed field attributes; validate the annotation is bare, unique and allowed. Populate the bool and emit it only when true. Reject illegal descriptor combinations in `Schema::validate_actions`, not just in the compiler. Keep handler/result type generation unchanged.
- [ ] Modify history compatibility comparison to ignore only the policy field on each output descriptor, preserving all shape fields and actual version checks:

```text
contract_outputs(outputs):
    clone each output descriptor
    remove its top-level ephemeral field only
    return the resulting ordered descriptors
compare contract_outputs(old) with contract_outputs(new)
retain the new policy in the current version's stored snapshot
```

  Test false→true and true→false at the same Action version; no new handler version appears. Test genuine Model/cardinality/read-version changes still reject at the same version, and unrelated older Action versions retain their descriptors. Missing and false are equivalent. Never strip arbitrary business fields named ephemeral recursively.
- [ ] Run the compiler/core suites again and confirm schema round-trip/reopen metadata retains the flag without changing Model storage compatibility. Commit with `feat: define ephemeral Action output policy`.

## Task 2: Separate result resolution from authority contribution

**Files:** Create `crates/server/src/action_results.rs`; modify `crates/server/src/{actions,lib}.rs`; extend `crates/server/tests/actions.rs`.

**Interfaces:** Move existing result assembly into the new module with the following crate-private boundary, preserving executor ownership of transactions:

```rust
pub(crate) struct ResultReadback<'a> {
    pub records: &'a [AuthorityRecord],
    pub models: &'a BTreeMap<String, u64>,
}
pub(crate) async fn assemble_result(
    config: &Config,
    owner: &str,
    action: &ActionDescriptor,
    args: &Value,
    outputs: &Value,
    readback: ResultReadback<'_>,
    host: &impl Host,
) -> Result<(Value, Vec<AuthorityRecord>)>;
```

  The snippet defines the interface, not a second public execution entry point. Add imports using the same types currently used by `actions.rs`.

- [ ] Extend the existing Action Host fixture with counted Loader/EnsureStamp calls and configurable mixed outputs. Assert a pure ephemeral result is populated while additional authority is empty and no stamp is allocated solely for it. Run `cargo test -p axton-server --test actions --locked` and confirm the existing all-output materialization violates the new assertion.
- [ ] Extract result assembly without changing Handler execution, readback/publication, claim/save/replay or public APIs. Establish positive materialization requirements before loading outputs:

```text
required = existing mutation/extra-change authority
ordinary_ids = union of identities selected by non-ephemeral Model outputs
ensure stamp evidence for ordinary_ids missing from required
for every output position:
    read Loader at that output's retained result version
    preserve named shape, nullability, list order and duplicates
for every ordinary identity not already required:
    resolve authority at the client's declared read version
return named result plus ordinary additional authority
```

- [ ] Cache Loader reads by canonical identity and read version within the invocation; seed only from matching-version readback. Load an ephemeral-only output at its result read version without an unnecessary authority-version read. Keep existing read-contract declaration validation. Establish required stamp evidence in the same transaction; never substitute an ephemeral read from another version or transaction.
- [ ] Test identical identities in ordinary/ephemeral outputs in both declaration orders, duplicate/composite keys, nullable absence, required/list absence, Loader refusal, and both matching/different result read versions. Test inferred mutation and handler-extra-change overlaps still settle with required authority. No global identity subtraction or client-side denylist is allowed.
- [ ] Run `cargo test -p axton-server --locked`; commit with `feat: separate ephemeral results from materialized authority`.

## Task 3: Replay, transactions and client reconciliation

**Files:** Extend `crates/server/tests/actions.rs`, `crates/sqlite/tests/actions.rs`, `integration/persistence/server/actions.test.mjs`; inspect `crates/client/src/actions.rs`, `crates/core/src/protocol.rs` and the existing acknowledgment path, making only changes demonstrated necessary by failing tests.

**Interfaces:** Keep `DirectActionResponse`, `CallCompletion`, `PushReceipt`, `Client::apply_action_response` and `Engine::apply_records` unchanged. A response's `records` remains authoritative independently of the local descriptor's current ephemeral metadata.

- [ ] Create SQLite cases for ephemeral-only direct results with empty records, durable completion bookkeeping, cached older/newer records, pending local edits, selected missing identities and independent later live authority. Assert both content and stamp remain unchanged solely from ephemeral outputs; then assert a later ordinary delivery updates them normally.
- [ ] Run `cargo test -p axton-sqlite --test actions --locked` and `cargo test -p axton-core --locked`. If existing envelope/client behavior already passes, retain tests and avoid unnecessary client runtime edits.
- [ ] Add backend policy-update/replay tests in both directions:

```text
execute call X under ordinary policy; save result and authority
change the same Action version to ephemeral
replay X: identical saved result/authority, no Handler or Loader execution
execute new call Y: result present, no output-only authority
repeat with original ephemeral X and later ordinary policy
```

  A queued call first executed after deployment uses the deployed backend policy. Existing cached rows are not deleted by changing policy. Client schema reopen with the policy changed must not discard pending Actions or reinterpret saved authority.
- [ ] Use real PostgreSQL to assert business writes, result snapshot and saved call outcome commit together; output Loader failure rolls back that Action's savepoint while unrelated Actions can succeed. Cover ephemeral result snapshots while concurrent writes occur and the mixed-output stamp/content consistency inherited from #142. Keep external-effect rollback claims unchanged.
- [ ] Run `cargo test -p axton-core -p axton-server -p axton-sqlite --locked` and `bash integration/persistence/server/run.sh`. Commit tests and any narrowly required fix with `test: preserve ephemeral replay and reconciliation boundaries`.

## Task 4: Generated SDK and assembled acceptance

**Files:** Extend `integration/action-contract/schema.model`, its positive/negative TypeScript/Dart cases and generated fixtures; extend `integration/action-runtime-ts/source/app.model`, `integration/action-runtime-ts/{types.ts,client.test.mts,backend.test.mts}`; extend `integration/action-runtime-dart/schema.model`, `integration/action-runtime-dart/generated_test.dart`; extend `integration/action-e2e/source/app.model`, `integration/action-e2e/{backend-fixture.ts,action.test.mts}`. Extend `integration/bindings/client-js/actions.test.mjs`, `integration/bindings/client-react-native/actions.test.mjs`, `packages/dart/test/actions_test.dart` where native boundary assertions belong.

**Interfaces:** Handler outputs are still generated identity objects; frontend outputs are still generated Model snapshots. Generated metadata carries the bool. No new frontend method/options or native network command is introduced.

- [ ] Add an ephemeral SearchTodos alongside an ordinary read and a mixed-output Action in generated fixtures. Assert existing typed calls compile without additional user arguments and retain both direct and durable result types. Regenerate source/history artifacts via the existing fixture scripts rather than hand-editing generated bodies.
- [ ] In the assembled fixture, use a client with no subscriptions and a backend-only Todo. For direct SearchTodos assert the returned title matches Loader output but local get remains absent; repeat through durable call/wait. Then run ordinary OpenTodo and assert it becomes locally readable before successful completion is delivered.

```ts
const result = await client.actions.call.searchTodos({ query: "meeting" });
assert.equal(result.todos[0].id, "remote-only");
assert.equal(await client.models.todo.get({ id: "remote-only" }), null);
const call = await client.actions.searchTodos({ query: "meeting" });
const outcome = await call.wait();
assert.equal(outcome.error, null);
assert.equal(outcome.result!.todos[0].id, "remote-only");
```

  Use the repository's actual missing-record representation in the fixture if it differs from null; assert absence explicitly. Seed fixture data under its established transaction rules.
- [ ] Count Model watch value emissions, distinguishing them from durable Action status/metadata notifications. An ephemeral-only result must not change the observed Model value. Test pending optimism B versus returned Loader snapshot A and mixed output overlap through both SDK routes; record React Native host versus device scope honestly.
- [ ] Run `npm ci` and `bash scripts/build.sh` if prerequisites are absent. Run `bash integration/action-runtime-ts/verify.sh`, `bash integration/generated-api/verify.sh`, `npm run typecheck`, `bash integration/action-e2e/run.sh`, JS/RN Action host tests and Dart tests per the test guide. Inspect regenerated diffs and commit with `test: cover ephemeral outputs across generated clients`.

## Task 5: Documentation, final review and execution handoff

**Files:** Update `docs/engineering/architecture/schema/actions.md`, `docs/engineering/architecture/protocol/actions.md`, `docs/engineering/architecture/sdks/typed-api/{client,server}.md`, `docs/engineering/guarantees.md`, `website/docs/schema/reference.md` and the relevant existing Action examples.

- [ ] Document syntax, unchanged result types, no policy-only version bump, backend first-execution policy selection, immutable saved replay, overlap with ordinary/mutation authority, and notification scope. Remove statements that all Action Model outputs always materialize or that this policy is still wholly unimplemented, only after implementation passes.
- [ ] Reconcile latest main with the #150/#151 work: preserve generated Scopes and their worker plumbing. Review shared codegen/native/example/documentation hunks individually. Never replace their work with this branch's older files.
- [ ] Run `bash scripts/test.sh` for the completed implementation, plus `git diff --check` and affected links/examples. Re-run affected checks after conflict fixes. The full gate includes Rust formatting/lint, language tests, persistence, generated APIs and end-to-end coverage; document actual unavailable prerequisites instead of claiming success.
- [ ] Review the final implementation against spec sections 2–8 and the verification matrix. Pay particular attention to accidentally dropping required authority, accepting illegal descriptors, changing public result shapes, invoking a wrong-version Loader, and recomputing saved outcomes under a new policy. Resolve findings before considering it ready.
- [ ] When assigned implementation, open the reviewed PR with `Closes #116`, attach it to the task if supported, and post executed evidence to the issue. Follow the execution assignment's merge authorization and required CI; this preparation task ends with spec/plan delivery and performs no runtime implementation or merge.
