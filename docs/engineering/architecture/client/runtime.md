# Runtime

## 1. Introduction and Goals

ClientRuntime owns task admission, transactions, prerequisites, connection lifecycles, direct reads and observers. It drives protocol-5 handshake, Uplink and finite delivery effects. Priority close cancels parked remote tasks and releases the actor without waiting for a network response; post-close results cannot install Models. SDKs submit tasks and carry effects rather than making retry or coverage decisions.

Settlement needs no request of its own: an accepted receipt raises the delivery head and the Stream brings the evidence. A Store whose context changed drops staging of the old context; already dispatched Store work retains its authority admission and reports completion before the next apply is scheduled. After a reset (a new Store ID) the worker reconnects. After an upgrade (the same Store ID in a new materialization) it drops pulls and applies of the old context and schedules from the committed positions without a second handshake; a push in flight still acknowledges, and a Query or Fetch response from the previous context is returned and not installed. Stored reads wait only for the first handshake.

Every publication path refreshes retained pending, failure and refusal observers before consuming the commit mark for Model watches. Application tasks and protocol-5 Store worker commits use this same boundary; listeners receive changed committed snapshots without polling.

Ordinary Model watches retain an identity-ordered result cache. Exact committed
Model keys refresh only their own rows; metadata and unrelated Models do no
business-data reads. Bulk/unknown changes and failed reads retain a full-refresh
fallback. Negotiated observers record ordered splices while updating that cache,
so later publication copies only changed rows across the
[client bridge](../protocols/client-bridge.md). Legacy callers still receive full
results. `watchSql` continues to requery by its SQLite dependency tables.

An open application transaction has exclusive scheduling of its continuation
commands until its callback returns and commits or rolls back. Ordinary tasks,
including reads through the committed SQLite reader, wait outside that
transaction. A slow callback can therefore delay local reads even though WAL
would permit reading the previous committed state. This is runtime scheduling,
not a server Store progress lock. The
[transaction regression](../../../../crates/sqlite/tests/runtime_transactions.rs)
`a_callback_transaction_owns_the_writer_until_its_result_commits` records the
current behavior; allowing independent committed reads during the callback would
require a separate scheduling change and coverage for observer and close ordering.

## 5. Building Block View

[Implementation](../../../../crates/client/src/runtime/mod.rs) owns this component. [Protocol 5](../protocols/wire.md) owns shared context, delivery and settlement rules.
