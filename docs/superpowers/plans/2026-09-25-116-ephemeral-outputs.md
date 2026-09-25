# Per-call Action result storage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add call-time `store` options for explicit Model outputs while preserving durable delivery, saved replay and required mutation reconciliation.

**Architecture:** Carry a typed policy in each Action intent and durable queue record. The shared server resolver selects additional output authority while always returning versioned Loader snapshots. Clients continue to apply response records through the existing authority path.

**Tech Stack:** Rust core/client/compiler/server, SQLite, PostgreSQL host adapters, generated TypeScript and Dart, JS/React Native bindings.

## Global constraints

- Follow the [spec](../specs/2026-09-25-116-ephemeral-outputs-design.md). This plan supersedes schema @ephemeral and backend-deployed-policy proposals. The public name is store.
- Work in an isolated codex/ checkout. Preserve concurrent #150/#151 work; do not alter subscription, Downlink worker or Bootstrap behavior.
- Default store is true. Maps target only explicit Model outputs. Required mutation/extra-change authority cannot be disabled.
- Preserve Handler and result contracts, Action version compatibility, saved outcomes and frozen request bytes. No annotation/history policy or new result lifetime mechanism.
- Documents are English. Follow the repository testing strategy and prerequisites in docs/engineering/testing/running.md. These checkboxes describe future implementation, not evidence that it passed.

## Task 1: Typed invocation policy and wire identity

**Files:** `crates/core/src/actions.rs`, `crates/core/src/protocol.rs`, `crates/server/src/actions.rs`; tests in `crates/core/tests/contracts.rs` and `crates/server/tests/actions.rs`.

**Interfaces:** Define core `ActionStore` (All, None, Outputs(BTreeMap<String, bool>)), default All; add it to ActionIntent. Add a descriptor-aware validator and an enabled-output predicate. Serialize All as an omitted intent field, None as false and Outputs as a sorted object. Do not put it in ActionOutputDescriptor.

- [ ] Add failing decoding/validation tests for omitted/true/false/maps, invalid value kinds, and unknown/scalar/implicit/Delete map keys. Include a legitimate business input named store to prove args remain separate. Run `cargo test -p axton-core --test contracts --locked` and inspect expected failures.
- [ ] Implement typed serialization and validation. Keep raw envelope validation separate from Action semantic validation; perform semantic validation before Handler execution using the existing saved per-call failure path.
- [ ] Extend request conversions, including DirectActionResponse's synthetic push request, without changing response envelopes. Canonical call identity includes store; omit All consistently so existing default request fingerprints stay stable.

```text
fingerprint = canonical_json({callId, name, version, args, models,
                              store only when policy != All})
selected(output) = All => true; None => false;
                   Outputs(map) => map.get(output.name).unwrap_or(true)
```

- [ ] Test identical calls with omitted/true policy and reordered map keys replay normally; changed store produces call.identity_conflict without Handler/Loader execution. Test invalid semantic policy in a batch rejects only its own call. Do not silently drop unknown keys whose value is true.
- [ ] Run `cargo test -p axton-core -p axton-server --locked`; commit the protocol contract and focused tests.

## Task 2: Durable queue and direct preparation

**Files:** `crates/client/src/{actions,lib,ddl,queue,push}.rs`, `bindings/common/src/lib.rs`; tests in `crates/sqlite/tests/actions.rs` and `integration/bindings/client-js/actions.test.mjs`.

**Interfaces:** Add store to Mutation. Keep existing Rust submit_action/prepare_action as default-All wrappers and introduce submit_action_with_options/prepare_action_with_options using an ActionCallOptions struct with typed store. Bindings pass options separately from args for submitAction and engine prepareAction.

- [ ] Add a failing SQLite test: submit an offline store:false call, close/reopen the database, freeze it, reopen again, and assert the policy and frozen bytes are unchanged. Also test policy maps and old rows with no store value. Run `cargo test -p axton-sqlite --test actions --locked`.
- [ ] Add nullable store TEXT to both framework DDL and ADDED_COLUMNS. Write canonical policy JSON (NULL for All) and read it with queue args/callId. Keep policy, optimism and enqueue atomic.

```text
validate args and store against retained Action
transaction:
    persist mutation(callId, args, store)
    apply existing inferred optimistic operations
freeze:
    serialize each Action intent including its stored policy
retry:
    retain the same frozen request
```

- [ ] Extend push construction and direct preparation; update all struct initializers. Validate before durable local writes/direct dispatch. Do not clear pending work or regenerate existing frozen bytes to add defaults.
- [ ] Add tests that invalid options leave no optimistic row/queued call, a business store input is preserved, and new calls can choose different policy independently. Carry policy through native bindings and direct response validation.
- [ ] Run `cargo test -p axton-client -p axton-sqlite --locked`; commit queue/binding plumbing with recovery tests.

## Task 3: Policy-aware result and authority resolution

**Files:** `crates/server/src/actions.rs`; create `crates/server/src/action_results.rs` and register it in `crates/server/src/lib.rs` if extracting assembly; extend `crates/server/tests/actions.rs`.

**Interfaces:** Existing result assembly gains validated ActionStore. Executor keeps transaction, claim/save, Handler, required readback and publication ownership. Assembly returns the named result plus additional authority.

- [ ] Add failing Host-fixture tests counting Loader/EnsureStamp calls: store:false returns a full Model result with no additional records or output-only stamp allocation. Run `cargo test -p axton-server --test actions --locked`.
- [ ] Implement positive authority selection, preserving required records before output policy selection:

```text
required = mutation and extra-change readback
additional = union of identities from enabled explicit Model outputs
ensure stamp evidence for additional identities not covered by required
result = resolve all outputs at their retained result read versions
authority = resolve additional at client-declared read versions
return result, required union authority
```

- [ ] Cache reads by Model + canonical identity + read version. Resolve disabled-only results without the unnecessary authority-version read. Reuse mutation readback only at its actual version; establish enabled authority stamp evidence before loading its content.
- [ ] Test shared identities across disabled/enabled outputs in both orders, mutation-target and handler-extra-change overlaps, composite IDs, list duplicates/order, null selections, authoritative absence, missing required/list records and Loader refusal. Assert empty/null business selections cannot delete unrelated data.
- [ ] Test saved replay after backend data changes returns original result/records with no new Loader or Handler calls. Include mixed result/current-authority read versions and a changed-policy conflict.
- [ ] Run `cargo test -p axton-server --locked`; commit resolver behavior and tests.

## Task 4: Generated SDK options and language contracts

**Files:** `crates/compiler/src/emit.rs`, `packages/client-js/runtime.mts`, `packages/dart/lib/src/client.dart`; fixtures under `integration/action-contract`, `integration/action-runtime-ts`, `integration/action-runtime-dart`; tests in `crates/compiler/tests/compiler.rs`, `packages/dart/test/actions_test.dart`, `integration/bindings/client-react-native/actions.test.mjs`.

**Interfaces:** Generated TS methods add an optional final options argument; runtime invokeAction/invokeDirectAction forward it separately from encoded args. Generated Dart methods use typed per-Action selectors per the spec. Existing calls/results remain source-compatible.

- [ ] Add positive/negative generated type fixtures for false, enabled/disabled named outputs, invalid names/scalar keys, both routes, zero eligible outputs and a business input named store. Confirm failures before generator changes using `bash integration/action-runtime-ts/verify.sh` and the generated API runner `bash integration/generated-api/verify.sh` after prerequisites.
- [ ] Generate the TS type and signatures:

```ts
type ActionOptions<K extends string> = {
  store?: boolean | Partial<Record<K, boolean>>;
};
// SearchTodos has the explicit Model output todos:
searchTodos(args: SearchTodosInput,
            options?: ActionOptions<'todos'>): Promise<ActionCall<SearchTodosOutput>>;
// Its direct counterpart returns Promise<SearchTodosOutput>.
```

  Generate boolean-only options when no Model keys are eligible. Forward options through runtime methods and native requests; do not merge with encoded business args.
- [ ] Generate Dart all/none/outputs selector constructors and serialize them to the same bool/map wire shape. Resolve any generated option/business parameter collision explicitly and add an example proving it. Keep runtime bridge types shared between both entry points.
- [ ] Rebuild native artifacts and generated fixtures, then run TS runtime/generated API checks and `(cd packages/dart && dart analyze && dart test)` with the library environment from the running guide. Verify native host tests on JS and React Native; report device testing separately.
- [ ] Confirm compiler history snapshots and backend Handler signatures have no new policy field or version requirement. Commit generator, runtimes and fixtures.

## Task 5: Cross-path acceptance, documentation and review

**Files:** `integration/action-e2e/{source/app.model,backend-fixture.ts,action.test.mts}`, `integration/persistence/server/actions.test.mjs`, `crates/sqlite/tests/actions.rs`; docs `docs/engineering/architecture/protocol/{actions,push}.md`, `docs/engineering/architecture/sdks/typed-api/client.md`, `docs/engineering/architecture/schema/actions.md`, and affected website Action examples.

- [ ] Add both-route E2E cases for default storage, disabled-only storage, mixed outputs and required write reconciliation. Assert result title A while local pending title B remains; repeat with newer local authority. Observe local content/stamps and Model notifications, not only response length.
- [ ] Exercise actual PostgreSQL saved-call transactions: lost response/retry, no Loader rerun, changed-policy identity conflict, saved semantic failure isolation, and consistent content/stamp under concurrent writes. Run `bash integration/persistence/run.sh` and `bash integration/action-e2e/run.sh` after documented prerequisites.
- [ ] Document store as per-invocation output storage: defaults, selective map, two entry points, no Action version bump, required reconciliation and saved retry policy. Do not present it as read-only, cache eviction, or absence of backend persistence. Remove schema @ephemeral guidance wherever this implementation owns it; retain historical task records as history.
- [ ] Run `bash scripts/test.sh`, inspect generated changes, run whitespace/link checks, and resolve failures. Do not claim a skipped prerequisite-dependent suite passed.
- [ ] Review the complete diff against every acceptance bullet in the spec. Inspect invalid-policy-before-enqueue, every intent conversion, fingerprint normalization, frozen/reopen persistence, overlap union, stamp timing and read-version reuse specifically. Fix findings before merging.
- [ ] Open a PR with Closes #116 and actual verification evidence; follow repository issue labels/state updates. Self-review the PR and required CI, resolve findings, merge once permitted, then verify merged main/issue status. Do not stop at opening a PR or bypass branch protection.
