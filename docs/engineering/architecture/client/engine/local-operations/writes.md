# Writes

## 1. Introduction and Goals

Local writes become visible in their transaction and durable at commit. The only local write is a named Mutation: it retains input and optimism. Later authority replaces the base beneath pending operations, which replay in committed local order.

## 3. Context and Scope

| Write | Application entry point | Sent to the server | Local fate |
| --- | --- | --- | --- |
| Named Mutation | Generated `mutations.<name>` or `tx.mutations.<name>` | Frozen named input only | Refusal removes this Call's optimism |

A Model row changes only through a named Mutation's optimism or through server delivery. `tx.models` only reads. One application transaction may read and submit several Calls. They commit locally together, but Calls have independent backend outcomes. The transaction body is never persisted or replayed. See the [typed client](../../../frontend-sdk/api.md) for transaction scope and the two Mutation completion boundaries.

## 5. Building Block View

- [mutate.rs](../../../../../../crates/client/src/mutate.rs) applies operations, extends cascades and rebuilds dirty projections.
- [mutation_queue.rs](../../../../../../crates/client/src/mutation_queue.rs) retains named input, prerequisites and dependency metadata.
- [authority.rs](../../../../../../crates/client/src/authority.rs) installs proved authority beneath pending edits and reports replay divergence.
- [settlement.rs](../../../../../../crates/client/src/settlement.rs) reconciles each Call's outcome and owned work.
- [rows.rs](../../../../../../crates/client/src/rows.rs) owns Model row statements and value encoding.

A dirty record has pending operations. Its before image holds the base beneath them. `axton_local_write` retains the receipt-finalized state of a target the Stream will not deliver, as `accepted`, at its wire operation's position. Rebuild combines that base, pending operations and accepted rows. Accepted rows with no earlier pending operation fold into the base. When replay cannot apply a queued operation, the current base remains visible, the durable Call remains queued, and a `diverged` report identifies its ordinal.

## 6. Runtime View

Submitting a named Mutation validates retained input and binding rules, applies optimism, derives dependencies/prerequisites and persists its ordinal in the local transaction. Savepoint or transaction failure rolls back the corresponding work; unawaited commands cannot escape the transaction's lifetime.

A delete cascades through declared local relations once per identity, including cycles. These child effects stay local and follow their owning Call; they are not additional wire operations. Replaying a queued delete extends it to newly delivered children while respecting later accepted recreations. Business handlers must delete and explicitly invalidate their server-side children themselves.

Records have no publication stamp. Stream cursor guards and call-owned evidence decide authority installation; duplicate delivery cannot undo newer authority. Ordinary null-cursor Query/Fetch records can populate unprotected cache, but cannot replace protected Stream state, clear a tombstone or advance progress. [Protocol 5](../../../protocols/wire.md) owns these rules.

## 10. Quality Requirements

Current source fixtures cover atomic local writes, accepted-write replay, cascade, and authority beneath pending edits:

- [runtime transactions](../../../../../../crates/sqlite/tests/runtime_transactions.rs)
- [settlement](../../../../../../crates/sqlite/tests/settlement.rs) and [authority](../../../../../../crates/sqlite/tests/authority.rs)

These links identify maintained coverage, not a new execution certificate. Installed-package and mobile evidence have separate [adoption gates](../../../../protocol5-adoption.md).

## 11. Risks and Technical Debt

An application transaction body can hold its own Store transaction until it finishes; independent Clients remain responsive. Replay/cascade work grows with the retained queue and dependency component. Whole authority units cannot be split to reduce writer time; measure actual staging space and capacity on target devices.
