# Writes

## 1. Introduction and Goals

A write must be visible immediately and still be undoable. Writes achieves both by applying every change to the visible table at once and, for records that now have a pending mutation, remembering the last row the server sent in a *before image*. Later components rebuild the visible row from that before image when the server's receipt accepts or rejects the change, or a page overrides it.

## 3. Context and Scope

Two kinds of write enter here, both as operations `{model, op, identity, values}` on one record:

| Kind | Entry point | Sent to the server | Undone on rejection |
| --- | --- | --- | --- |
| Named mutation (wire operations, plus optional *companion* operations) | `enqueue` | wire operations yes, companions never | yes, with its companions |
| Direct write | `direct` | never | no: it is final at commit |

Writes owns the visible table and the before-image table of every model ([Storage / Reconciliation](../../storage/reconciliation.md) creates them). It hands finished mutations to the [Queue](../push/queue.md) and calls [Dependencies](../push/dependencies.md) to derive what each mutation waits for. [Pull](../pull.md) and [Settlement](../settlement.md) call back into it to record server truth and to rebuild rows.

## 5. Building Block View

Three ideas carry the design:

- **Dirty.** A record is dirty while any queued mutation names it. Dirtiness decides which table holds the server's version.
- **Before image.** The first mutation that touches a clean record copies its visible row aside. Later mutations on the same record do not copy again, so the before image always holds the row as it was before the *oldest* pending mutation. When nothing pending touches the record any more, the before image is dropped.
- **Rebuild.** The visible row of a dirty record is a function of two inputs: the before image and the still-queued operations on that record, replayed in ordinal order. Whenever either input changes, the row is recomputed. If a queued operation no longer applies (an update after an authoritative delete, say), the replay stops and the before image is shown.

Code: `enqueue`, `direct`, `hold_truth`, `rebuild`, `set_authority`, `descendants` in [client/mutate.rs](../../../../../../crates/client/src/mutate.rs); row statements and the JSON to SQL codec in [client/rows.rs](../../../../../../crates/client/src/rows.rs).

## 6. Runtime View

**Enqueueing a mutation.** Each operation is normalized against the schema (identity, full state for a create, patch for an update). For every operation, in order: copy the record aside if it is clean, apply the operation to the visible table, and if it is a delete, do the same for every child the schema cascades to, recording those child deletes as *effects* of the mutation. Then dependency and prerequisite metadata is derived and the mutation is stored with a new ordinal. Everything happens in one savepoint, so a failure such as a unique-index violation undoes only this mutation.

**A direct write.** The operation is applied to the visible table, with the same cascade for deletes. If the record is dirty and exists in authority (its before image holds a row), the write is also folded into the before image, so that a later rejection of the pending mutation does not undo it (guarantee L4). If the record's existence is itself pending (the before image is "absent"), there is no base to advance: the write lives only in the visible row and goes with the create if the create is rejected, or is replaced by the authoritative row the receipt carries once the create is accepted. A direct write on a clean record touches only the visible table.

**Direct writes and stamps.** A direct create, update or delete does not create or advance the record's stamp. Existing stamp metadata is retained; without it, comparison uses `0`. The stamp tracks the last applied server version, not local edits. A later server change with a higher stamp can replace the local data; equal or older stamps do not overwrite it. [Pull](../pull.md#5-building-block-view) owns the comparison; the application rules below determine the visible result when mutations are pending.

**Server truth arriving.** When [Pull](../pull.md) delivers a record, Writes puts it where it belongs: into the before image and then rebuilds the visible row if the record is dirty, straight into the visible table otherwise. A delivered delete first cascades to local children. Afterwards queued deletes are extended to any children that appeared since they were queued, so a later page cannot resurrect a child of a record the user already deleted.

## 8. Crosscutting Concepts

Normalization of values and identities is shared with the server and the wire ([Protocol / Common](../../../protocol/common.md)); cascade rules come from [Relations](../../../schema/relations.md).

## 10. Quality Requirements

- **A write is readable at once by the same transaction and by every reader after commit** (guarantee L1). Evidence: [sqlite/tests/client.rs](../../../../../../crates/sqlite/tests/client.rs) `session_reads_own_writes_without_notifying_until_commit_and_blocks_other_writes`; [crates/sim/tests/local.rs](../../../../../../crates/sim/tests/local.rs) `l1_merged_view_shows_pending_edits_in_order`.
- **The before image is taken once and dropping the last pending mutation restores it** (guarantee L3 for the mutation savepoint). Evidence: `optimistic_edit_holds_truth_once_and_rejection_rebuilds_from_it`, `local_transaction_and_mutation_savepoint_have_independent_fate`, `declared_unique_constraint_is_atomic`.
- **A direct write never enters the queue, and a direct write on a dirty authoritative record survives the mutation's rejection** (guarantee L4). Evidence: `l4_direct_write_is_never_pushed_and_survives_rejection`; [sqlite/tests/push.rs](../../../../../../crates/sqlite/tests/push.rs) `rejection_removes_optimism_preserves_direct_truth_and_has_durable_inbox`.
- **A direct write on a pending-create record is removed with the rejected create and never becomes a row without a base; a stale page does not undo a direct write** (guarantee L4). Evidence: [crates/sim/tests/local.rs](../../../../../../crates/sim/tests/local.rs) `l4_direct_write_on_pending_create_goes_with_the_rejected_create`, `l4_stale_duplicate_page_does_not_undo_a_direct_write`; the random run with direct writes on, [crates/sim/tests/invariants.rs](../../../../../../crates/sim/tests/invariants.rs) `random_sequences_with_direct_writes`, is no longer ignored.
- **Deletes cascade to declared local children exactly once, including through cycles** (guarantee L5). Evidence: `schema_cascade_is_optimistic_same_fate_and_not_extra_wire_operations`, `direct_cascade_handles_cyclic_relationships_once`.
- **Server truth lands beneath pending edits and they replay on top** (guarantee A1). Evidence: [sqlite/tests/downlink.rs](../../../../../../crates/sqlite/tests/downlink.rs) `newer_authority_lands_beneath_pending_edits_and_replays_them`.

Verified 2026-09-14: `cargo test -p axton-sim --locked` passed with the two L4 scenarios and the direct-write random run enabled; the earlier rows were read, not executed.

## 11. Risks and Technical Debt

**Potential risk: a failed replay is silent.** *Condition:* a queued operation no longer applies to the current before image. *Consequence:* the visible row shows the before image while the mutation stays queued and will still be sent; nothing reports the divergence. *Evidence:* the `failed` branch of `rebuild`. No test covers it. **To confirm:** whether this case should be surfaced ([#122](https://github.com/zanminwang/axton/issues/122), split out of #55).

**Potential risk: cost grows with queue length.** Extending queued deletes to new children re-scans the whole queue and recomputes descendants on every delivered record. Not measured; performance work is [#12](https://github.com/zanminwang/axton/issues/12).
