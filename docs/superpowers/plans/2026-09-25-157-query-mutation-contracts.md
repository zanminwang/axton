# Query and Mutation contracts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans or superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Generate distinct Query/Mutation APIs with direct/durable defaults and explicit overrides, while preserving Loader/store and sync correctness.

**Architecture:** A retained CallKind classifies backend business behavior; generated namespaces choose an existing delivery path. A restricted Query context and Rust settlement check enforce the framework-visible read contract. Both kinds share call identity, queue/receipt machinery, result assembly and local authority application.

**Tech Stack:** Rust compiler/core/client/server and native bindings, TypeScript Node/React Native SDKs, Dart SDK, SQLite and PostgreSQL test hosts.

## Global constraints

- Follow the [spec](../specs/2026-09-25-157-query-mutation-contracts-design.md) and [issue #157](https://github.com/zanminwang/axton/issues/157). This is a future implementation plan; the current preparation changes documents only.
- Mutation defaults durable; Query defaults direct. Explicit overrides are mutations.call and queries.enqueue. Return types are fixed per method; no delivery option changes a return type.
- Query has no business mutation operands, changes or publish. Framework result storage and call/stamp bookkeeping remain allowed. Application-owned Tx is not an enforced SQL sandbox.
- Preserve #116 store policy, Loader snapshots, Mutation reconciliation, saved call replay, frozen requests and local-only models/transactions. No new HTTP path, queue worker or database column merely for naming.
- Retain internal Action/protocol and legacy queue Mutation names where a rename would expand scope. Public generated types use Call vocabulary. No client.actions alias is required.
- #151 is independent. Do not edit its worker, Bootstrap/subscription ledger, cursor boundaries or recovery. Integrate the other branch's merge before final verification if it lands first.
- Follow [running tests](../../engineering/testing/running.md), [strategy](../../engineering/testing/strategy.md) and [writing conventions](../../writing/README.md). Report actual checks and missing prerequisites, not intended checks.

## Task 1: Schema kinds and source validation

**Files:** `crates/compiler/src/parse.rs`, `crates/compiler/src/validate.rs`, `crates/compiler/src/generate.rs`, `crates/core/src/actions.rs`, `crates/core/src/schema.rs`, public exports in `crates/core/src/lib.rs`; tests `crates/compiler/tests/parse.rs`, `crates/compiler/tests/compiler.rs`, `crates/core/tests/contracts.rs`.

**Interfaces:** Add core `CallKind { Mutation, Query }` with serde lowercase values and default Mutation. Add kind to the existing Action declaration, validated operation and descriptor structures. New descriptors explicitly emit kind; legacy omitted kind decodes as Mutation. Keep the internal actions collection to reuse the executor.

- [ ] Add failing parse/compile assertions for these declarations:

```text
mutation AddTodo(todo Todo.create) {}
query FindTodos(text String, cursor String?) {
  todos Todo[]
  nextCursor String?
}
```

  Assert descriptors contain mutation/query kinds and inferred/explicit outputs retain their identities/read versions. Add negative cases for Query create/update/delete operands, @sequence, duplicate cross-kind names, generated call/enqueue collisions and unknown descriptor kinds. Run `cargo test -p axton-compiler -p axton-core --locked` to observe the new missing behavior.
- [ ] Route parenthesized mutation/query parsing through the existing Action input/output parser. Keep legacy `mutation Name { slots }` fixture parsing explicitly separate by the next token. Add @version support to both new keywords; retain @sequence for parenthesized Mutations only.

```text
kind = keyword == query ? Query : Mutation
parse existing parenthesized inputs and named outputs
if kind == Query:
    reject any Model mutation operand
    reject sequence policy
emit the ordinary shared descriptor plus kind
```

- [ ] Apply the same Query restrictions in core schema validation so native callers and hand-authored descriptors cannot bypass them. Reserve new neutral public names and namespace member names in compiler name validation.
- [ ] Migrate current Action parser fixtures to the new keywords and add a clear diagnostic for unsupported old action source syntax at the final public compiler boundary. Keep low-level legacy mutation descriptors/fixtures working; do not confuse their slot policy with the new semantic kind.
- [ ] Run the same suites, inspect diagnostics for source positions, and commit `feat: declare query and mutation operation kinds`.

## Task 2: Retained history and descriptor round trips

**Files:** `crates/compiler/src/history.rs`, `crates/compiler/src/main.rs`, `crates/core/src/actions.rs`, `crates/core/src/schema.rs`; tests `crates/compiler/tests/history.rs`, `crates/compiler/tests/cli.rs`, `crates/core/tests/compatibility.rs`, `crates/core/tests/contracts.rs`.

**Interfaces:** Capture kind in each shared operation history snapshot. Normalize omitted kind to mutation for comparison and decoding. Current operation name/version still selects exactly one retained kind. Internal history file/keys stay unchanged.

- [ ] Add failing history tests: omitted legacy kind equals explicit mutation at the same version; mutation-to-query at the same version rejects; v2 may change kind while v1 stays Mutation; unrelated histories stay unchanged. Assert store/delivery choices do not enter history or cause a version bump.
- [ ] Compare normalized kinds before existing input/output compatibility checks:

```text
oldKind = old.kind or mutation
newKind = new.kind or mutation
if oldVersion == newVersion and oldKind != newKind:
    reject incompatible backend contract; require a new version
retain kind independently for every version
```

- [ ] Preserve kind through generated descriptors, backend retained config and persisted client schema round trips. Confirm a Query-only schema with no Models remains valid when its ordinary output shape was previously supported.
- [ ] Keep Model storage compatibility unaffected by kind alone. Do not add a queue migration or wipe pending operations to make fixtures pass. Intent name/version must keep resolving to its original retained contract.
- [ ] Run `cargo test -p axton-compiler -p axton-core --locked`; commit retained-kind contracts separately from SDK dispatch changes.

## Task 3: Backend Query capabilities and effect isolation

**Files:** `packages/server/index.mts`, `crates/compiler/src/emit.rs`, `crates/server/src/actions.rs`, config validation in `crates/server/src/lib.rs`; tests `crates/server/tests/actions.rs`, `integration/action-contract/backend.ts`, `integration/action-runtime-ts/backend.test.mts`, `integration/persistence/server/actions.test.mjs`.

**Interfaces:** Generated Mutations<Tx>/Queries<Tx> maps feed the existing kind-aware shared handler dispatcher. MutationContext has tx/userId/callId/changes/publish; QueryContext has tx/userId/callId. Both handlers return existing typed value/Model-identity outputs. Public rejection type becomes CallRejected with existing semantics.

- [ ] Add negative TS fixtures accessing `ctx.changes` or `ctx.publish` in a Query; add positive Mutation access and typed Model identity returns. Add startup failures for missing, extra and wrong-kind registrations, including retained v1 Mutation plus v2 Query of one name.
- [ ] Build Query contexts without effect properties at runtime. Group retained handlers by each version's kind, not only the latest kind. Accept shorthand functions only for a single retained v1 in the matching registration group; omit empty maps when appropriate.
- [ ] Add a failing Rust Host test returning a forged Query settlement with changes/publications. Run `cargo test -p axton-server --test actions --locked`; confirm current executor would accept the unsupported business effects.
- [ ] Enforce this check after obtaining the handler settlement and before any extra-change stamping/readback/publication:

```text
if descriptor.kind == Query and (changes is nonempty or publications is nonempty):
    return call-level error query.effects_forbidden
```

  Reuse existing call-level savepoint rollback, error completion and saved rejection. Keep default store authority generation after this check; initializing a result record's framework stamp is permitted. Do not mark the application transaction globally SQL-read-only.
- [ ] Test both direct and queued Query paths. In a mixed push, place a violating Query next to a valid Mutation and assert the Query produces no records/publications while the Mutation commits. Use a real database test to verify rollback of any same-transaction test write made before a forbidden settlement; do not claim external effects are rollbackable or all SQL writes detectable.
- [ ] Run `cargo test -p axton-server --locked`, build prerequisites, `bash integration/action-runtime-ts/verify.sh` and `bash integration/persistence/server/run.sh`; commit Query capabilities with evidence.

## Task 4: Generated routes and shared client call types

**Files:** `crates/compiler/src/emit.rs`, `crates/compiler/src/validate.rs`, `packages/client-js/actions.mts`, `packages/client-js/runtime.mts`, `packages/dart/lib/src/actions.dart`, `packages/dart/lib/src/client.dart`, SDK barrel exports found under `packages/client-js/index.mts`, `packages/client-react-native/index.ts`, `packages/dart/lib/axton.dart`; bridge only if needed in `bindings/common/src/lib.rs`.

**Interfaces:** Neutral public Call/CallStatus/CallOutcome/CallError/CallOptions (Dart also CallSuccess/CallFailure). Existing runtime submit/direct internals may keep Action names. Generated root contains models, mutations, queries, scopes, retained channels and existing utility members; it no longer exposes actions. Per-operation Options and Dart Store selectors retain #116 typing.

- [ ] Add compile-time assertions for every route:

```ts
const local: Promise<Call<AddTodoOutput>> = client.mutations.addTodo({ todo });
const confirmed: Promise<AddTodoOutput> = client.mutations.call.addTodo({ todo });
const read: Promise<SearchTodosOutput> = client.queries.searchTodos({ text: 'x', cursor: null });
const queued: Promise<Call<SearchTodosOutput>> = client.queries.enqueue.searchTodos({ text: 'x', cursor: null });
```

  Add negative cases for wrong namespace, old client.actions, calling wait on a direct result, reading result directly from Call, invalid store keys, and either namespace inside tx. Cover empty Query/Mutation namespaces and collisions with call/enqueue.
- [ ] Route generated methods through the existing two ports:

```text
mutations.name         -> invokeAction
mutations.call.name    -> invokeDirectAction
queries.name           -> invokeDirectAction
queries.enqueue.name   -> invokeAction
```

  Rename public handle/options/error exports consistently without duplicating registries. Keep weak ownership, repeated wait, close/error outcomes and local commit timing. Direct completion still applies authority before resolving.
- [ ] Generate matching Dart namespace classes and Future return types. Rename ActionStore's public base to CallStore, preserving per-operation selectors and business store/outputStore collision handling. Exports must match generated imports on Node, React Native and Dart.
- [ ] Run generation and type fixtures via `bash integration/generated-api/verify.sh`, `bash integration/action-runtime-ts/verify.sh`, and Dart checks from the running guide. Adapt `integration/action-runtime-dart` and `integration/action-contract` fixtures and their negative runners. Ensure generated Node/React Native clients share contracts, not only names.
- [ ] Commit SDK/generation changes with positive and negative type evidence. No changes to connection/downlink/subscription orchestration belong in this task.

## Task 5: Delivery, persistence and store acceptance

**Files:** tests in `crates/sqlite/tests/actions.rs`, `crates/core/tests/contracts.rs`, `crates/server/tests/actions.rs`, `integration/bindings/client-js/actions.test.mjs`, `integration/bindings/client-react-native/actions.test.mjs`, `packages/dart/test/actions_test.dart`, `integration/action-e2e/source/app.model`, `integration/action-e2e/backend-fixture.ts`, `integration/action-e2e/action.test.mts`. Modify `crates/client/src/actions.rs` or queue/protocol code only if failing behavior requires it.

- [ ] Verify default Mutation enqueues offline and commits Model optimism; Query enqueue commits a real durable intent with zero local operations. Close/reopen and freeze both, then settle. Assert store policy and request bytes survive; do not filter Query intents out because their operation arrays are empty.
- [ ] Verify default Query and direct Mutation do not add local queue rows, do not auto-fallback when offline and do not generate client optimism. Exercise TypeScript/React Native transaction guards and Dart's corresponding rule.
- [ ] For all four routes, test default true, false and typed selective store. Cover identical identities in enabled/disabled outputs, inferred mutation readback unaffected by false, result snapshot A versus local pending B, and differing result/current-authority Loader versions.
- [ ] Verify independent direct Query calls use fresh IDs and observe fresh data; retry of the same call ID returns the saved result without another handler/Loader invocation. Retain same-ID changed-args/store conflict checks and backend saved-call transaction behavior. Queued Query result is read at execution time, not enqueue time.
- [ ] Run `cargo test -p axton-core -p axton-client -p axton-server -p axton-sqlite --locked`, native build and targeted language tests, then `bash integration/action-e2e/run.sh` and `bash integration/persistence/server/run.sh`. Record actual observations and any prerequisite limits.
- [ ] Commit acceptance coverage and only the minimum runtime fixes it demonstrates necessary.

## Task 6: Documentation, integration with Bootstrap and final review

**Files:** `docs/engineering/architecture/schema/actions.md`, `docs/engineering/architecture/schema/README.md`, `docs/engineering/architecture/protocol/actions.md`, `docs/engineering/architecture/protocol/push.md`, `docs/engineering/architecture/sdks/typed-api/client.md`, `docs/engineering/architecture/sdks/typed-api/server.md`, `docs/engineering/guarantees.md`, `docs/engineering/architecture.md`, current README/website/example sources and generated assets containing client.actions or action declarations. Do not rewrite historical task plans to pretend they used the new terminology.

- [ ] Update the owning schema page to describe both kinds; a new filename is optional only if all incoming links/anchors are fixed. Explain backend business effects versus framework local storage, all four completion boundaries, queued Query execution-time freshness and the trusted-code limit of Query enforcement.
- [ ] Update current guides/demo fixtures using `rg` to locate actual consumers. Changing an old Action source spelling to Mutation alone does not bump versions; reclassifying a retained operation as Query does. Keep old versions with appropriate registration contexts. Do not silently reset example history to bypass compatibility checks.
- [ ] Include a short decision table and examples demonstrating plain-value Mutations, default Queries, both overrides and store:false. Do not label arbitrary online effects Queries. Do not document automatic pagination, query caching, or Bootstrap completeness beyond their own contracts.
- [ ] Fetch current main before integration. If #151 has merged, merge/rebase it and review overlaps in bindings/common/src/lib.rs, protocol/contracts, server/client exports, SDK exports and persistence tests. Keep its new Bootstrap code and docs; do not replace files with this branch's older versions.
- [ ] When #151 code is present, run its actual tests (currently `cargo test -p axton-server --test bootstrap --locked` and `cargo test -p axton-sqlite --test bootstrap --test bootstrap_worker --locked`) plus its host tests selected from the merged tree. If #151 is still unmerged, say so and require the second branch to perform combined regression; do not claim Bootstrap integration verified.
- [ ] Run `bash scripts/test.sh`, inspect regenerated artifacts, verify relative links/anchors and `git diff --check`. Review spec coverage against compiler, backend capability, all four routes, version dispatch, store, and Bootstrap separation. Fix every actionable finding and rerun affected checks.
- [ ] When separately assigned implementation, follow the issue workflow through PR/review/CI/merge under that assignment's authorization. Include actual evidence, limits and Closes #157. This preparation request does not itself start implementation or create a PR.

## Preparation evidence and handoff boundary

Prepared from main 544e3d0 in the isolated codex/query-mutation-contracts worktree. Baseline command `cargo test -p axton-core -p axton-compiler -p axton-server --locked` passed 238 tests in 20 suite summaries, with no failures. This is existing-behavior evidence only; it does not test Query/Mutation support. Implementation checkboxes remain uncompleted.

Parallelism is safe at the feature-contract level, with known shared-file integration work. The current Bootstrap implementation owns Downlink scheduling, Bootstrap progress/HTTP handling and subscription status; #157 owns schema business kinds, generated route defaults and Query capabilities. No cross-task runtime modification was made during preparation.
