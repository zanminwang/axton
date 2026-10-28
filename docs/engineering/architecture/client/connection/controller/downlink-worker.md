# Downlink worker

## 1. Introduction and Goals

DeltaApplier validates context, header and part digests, then admits bounded fragments into DeliveryQueue. It applies a complete constraint-safe unit in one SQLite transaction with per-key guards and durable coverage. Incomplete fragments never become a prefix. Expiry, gaps and blocked apply leave progress unclaimed and permit explicit repair.

An idle receiver admits a live response directly. While ordinary delivery is
staged or an HTTP cursor request is outstanding, new socket responses are
validated and coalesced into the highest observed head. Their payload is not
queued and their header grants no coverage. After the admitted work commits,
the controller pulls current state from committed B/C to that head. Fragments
belonging to an already admitted response still finish its atomic unit. Owned
materialization remains separately correlated and is not coalesced away.

`delivery.coalesce` diagnostics count these normal hint-only offers. Queue,
memory and disk limits remain unchanged; malformed hints still fail validation.

## 5. Building Block View

[Implementation](../../../../../../crates/client/src/sync05/delta_applier.rs) owns this component. [Protocol 5](../../../protocols/sync.md) owns shared context, delivery and settlement rules.
