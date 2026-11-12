# AXTON holds a projection and a queue, nothing else

Issue: none yet. Status: implemented on branch codex/projection-and-queue (2026-11-10). Slug: `projection-and-queue`.

A Store holds two things: a partial projection of server data and a queue of Mutations. It holds no device-only data. The client API has three verbs: read local data, send Mutations, read from the server. Direct Model writes, Mutation companion writes and the Mutation input callback are removed, with every table, command and type that served only them. Done means no SDK, bridge command or engine function writes a Model row except optimistic Mutation application and server delivery, and every remaining test passes without a direct write.

This spec assumes [upgrade by overwrite](2026-11-09-upgrade-by-overwrite-design.md) has landed: an upgrade fixes table shapes, runs Bootstrap again and overwrites, deletes nothing, keeps the Stream position, and the Batch identity carries no schema version. Nothing here depends on its internals.

## Contents

1. [Problem and evidence](#1-problem-and-evidence)
2. [Design](#2-design)
3. [Changes](#3-changes)
4. [Decisions](#4-decisions)
5. [Verification](#5-verification)
6. [Out of scope](#6-out-of-scope)
7. [Open questions](#7-open-questions)

## 1. Problem and evidence

AXTON today offers three kinds of local write ([writes.md](../../engineering/architecture/client/engine/local-operations/writes.md)): a named Mutation, a direct Model write that is never sent, and a companion write owned by a Mutation's input callback. The last two exist so an app can keep device-only data in the same SQLite file and commit it atomically with a Mutation. They cost the engine a second ordering system beside the queue:

- **A journal for writes the server never sees.** A direct write on a record with pending Mutations is stored in `axton_local_write` as `independent` so later settlement neither undoes nor reorders it (`crates/client/src/mutate.rs:495-510`, `queue.rs:24-45`). Rebuild interleaves pending operations with those rows by a four-part order key (`mutate.rs:21-35`, `:216-262`).
- **A third operation kind.** Queue operations are `wire`, `companion` or `effect` (`queue.rs:10-16`). Companions settle with their owner: kept on acceptance when the owner still owns the record, removed on refusal (`settlement05.rs:343-355`). A companion may only follow the latest unsent Mutation with no independent write after it (`mutate.rs:393-417`).
- **A nested callback protocol.** The Mutation input callback runs as a restricted capability inside an open transaction: a `mutationLocal` effect, a `companionId` token, a deferred Mutation whose writes are previewed in a savepoint, rolled back, then replayed as companions once the callback returns its input (`crates/client/src/runtime/transactions.rs:63-80`, `:316-348`, `:378-500`; `crates/protocols/src/client_bridge/mod.rs:68-108`, `:444-474`, `:755-765`).
- **Rows the data cannot classify.** A direct write leaves an `axton_authority` row with no Stream evidence (`settlement05.rs:14-18`), the same shape a cache install leaves. The previous rollover needed a rule for "direct write on a Stream-evidenced record" ([guarantees.md](../../engineering/guarantees.md):106), and guarantee L4 exists only to say how direct writes interact with protection.
- **A second meaning of "Model".** A Model whose Loader is left out is device-only ([backend api](../../../website/docs/backend/api.md):203-212, `crates/server/src/lib.rs:152-170`, `packages/backend/server/bindings/effects.mts:302-322`), so every publish path checks for it.

Device-only data also makes an upgrade a migration problem: rows that exist nowhere else cannot be pulled again. With none in the Store, "pull everything again and overwrite" is the whole upgrade story.

Capso is the only consumer. It uses direct writes for three device-only Models (`Composition`, `ChatHistory`, `MediaUrl`) and for one synced Model (`SpaceShelfEntry`); its other Mutation callbacks only read.

## 2. Design

### 2.1 What a Store holds

| Part | Written by | Read by |
| --- | --- | --- |
| Projection: Model rows, before-images, `axton_authority` | Stream delivery, Query and Fetch cache installs, receipt finalization | local reads and watches |
| Queue: `axton_mutation_queue` and its operations, dependencies, prerequisites | `mutations.<name>` | uplink, settlement, optimistic rebuild |

The screen shows the projection with the queue's pending operations applied on top. Nothing else writes a Model row. Two paths delete rows and write none: `resetStore` (`crates/client/src/sync05/reset.rs:33`) and the reset of an incompatible Model at upgrade (upgrade by overwrite §2.2). An app that needs device-only data (drafts, scroll positions, URL caches) keeps it in its own store.

### 2.2 Client API

| Verb | Surface |
| --- | --- |
| Read local data | `client.models.<model>.get / query / watch / <relation>`, SQL reads and watches, `status`, `recordStatus`, `outbound`, `rejections`, `failures` |
| Send Mutations | `client.mutations.<name>(input)`, `tx.mutations.<name>(input)`, `call.wait()`, and the unsent-work resolutions (`dismiss`, `retry`, `drop`) |
| Read from the server | `client.queries.<name>`, `client.fetch.<model>`, `client.bootstrap()` |

`client.transaction(body)` stays: one local atomic boundary for local reads, several Mutations and unsent-work resolutions, with savepoints. `tx.models` is read-only. Lifecycle (`open`, `connect`, `close`, `resetStore`) is unchanged.

Removed:

- `client.models.<model>.create / update / delete` and `tx.models.<model>.create / update / delete`.
- The callback form of a Mutation call: TypeScript `mutations.<name>((tx) => input)`, Dart `mutations.<name>.withTransaction(...)`, and `CompanionContext`. A caller that reads before building its input does so in `client.transaction` and then calls `tx.mutations.<name>(input)`; both run in the same local transaction, as they did inside the callback.

There is no atomic boundary between app-owned device data and a Mutation. An app orders the two steps and reconciles by identity: the Mutation's `callId` and the record identities it sent are stable across retries and reopen.

### 2.3 Engine

One ordering system remains: the queue.

- **Operation kinds.** `wire` (a Mutation input) and `effect` (a cascade delete derived from a wire delete) stay. `companion` is deleted with `Mutation.companion`, `append_companion`, `preview_callback` and the `companions` parameter of `enqueue05` and `submit_mutation05`.
- **`axton_local_write`.** It keeps one use: the receipt-finalized state of a target the Stream will not deliver (a Private target, or an unmarked historical one), retained as `accepted` at its wire operation's position until no earlier pending operation on the record needs it (`settlement05.rs:214-218`, `:296-341`). The `independent` disposition is deleted with `direct`, `direct_one`, `insert_local_write` (its only caller is the direct write, `mutate.rs:504`) and the second arm of the rebuild order key. `independent_writes_after` goes earlier, with its only caller `append_companion` (`mutate.rs:404`). `disposition` then has one value, and the plan drops the column or the enum, whichever is the smaller diff.
- **`axton_local_replica_layer`.** At `main` 864b002 this table has no production reader: `retain_local_operation` appends to it and coalesces, `stage_one` clears it, and only `crates/sqlite/tests/protocol05_authority.rs:152-250` reads it. It is deleted with `retain_local_operation`, `local_layer` and `clear_local_layer`.
- **Stays, because optimistic Mutations need it:** before-images and `hold_truth`, `rebuild` and its divergence report, `refresh_pending` for wire deletes, `owner_history` and `owned05` (they guard receipt finalization of a wire target), lifecycle and sequence dependencies, prerequisites, and `direct_evidence05`, whose remaining caller is the cascade of a delivered deletion record (a `Record` with null state) to locally created children (`settlement05.rs:492-505`); it is renamed for that use.
- **Runtime and bridge.** `Command::Direct`, `TransactionCommand::Direct`, `Operation::MutationLocal`, `companionId` on `TransactionCommand` and `CallbackResult`, `CallbackResult.input`, `SubmitMutation.local`, and the runtime's `LocalCallback`, `DeferredMutation` and `finish_local` are deleted. A transaction has one capability, its own.
- **Evidence.** No new `axton_authority` row comes from a local write. A row with empty evidence is then always a cache install or a child cascaded by a delivered delete, or a row an older version wrote directly: that row also has empty evidence and stays as cache until a Stream change or a Query result replaces it. Receipt finalization writes no authority row.

### 2.4 Backend

Every concrete Model has a Loader. `createBackend` fails at startup when one is missing, naming the Model, and that is the only check. `Config.loaders` then always equals the schema's Models, so the field is deleted with everything that read it: the per-call device-only refusals in `invalidate`, `track` and `publish` (`crates/server/src/settlement.rs:15`, `:19`, `:68`), the startup check that a Mutation or Query names a Loader-less Model, and the `loader.unregistered` error code (`crates/protocols/src/server_bridge/error.rs:43`). The materialization ID does not read `loaders` (`crates/server/src/lib.rs:304-306`), so this changes no Store's contract.

### 2.5 Release

The wire protocol does not change: none of this was ever sent. The SDK bridge contract and the generated API change incompatibly, so this is a breaking release. The SQLite format number stays 5; open drops `axton_local_replica_layer` when it exists. A file that still holds `companion` operations or `independent` local writes is purged at open: one transaction deletes those rows and rebuilds the records they touched. Both kinds exist only while a Mutation is queued (`settlement05.rs:362-366`, `mutate.rs:219-225`). The Mutations themselves stay queued and upload.

## 3. Changes

Surfaces in landing order. Everything this change leaves unused is deleted in the same release: code, types, SQL, tests, fixtures and docs.

**Protocol (`crates/protocols`)**
- `src/client_bridge/mod.rs`: delete `Command::Direct` (`:193`), `TransactionCommand::Direct` (`:444`), `SubmitMutation.local` (`:473`), `Operation::MutationLocal` (`:761`), `companion_id` on `TransactionCommand` (`:81`) and `CallbackResult` (`:103`), `CallbackResult.input`, the `companionId` identity row and its fixtures (`:1146-1210`). The `Input` envelope does not deny unknown fields today (`:45`); the variants that carried `companionId` and `input` gain `deny_unknown_fields`, so a stale SDK is refused instead of silently ignored.

**Client (`crates/client`)**
- `mutate.rs`: delete `direct`, `direct_one`, `insert_local_write`, `append_companion`, `preview_callback`, the companion branch of `refresh_pending` and the `Independent` order arm.
- `queue.rs`: delete `OpKind::Companion`, `LocalWriteKind::Independent`, `independent_writes_after`.
- `mutation_queue.rs`: `enqueue05` and `submit_mutation05` lose `companions` (`:70`, `:111-135`, `:384`).
- `settlement05.rs`: delete the companion settlement loop (`:343-355`); rename `direct_evidence05`.
- `authority.rs`, `store05.rs`: delete the local replica layer functions, its DDL (`store05.rs:25`) and its `RECORD_STATE` entry (`:290`).
- `frontend_interface.rs`: delete `ClientTransaction::direct`, `append_companion`, `preview_callback`, `Mutation.companion`, and `Session.submitted` with its savepoint copy, which only admit companions (`:91`, `:98`, `:349`, `:375`, `:560`).
- `runtime/commands.rs` (`:38-42`, `:140-144`), `runtime/transactions.rs`, `runtime/tasks.rs` (`:62-92`): delete the direct commands and the local callback.

**Compiler (`crates/compiler`)**
- `src/emit.rs`: delete `WritePort` (`:222`), `{Model}TxModel` for TypeScript (`:1996`) and Dart (`:2074`); `TxModels` is built from the read-only Model facade over `ReadPort` (`:416`, `:440`, `:1913`). `LivePort` extends `ReadPort` and `{Model}LiveModel` extends `{Model}Model`, which they reach through the deleted types today (`:222`, `:1997`, `:2072`). Loaders become required in the generated backend types (`:1095`).
- `src/emit_transactions.rs`: delete `CompanionContext` and the callback overloads (`:27-34`, `:44`, `:63`); `SubmitMutationPort.submitMutation` takes an input object only. `src/validate.rs:765` drops `WritePort` from the reserved names.
- Regenerate every checked-in `generated.ts` and `generated.dart` under `integration/` and `examples/`.

**SDKs**
- `packages/frontend/client-js`: in `api/local.mts` delete `LocalTransaction`, `LocalCallback`, `runLocal`, `callbackRefusal`, `expiredRefusal` and the `local` argument of `MutationPort.submit`; move what is left (`submitMutation`, `SubmissionHost`, `MutationPort`, `:62-110`, used by both transaction modules) into `api/transaction.mts` and delete the file; `direct` from `api/runtime.mts` (`:217`, `:478-486`) and `api/transaction.mts` (`:192`); the `mutationLocal` route in `bindings/bridge.mts` (`:278-340`, `:589-604`).
- `packages/frontend/client-react-native/api/transaction.mts`: delete `direct` (`:160`); the plan checks which of its callback guards served only the Mutation callback.
- `packages/frontend/dart`: delete `WritePort` (`lib/src/api/port.dart:27-43`), `Client.direct` (`client.dart:285`), `Transaction.direct` (`:904`), `LocalTransaction` (`:979-1107`), the `input:` parameter of `submitMutation` (`:193`, `:803-820`) and `_mutationLocal` (`lib/src/bindings/bridge.dart:855`, `:922-960`).

**Backend**
- `crates/server/src/lib.rs:152-170`, `crates/server/src/settlement.rs:15`, `:19`, `:68`, `crates/protocols/src/server_bridge/error.rs:43`, `packages/backend/server/api/backend.mts:235`, `:339-371`, `packages/backend/server/bindings/effects.mts:302-375`: require every Loader in `createBackend`; delete `Config.loaders`, the device-only checks and `LOADER_UNREGISTERED`.

**Tests and fixtures**
- Delete: `crates/sqlite/tests/protocol05_companions.rs`, `protocol05_companion_cascade.rs`, `protocol05_callback.rs`, the direct-write and companion cases of `protocol05_runtime_transactions.rs`, `protocol05_authority.rs`, `protocol05_local_crash.rs` and `protocol05_settlement.rs`, the companion contracts in `integration/action-contract`, `packages/frontend/client-js/mutation-native05.test.mjs` callback cases, and the device-only cases in `integration/action-e2e`, `integration/persistence/server/startup.test.mjs` and `integration/e2e/parity*`.
- Rewrite: about 90 test and integration files use a direct write or a callback (for example `crates/sqlite/tests/model_watch_incremental/mod.rs`, 29 uses; `packages/frontend/dart/test/client_test.dart`, 25; also `integration/platform/ios_smoke/lib/main.dart:83-98` and `integration/release/installed_dart/bin/check.dart:45-52`). Each use is one of three classes:
  - **seed**: sets up rows for a test about something else. It moves to the seed route that gives the evidence state the test needs: `install_authority05` after `initialize_stream05` for a protected row, `install_cache05` for an unprotected row, or an offline Mutation. Node and Dart have no native seam for Stream seeding short of a fake HTTP server (`integration/bindings/client-js/store-fixture.mjs:44`, `packages/frontend/dart/test/protocol5_transport.dart:23`), so they seed with Mutations wherever the test does not assert pending state.
  - **subject**: tests the direct write, the companion or the callback itself. Deleted.
  - **interaction**: tests how an unevidenced local row meets another path (`protocol05_cache.rs:63-78`, `stream_upgrade.rs:71-80`, `protocol05_downlink.rs:967-990`, `crates/sim/src/scenario05.rs:63-72`). Reseeding through the Stream would protect the row and change what the test checks. Each is rewritten with a pending Mutation or a cache install when the behavior still exists, and deleted when it does not.
  The plan carries the per-file list.

**Docs**
- `docs/engineering/guarantees.md`: delete L4 and the Mutation-companion wording in line 19 and P5; the rollover paragraph loses its direct-write sentences. A2 and the Bootstrap paragraph use "companion" for records delivered in the same Stream unit, which is unrelated and stays.
- `docs/engineering/architecture/frontend-sdk/api.md` (surface table and the companion paragraphs), `architecture/backend-sdk/api.md`, `architecture/client/engine/local-operations/writes.md` and `README.md`, `architecture/client/frontend-interface.md`, `architecture/client/storage/protocol5.md`, `architecture/compiler/generate.md`, `architecture/protocols/sync.md:37`, `architecture/protocols/client-bridge.md`.
- `website/docs/concepts.md`, `frontend/client-api.md`, `frontend/sync.md`, `frontend/storage.md`, `frontend/runtime.md`, `backend/api.md` (delete "Device-only Models"), `api-index.md`, the `loader.unregistered` row at `frontend/client-api.md:167`, `packages/frontend/dart/README.md`, `packages/frontend/client-react-native/README.md:46`.

## 4. Decisions

- **AXTON is a partial projection plus a Mutation queue; it holds no device-only data** (user, 2026-11-09). Rejected: an `@@local` marker that classifies device-only Models. It makes the classification explicit but keeps the second write path, its journal and its upgrade problem.
- **No direct Model writes, on any Model.** Rejected: keeping direct writes for unsynced Models only. The engine cannot tell an unsynced Model from a synced one (Loaders live in server config), which is how Capso came to write a synced `SpaceShelfEntry` directly.
- **No companion writes and no atomicity between app-owned device data and a Mutation** (user, 2026-11-09). Rejected: keeping companions as the one atomic link. No current use needs it; ordering the two steps and reconciling by identity covers the known cases, and companions are the reason for the third operation kind and the nested callback protocol.
- **`client.transaction` keeps reads plus Mutations** (user, 2026-11-09). Rejected: removing transactions. Several Mutations that must commit locally together, and reads that must see the same snapshot as the Mutation they feed, still need one boundary.
- **The Mutation input callback is removed, not made read-only.** Its scope existed to own companions. While an application transaction is open the runtime runs only that transaction (`runtime/tasks.rs:130`, `runtime/mod.rs:232`, `frontend_interface.rs:211`), so a read and a `tx.mutations` call see one snapshot, as the callback did. Rejected: a read-only callback, which keeps the `mutationLocal` effect, the capability token and the deferred submission for something `client.transaction` already does. Cost: Capso rewrites five callbacks.
- **Every Model has a Loader, checked once in `createBackend`.** Rejected: allowing a Loader-less Model, which after this change is a table nothing can ever fill. Rejected: keeping `Config.loaders` and the per-call checks, which can no longer fail.
- **No "send all queued Mutations now" command.** `connection.wake()` asks the runtime to look for work (`packages/frontend/client-js/api/runtime.mts:676`, `packages/frontend/dart/lib/src/bindings/connection.dart:250`), `client.outbound` watches the unsettled queue and `call.wait()` awaits one Call. The command was proposed for "drain before upgrade", which upgrade by overwrite does not need. Rejected: adding it now for a use nobody has.
- **An old file's companion operations and independent writes are purged at open.** Rejected: a terminal error naming the reset. A reset discards the queue, and queued Mutations are never deleted (user, 2026-11-09). The purge is about as small as the detection.
- **Delete `axton_local_replica_layer`.** It has no production reader. Rejected: keeping it for a future use nobody has named.
- **Wire protocol unchanged; SQLite format 5 kept.** Rejected: a format bump, which would refuse every existing file and discard its queue for a change that only removes tables and row kinds.

Implementation (2026-11-10) changed:

- `submitMutation`, `SubmissionHost` and `MutationPort` moved from `api/local.mts` to a new platform-neutral `packages/frontend/client-js/api/submission.mts`, not into `api/transaction.mts`, because React Native imports them too.
- `deny_unknown_fields` is on the whole bridge `Input` envelope, not only on the variants that carried `companionId` and `input`.
- `axton_local_write` keeps its `disposition` column with the single value `accepted`; dropping it would rebuild the table in every existing file.
- `createBackend` reports a missing Loader through its existing per-version Loader check (`Missing loader <key> for <Model> v<n>`), and keeps refusing a Loader key that names no Model. The generated backend descriptor's `loaders` list goes with `Config.loaders`.
- Fixture Models that had no Loader (`Composition` in `integration/action-e2e`, `Draft` in `integration/e2e/behavior` and `integration/v05-sdk`) are removed from their schemas and model histories.

## 5. Verification

| Check | Layer |
| --- | --- |
| The bridge refuses `direct`, `local` and `companionId` as unknown fields; no SDK sends them | `crates/protocols` fixtures, `integration/bindings/client-js/runtime-bridge.test.mjs`, `packages/frontend/dart/test/runtime_bridge_test.dart` |
| Generated TypeScript and Dart expose no `create`, `update`, `delete` on any Model facade and no callback overload; misuse fails to compile | `crates/compiler/tests/transactions.rs`, `integration/generated-api/negative` |
| A transaction that reads a row and then submits a Mutation built from it commits both atomically; a rollback discards the Call | `crates/sqlite/tests/protocol05_runtime_transactions.rs` |
| Optimistic apply, refusal rollback, cascade effects, divergence and receipt finalization behave as before | existing `protocol05_settlement.rs`, `protocol05_authority.rs`, `protocol05_queue.rs` cases, reseeded |
| After every step of the simulation, each Model row equals the rebuild of its key from its base, its pending operations and its accepted writes | `crates/sim/src/scenario05.rs` invariant |
| Through each SDK, a transaction that reads a row and submits a Mutation built from it commits both; a rollback discards the Call | `integration/bindings/client-js/transaction.test.mjs`, `packages/frontend/dart/test/client_test.dart` |
| A file built from the old DDL with a queued Mutation that has a companion operation and an independent write opens; those rows are gone, the records are rebuilt, and the Mutation uploads | `crates/sqlite/tests/protocol05_client.rs` |
| The iOS smoke app and the installed-package check run without a direct write | `integration/platform/ios_smoke`, `integration/release/installed_dart` jobs |
| `createBackend` fails at startup for a Model without a Loader, naming it | `integration/persistence/server/startup.test.mjs` |
| An existing format-5 file with an empty queue opens, and `axton_local_replica_layer` is gone | `crates/sqlite/tests/protocol05_client.rs` |
| Capso builds and passes its mobile and backend suites on this release after its own migration | Capso CI |

## 6. Out of scope

- **Capso's migration**, a prerequisite done in Capso before it takes this release:
  - move `Composition`, `ChatHistory` and `MediaUrl` to an app-owned store and remove them from the schema, copying existing rows out while the installed SDK can still read them;
  - stop the direct `SpaceShelfEntry` bump in `space_shelf_order.dart` and inside the `placeMoment` callback (`page_commands.dart:387`). The second needs a Mutation or a server-side effect of `placeMoment`; that is a Capso design question;
  - turn its five read-only Mutation callbacks into `client.transaction` bodies.
- An app-owned device store. AXTON ships none and recommends none.
- The upgrade path itself ([upgrade by overwrite](2026-11-09-upgrade-by-overwrite-design.md)).
- Prerequisite tasks, the unsent-work views and `resetStore`; all unchanged.

## 7. Open questions

None. The four questions of the first draft are answered in §4 (callback removed, no send-all command, Loader required, purge at open); the user delegated them on 2026-11-09 and the design review agreed with each.
