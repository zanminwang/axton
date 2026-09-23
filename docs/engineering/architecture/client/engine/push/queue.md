# Queue

## 1. Introduction and Goals

- Persist pending mutations so local writes survive restart and can later reach the server.

## 3. Context and Scope

- [Local operations](../local-operations/README.md) stores each mutation alongside its optimistic changes in the same transaction.
- [Dependencies](dependencies.md) supplies ordering and prerequisite metadata; [Batching](batching.md) assigns mutations to a push.
- [Settlement](../settlement.md) owns rejection and removal of completed mutations.

## 5. Building Block View

- Each mutation has a durable ordinal, ordered operations and an optional push number.
- Wire operations are sent. Companion operations and derived cascade effects stay local and share the mutation's fate.
- `axton_client` holds the counters (`next_ordinal`, `next_push`), `last_completed_push` (the sequence of the last batch a receipt completed; a batch is in flight while its push number is above it) and `push_models` (the read contracts frozen with the batch in flight, released on completion). There is no per-batch table: the batch is the set of mutations sharing a push number.
- Code: [queue.rs](../../../../../../crates/client/src/queue.rs); table definitions in [ddl.rs](../../../../../../crates/client/src/ddl.rs).

## 10. Quality Requirements

- Restart preserves queued operations and their order: [queue reconstruction test](../../../../../../crates/sqlite/tests/engine.rs) `queue_rows_reconstruct_mutations_and_cascade_on_delete` and [restart test](../../../../../../crates/sqlite/tests/push.rs) `offline_queue_and_frozen_bytes_survive_restart_and_receipt_completes_at_once`; the frozen declaration survives with the batch: [settlement.rs](../../../../../../crates/sqlite/tests/settlement.rs) `frozen_batch_and_its_declaration_survive_restart_until_completed`.
- Ordinals and push numbers are allocated transactionally within the protocol's safe integer range; exhaustion returns an error.

## 11. Risks and Technical Debt

- Queue preservation across schema changes lacks direct test coverage; see [Storage](../../storage/reconciliation.md).
