# Client

## 1. Introduction and Goals

The generated API names application Models, Mutations and Queries. The generic SDK translates those typed calls into tasks for one Rust runtime. It owns language scopes, decoding and handles; Rust owns durable requests, cache admission, authority, progress and settlement.

## 3. Context and Scope

| Verb | Surface | Boundary |
| --- | --- | --- |
| Read local data | `client.models.<model>.get / query / watch / <relation>`, SQL reads and watches, `status`, `recordStatus`, `outbound`, `rejections`, `failures` | Read committed local projections: the server data the Store holds with pending Mutations applied. Observers report distinct committed results. |
| Send Mutations | `client.mutations.<name>(input)`, `tx.mutations.<name>(input)`, `call.wait()`, `dismiss`, `retry`, `drop` | Commit typed input, inferred optimism and durable intent; answer a `Call<Output>`. `call.wait()` waits for backend outcome and this Store's committed settlement, not for other clients. |
| Read from the server | `client.queries.<name>(input, options?)`, `client.fetch.<model>(identity, options?)`, `client.bootstrap()` | A Query is a direct named read: it awaits the invocation result and permitted local cache commit and is never enqueued. Fetch is a direct versioned Model Loader read. Bootstrap awaits finite coverage at a captured head. |

`GeneratedClient.open({path, stream, connection?})` opens one physical SQLite file for one Stream. Rust creates the Store identity; transport starts only when a connection is supplied. `client.connection`, `connect`, `close`, `resetStore` and the sync and recovery APIs manage lifecycle and recovery within the same Store binding. Reset changes incarnation; ordinary reconnect and reopen do not.

`client.transaction(body)` is one local atomic boundary for local reads, several named Mutations and unsent-work resolutions, with savepoints. It returns the callback value after commit. `tx.models` only reads. A caller that builds Mutation input from local data reads it in the transaction and then calls `tx.mutations.<name>(input)`; both see one snapshot. React Native uses the generated TypeScript surface with its own transaction adapter.

Every Model row comes from the server or from a pending Mutation; there is no local write that is not sent. An app keeps device-only data, such as drafts or scroll positions, in its own store. It orders that write and the Mutation itself and reconciles them by identity: a Call's `callId` and the record identities it sent are stable across retries and reopen.

The [public guide](../../../../website/docs/frontend/client-api.md) owns runnable TypeScript/Dart examples. One local transaction is not a distributed backend transaction: its Calls retain independent outcomes. Backend atomic work belongs in one named Mutation.

Public anonymous mutations, direct Mutation `call`, queued Query `enqueue`, Load jobs, custom incoming-store hooks and multiple Stream subscriptions are retired.

## 5. Building Block View

[Compiler emission](../../../../crates/compiler/src/emit.rs) and [transaction emission](../../../../crates/compiler/src/emit_transactions.rs) generate typed facades/codecs. [Node runtime](../../../../packages/frontend/client-js/api/runtime.mts), [Dart client](../../../../packages/frontend/dart/lib/src/api/client.dart) and the [RN adapter](../../../../packages/frontend/client-react-native/api/transaction.mts) submit through native Bridges. [Bindings](bindings.md) own platform loading and per-client actors. [Protocol 5](../protocols/sync.md) owns persisted semantics.

[Command accounting](../../../../packages/frontend/client-js/api/command-accounting.mts) owns pending work, draining and first command failure for Node and RN transaction scopes. Each adapter retains its own admission and lifetime guards; Node savepoint rollback restores the enclosing scope's recorded failure.

## 6. Runtime View

A Mutation invocation resolves after local commit; a Call obtained inside an outer transaction remains provisional until that commit. `wait()` before commit refuses promptly with `transaction_uncommitted`; rollback ends the handle with `transaction_rolled_back`. Input encoding errors reject the invocation and roll back its local scope without queuing a Call. Backend acceptance followed by local apply failure remains accepted-awaiting-settlement. Retry and reopen replay retained operations, not application code.

Query/Fetch use request-level boolean `store`, default true. Results always describe the invocation snapshot. Ordinary returned Model content has `cursor:null`; true writes only where no current Stream content/deletion guard prevents it. A protected no-op succeeds and may return content different from the Store. False installs no Model/authority. Returning a Model does not track it, and missing results do not mean canonical deletion.

The protocol-5 facades submit a fresh task for every Query invocation. Public `once`, `refresh` and Query invalidation controls are removed; authentication refresh remains. Open takes a path, schema and Stream with an optional connection, allowing offline operation. Joined native lifecycle evidence belongs to `integration/v05-sdk/run-host.sh`; installed mobile evidence remains separate.

Model watch receivers apply Rust's versioned ordered splices to a private decoded
cache. Generated watches decode only incoming rows and copy values for each
application snapshot: TypeScript copies objects, Lists and Dates; Dart reuses
immutable scalar-only Models and copies List-bearing Models. Each Dart listen
owns its cache. Public watch signatures still return complete result lists, so
building a delivered list remains proportional to its length. Legacy generated
watches use the generic row API and remain supported, but retain their full-list
Model conversion until regenerated. SQL watches keep their
full result and dependency-table refresh behavior.

A malformed patch or failed decode/copy reports an observer error and requests
one full cache snapshot. Further patches are ignored until a valid full result
recovers it; a failed recovery does not trigger a resnapshot loop. Listener
exceptions do not request recovery. Native read errors retain the runtime's
existing retry path. Receiver and native coverage lives in the
[JavaScript tests](../../../../packages/frontend/client-js/model-watch.test.mjs),
[Dart tests](../../../../packages/frontend/dart/test/model_watch_test.dart) and
[SQLite tests](../../../../crates/sqlite/tests/model_watch_incremental/mod.rs).

Language scopes refuse captured/expired capabilities and unawaited operations before commit. Node uses async-context ownership. RN's guard is coarse: while a callback runs, every public call counts as inside it; ordinary reads can queue behind the active transaction. Cross-Store overlapping RN callbacks require async-context support and are refused. Bootstrap and named Mutation entry checks prevent waiting behind their own callback. Priority close settles outstanding tasks and cancels runtime effects without waiting for unresolved user code.

## 10. Quality Requirements

- Bound files, lifecycle, requests, observations, cache and queues stay independent. Evidence: [native protocol tests](../../../../crates/sqlite/tests), [RN binding tests](../../../../integration/bindings/client-react-native) and [protocol-5 host](../../../../integration/v05-sdk/README.md).
- Typed input, and reads followed by Mutations in one transaction, preserve local atomicity and independent Call fates. Evidence: [generated contract checks](../../../../integration/action-contract), [Node native Mutation checks](../../../../packages/frontend/client-js/mutation-native05.test.mjs) and [Dart package tests](../../../../packages/frontend/dart/test).
- Ordinary reads cannot overwrite current Stream protection; result snapshots remain independent. Evidence: [Query/Fetch bindings](../../../../integration/bindings/client-js) and the [protocol-5 host](../../../../integration/v05-sdk/README.md).
- Installed generated APIs and bundled libraries work outside the checkout. Evidence: [installed-package runner](../../../../integration/release/verify-installed.sh).


## 11. Risks and Technical Debt

RN lacks Node's exact async-context ownership; its conservative guards and refusal of overlapping callbacks are intentional. A hung user callback can hold this Store's transaction until priority close cancels it. Remote reads remain concurrent with synchronization; no SDK serializes whole remote request lifecycles. Required-group Loader/constraint failures can delay initialization or settlement and remain observable rather than fabricating completion.
