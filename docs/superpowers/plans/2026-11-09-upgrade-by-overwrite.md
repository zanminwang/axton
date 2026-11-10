# Upgrade by overwrite implementation plan

> Steps use checkbox syntax for tracking. Each task is test-first: write the named test, watch it fail for the stated reason, then change the code. Commit once per task.

**Goal:** Replace the rollover (hold, drain, clear, Bootstrap) with one local transaction that fixes table shapes and asks for Bootstrap again, and take the schema version out of the Batch identity.

**Architecture:** Tasks 1 to 4 each add one rule that is safe while the old rollover still runs, so each leaves the suite green. Task 5 swaps the rollover for the upgrade transaction and deletes everything the rollover owned. Task 6 updates the docs.

**Spec:** [Upgrade by overwrite](../specs/2026-11-09-upgrade-by-overwrite-design.md).

## Global constraints

- Baseline: `main` at 864b002.
- Wire protocol stays 6: 0.6.0 is unpublished and its release PR is on hold (spec §7).
- SQLite format stays 5.
- No SDK API change except the removal of the `schema_pending` error kinds in Task 5.
- Nothing this plan leaves unused survives: code, SQL, tests, fixtures, docs.
- No Capso change. Publish no package. Do not touch the release PR.
- Gate for every task: `scripts/test.sh` passes in full, and strict Clippy passes for the crates touched.

## Task 1: Evidence belongs to one contract (spec §2.3)

**Files:** `crates/core/src/authority.rs`, `crates/protocols/src/sync/delivery.rs`, `crates/client/src/settlement05.rs`; tests in `crates/core`, `crates/sqlite/tests/protocol05_cache.rs`, `protocol05_authority.rs`.

- [x] Test (core): install under `a` at 5, under `b` at 5, under `a` at 5 again. The third install is admitted. Fails today as `Duplicate`.
- [x] `RecordEvidence::install` replaces `history` with its one entry. Search every reader of `history` (`covers`, `owned05`, `target_evidence05`, `validate`) and confirm none needs an older entry; if one does, stop and report.
- [x] Test: a record whose `current` is under `m1` accepts a cache install when the Store's materialization is `m2`, and refuses it under `m1`. Same for a `current.deleted` record and for a child of a deleted parent.
- [x] `RecordEvidence::allows_cache(materialization)`; `stage_cache05` and its deleted-parent check pass the Store's materialization. `target_evidence05` sets `protected` by the same rule.

## Task 2: The Batch carries no schema version (spec §2.4)

**Files:** `crates/protocols/src/sync.rs`, `src/sync/mutation.rs`, `fixtures/protocol/0.5.json`; `crates/server/src/mutation_batch.rs`; `crates/client/src/mutation_queue.rs`, `settlement05.rs`, `store05.rs`; tests in `crates/protocols/tests/`, `crates/server/tests/`, `crates/sqlite/tests/protocol05_queue.rs`, `protocol05_settlement.rs`, `protocol05_client.rs`; every caller that builds a `MutationRequest` or an `Accepted` outcome by hand.

- [x] Test (protocols): two requests that differ only in `context.materialization` have the same digest; the digest differs from the `axton:batch-intent:1` value.
- [x] `batch_body` removes `materialization`; domain `axton:batch-intent:2`; `validate_acknowledgement` compares Store ID and Stream.
- [x] `MutationOutcome::Accepted.materialization` (required). The server sets it from `request.context.materialization` when the member executes; the saved result carries it. Tests (server): a replay under another request materialization returns the saved label; a Batch whose first member ran under `m1` and is resent under `m2` returns `m1` for the first member and `m2` for the second.
- [x] Test (client): freeze a Batch, change the Store's materialization directly in the test, reconstruct: same digest, new context. Fails today on the stored `batch_materialization`.
- [x] Delete the `batch_materialization` column from `DDL`, `freeze05`, `reconstruct_batch05` and the acknowledgement replay comparison; `axton_queue_envelope` guards `batch_digest` only. `open05` drops the column and recreates the trigger when an existing file has it. Test: a file created with the old DDL opens and the trigger still rejects a changed digest.
- [x] Settlement installs a receipt snapshot only when the outcome's materialization is the Store's; otherwise the target settles with no install. Acknowledgement validation stays on the Mutation's retained schema in this task; Task 5 moves it. Tests in `protocol05_settlement.rs` for a matching and a non-matching label, for a `Private` and a historical unmarked target.
- [x] Regenerate `fixtures/protocol/0.5.json`; fix hand-built requests and outcomes across tests, `crates/sim`, `integration/`.

## Task 3: Pieces from another contract are skipped (spec §2.5)

**Files:** `crates/core/src/schema.rs`; `crates/protocols/src/sync/mutation.rs`; `crates/server/src/mutation_batch.rs`; `crates/client/src/mutate.rs`, `rows.rs`, `queue.rs`, `settlement05.rs`; tests in `crates/core`, `crates/server/tests/`, `crates/sqlite/tests/protocol05_queue.rs`, `protocol05_defaults.rs`.

- [x] Extract the per-Model body of `Schema::compatibility` as a function of two schemas and a Model name; `compatibility` calls it. A Model missing from the incoming schema is incompatible. Add a unit test that a change to Model `A` leaves Model `B` compatible.
- [x] The shared check: (Mutation's retained descriptor, Model) is shared when the per-Model check finds nothing to reset. Cache the answer per (descriptor, Model) for the Engine's lifetime.
- [x] Tests: build a Store whose queue holds a Mutation enqueued under an older schema, with operations on Model `A` (incompatible now, in three variants: changed field type, changed identity, removed) and Model `B` (unchanged). The tests construct the older descriptor directly, since open still refuses an incompatible schema until Task 4.
  - Rebuild: `A`'s operation is skipped and the Mutation is marked diverged; `B`'s applies.
  - Acknowledge with a receipt that has no target for `A`: accepted. Fails today on `record_key` or the mandatory-target check.
  - Rejection of that Mutation: completes.
  - Settlement: `A`'s target is neither awaited nor installed.
- [x] Apply the shared check on every path that keys a Mutation's operations: acknowledgement validation, `validate_acknowledgement` (it takes the Models whose targets are required), rejection, settlement, rebuild.
- [x] Test: a create enqueued before a defaulted required field existed replays as a valid row holding the default. Fails today: replay writes NULL (`rows.rs:31`) into a `NOT NULL DEFAULT` column and falls back silently.
- [x] Replay fills the declared default for each field an operation does not carry, accepted local writes included.
- [x] Test (server): a Batch whose second member's input cannot be keyed under the current schema returns an ordinary rejection for that member and results for the others. Fails today with `request_invalid` for the Batch. Change `mutation_batch.rs:213`, `:240` to refuse the member, with the rejection code the unsupported-version path uses unless a better existing code fits; record the choice here. Choice: `mutation_version_unsupported`, the code the unsupported-version path uses: to the app, a retained Mutation version this backend cannot key is a version it no longer serves, and `model_version_unsupported` names a read contract instead. The input is keyed before the handler runs, so a refused member runs no handler (it fails today with `action.invalid` from `input_identities`, not `request_invalid`).

## Task 4: Reset an incompatible Model (spec §2.2)

**Files:** `crates/client/src/ddl.rs`, `store05.rs`; tests in `crates/sqlite/tests/protocol05_ddl.rs`.

- [x] Test per reset reason (removed field, changed type, changed nullability, changed identity, unique, relations, enum values, required field with no default, removed Model): reopen succeeds; that Model's table, before-image, `axton_authority`, `axton_local_replica_layer` and `axton_local_write` rows are gone; another Model's rows and evidence are untouched; the queue is untouched.
- [x] `ddl::reconcile` takes the stored schema, resets incompatible Models, keeps the additive path and returns whether it reset any. A removed Model's tables are dropped.
- [x] `open05` stops returning `Incompatible`; "Model version regressed" stays. Delete `ddl::incompatibility` if it has no caller left.
- [x] Until Task 5 a reset with an unchanged materialization leaves the Model empty with nothing asking for Bootstrap. Bridge it in this task: when `reconcile` reset a Model and the materialization did not change, set `bootstrap_cursor = NULL` and `start_cursor = cursor` here. Task 5 folds this into the upgrade function.

## Task 5: The upgrade transaction replaces the rollover (spec §2.1, §2.6)

**Files:** `crates/client/src/store05.rs`, `mutation_queue.rs`, `settlement05.rs`, `runtime/mod.rs`, `runtime/tasks.rs`, `runtime/direct.rs`, `sync05/downlink.rs`, `sync05/delta_applier.rs`, `sync05/reset.rs`; `crates/protocols/src/client_bridge/mod.rs`, `src/sync.rs` (read correlation); SDK error tables; tests in `crates/sqlite/tests/protocol05_ddl.rs`, `protocol05_downlink.rs`, `protocol05_queue.rs`, `stream_upgrade.rs`; `integration/v05-sdk/` (`client.mjs`, `rollover/`, `server.mjs:12-16`, `:196`, `run-host.sh:9`, both READMEs).

- [x] Test: reopen with an added nullable field on a Store with rows, `C = 40`, `B = S = 25`. After open: all rows present, `C = 40`, `B` null, `S = 40`, `materialization` is the new ID, a stored read and a Query both succeed with no connection, and a new Batch freezes.
- [x] Test: after that reopen, a Bootstrap unit over `(0, 40]` in the new context overwrites a held `@@bootstrap` row at its unchanged position and fills the new field; a Sync unit after 40 carrying a Remove deletes its kept row.
- [x] Test: a changed unique constraint resets the Model, the materialization is unchanged, and `B` is null with `S = C` after open.
- [x] Test: a handshake reporting a new projection generation commits the same upgrade. No second handshake is sent; the delivery queue is empty; an acknowledgement for a push sent before it is accepted; a Query response in the previous context is returned to the caller and installs nothing.
- [x] `open05` and `adopt_generation05` share one function: set `materialization`, `bootstrap_cursor = NULL`, `start_cursor = cursor`, clear delivery staging. It runs when the materialization changes or `reconcile` reset a Model.
- [x] Sync worker: on a context change with the same Store ID, empty the delivery queue, drop pending apply jobs, adopt the context and schedule. Keep the reset path's branch at `downlink.rs:475`; delete only the rollover part (`:479-497`).
- [x] A Query or Fetch response in another context (`crates/protocols/src/sync.rs:660`) is returned and not installed.
- [x] Acknowledgement validates a receipt snapshot against the current schema, and only when its Model is shared and the outcome's materialization is the Store's (moved from Task 2). `reconcile_ready05` reads bootstrap flags from the current schema.
- [x] Delete `rollover05`, `rollover_pending05`, `desired_materialization`, `enabled_descriptor` (DDL, `DESIRE_DESCRIPTOR`, `admit05`, `reset.rs`; open drops the two columns from an existing file), the freeze hold, the `Needs` and `Initialize` rollover branches, the parked-read condition, `read.schema_pending` and `fetch.schema_pending`.
- [x] Replace the rollover cases in `protocol05_queue.rs` and `integration/v05-sdk/` (drain, clear fixture, generation `"2"` fresh file) with the spec §5 checks. Add the lost-receipt case to `client.mjs`.
- [x] Run the spec §5 `git grep` and record its output for the PR.

## Task 6: Docs

**Files:** `docs/engineering/guarantees.md`, `docs/engineering/architecture/client/storage/protocol5.md`, `storage/reconciliation.md`, `client/runtime.md`, `protocols/sync.md`, `server/protocol5.md`, `docs/engineering/testing/end-to-end.md`, `website/docs/frontend/sync.md`, `setup.md`, `client-api.md`, `loads.md`, `schema/reference.md`.

- [x] Guarantees: replace the rollover paragraph with spec §2.7 and the §2.3 rules; delete the release rule; rewrite D9 and D10 (`:101-102`), since `S` is no longer only a handshake head but `C >= S` still holds; rewrite line 123 (incompatible storage no longer fails).
- [x] Architecture and website pages: describe the upgrade as §2.1 and the table rules as §2.2; remove every mention of drain, clear and `schema_pending`.
- [x] Mark stream-only-downlink §2.3 as superseded by this spec in one line at its top; do not rewrite it.
