# Downlink worker

## 1. Introduction and Goals

DeltaApplier validates context, header and part digests, then admits bounded fragments into DeliveryQueue. It applies a complete constraint-safe unit in one SQLite transaction with per-key guards and durable coverage. Incomplete fragments never become a prefix. Expiry, gaps and blocked apply leave progress unclaimed and permit explicit repair.

Handshake and delivery headers supply the observed head used for cursor repair.
A Mutation acknowledgment's `syncCursor` is only a durable settlement fence;
it neither raises that head nor triggers an ordinary Pull. The Store checks
settlement after acknowledgment and delivery commits, so either arrival order
can complete pending work. Missing target authority can still require separately
owned [settlement materialization](../../engine/settlement.md).

An idle receiver admits a live response directly, even while an HTTP cursor
request is outstanding. While ordinary delivery is staged, new socket responses are
validated and coalesced into the highest observed head. Their payload is not
queued and their header grants no coverage. After the admitted work commits,
the controller pulls current state from committed B/C to that head. Fragments
belonging to an already admitted response still finish its atomic unit. An HTTP
wait cannot prevent a live response from advancing an available prefix. Offers
that cannot fill the committed prefix remain hints while HTTP repairs the gap. Owned
materialization remains separately correlated and is not coalesced away.

`delivery.coalesce` diagnostics count these normal hint-only offers. Queue,
memory and disk limits remain unchanged; malformed hints still fail validation.

The [receipt regressions](../../../../../../crates/sqlite/tests/protocol05_downlink/receipts.rs)
exercise both arrival orders, partial progress below the settlement fence and
reconnect repair through the real SQLite Store worker.

## 5. Building Block View

[Implementation](../../../../../../crates/client/src/sync05/delta_applier.rs) owns this component. [Protocol 5](../../../protocols/sync.md) owns shared context, delivery and settlement rules.
