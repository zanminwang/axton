# Client

## 1. Introduction and Goals

An application should write `client.mutate.editEntry(...)` and `client.models.entry.watch(...)` and never see JSON or cursors. The client typed API is that layer: a generic runtime class per language that knows how to talk to Rust, and generated classes that give it the application's model names and types.

## 3. Context and Scope

What an application sees:

| Surface | Purpose |
| --- | --- |
| `GeneratedClient.open({path, server?, connection?})` | open the local database; with `server`, connect and keep syncing |
| `client.models.<model>.get / query / watch / <relation>` | reads on the last commit; `watch` re-emits when results change |
| `client.mutate.<mutation>(args)` | one named mutation in its own local transaction |
| `client.transaction(tx => …)` with `tx.models.<model>.create / update / delete` | local reads and direct writes in one local transaction |
| `client.channels.subscribe / unsubscribe` | choose which server channels to follow |
| `client.syncState()`, `client.models.<model>.syncState(identity)` | the client's and one record's sync state; the record form is typed by model and mutation names |
| `clientId`, `connect()`, `pendingTasks()`, `setReadiness()`, `runPrerequisites()`, `drop()`, `dismissRejection()`, `querySpec()`, `readSql()`, `close()` | identity, connection, recovery and escape hatches, on the same object |

Every call becomes one command through the [bindings](../bindings.md). Generated code depends on the runtime package (`@axton/client`, `package:axton`); the generic runtime depends on nothing generated.

## 5. Building Block View

- **Ports.** Read, write, live, and mutation ports separate capabilities: a write port adds `direct` to reads; a live port adds `watch`; a mutation port belongs to the client facade. A transaction implements the write port and cannot watch or enqueue a named mutation. [Generated code](../../compiler/generate.md) binds model and mutation classes to these ports.
- **TypeScript hosts.** Node and React Native share [client orchestration](../../../../../packages/client-js/runtime.mts). Each entry point supplies its native string carrier, transaction scope, and server transport. Node retains AsyncLocalStorage savepoints; React Native uses an explicit transaction scope without a nested-savepoint API.
- **Client.** One promise chain per client serializes every command, so calls from the application, the connection and watchers never interleave inside Rust. `transaction` sends `begin`, runs the body against a transaction object, then `finish` and `commit`, or `rollback` on any error. A standalone `client.mutate` privately encloses optimistic writes and enqueueing in one SQLite transaction, then returns a local ordinal after commit.
- **Transaction.** Commands are queued in submission order and marked as belonging to the transaction. `finish` fails if any command was never awaited, if any command failed even though the application caught the error, or if savepoints overlapped. `savepoint(body)` nests via async context (TypeScript) or zone values (Dart) so a failure inside it is confined to that scope.
- **Captured client guard.** A `client.mutate` call from its own active transaction callback fails before it can wait on the client's queue. Node uses async context and Dart a zone token to identify that callback. React Native rejects any `client.mutate` while a public transaction is active, including an unrelated concurrent caller; retry after the transaction settles. [Client runtime](../../../../../packages/client-js/runtime.mts) owns the shared guard and enqueue flow.
- **Watch.** Re-runs the query after every commit notification and emits only when the JSON result differs; Dart exposes a broadcast stream.
- **Generated code.** Types, codecs (dates to `Date`/`DateTime`), mutation builders, model classes and the two facades `GeneratedClient` and `GeneratedTransaction` ([Compiler / Generate](../../compiler/generate.md)). The client owns named mutations and watch; the transaction owns local model reads and writes. Presence is expressed as an omitted key versus `null` in TypeScript and as `Present<T>?` in Dart; both encode to the same wire patch.

Code: [React Native adapter](../../../../../packages/client-react-native/index.ts), [shared runtime](../../../../../packages/client-js/runtime.mts), [client-js/index.mts](../../../../../packages/client-js/index.mts), [client-js/transaction.mts](../../../../../packages/client-js/transaction.mts), [dart/client.dart](../../../../../packages/dart/lib/src/client.dart), [dart/port.dart](../../../../../packages/dart/lib/src/port.dart).

## 9. Architecture Decisions

**One client object ([#133](https://github.com/zanminwang/axton/issues/133)).** The generated client is the only client an application sees. It keeps the `models` / `mutate` / `channels` namespaces (mutation names are schema-chosen and may span models, so they never share a namespace with model built-ins) and carries the runtime members directly; the runtime `Client` is an internal handle (`client.client`) used by the framework's own tests. The per-record sync state lives beside `get` and `query` on each model and is typed by that model's identity; its pending names are a union of the schema's mutations (TypeScript) or strings (Dart).

**Automatic model-version declaration ([#91](https://github.com/zanminwang/axton/issues/91)).** Generated client configuration identifies the [model versions](../../schema/models.md#9-architecture-decisions) expected by its generated types: the schema embedded in `generated.ts` and `generated.dart` carries each model's `version`, and `open` passes it to the Rust runtime unchanged. The runtime declares those read contracts to the server as `models` on every pull and on the subscribe frame ([Protocol / Pull](../../protocol/pull.md)); application code does not supply versions on each `channels.subscribe(...)` call. HTTP catch-up and WebSocket delivery must use the same selected contracts. This adds no protocol policy to the SDK: generated metadata passes through bindings to the runtime. Wire placement and validation remain to be designed; read-error behavior follows [failure isolation](../../server/engine/pull.md#9-architecture-decisions).

## 10. Quality Requirements

- **An unawaited or escaped call poisons the transaction, and a caught failure still rolls it back unless confined to a savepoint** (binding half of guarantee L3). Evidence: [transaction.test.mjs](../../../../../integration/bindings/client-js/transaction.test.mjs); [dart/test/client_test.dart](../../../../../packages/dart/test/client_test.dart) `Dart callbacks read their writes, rollback and reopen through native Rust`.
- **Generated code forwards calls unchanged and rejects misuse at compile time**. Evidence: [integration/generated-api/test.ts](../../../../../integration/generated-api/test.ts) (positives and `@ts-expect-error` negatives); [generated_test.dart](../../../../../integration/generated-api/generated_test.dart) (positives only).
- **One end-to-end flow per language works against a real backend**. Evidence: [round-trip.test.mjs](../../../../../integration/e2e/round-trip.test.mjs).

Tests read, not executed.

## 11. Risks and Technical Debt

**Accepted limitation (planned change).** `watch` re-runs its query on every commit, whatever table changed; the binding already reports touched tables but neither client uses them, and there are no status streams. Scoped notifications are [#16](https://github.com/zanminwang/axton/issues/16).

**Accepted limitation.** `Client.open` and the generated `open` still accept a `migration` option that the runtime ignores; its future is part of [#20](https://github.com/zanminwang/axton/issues/20).

**To confirm.** No test runs one script through the Rust, TypeScript and Dart clients and compares state; the two clients are separate implementations of the same session logic ([Live session](../../client/connection/controller/live-session.md)), sharing the Rust engine alone does not establish equivalent behavior across SDK boundaries. The shared scenarios in `fixtures/scenarios` are prose READMEs, not an executable cross-language runner.
