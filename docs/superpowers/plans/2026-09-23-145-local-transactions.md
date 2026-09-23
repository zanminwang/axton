# Local-only Public Transactions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove transaction-scoped named mutations from public TypeScript, React Native and Dart APIs while preserving standalone mutation atomicity and local transaction behavior.

**Architecture:** Separate TypeScript generated `MutatePort` from `WritePort` and Dart runtime-owned `MutatePort` from `WritePort`; remove enqueue from exported transaction instances. Keep `enqueue` as a framework-owned database transaction command called by the client-level mutation method, with an active-callback guard before serializing it.

**Tech Stack:** Rust compiler, TypeScript/Node and React Native adapters, Dart SDK, SQLite core, generated API fixtures.

## Global Constraints

- #145 only; keep `client.mutate.<name>` spelling and existing wire/queue contracts. Do not add Action schema or runtime APIs.
- Preserve local reads, CRUD, rollback, Node/Dart savepoints, lifetime checks, and watch outside callbacks.
- Start in the existing `codex/145-local-transactions` worktree. Read `docs/engineering/architecture/sdks/typed-api/client.md`, `docs/engineering/architecture/client/frontend-interface.md`, `docs/engineering/guarantees.md` and testing guides before edits.
- Follow red/green test cycles; after each task, review its diff and commit only related files.

## File map

`crates/compiler/src/emit.rs` owns generated ports and facade composition; `integration/generated-api` owns emitted fixtures and negative type checks. `packages/client-js/{runtime,transaction}.mts`, `packages/client-react-native/transaction.mts` and `packages/dart/lib/src/{client.dart,port.dart}` own SDK transaction capability; shared `packages/client-js/runtime.mts` owns Node/RN client enqueue orchestration. `integration/bindings` owns host tests. Website and typed API architecture own published behavior.

### Concrete internal interfaces and test shape

In the shared JS runtime, narrow its transaction constructor contract to `finish(): Promise<void>`, `runCallback<T>(body: () => Promise<T>): Promise<T>`, and `inCallback(): boolean`. `runCallback` establishes the callback context; `inCallback` is checked synchronously by public `Client.mutate` before `#exclusive`. Node's `Transaction` uses its `AsyncLocalStorage` for exact context identification. RN `Transaction` sets an active-callback flag from entry until the callback resolves, so any `client.mutate` during that interval rejects with `transaction_active`; unrelated reads/transactions retain their existing serialization. Both transaction classes keep `#send` private. The shared client owns `#submitMutation(mutation: object): Promise<number>`, which runs begin/enqueue/commit under one `#exclusive`, rolls back on failure, and emits work only after commit. Dart parallels this with private `_submitMutation(Map<String,dynamic>): Future<int>` and a Zone token checked by public `mutate` before `_exclusive`.

Representative failing host test (extend `integration/bindings/client-react-native/runtime.test.mjs` using its existing `Client`/SQLite fixture):

```js
await assert.rejects(
  client.transaction(async () => client.mutate({name:'Edit',operations:[{model:'Entry',op:'update',identity:{id:'e'},values:{text:'B'}}]})),
  /transaction_active/
);
assert.equal((await client.syncState()).pending, 0);
```

Use `fixtures/schemas/entry.json` and create Entry `e` first. The descriptor above uses the current `operations` wire shape. Race the rejection against a short timeout, then release the callback and close the client so a deadlock cannot hang cleanup. The assertion checks rejection before entering the client's serial queue. In Node, add a concurrent caller test that starts a callback, invokes `client.mutate` from a separate async context, then releases the callback and observes normal serialization. RN's analogous concurrent call must reject promptly by policy.

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
      try { await this.#send({op:'rollback'}); }
      catch (rollbackError) { throw new AggregateError([error, rollbackError], 'mutation submission and rollback failed'); }
      throw error;
    }
  });
}
```

Preserve the original enqueue failure when rollback succeeds; if rollback also fails, retain both errors for diagnosis (for example with `AggregateError`) rather than silently swallowing rollback or replacing the useful enqueue error. Keep the scoped transaction poisoned/closed after such a failure. `transaction` wraps only `body(tx)` with `tx.runCallback`, sets and clears `#activePublicTx` in a `finally`, and still owns `tx.finish()`/commit/rollback. The internal `#submitMutation` never constructs or exposes a public `Transaction`. For Node, add `#publicContext = new AsyncLocalStorage<symbol>()` and `#publicToken = Symbol()` to `Transaction`; `runCallback` calls `#publicContext.run(#publicToken, body)` and `inCallback()` checks `#publicContext.getStore() === #publicToken`. Keep this distinct from the existing savepoint context. For RN, `runCallback` toggles an active boolean until the returned promise settles; `inCallback()` reads it. Dart `transaction` calls its public body in `runZoned` with a unique per-client `_txZoneKey` and unique per-callback token, sets `_activeTxToken` before entry and clears it in `finally`, while `mutate` checks `_activeTxToken != null && identical(Zone.current[_txZoneKey], _activeTxToken)` before `_exclusive`, and private `_submitMutation` runs `_send({'op':'begin'})`, `_send({'op':'enqueue','mutation': mutation})`, commit/rollback in one `_exclusive` scope. Both language implementations notify work only after commit.

Representative Dart negative analyzer line and local-only positive check:

```dart
void misuse(GeneratedTransaction tx) {
  tx.mutate; // analyzer: getter 'mutate' isn't defined for GeneratedTransaction
}
// Positive runtime test: await client.transaction((tx) => tx.models.entry.get(identity));
```

Run Dart tests from the package directory as documented: `(cd packages/dart && dart test test/client_test.dart)`, after setting `AXTON_DART_LIBRARY` to the built dylib/so absolute path. `bash integration/generated-api/verify.sh` exists and runs compiler, generator, TS, Node and Dart checks; its generated outputs must be reviewed for unintended history changes.

### Task 1: Split generated public ports and facades

**Files:** Modify `crates/compiler/src/emit.rs`; test `crates/compiler/tests/compiler.rs`. Task 4 owns regeneration of `integration/generated-api/{generated.ts,generated.dart,client.ts}` together with consumer migration. Do not run the full runner while its old `tx.mutate` examples still exist.

- [ ] Add compiler assertions that emitted `WritePort` has `direct` but no `mutate`, `GeneratedTransaction` has `models` but no `mutate`, and client-level `Mutate` still accepts `MutatePort`. Check TypeScript ports and the Dart generated facade; `WritePort` itself is runtime-owned in `packages/dart/lib/src/port.dart` and is checked through Dart analysis in Task 3. The old emission must fail these assertions.
- [ ] In `emit.rs`, remove the TypeScript `MutatePort` extension from `WritePort`; remove `GeneratedTransaction.mutate` and Dart equivalent; retain client-level `Mutate(client)`. Do not remove mutation builders, codecs or schema descriptors.
- [ ] Run `cargo test -p axton-compiler --locked`; expect pass. Run compiler emission tests, then defer the full generated API runner until Task 4 has migrated all positive and negative fixture consumers. Inspect generated output only after that coordinated update.
- [ ] Commit compiler emission and its focused tests. Generated fixtures remain on their old baseline until Task 4 updates the consumers and regenerates them together.

### Task 2: Internal atomic enqueue on Node

**Files:** Modify `packages/client-js/runtime.mts`, `packages/client-js/transaction.mts`; test `integration/bindings/client-js/transaction.test.mjs` and a new `integration/bindings/client-js/runtime.test.mjs` using the existing native SQLite harness.

- [ ] Add a host test that inspects the runtime transaction object and asserts no public `mutate` property, plus a rejection test for captured `client.mutate` called inside `client.transaction`; use a bounded timeout so a deadlock is a test failure. Keep tests for a standalone call made after the callback.
- [ ] Add a failure-injection test around standalone enqueue, asserting a rejected call leaves no pending entry and no optimistic record; add a reopen test asserting a successfully queued entry and optimistic value survive closing and reopening the SQLite database. Reuse existing fixture schema and harness, not a mock queue.
- [ ] Run `node --test integration/bindings/client-js/transaction.test.mjs integration/bindings/client-js/runtime.test.mjs`; confirm new tests fail against the original surface.
- [ ] Implement a private client helper that issues begin → enqueue → commit, rolling back on error and notifying work after commit. Keep the engine `enqueue` command inside the transaction. Remove `Transaction.mutate`; leave read/direct/savepoint/finish behavior in place. Check active callback context before entering the client's exclusive queue. Node should distinguish unrelated concurrent callers using async context; do not rely solely on `#exclusive`, which can wait behind the active callback indefinitely.
- [ ] Re-run the focused host tests and the existing client mutation tests; expect pass. Commit.

### Task 3: Match React Native and Dart transaction boundaries

**Files:** Modify `packages/client-react-native/transaction.mts` and shared `packages/client-js/runtime.mts` as required; modify `packages/dart/lib/src/{client.dart,port.dart}`; test `integration/bindings/client-react-native/transaction.test.mjs`, `runtime.test.mjs`, `packages/dart/test/client_test.dart`.

- [ ] Add tests that public raw transactions lack `mutate`, standalone `client.mutate` still enqueues atomically, and captured calls inside callbacks fail without a hang. Assert Dart's Zone guard distinguishes callback work from independent callers. For React Native, use the conservative fail-fast guard while a public transaction is active and test the documented concurrent-call limitation.
- [ ] Run focused RN Node tests and `(cd packages/dart && dart test test/client_test.dart)` with `AXTON_DART_LIBRARY` set per `docs/engineering/testing/running.md`; confirm red where supported.
- [ ] Route each standalone client mutation through an internal begin/enqueue/commit/rollback sequence, then remove public `Transaction.mutate` and change Dart `WritePort implements ReadPort` in `port.dart` (keep `Client implements MutatePort`). Keep Dart savepoint and RN lifetime behavior unchanged. Do not expose the internal helper from SDK exports.
- [ ] Re-run focused tests and Dart analyzer. Commit.

### Task 4: Migrate fixtures, examples and docs without changing semantics

**Files:** Modify `integration/generated-api/{test.ts,native.mts,generated_test.dart,negative/misuse.dart}`, affected examples found by `rg -n 'tx\.mutate|transaction\(' examples integration website/docs --glob '!*.svg'`, `docs/engineering/architecture/sdks/typed-api/client.md`, `website/docs/frontend/client-api.md`, `website/docs/api-index.md` and `packages/client-react-native/README.md`.

- [ ] Regenerate emitted fixtures and add TypeScript `@ts-expect-error` checks for `tx.mutate`/`tx.actions`, then replace transaction-scoped mutation examples with standalone calls outside callbacks in the same change, including the Dart negative analyzer expectations. Keep existing slot argument and deprecation negatives by changing those calls to `client.mutate`; add separate missing-getter cases for both generated and raw transactions. In `integration/generated-api/test.ts`, remove `mutate` from the fake `WritePort` object while keeping the separate `MutatePort` test. For fixtures that used two mutations in one transaction, either represent one declared business mutation with the same tested effect or change the assertion to explicitly test independent calls; never claim the atomicity remains shared. Keep local multi-write transaction tests as local CRUD.
- [ ] Document the captured-client deadlock guard, ordinal semantics, migration tradeoff and watch boundary. Update architecture tables and API index, linking to the source rather than duplicating internals.
- [ ] Run `bash integration/generated-api/verify.sh`, focused SDK tests, `python3 website/scripts/check_examples.py` after documented prerequisites, and `rg -n 'tx\.mutate|tx\.actions' packages integration website/docs docs/engineering --glob '!*.svg'`; expect no live public use. Review generated diff for unrelated history changes. Commit.

## Verification and handoff

Run the relevant host suites and generated API runner with prerequisites from `docs/engineering/testing/running.md`; run `cargo test -p axton-compiler --locked`. Validate a real SQLite reopen and rejection path, then inspect `git diff --check`, generated fixtures, and the final public surface. Record commands, results and limits. A close/reopen test is not a process-crash test. Report the branch and commits; do not begin #141 until this branch is completed and incorporated into its baseline.
