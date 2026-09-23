# Local-only Public Transactions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove transaction-scoped named mutations from public TypeScript, React Native and Dart APIs while preserving standalone mutation atomicity and local transaction behavior.

**Architecture:** Separate the generated `MutatePort` from read/direct-write `WritePort`; remove enqueue from exported transaction instances. Keep `enqueue` as a framework-owned database transaction command called by the client-level mutation method, with an active-callback guard before serializing it.

**Tech Stack:** Rust compiler, TypeScript/Node and React Native adapters, Dart SDK, SQLite core, generated API fixtures.

## Global Constraints

- #145 only; keep `client.mutate.<name>` spelling and existing wire/queue contracts. Do not add Action schema or runtime APIs.
- Preserve local reads, CRUD, rollback, Node/Dart savepoints, lifetime checks, and watch outside callbacks.
- Start in the existing `codex/145-local-transactions` worktree. Read `docs/engineering/architecture/sdks/typed-api/client.md`, `docs/engineering/architecture/client/frontend-interface.md`, `docs/engineering/guarantees.md` and testing guides before edits.
- Follow red/green test cycles; after each task, review its diff and commit only related files.

## File map

`crates/compiler/src/emit.rs` owns generated ports and facade composition; `integration/generated-api` owns emitted fixtures and negative type checks. `packages/client-js/{runtime,transaction}.mts`, `packages/client-react-native/transaction.mts` and `packages/dart/lib/src/client.dart` own SDK transaction capability; shared `packages/client-js/runtime.mts` owns Node/RN client enqueue orchestration. `integration/bindings` owns host tests. Website and typed API architecture own published behavior.

### Concrete internal interfaces and test shape

In the shared JS runtime, narrow its transaction constructor contract to `finish(): Promise<void>`, `runCallback<T>(body: () => Promise<T>): Promise<T>`, and `inCallback(): boolean`. `runCallback` establishes the callback context; `inCallback` is checked synchronously by public `Client.mutate` before `#exclusive`. Node's `Transaction` uses its `AsyncLocalStorage` for exact context identification. RN `Transaction` sets an active-callback flag from entry until the callback resolves, so any `client.mutate` during that interval rejects with `transaction_active`; unrelated reads/transactions retain their existing serialization. Both transaction classes keep `#send` private. The shared client owns `#submitMutation(mutation: object): Promise<number>`, which runs begin/enqueue/commit under one `#exclusive`, rolls back on failure, and emits work only after commit. Dart parallels this with private `_submitMutation(Map<String,dynamic>): Future<int>` and a Zone token checked by public `mutate` before `_exclusive`.

Representative failing host test (extend `integration/bindings/client-react-native/runtime.test.mjs` using its existing `Client`/SQLite fixture):

```js
await assert.rejects(
  client.transaction(async () => client.mutate({name:'Edit',version:1,slots:{}})),
  /transaction_active/
);
assert.equal(await client.syncState().then(s => s.pending), 0);
```

The test's mutation descriptor must use a valid existing fixture; the assertion specifically checks prompt rejection before entering the client's serial queue. In Node, add a concurrent caller test that starts a callback, invokes `client.mutate` from a separate async context, then releases the callback and observes normal serialization. RN's analogous concurrent call must reject promptly by policy.

The generated TS negative test belongs in a typechecked fixture, not a runtime-only assertion:

```ts
// @ts-expect-error transactions have no named mutation namespace
void tx.mutate.createEntry({entry: row});
// @ts-expect-error Action namespace is not part of #145 transactions
void tx.actions;
```

Dart's `misuse.dart` must reference `tx.mutate` as a missing getter, and `negative/check.sh` must expect that analyzer message rather than old slot-specific errors from `tx.mutate`.

Representative shared-runtime implementation shape (adapt to current private-field style; `#activePublicTx` is set only while the public callback runs):

```ts
#activePublicTx: Tx | undefined;
mutate(mutation: object): Promise<number> {
  if (this.#activePublicTx?.inCallback())
    return Promise.reject(Error('transaction_active'));
  return this.#submitMutation(mutation);
}
#submitMutation(mutation: object): Promise<number> {
  return this.#exclusive(async () => {
    await this.#send({op:'begin'});
    try {
      const ordinal = await this.#send({op:'enqueue',mutation}) as number;
      await this.#send({op:'commit'});
      this.#events.emit('work');
      return ordinal;
    } catch (error) {
      await this.#send({op:'rollback'});
      throw error;
    }
  });
}
```

Use the current transaction rollback error handling pattern in `runtime.mts` so a rollback failure is not silently lost. `transaction` wraps only `body(tx)` with `tx.runCallback`, sets and clears `#activePublicTx` in a `finally`, and still owns `tx.finish()`/commit/rollback. The internal `#submitMutation` never constructs or exposes a public `Transaction`. For Node, add `#publicContext = new AsyncLocalStorage<symbol>()` and `#publicToken = Symbol()` to `Transaction`; `runCallback` calls `#publicContext.run(#publicToken, body)` and `inCallback()` checks `#publicContext.getStore() === #publicToken`. Keep this distinct from the existing savepoint context. For RN, `runCallback` toggles an active boolean until the returned promise settles; `inCallback()` reads it. Dart `transaction` calls its public body in `runZoned` with a private per-client `_txZoneKey` and active token, while `mutate` checks `Zone.current[_txZoneKey] == _activeTxToken` before `_exclusive`, while private `_submitMutation` runs `_send({'op':'begin'})`, `_send({'op':'enqueue','mutation': mutation})`, commit/rollback in one `_exclusive` scope. Both language implementations notify work only after commit.

Representative Dart negative analyzer line and local-only positive check:

```dart
void misuse(GeneratedTransaction tx) {
  tx.mutate; // analyzer: getter 'mutate' isn't defined for GeneratedTransaction
}
// Positive runtime test: await client.transaction((tx) => tx.models.entry.get(identity));
```

Run Dart tests from the package directory as documented: `(cd packages/dart && dart test test/client_test.dart)`, after setting `AHEAD_DART_LIBRARY` to the built dylib/so absolute path. `bash integration/generated-api/verify.sh` exists and runs compiler, generator, TS, Node and Dart checks; its generated outputs must be reviewed for unintended history changes.

### Task 1: Split generated public ports and facades

**Files:** Modify `crates/compiler/src/emit.rs`; test `crates/compiler/tests/compiler.rs`; regenerate `integration/generated-api/generated.ts`, `generated.dart`, `client.ts` and related checked-in outputs with `integration/generated-api/verify.sh`.

- [ ] Add compiler assertions that emitted `WritePort` has `direct` but no `mutate`, `GeneratedTransaction` has `models` but no `mutate`, and client-level `Mutate` still accepts `MutatePort`. Check both TypeScript and Dart output. The old emission must fail these assertions.
- [ ] In `emit.rs`, remove the `MutatePort` extension from `WritePort`; remove `GeneratedTransaction.mutate` and Dart equivalent; retain client-level `Mutate(client)`. Do not remove mutation builders, codecs or schema descriptors.
- [ ] Run `cargo test -p ahead-compiler --locked`; expect pass. Regenerate fixtures using the documented generated API command, then inspect the diff for intentional changes only.
- [ ] Add TypeScript `@ts-expect-error` checks for `tx.mutate` and `tx.actions` in `integration/generated-api/test.ts`, and Dart negative references in `integration/generated-api/negative/misuse.dart` with expected analyzer diagnostics in its check script. Keep positive local CRUD compile checks.
- [ ] Commit the compiler/fixture contract change.

### Task 2: Internal atomic enqueue on Node

**Files:** Modify `packages/client-js/runtime.mts`, `packages/client-js/transaction.mts`; test `integration/bindings/client-js/transaction.test.mjs` and `integration/bindings/node/transaction-bridge.test.mjs`.

- [ ] Add a host test that inspects the runtime transaction object and asserts no public `mutate` property, plus a rejection test for captured `client.mutate` called inside `client.transaction`; use a bounded timeout so a deadlock is a test failure. Keep tests for a standalone call made after the callback.
- [ ] Add a failure-injection test around standalone enqueue, asserting a rejected call leaves no pending entry and no optimistic record; add a reopen test asserting a successfully queued entry and optimistic value survive closing and reopening the SQLite database. Reuse existing fixture schema and harness, not a mock queue.
- [ ] Run `node --test integration/bindings/client-js/transaction.test.mjs integration/bindings/node/transaction-bridge.test.mjs`; confirm new tests fail against the original surface.
- [ ] Implement a private client helper that issues begin → enqueue → commit, rolling back on error and notifying work after commit. Keep the engine `enqueue` command inside the transaction. Remove `Transaction.mutate`; leave read/direct/savepoint/finish behavior in place. Check active callback context before entering the client's exclusive queue. Node should distinguish unrelated concurrent callers using async context; do not rely solely on `#exclusive`, which can wait behind the active callback indefinitely.
- [ ] Re-run the focused host tests and the existing client mutation tests; expect pass. Commit.

### Task 3: Match React Native and Dart transaction boundaries

**Files:** Modify `packages/client-react-native/transaction.mts` and shared `packages/client-js/runtime.mts` as required; modify `packages/dart/lib/src/client.dart`; test `integration/bindings/client-react-native/transaction.test.mjs`, `runtime.test.mjs`, `packages/dart/test/client_test.dart`.

- [ ] Add tests that public raw transactions lack `mutate`, standalone `client.mutate` still enqueues atomically, and captured calls inside callbacks fail without a hang. Assert Dart's Zone guard distinguishes callback work from independent callers. For React Native, use the conservative fail-fast guard while a public transaction is active and test the documented concurrent-call limitation.
- [ ] Run focused RN Node tests and `(cd packages/dart && dart test test/client_test.dart)` with `AHEAD_DART_LIBRARY` set per `docs/engineering/testing/running.md`; confirm red where supported.
- [ ] Route each standalone client mutation through an internal begin/enqueue/commit/rollback sequence, then remove public `Transaction.mutate`. Keep Dart savepoint and RN lifetime behavior unchanged. Do not expose the internal helper from SDK exports.
- [ ] Re-run focused tests and Dart analyzer. Commit.

### Task 4: Migrate fixtures, examples and docs without changing semantics

**Files:** Modify `integration/generated-api/{test.ts,native.mts,generated_test.dart,negative/misuse.dart}`, affected examples found by `rg -n 'tx\.mutate|transaction\(' examples integration website/docs --glob '!*.svg'`, `docs/engineering/architecture/sdks/typed-api/client.md`, `website/docs/frontend/client-api.md`, `website/docs/api-index.md` and `packages/client-react-native/README.md`.

- [ ] Replace transaction-scoped mutation examples with standalone calls outside callbacks. For fixtures that used two mutations in one transaction, either represent one declared business mutation with the same tested effect or change the assertion to explicitly test independent calls; never claim the atomicity remains shared. Keep local multi-write transaction tests as local CRUD.
- [ ] Document the captured-client deadlock guard, ordinal semantics, migration tradeoff and watch boundary. Update architecture tables and API index, linking to the source rather than duplicating internals.
- [ ] Run `bash integration/generated-api/verify.sh`, focused SDK tests, `python3 website/scripts/check_examples.py` after documented prerequisites, and `rg -n 'tx\.mutate|tx\.actions' packages integration website/docs docs/engineering --glob '!*.svg'`; expect no live public use. Review generated diff for unrelated history changes. Commit.

## Verification and handoff

Run the relevant host suites and generated API runner with prerequisites from `docs/engineering/testing/running.md`; run `cargo test -p ahead-compiler --locked`. Validate a real SQLite reopen and rejection path, then inspect `git diff --check`, generated fixtures, and the final public surface. Record commands, results and limits. A close/reopen test is not a process-crash test. Report the branch and commits; do not begin #141 until this branch is completed and incorporated into its baseline.
