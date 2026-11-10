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

- [ ] List every test and integration file that calls a direct write or a Mutation callback (include `integration/platform/ios_smoke/lib/main.dart` and `integration/release/installed_dart/bin/check.dart`), and mark each use as *seed*, *subject* or *interaction* as the spec defines them (§3 Tests). Commit the list into this plan.
- [ ] Rust seeding helpers in `common05`: a protected row (`initialize_stream05` then `install_authority05`), an unprotected row (`install_cache05`), and an offline Mutation. Node and Dart seed with Mutations; use the fake-server transport only where a test needs a delivered row.
- [ ] Move every *seed* use to the helper whose evidence state the test needs, file by file. No production code changes in this task; the suite stays green throughout.
- [ ] Rewrite each *interaction* test with a pending Mutation or a cache install when its behavior survives the spec; mark the rest for deletion in Task 3.

## Task 2: Remove the Mutation input callback and companions (spec §2.2, §2.3)

**Files:** `crates/compiler/src/emit_transactions.rs`; `crates/protocols/src/client_bridge/mod.rs`; `crates/client/src/runtime/transactions.rs`, `runtime/tasks.rs`, `runtime/commands.rs`, `mutate.rs`, `queue.rs`, `mutation_queue.rs`, `settlement05.rs`, `frontend_interface.rs`; `packages/frontend/client-js/bindings/bridge.mts`, `api/transaction.mts`; `packages/frontend/client-react-native/api/transaction.mts`; `packages/frontend/dart/lib/src/api/client.dart`, `bindings/bridge.dart`.

- [ ] Test (compiler negative): `mutations.<name>((tx) => input)` and Dart `withTransaction` fail to compile against generated code.
- [ ] Test (bridge): `local` on `SubmitMutation`, `companionId` on a transaction command and `input` on a callback result are refused as unknown fields. Add `deny_unknown_fields` to the `Input` variants that carried them.
- [ ] Test (each SDK): the same read-then-submit transaction through `integration/bindings/client-js/transaction.test.mjs` and Dart `client_test.dart`.
- [ ] Test (runtime): a transaction that reads a row and submits a Mutation built from it commits both; a rollback discards the Call. This is the replacement for the callback and must pass before the callback is deleted.
- [ ] Delete `CompanionContext` and the callback overloads; `SubmitMutationPort.submitMutation` takes an input object.
- [ ] Delete `Operation::MutationLocal`, `companion_id`, `CallbackResult.input`, `SubmitMutation.local`, the runtime's `LocalCallback`, `DeferredMutation`, `finish_local`, and the SDK `mutationLocal` routes. In `client-js/api/local.mts` delete the callback half and move `submitMutation`, `SubmissionHost` and `MutationPort` into `api/transaction.mts`. In React Native, delete the callback guards that have no caller left.
- [ ] Delete `OpKind::Companion`, `append_companion`, `preview_callback`, `independent_writes_after` (its only caller is `append_companion`; Clippy runs with `-D warnings`), `Session.submitted` and its savepoint copy, the `companions` parameter of `enqueue05` and `submit_mutation05`, the companion branch of `refresh_pending`, and the companion settlement loop.
- [ ] Delete `protocol05_companions.rs`, `protocol05_companion_cascade.rs`, `protocol05_callback.rs` and the companion contracts in `integration/action-contract`. Regenerate checked-in generated files.

## Task 3: Remove direct Model writes (spec §2.2, §2.3)

**Files:** `crates/compiler/src/emit.rs`, `validate.rs`; `crates/protocols/src/client_bridge/mod.rs`; `crates/client/src/mutate.rs`, `queue.rs`, `frontend_interface.rs`, `runtime/commands.rs`, `runtime/tasks.rs`; `packages/frontend/client-js/api/local.mts`, `api/runtime.mts`, `api/transaction.mts`; `packages/frontend/client-react-native/api/transaction.mts`; `packages/frontend/dart/lib/src/api/port.dart`, `client.dart`.

- [ ] Test (compiler negative): `create`, `update` and `delete` on a Model facade, at the client and inside a transaction, fail to compile.
- [ ] Test (bridge): `direct` is refused as an unknown command.
- [ ] Add the `crates/sim/src/scenario05.rs` invariant: after every step, each Model row equals the rebuild of its key from its base, its pending operations and its accepted writes. It must pass before and after the engine change below.
- [ ] Delete `WritePort`, `{Model}TxModel` and the write half of the Model facades; `TxModels` is the read-only facade; `LivePort` extends `ReadPort` and `{Model}LiveModel` extends `{Model}Model`.
- [ ] Delete `Command::Direct`, `TransactionCommand::Direct`, the SDK `direct` methods, Dart `LocalTransaction`.
- [ ] Delete `direct`, `direct_one`, `insert_local_write`, `LocalWriteKind::Independent` and the `Independent` arm of the rebuild order key. Drop the `disposition` column or keep it single-valued, whichever is the smaller diff; record the choice here.
- [ ] Rename `direct_evidence05` for its remaining use (a delivered delete cascading to locally created children).
- [ ] Delete the *subject* tests and the *interaction* tests marked for deletion in Task 1's list. Regenerate checked-in generated files. Update the iOS smoke app and the installed-package check.
- [ ] Open purges an old file (spec §2.5): one transaction deletes `companion` operations and `independent` local writes and rebuilds the records they touched. Test with a file built from the old DDL that has a queued Mutation carrying both; the Mutation still uploads.

## Task 4: Delete `axton_local_replica_layer` (spec §2.3)

**Files:** `crates/client/src/authority.rs`, `store05.rs`, `ddl.rs`; `crates/sqlite/tests/protocol05_authority.rs`, `protocol05_client.rs`.

- [ ] Confirm by search that no production code reads the table; if one does, stop and report.
- [ ] Delete `retain_local_operation`, `local_layer`, `clear_local_layer`, the table's DDL and its `RECORD_STATE` entry; open drops the table from an existing file. Test: a file with the table opens and the table is gone.
- [ ] Remove the cases in `protocol05_authority.rs` that read it.

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
