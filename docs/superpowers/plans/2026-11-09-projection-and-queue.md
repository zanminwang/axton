# Projection and queue implementation plan

> Proposed. Do not start before the spec is approved. Steps use checkbox syntax for tracking.

**Goal:** Remove direct Model writes, Mutation companion writes and the Mutation input callback, with every table, command and type that served only them.

**Architecture:** Tests stop depending on direct writes first (Task 1), while the write paths still exist. The callback and companions go next (Task 2), then direct writes (Task 3), then the storage that only they used (Task 4), then the backend's device-only Model (Task 5). Each task merges on its own with the suite green.

**Spec:** [AXTON holds a projection and a queue, nothing else](../specs/2026-11-09-projection-and-queue-design.md).

## Global constraints

- Lands after [upgrade by overwrite](2026-11-09-upgrade-by-overwrite.md).
- Wire protocol unchanged. SQLite format stays 5.
- This is a breaking SDK release. It is not published until Capso has finished its own migration (spec §6); merging the tasks does not wait for Capso.
- Nothing this plan leaves unused survives: code, SQL, generated files, tests, fixtures, docs.
- Gate for every task: `scripts/test.sh` passes in full, strict Clippy passes for the crates touched, and the checked-in generated files match the compiler's output.

## Task 1: Seed tests without direct writes

**Files:** `crates/sqlite/tests/common05/`, the Node and Dart test support folders; about 90 test and integration files (largest: `crates/sqlite/tests/model_watch_incremental/mod.rs`, `packages/frontend/dart/test/client_test.dart`).

- [x] List every test and integration file that calls a direct write or a Mutation callback (include `integration/platform/ios_smoke/lib/main.dart` and `integration/release/installed_dart/bin/check.dart`), and mark each use as *seed*, *subject* or *interaction* as the spec defines them (§3 Tests). Commit the list into this plan.
- [x] Rust seeding helpers in `common05`: a protected row (`initialize_stream05` then `install_authority05`), an unprotected row (`install_cache05`), and an offline Mutation. Node and Dart seed with Mutations; use the fake-server transport only where a test needs a delivered row.
- [x] Move every *seed* use to the helper whose evidence state the test needs, file by file. No production code changes in this task; the suite stays green throughout.
- [x] Rewrite each *interaction* test with a pending Mutation or a cache install when its behavior survives the spec; mark the rest for deletion in Task 3.

### Task 1 list

Uses are counted by call site. Routes: *cache* is `common05::cached` (unprotected, unpending, as a direct write left it); *delivery* is `common05::protected` in Rust or the fake-server transport in Node/Dart (a protected row); *offline Mutation* is a queued, optimistically applied Mutation that is never sent. Paths are relative to the repository root.

**Rust** (helpers: `crates/sqlite/tests/common05/mod.rs` `start_stream`, `record`, `protected`, `cached`, `offline`, `with_crud` with `create_call`/`update_call`/`delete_call`; `seed()` now installs cache)

| file | uses | class | route |
| --- | --- | --- | --- |
| crates/sqlite/tests/common05/mod.rs `seed()` | 1 direct create | seed | cache |
| crates/sqlite/tests/protocol05_client.rs | 6 `seed()`, 12 direct | seed | cache via `seed`; offline Mutations `Edit` and `with_crud` |
| crates/sqlite/tests/query.rs | 1 `seed()`, 6 direct | seed | cache ×4, offline Mutation ×2 |
| crates/sqlite/tests/protocol05_defaults.rs | 14 direct | seed | offline Mutations `Add`/`Edit` ×12, cache ×2 |
| crates/sqlite/tests/protocol05_defaults.rs | 1 direct (identity key in state, split into its own test), 1 companion | subject | delete in Task 3; delete in Task 2 |
| crates/sqlite/tests/protocol05_settlement.rs | 2 direct (constraint test) | seed | cache |
| crates/sqlite/tests/protocol05_settlement.rs | 8 direct ("later direct", direct delete) in 7 tests, 1 companion | subject | delete in Task 3; delete in Task 2 |
| crates/sqlite/tests/protocol05_queue.rs | 6 direct base rows | seed | cache |
| crates/sqlite/tests/protocol05_queue.rs | 1 direct ("device" row in `an_upgrade_keeps_every_row...`) | interaction | delete in Task 3 (cached, delivered and pending rows already cover it) |
| crates/sqlite/tests/protocol05_unsent.rs | 1 direct (`seeded`) | seed | cache (the test counts pending) |
| crates/sqlite/tests/protocol05_ddl.rs `populated()` | 2 direct (dirty and clean) | interaction | delete in Task 3: it needs replica-layer and independent local-write rows, which only direct writes create |
| crates/sqlite/tests/protocol05_authority.rs | 5 direct, 1 companion | subject | delete in Task 3; delete in Task 2 |
| crates/sqlite/tests/protocol05_local_crash.rs | 1 direct, 1 companion | subject | delete in Task 2 |
| crates/sqlite/tests/protocol05_companions.rs | 9 direct, companions | subject | delete in Task 2 |
| crates/sqlite/tests/protocol05_companion_cascade.rs | 5 direct, companions | subject | delete in Task 2 |
| crates/sqlite/tests/protocol05_callback.rs | 17 callback/companion | subject | delete in Task 2 |
| crates/sqlite/tests/protocol05_cache.rs | 1 direct delete (unloaded parent) | interaction | rewritten: pending `DeleteParent` Mutation |
| crates/sqlite/tests/protocol05_downlink.rs | 1 direct update | interaction | rewritten: pending `Edit` |
| crates/sqlite/tests/stream_upgrade.rs | 1 direct create | interaction | rewritten: cache |
| crates/sim/src/scenario05.rs, crates/sim/tests/scenario05.rs | 1 direct ("later direct") | interaction | rewritten: later pending `Edit`; the trace reads the row with it applied, then after discarding it |
| crates/sqlite/tests/protocol05_runtime_transactions.rs | ~38 direct probes (transaction, savepoint, failure, close, failed-commit, watched-statement and observer tests) | seed | task `submitAction` / transaction `submitMutation` of `with_crud` Mutations; a failing write is the unknown Mutation `Nope`; `unrelated` is a cache install |
| crates/sqlite/tests/protocol05_runtime_transactions.rs | 18 (`create`/`delete`/`missing_model`, raw direct) in the 6 local-callback tests | subject | delete in Task 2; the old direct helpers stay only for these |
| crates/sqlite/tests/model_watch_incremental/mod.rs | ~35 direct creates, updates, deletes | seed | offline `with_crud` Mutations; the random delta-splice test uses deliveries |
| crates/sqlite/tests/runtime_prerequisites.rs | 3 raw direct | seed | cache (a Mutation would queue the `Scan` task) |
| bindings/common/tests/runtime.rs | 10 raw direct (6 task, 4 transaction) | seed | `submitAction`/`submitMutation` of a test `PutEntry` |
| crates/compiler/tests/defaults.rs, compiler.rs, transactions.rs, v04_facade.rs | generated create/update/delete and callback typing | subject | delete in Task 3; delete in Task 2 |
| crates/protocols/src/client_bridge/mod.rs fixtures, fixtures/bridge/envelopes.json, fixtures/compiler/transactions.model | `direct`, `companionId`, `mutationLocal` envelopes | subject | delete in Task 2/3 |

**Node** (helpers: `integration/bindings/client-js/store-fixture.mjs` `deliveringNetwork`, `deliver`, `streamServer`)

| file | uses | class | route |
| --- | --- | --- | --- |
| integration/bindings/client-js/runtime.test.mjs | 11 direct | seed | offline Mutations `Create`/`Edit`; delivery in the 2 tests that count pending |
| integration/bindings/client-js/runtime-bridge.test.mjs | 11 raw direct | seed | raw `submitMutation` (`Create`/`Edit`); the failing command is an unknown Mutation |
| integration/bindings/client-js/runtime-bridge.test.mjs | 6 `companionId`/`mutationLocal` | subject | delete in Task 2 |
| integration/bindings/client-js/prerequisite-harness.mjs | 1 | seed | offline Mutation `Create` |
| integration/bindings/client-js/watch-sql-harness.mjs | 13 | seed | offline Mutations `Put<Model>`/`Change<Model>` |
| integration/bindings/client-js/unsent-harness.mjs | 2 | seed | delivery |
| integration/bindings/client-js/schema-reconciliation.test.mjs | 1 | seed | delivery |
| integration/bindings/client-js/worker.test.mjs, worker-transaction.mjs | 4 | seed | offline Mutation `Create` |
| integration/bindings/client-js/connection.test.mjs | 1 | seed | transaction `Ping` Mutation |
| integration/bindings/client-js/transaction.test.mjs | 9 `tx.direct` as a generic command | seed | `tx.read` |
| integration/bindings/client-js/transaction.test.mjs | 1 callback, 1 `tx.direct`, `local:true` | subject | delete in Task 2 |
| integration/bindings/client-js/actions.test.mjs | 2 | subject | delete in Task 3 |
| integration/bindings/client-js/mutations-harness.mjs | 5 direct/companion, 6 callback tests | subject | delete in Task 2 |
| integration/bindings/client-react-native/runtime.test.mjs | 8 direct | seed | offline Mutations; delivery where pending is counted |
| integration/bindings/client-react-native/runtime.test.mjs | 1 callback with companion | subject | delete in Task 2 |
| integration/bindings/client-react-native/transaction.test.mjs | 3 generic `tx.direct`; 1 callback | seed; subject | `tx.query`/`tx.read`; delete in Task 2 |
| integration/bindings/client-react-native/actions.test.mjs | 1 `models.todo.create` | seed | delivery |
| packages/frontend/client-js/command-accounting.test.mjs | 8 generic `tx.direct`; 1 callback | seed; subject | `tx.read`/`tx.query`; delete in Task 2 |
| packages/frontend/client-js/mutation-native05.test.mjs | 1 foreign-scope `tx.direct`; 2 callback tests | seed; subject | Mutation `Publish`; delete in Task 2 |
| packages/frontend/client-js/mutation-input.test.mjs, packages/frontend/client-react-native/mutation-native05.test.mjs | 1 callback each | subject | delete in Task 2 |
| integration/generated-api/native.mts | 8 Entry/Book/Comment seeds | seed | delivery at Bootstrap and in live units |
| integration/generated-api/native.mts | 1 `entry.update`, local Book CRUD with watch, 3 Draft default creates | subject | delete in Task 3 (no Draft create Mutation exists; adding one changes `fixtures/compiler`, which feeds a checked-in `generated.ts`) |
| integration/generated-api/test.ts, negative/misuse.ts | create/delete typing; callback/`CompanionContext` typing | subject | delete in Task 3; delete in Task 2 |
| integration/action-runtime-ts/client.test.mts, types.ts | 5 CRUD/typing; 4 callback | subject | delete in Task 3; delete in Task 2 (keep its "generated optimism durable across reopen" half as plain Mutations) |
| integration/action-contract/positive.ts, negative.ts | 4 callback, 4 create/update typing | subject | delete in Task 2; delete in Task 3 |
| integration/action-e2e/action.test.mts | 1; 1; 2 tests | interaction; seed; subject | rewritten: pending `updateTodo`; `addTodo`; delete in Task 2 |
| integration/e2e/behavior/acceptance.test.mjs | 2 | interaction | rewritten: pending `renameItem` |
| integration/e2e/todo.test.mjs | 2 | interaction | rewritten: `addTodo` plus a server-side delete; a fresh offline Store for the duplicate create |
| integration/e2e/parity.test.mjs, round-trip.test.mjs | 1 device-only create; 1 callback | subject | delete in Task 3; delete in Task 2 |
| integration/v05-sdk/client.mjs | 1 Draft create while a Fetch is parked | seed | offline `Publish` |
| integration/v05-sdk/client.mjs | 2 "later direct"/"later local" Entry updates after a private acceptance | interaction | not moved: needs an Entry update Mutation in `entry.model`, which feeds a checked-in `generated.ts`; Task 3 adds it with the regeneration or deletes the cases |
| integration/v05-sdk/client.mjs | 1 direct-delete refill; 1 subject create; 2 callbacks | interaction; subject | delete in Task 3; delete in Task 3; delete in Task 2 |
| integration/persistence/client/reopen.mts (+ schema.model, compiled to a temp dir) | 2 | seed | offline Mutations `Add`/`Retitle` |
| integration/platform/react-native/smoke.ts | 1 | seed | not moved: its generated client is stale and nothing runs or typechecks it |
| integration/v04-sdk/host.test.mts, positive.ts | 1 transaction create, 3 callbacks | subject | delete in Task 2/3; nothing runs `integration/v04-sdk` |

**Dart** (helpers: `packages/frontend/dart/test/store_fixture.dart` `declareRowMutations`, `createRow`, `updateRow`; `protocol5_transport.dart` `deliverRecords`)

| file | uses | class | route |
| --- | --- | --- | --- |
| packages/frontend/dart/test/client_test.dart | 1 setUp create | seed | delivery (keeps the subject tests' pending counts exact) |
| packages/frontend/dart/test/client_test.dart | 14 direct | seed | offline Mutations `Edit`/`Publish` |
| packages/frontend/dart/test/client_test.dart | 9 callback, 8 companion, 2 direct in the callback-input test | subject | delete in Task 2 |
| packages/frontend/dart/test/runtime_bridge_test.dart | 14 direct | seed | offline `CreateEntry`/`UpdateEntry` |
| packages/frontend/dart/test/runtime_bridge_test.dart | `mutationLocal` effect test, `companionId` envelope | subject | delete in Task 2 |
| packages/frontend/dart/test/watch_sql_test.dart | 12 | seed | offline `Create<Model>`/`Update<Model>` |
| packages/frontend/dart/test/unsent_test.dart | 2 | seed | delivery |
| packages/frontend/dart/test/prerequisite_test.dart | 1 | seed | one offline `CreateEntry` carrying `note` |
| packages/frontend/dart/test/diagnostics_test.dart, lost_isolate.dart | 2, 2 | seed | offline `CreateEntry` |
| packages/frontend/dart/test/call_handles_test.dart | 1 typed create (creation defaults) | subject | delete in Task 3 |
| integration/generated-api/generated_test.dart | 11 | seed | delivery for Entry/Placement/Book/Comment; `publishEntry` for the DateTime Entry writes |
| integration/generated-api/generated_test.dart | 7 default/pending-row writes; 1 callback, 1 companion, fake `WritePort` | subject | delete in Task 3; delete in Task 2 |
| integration/generated-api/negative/misuse.dart | callback/`CompanionContext` negatives | subject | delete in Task 2 |
| integration/persistence/client/reopen.dart | 2 | seed | offline `Add`/`Retitle` |
| integration/platform/ios_smoke/lib/main.dart | 1 | seed | offline Mutation `Create` added to its inline schema |
| integration/release/installed_dart/bin/check.dart | 1 | seed | offline Mutation `Create` added to its inline schema |
| integration/action-contract/positive.dart, negative.dart | callback, companion, create/update typing | subject | delete in Task 2; delete in Task 3 |
| integration/action-runtime-dart/generated_test.dart | 5 local CRUD, 1 callback, 1 companion | subject | delete in Task 3; delete in Task 2 |
| integration/action-runtime-dart/action_e2e_publish.dart, action_e2e_hook.dart, negative.dart | 6 direct, 5 callback, 4 companion (device-only `Composition`) | subject | delete in Task 2 |
| integration/e2e/parity_client.dart | 1 device-only create | subject | delete in Task 3 |
| integration/v05-sdk/client.dart | 2 callback, 2 companion | subject | delete in Task 2 |
| integration/v04-sdk/host.dart, native.dart, positive.dart | 5 callback, 5 companion, 1 create in a callback test | subject | delete in Task 2 |
| integration/v04-sdk/native.dart | 1 foreign-scope create | seed | not moved: nothing runs `integration/v04-sdk` |

**Regenerated in Task 2/3, not tests and not edited:** the checked-in `generated.ts`/`generated.dart` under `integration/` (action-contract; action-e2e and `evolved`; action-runtime-dart with `model_only` and `model_free`; action-runtime-ts; e2e/behavior; e2e/fixtures/round-trip/generated; generated-api; performance/fixture/generated; platform/react-native/generated; v04-sdk; v05-sdk with `upgraded` and `versioned`) and `examples/todo/generated/{mobile,node}`.

## Task 2: Remove the Mutation input callback and companions (spec §2.2, §2.3)

**Files:** `crates/compiler/src/emit_transactions.rs`; `crates/protocols/src/client_bridge/mod.rs`; `crates/client/src/runtime/transactions.rs`, `runtime/tasks.rs`, `runtime/commands.rs`, `mutate.rs`, `queue.rs`, `mutation_queue.rs`, `settlement05.rs`, `frontend_interface.rs`; `packages/frontend/client-js/bindings/bridge.mts`, `api/transaction.mts`; `packages/frontend/client-react-native/api/transaction.mts`; `packages/frontend/dart/lib/src/api/client.dart`, `bindings/bridge.dart`.

- [x] Test (compiler negative): `mutations.<name>((tx) => input)` and Dart `withTransaction` fail to compile against generated code.
- [x] Test (bridge): `local` on `SubmitMutation`, `companionId` on a transaction command and `input` on a callback result are refused as unknown fields. Add `deny_unknown_fields` to the `Input` variants that carried them.
- [x] Test (each SDK): the same read-then-submit transaction through `integration/bindings/client-js/transaction.test.mjs` and Dart `client_test.dart`.
- [x] Test (runtime): a transaction that reads a row and submits a Mutation built from it commits both; a rollback discards the Call. This is the replacement for the callback and must pass before the callback is deleted.
- [x] Delete `CompanionContext` and the callback overloads; `SubmitMutationPort.submitMutation` takes an input object.
- [x] Delete `Operation::MutationLocal`, `companion_id`, `CallbackResult.input`, `SubmitMutation.local`, the runtime's `LocalCallback`, `DeferredMutation`, `finish_local`, and the SDK `mutationLocal` routes. In `client-js/api/local.mts` delete the callback half and move `submitMutation`, `SubmissionHost` and `MutationPort` into `api/transaction.mts`. In React Native, delete the callback guards that have no caller left.
- [x] Delete `OpKind::Companion`, `append_companion`, `preview_callback`, `independent_writes_after` (its only caller is `append_companion`; Clippy runs with `-D warnings`), `Session.submitted` and its savepoint copy, the `companions` parameter of `enqueue05` and `submit_mutation05`, the companion branch of `refresh_pending`, and the companion settlement loop.
- [x] Delete `protocol05_companions.rs`, `protocol05_companion_cascade.rs`, `protocol05_callback.rs` and the companion contracts in `integration/action-contract`. Regenerate checked-in generated files.

## Task 3: Remove direct Model writes (spec §2.2, §2.3)

**Files:** `crates/compiler/src/emit.rs`, `validate.rs`; `crates/protocols/src/client_bridge/mod.rs`; `crates/client/src/mutate.rs`, `queue.rs`, `frontend_interface.rs`, `runtime/commands.rs`, `runtime/tasks.rs`; `packages/frontend/client-js/api/local.mts`, `api/runtime.mts`, `api/transaction.mts`; `packages/frontend/client-react-native/api/transaction.mts`; `packages/frontend/dart/lib/src/api/port.dart`, `client.dart`.

- [x] Test (compiler negative): `create`, `update` and `delete` on a Model facade, at the client and inside a transaction, fail to compile.
- [x] Test (bridge): `direct` is refused as an unknown command.
- [x] Add the `crates/sim/src/scenario05.rs` invariant: after every step, each Model row equals the rebuild of its key from its base, its pending operations and its accepted writes. It must pass before and after the engine change below.
- [x] Delete `WritePort`, `{Model}TxModel` and the write half of the Model facades; `TxModels` is the read-only facade; `LivePort` extends `ReadPort` and `{Model}LiveModel` extends `{Model}Model`.
- [x] Delete `Command::Direct`, `TransactionCommand::Direct`, the SDK `direct` methods, Dart `LocalTransaction`.
- [x] Delete `direct`, `direct_one`, `insert_local_write`, `LocalWriteKind::Independent` and the `Independent` arm of the rebuild order key. Drop the `disposition` column or keep it single-valued, whichever is the smaller diff; record the choice here. Kept single-valued: every row is `accepted`, the decoder refuses any other value, and the DDL and the format-5 open check are unchanged (dropping the column would rebuild the table in every existing file).
- [x] Rename `direct_evidence05` for its remaining use (a delivered delete cascading to locally created children).
- [x] Delete the *subject* tests and the *interaction* tests marked for deletion in Task 1's list. Regenerate checked-in generated files. Update the iOS smoke app and the installed-package check.
- [x] Open purges an old file (spec §2.5): one transaction deletes `companion` operations and `independent` local writes and rebuilds the records they touched. Test with a file built from the old DDL that has a queued Mutation carrying both; the Mutation still uploads. With the purge, `ops_by_ordinal` no longer filters `companion` rows; Task 2's `a_stored_companion_operation_is_never_read_or_sent` is replaced by `an_old_file_s_companion_operations_and_independent_writes_are_purged_at_open` (`crates/sqlite/tests/protocol05_client.rs`).

## Task 4: Delete `axton_local_replica_layer` (spec §2.3)

**Files:** `crates/client/src/authority.rs`, `store05.rs`, `ddl.rs`; `crates/sqlite/tests/protocol05_authority.rs`, `protocol05_client.rs`.

- [x] Confirm by search that no production code reads the table; if one does, stop and report. Only `rebuild` called `retain_local_operation`, whose only reader was `local_layer`.
- [x] Delete `retain_local_operation`, `local_layer`, `clear_local_layer`, the table's DDL and its `RECORD_STATE` entry; open drops the table from an existing file. Test: a file with the table opens and the table is gone.
- [x] Remove the cases in `protocol05_authority.rs` that read it. Tasks 2 and 3 already had; the last reader was `state()` in `protocol05_ddl.rs`. Test: `an_old_file_s_local_replica_layer_is_dropped_at_open` (`crates/sqlite/tests/protocol05_client.rs`).

## Task 5: Every Model has a Loader (spec §2.4)

**Files:** `crates/server/src/lib.rs`, `settlement.rs`; `packages/backend/server/api/backend.mts`, `bindings/effects.mts`; `crates/compiler/src/emit.rs` (generated backend types); `integration/persistence/server/startup.test.mjs`, `integration/action-e2e`, `integration/e2e/parity*`.

- [ ] Test: `createBackend` fails at startup for a Model without a Loader and names it.
- [ ] Make Loaders required in the generated backend types and in `createBackend`; delete `Config.loaders`, the per-call device-only refusals (`settlement.rs:15`, `:19`, `:68`), the startup check for Mutations and Queries that name a Loader-less Model, and `LOADER_UNREGISTERED`.
- [ ] Delete the device-only cases in the integration suites.

## Task 6: Docs

**Files:** the list in spec §3 Docs.

- [ ] Guarantees: delete L4 and the Mutation-companion wording; keep A2's "companion" (records in one Stream unit).
- [ ] API reference: the surface table becomes the three verbs of spec §2.2; delete the companion and device-only sections.
- [ ] Website: delete "Device-only Models"; add one paragraph saying an app keeps device-only data in its own store.
