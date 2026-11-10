# Upgrade by overwrite

Issue: none yet. Status: approved for implementation (user, 2026-11-09); design review applied. Slug: `upgrade-by-overwrite`.

A Store upgrade fixes the table shapes and pulls the `@@bootstrap` Models again over the rows it already holds. Nothing is deleted, nothing is drained and nothing waits. The Batch stops carrying the schema version in its identity, so queued Mutations upload after an upgrade exactly as before it. Done means the hold, drain and clear steps of rollover are gone, an upgrade commits in one local transaction with no network, and the checks in §5 pass.

This replaces §2.3 of [stream-only-downlink](2026-11-08-stream-only-downlink-design.md) (drain, clear, Bootstrap). The rest of that spec stands.

## Contents

1. [Problem and evidence](#1-problem-and-evidence)
2. [Design](#2-design)
3. [Changes](#3-changes)
4. [Decisions](#4-decisions)
5. [Verification](#5-verification)
6. [Out of scope](#6-out-of-scope)
7. [Open questions](#7-open-questions)

## 1. Problem and evidence

Terms: the **materialization** is the ID of the read contract a Store speaks, a hash of its Models' fields, identities, enums and bootstrap flags and the backend's projection generation (`crates/core/src/materialization.rs:44-63`). `S`, `B` and `C` are the Store's start, Bootstrap and Sync positions on its Stream.

Today a materialization change is a multi-step state the Store stays in until the network cooperates.

- **Hold.** While `materialization <> desired_materialization`, no new Batch freezes (`crates/client/src/mutation_queue.rs:342`) and every Query and Fetch fails or parks with `read.schema_pending` (`crates/client/src/runtime/direct.rs:366`, `runtime/tasks.rs:225`).
- **Drain.** Frozen Batches must push and settle under the previous materialization first (`crates/client/src/store05.rs:378-387`). The backend must still admit that materialization, or the Batch gets 409 `context_mismatch` and the upgrade never finishes (`crates/server/src/materialization.rs:29-31`).
- **Clear.** One transaction deletes every row with Stream evidence, except rows that unfrozen Mutations hold, which keep the row and lose their evidence (`store05.rs:388-412`). It nulls `S`, `B` and `C`.
- **Bootstrap.** The Store handshakes and pulls again. Rows of unmarked Models are gone until a Query or a Stream change brings them back.

Three couplings cause this.

1. **The Batch freezes the schema version into its identity.** `batch_materialization` is stored at freeze (`mutation_queue.rs:359`), hashed into the digest (`crates/protocols/src/sync/mutation.rs:4-31`) and compared on every acknowledgement (`crates/client/src/settlement05.rs:88`). A frozen Batch can only ever be sent in the contract it was frozen under, which is the only reason a drain exists.
2. **Clearing rows conflicts with the optimistic view.** A queued Mutation's local operations replay over base rows (`crates/client/src/mutate.rs:235-259`). Clearing the base under them is why the clear needs the dirty-key exception.
3. **Incompatible tables refuse to open.** A removed field, a changed type or a changed identity fails `open05` (`store05.rs:172-176`, `crates/client/src/ddl.rs:174-190`). The only way forward is `resetStore`, which discards the queue.

## 2. Design

### 2.1 What an upgrade is

An upgrade is one SQLite transaction. It runs when the materialization changes or when §2.2 resets a Model. (A changed unique constraint or relation resets a Model without changing the materialization.) That happens inside `open05` for a schema change, or inside the handshake commit when the backend reports another projection generation (`crates/client/src/sync05/delta_applier.rs:45-51`).

1. **Fix the tables** (§2.2).
2. **Adopt the contract.** `materialization` becomes the ID of the current schema and generation.
3. **Ask for Bootstrap again.** `bootstrap_cursor = NULL` and `start_cursor = cursor`. `cursor` (`C`) is not touched. A Store that never completed a handshake has no `C`, and this step does nothing.
4. **Drop network plans.** Delivery staging is cleared, as every open does today (`store05.rs:185-187`).

Model rows, before-images, authority evidence and the Mutation queue are not touched, except for a reset Model. There is no pending state afterwards: reads, Queries, Fetches and Batch freezing continue at once, and local reads work offline.

When the Store is next connected, the existing scheduler does the rest (`crates/client/src/sync05/downlink.rs:692-703`):

- Bootstrap pulls the `@@bootstrap` Models over `(0, S]` in the new contract. The server serves any `through` at or below its head (`crates/server/src/delivery_plan.rs:501-503`), and the client checks only `through <= S` (`delta_applier.rs:124-125`). Each delivered record overwrites the held row (§2.3).
- Sync continues from `C`. Every change, delete and Remove published after `C` still arrives. A Remove keeps the row's content and releases its protection, as it always has (`crates/core/src/authority.rs`, `remove`).

Setting `S = C` keeps `C >= S`, and keeps settlement rule 3 of stream-only-downlink true: a target at or below `S` in an unmarked Model is never redelivered, so it settles from the receipt.

**Work in flight.** Only a projection generation change can upgrade a connected Store. In that commit the sync worker sees its context change with the same Store ID: it empties its delivery queue, drops pending apply jobs, adopts the new context and schedules. It does not handshake again. The reset path keeps its own branch (`downlink.rs:475`). A push awaiting its receipt is acknowledged, directly or after one resend of the same Batch (§2.4). A Query or Fetch response that arrives in the previous context is returned to its caller and not installed.

### 2.2 Table shapes

`ddl::reconcile` decides per Model, using the checks `Schema::compatibility` already makes (`crates/core/src/schema.rs:527-631`), applied to one Model at a time.

| Change to a Model | Action |
| --- | --- |
| New Model | Create its tables (unchanged). |
| New nullable field | `ALTER TABLE ADD COLUMN` on the table and its before-image (unchanged). |
| Anything else: removed or renamed field, changed type or nullability, changed identity, unique constraints, relations or enum values, a required field with no default, a removed Model | **Reset the Model.** Drop and recreate its table and before-image (a removed Model's are only dropped), and delete its rows in `axton_authority`, `axton_local_replica_layer` and `axton_local_write`. |

A new required field is in the last row: a compiled schema carries no storage default, because `@default` is a create-time policy (`createDefault`, `crates/compiler/src/validate.rs:83`, `:514`).

A reset Model is empty until Bootstrap, a Stream change or a Query fills it. Open never fails for a schema change. A Model version that goes backwards still refuses to open (`store05.rs:166`).

### 2.3 Evidence belongs to one contract

Two rules make a held row give way to the current contract. Neither rewrites anything at upgrade.

- **Overwrite.** `RecordEvidence::install` replaces `history` with the one entry it installs. Admission is otherwise unchanged (`crates/core/src/authority.rs:79-99`): a record redelivered at its unchanged position under a materialization it has no entry for admits as `Newer`. Stream positions only move up (`packages/backend/postgres/src/sql.mts:103-105`, `:133-135`), so a redelivery is never below the held position. One entry also makes a return to an earlier materialization work: after A, B, A the record holds only B, so A installs again.
- **Protection.** `current` protects a record against cache installs (`settlement05.rs:535`) only while `current.materialization` is the Store's. This covers live content, absence (`current.deleted`) and the deleted-parent check in `stage_cache05`. A kept row of an unmarked Model can then be replaced by a Query result, and a tombstone written under an earlier projection cannot hide a record the current one shows. `RecordEvidence::allows_cache` and `TargetEvidence.protected` take the active materialization.

Absence protection exists for a Query response read before a delete and arriving after it. That race cannot cross an upgrade, because a response in the previous context is not installed (§2.1).

### 2.4 The Batch carries no schema version

A Batch's identity is its intent: Store, Stream, Batch number and the ordered Mutations, each with its own name, version and input.

- **Digest.** `batch_digest` excludes `context.materialization`, as it already excludes the protocol number. Its hash domain becomes `axton:batch-intent:2`.
- **Request.** `context.materialization` stays on the request as transport: it tells the server which contract to shape the receipt's snapshots in. `reconstruct_batch05` reads it from the Store at send time. The `batch_materialization` column and its half of the `axton_queue_envelope` trigger are deleted.
- **Receipt.** `MutationOutcome::Accepted` gains `materialization`, the contract its target snapshots were loaded in. The client stores it with the outcome (`axton_mutation_queue.outcome_materialization`). The server sets it when the Mutation executes and saves it with the result. A replayed result is returned as saved (`crates/server/src/mutation_batch.rs:126-133`), so it keeps the label of its first execution; a Batch that was partly executed before an upgrade returns outcomes with different labels.
- **Acknowledgement.** The client correlates a receipt by Store, Stream, Batch number and digest. It no longer compares the materialization.

Server admission is unchanged: a request in a materialization the backend does not serve gets 409 `context_mismatch`. That is now the client's current contract, the same one its pulls use, so a queue can no longer be stranded by a contract only old Batches name.

### 2.5 Queued Mutations across an upgrade

One rule covers every piece of a Mutation that was shaped by a schema: **a piece from another contract is not used. It is never an error, and the Mutation is never deleted.**

A Model is **shared** by a Mutation when its shape in the Mutation's retained descriptor is storage-compatible with the current schema: the per-Model check of §2.2 finds nothing to reset. The check ignores the Model's version number, which a retained descriptor is expected to trail. A Model missing from the current schema is not shared.

| Piece | When it is used | Otherwise |
| --- | --- | --- |
| The input (name, version, slots) | Always. It uploads as enqueued. The server chooses the handler by name and version. | |
| A local operation on a Model (the optimistic view) | The Model is shared. | Skipped everywhere the queue's operations or inputs are keyed (replay, dependency and sequence derivation at enqueue, dirty keys), and the Mutation reads as diverged in the unsent-work view. The flag is derived from the skip and not stored. |
| A settlement target | The Model is shared. | Not required in the receipt, not awaited, not installed. |
| A receipt snapshot (`fallback` or `Private` record) | Its Model is shared and the outcome's `materialization` is the Store's. It is validated against the current schema, not the Mutation's retained one (`settlement05.rs:107-125`). | Not installed. The target settles without it. |

Every client path that keys a Mutation's operations applies the shared check first: acknowledgement validation, the mandatory-target check (`crates/protocols/src/sync/mutation.rs:74-84`), rejection, settlement and rebuild (`settlement05.rs:177`, `:284`, `:344`). Without it, an operation on a removed Model fails `record_key`, the Batch is never acknowledged, and no later Batch freezes (`mutation_queue.rs:334-338`).

**Server requirement.** A Mutation whose input cannot be keyed under the server's current schema is refused as that one Mutation, with an ordinary rejection. Today it fails the whole Batch with `request_invalid` (`mutation_batch.rs:213`, `:240`), which strands the queue the same way.

Settlement asks one thing of a Stream target: has the Stream delivered this record at or after the target's position? It does not ask in which contract. A target delivered before the upgrade settles at once, and its row is overwritten by Bootstrap like any other held row. `TargetEvidence` loses its `materialization`, and `target_disposition` its comparison (`crates/protocols/src/sync/delivery.rs:316-319`). A target not yet delivered waits for Sync, or settles from the receipt when it is at or below `S` in an unmarked Model, as before.

The shared check compares shapes and needs no stored marker: a Mutation enqueued two upgrades ago is judged against the schema of today.

### 2.6 What is deleted

- The rollover state: `desired_materialization`, `enabled_descriptor`, `rollover_pending05`, `rollover05`, the freeze hold, `read.schema_pending` and `fetch.schema_pending`, and the read parking keyed on it.
- The release rule that a backend must retain the previous materialization until clients drain. A backend serves the contracts of the app versions still in use, as it must for their pulls.
- The dirty-key exception and the Stream-evidence classifier of the clear.
- Multi-entry evidence history.

The SQLite format number stays 5. Open drops the three columns (`desired_materialization`, `enabled_descriptor`, `batch_materialization`) when they exist and recreates `axton_queue_envelope` over `batch_digest` alone. A file with a Batch frozen under an earlier digest definition is handled as 0.6.0 handles it: one terminal error naming the reset, no retry, nothing deleted (`mutation_queue.rs:296-304`).

### 2.7 Guarantee and limits

Across an upgrade, queued Mutations are never deleted and are uploaded, and the Stream's records of `@@bootstrap` Models are pulled again.

- "Pulled again" means the records the Store's Stream already tracks. The backend's Bootstrap handler does not run again for an existing Store (`delivery_plan.rs:84-101`), so a Model newly marked `@@bootstrap` brings only its tracked records.
- A held row stays until something overwrites it. Between the upgrade and the end of Bootstrap the app reads rows in the previous contract's content; an added field is empty or its default.
- A row of an unmarked Model is cache after an upgrade. It is replaced by a later Stream change or Query result. If a new projection generation hides that record, nothing redelivers it (a Loader null becomes a delete only on redelivery, `delivery_plan.rs:288-292`), and the row stays until a later Stream change replaces or deletes it.
- Rows of a reset Model are gone until they are delivered or queried again.

## 3. Changes

**Protocol (`crates/protocols`)**
- `src/sync/mutation.rs`: remove `materialization` from `batch_body`; domain `axton:batch-intent:2`; `validate_acknowledgement` compares Store and Stream instead of the whole context, and takes the Models whose targets are required.
- `src/sync.rs`: `MutationOutcome::Accepted.materialization`.
- `src/sync/delivery.rs`: `TargetEvidence.protected` means protected in the active materialization.
- `src/client_bridge/mod.rs`: delete the `schema_pending` error kinds.
- `fixtures/protocol/0.5.json`: regenerate digests and receipts.

**Core (`crates/core`)**
- `src/authority.rs`: `install` keeps one history entry; `allows_cache(materialization)`.
- `src/schema.rs`: expose the per-Model compatibility check that `compatibility` runs today.

**Server (`crates/server`)**
- `src/mutation_batch.rs`: set `materialization` on accepted outcomes from the request context; refuse a member whose input cannot be keyed instead of failing the Batch.

**Client (`crates/client`)**
- `ddl.rs`: `reconcile` resets an incompatible Model instead of failing and reports whether it reset any; delete `incompatibility` if nothing else calls it.
- `store05.rs`: `open05` stops refusing incompatible schemas and runs the upgrade transaction; delete `rollover05`, `rollover_pending05`, the two rollover columns and the `batch_materialization` column; `adopt_generation05` runs the same upgrade.
- `mutation_queue.rs`: delete the freeze hold; `reconstruct_batch05` uses the Store's context.
- `settlement05.rs`: acknowledgement, rejection and settlement follow §2.5; `stage_cache05` follows §2.3; bootstrap flags come from the current schema.
- `mutate.rs`, `rows.rs`: replay skips operations and fills defaults per §2.5.
- `runtime/mod.rs`, `runtime/tasks.rs`, `runtime/direct.rs`, `sync05/downlink.rs`, `sync05/delta_applier.rs`: delete the rollover calls and the parked-read path; the context-change branch follows §2.1; a read response in another context is returned and not installed.

**Integration and docs**
- `integration/v05-sdk/`: replace the `rollover/` fixture, `server.mjs:12-16`, `:196`, `run-host.sh:9` and both READMEs.
- `docs/engineering/guarantees.md` (the rollover paragraph, the release rule, D9 and D10 on the initial boundary, line 123 on incompatible storage), `architecture/client/storage/protocol5.md`, `storage/reconciliation.md`, `client/runtime.md`, `protocols/sync.md`, `server/protocol5.md`, `docs/engineering/testing/end-to-end.md`, `website/docs/frontend/client-api.md`, `sync.md`, `setup.md`, `loads.md`, `schema/reference.md`.

## 4. Decisions

- **Overwrite; delete nothing** (user, 2026-11-09). Rejected: clear the server-derived rows and keep the queue. The optimistic view needs base rows, so the clear needs exceptions for every key a Mutation holds. Rejected: a new database per version, which has the same problem and also has to move the queue.
- **Keep `C`; set `S = C`.** Rejected: nulling all three positions, which forces either a clear or a sweep to find rows the server no longer gives. With `C` kept, deletes and Removes after `C` arrive by Sync. Rejected: `S` = the next handshake head, which makes the upgrade wait for the network and breaks `C >= S`.
- **No sweep after Bootstrap** (user, 2026-11-09). A kept row the Stream does not redeliver is cache, the same standing as a row a Query installed. Cost: the hidden-record limit in §2.7.
- **No migration scripts** (user, 2026-11-09). The server holds the data; a table that cannot be altered in place is emptied and filled again.
- **Reset an incompatible Model instead of refusing to open.** Rejected: today's refusal, whose only exit is `resetStore` and the loss of the queue.
- **The upgrade runs on a Model reset as well as on a materialization change.** Rejected: materialization change only, which leaves a Model emptied by a unique-constraint change with nothing to pull it again.
- **The Batch identity excludes the schema version** (user, 2026-11-09). Rejected: keeping the frozen materialization and only skipping the drain. A frozen Batch would still name a contract the backend may have dropped.
- **The receipt labels its snapshots with a materialization.** Rejected: a Model version on each snapshot, which does not see a projection generation change. Rejected: no label and trusting the request context, which mislabels a replayed result that first executed before the upgrade.
- **A piece from another contract is dropped, not converted** (user, 2026-11-09). Rejected: shaping receipt data by Mutation version, which puts two schema versions on the device.
- **Derive the shared check from descriptors.** Rejected: a per-Model reset marker in the Store, which is one more piece of state to keep true.
- **A new required field resets the Model.** Rejected: filling a default in replay. No compiled schema has a storage default, so the fill could never run.
- **Settlement evidence counts in any contract.** Rejected: waiting for redelivery in the new contract. A `@@bootstrap` target whose record was deleted before the upgrade would wait on a tombstone Bootstrap may not deliver, and an unsettled Batch blocks every later one (`mutation_queue.rs:334-338`).
- **All protection is per contract, absence included.** Rejected: keeping absence protection across contracts. A tombstone from a Loader null under an earlier projection would block a record the current projection shows, and it needs a second rule.
- **Evidence history holds one entry.** Rejected: one entry per materialization, which turns a return to an earlier materialization into a `Duplicate` and skips the overwrite (`authority.rs:83-86`).
- **A read response in another context is returned and not installed.** Rejected: failing it, which surfaces an error to the app for a change it did not make.
- **SQLite format 5 kept; open removes the dead columns.** Rejected: format 6 with no migration, which refuses every existing file and discards its queue for a change that only removes three columns.

Design review (2026-11-09) changed: the upgrade also runs on a Model reset; §2.5 gained the shared check on every keying path and the server requirement, because a Mutation on a removed Model stranded the queue; protection became one rule; history became one entry; defaults in replay were decided instead of left to a test; §2.1 states what the sync worker and reads in flight do; §2.7 states the hidden-record limit and what "pulled again" covers.

Code review (2026-11-10) changed: settlement evidence counts in any contract, which removes a wait that could block the queue; default filling in replay was removed as unreachable; sequence derivation at enqueue applies the shared check.

## 5. Verification

| Check | Layer |
| --- | --- |
| Reopen with an added nullable field: all rows remain, `C` is unchanged, `B` is null, `S = C`, reads and Queries succeed with no network, and a new Batch freezes | `crates/sqlite/tests/protocol05_ddl.rs` |
| After that reopen, Bootstrap overwrites a held `@@bootstrap` row at its unchanged position and fills the new field | `crates/sqlite/tests/protocol05_downlink.rs` |
| A changed unique constraint, which leaves the materialization unchanged, resets the Model and Bootstrap fills it again | `crates/sqlite/tests/protocol05_ddl.rs`, `protocol05_downlink.rs` |
| A return to an earlier materialization (generation A, B, A) overwrites the row each time | `crates/sqlite/tests/protocol05_authority.rs` |
| A kept row of an unmarked Model is replaced by a Query result after the upgrade, and is not replaced before it | `crates/sqlite/tests/protocol05_cache.rs` |
| A tombstone from before the upgrade does not block a cache install after it; one written after it does | `crates/sqlite/tests/protocol05_cache.rs` |
| After the upgrade, a delete delivered after `C` deletes its kept row, and a Remove after `C` arrives and releases the row's protection | `crates/sqlite/tests/protocol05_downlink.rs` |
| A Batch frozen before the upgrade is sent after it with the same digest and the new context, and the server accepts it | `crates/sqlite/tests/protocol05_queue.rs`, `integration/v05-sdk/client.mjs` |
| A Batch executed before the upgrade whose receipt was lost: the resend returns outcomes labelled with the old materialization; the client settles them and installs no snapshot | `integration/v05-sdk/client.mjs` |
| A Batch partly executed before the upgrade returns mixed labels; each outcome follows its own label | `crates/server/tests/`, `crates/sqlite/tests/protocol05_settlement.rs` |
| A handshake that reports a new projection generation upgrades in the same commit with no second handshake; a push in flight acknowledges; a Query in flight returns its result and installs nothing | `crates/sqlite/tests/protocol05_downlink.rs` |
| Reopen with a field's type changed: only that Model's rows, before-images and evidence are gone; other Models keep theirs; open succeeds | `crates/sqlite/tests/protocol05_ddl.rs` |
| A queued update on a reset Model uploads, is marked diverged and shows no optimistic row; a queued Mutation on another Model keeps its optimistic view | `crates/sqlite/tests/protocol05_queue.rs` |
| A queued Mutation on a removed Model, and one on a Model whose identity changed, are acknowledged or rejected as single Mutations; the next Batch freezes | `crates/sqlite/tests/protocol05_queue.rs`, `crates/server/tests/` |
| A new required field resets its Model; a queued create on it uploads and shows no optimistic row | `crates/sqlite/tests/protocol05_ddl.rs`, `protocol05_queue.rs` |
| A Mutation accepted before the upgrade whose target the Stream already delivered settles after the upgrade with no Bootstrap; so does one whose record was deleted before the upgrade | `crates/sqlite/tests/protocol05_settlement.rs` |
| With a diverged Mutation queued, enqueueing an action sequenced after it succeeds | `crates/sqlite/tests/protocol05_queue.rs` |
| A push answered after a generation upgrade is acknowledged through the sync worker, directly or after one resend | `crates/sqlite/tests/protocol05_downlink.rs` |
| A format-5 file written by 0.6.0 opens; the three columns are gone and the envelope trigger still rejects a changed digest | `crates/sqlite/tests/protocol05_client.rs` |
| `git grep` over `crates`, `bindings`, `packages`, `integration`, `website` and `docs/engineering` except `history/` finds no `rollover` or `schema_pending`, and finds `desired_materialization`, `enabled_descriptor` and `batch_materialization` only in the open-time column removal and its old-file test | run in the last task; recorded in the PR |

## 6. Out of scope

- Removing device-only data and direct writes: [projection-and-queue](2026-11-09-projection-and-queue-design.md). This design holds with or without it, because it deletes nothing.
- Removing a hidden record of an unmarked Model after a projection generation change (§2.7).
- Running the backend's Bootstrap handler again for an existing Store.
- How the server runs old Mutation versions against its current schema, beyond the requirement in §2.5.
- A Model version that goes backwards (an app downgrade).

## 7. Open questions

None for the design. One release decision is the user's: 0.6.0 (stream-only downlink, wire protocol 6) is merged but not published, and this design changes the wire (`Accepted.materialization`, the digest). Recommended, and the plan assumes it: publish 0.6.0 only after this lands, so protocol 6 is defined once. Otherwise this is protocol 7.
