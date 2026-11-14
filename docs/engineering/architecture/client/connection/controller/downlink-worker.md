# Downlink worker

## 1. Introduction and Goals

DeltaApplier validates context, header and part digests, then admits bounded fragments into DeliveryQueue. It applies a complete constraint-safe unit in one SQLite transaction with per-key guards and durable coverage. Incomplete fragments never become a prefix. Expiry, gaps and blocked apply leave progress unclaimed and permit explicit repair.

Handshake and delivery headers supply the observed head used for cursor repair.
An accepted Mutation acknowledgment raises that head to its `syncCursor`, so a
Store whose socket is silent pulls `(C, syncCursor]`. The Store checks
settlement after acknowledgment and delivery commits, so either arrival order
can complete pending work; target evidence arrives only through the Stream
([settlement](../../engine/settlement.md)).

An idle receiver admits a live response directly, even while an HTTP cursor
request is outstanding. While ordinary delivery is staged, new socket responses are
validated and coalesced into the highest observed head. Their payload is not
queued and their header grants no coverage. After the admitted work commits,
the controller pulls current state from committed B/C to that head. Fragments
belonging to an already admitted response still finish its atomic unit. An HTTP
wait cannot prevent a live response from advancing an available prefix. Offers
that cannot fill the committed prefix remain hints while HTTP repairs the gap.

`delivery.coalesce` diagnostics count these normal hint-only offers. Queue,
memory and disk limits remain unchanged; malformed hints still fail validation.

Bootstrap prefetch is enabled by default. The native process override
`AXTON_EXPERIMENTAL_BOOTSTRAP_PREFETCH=0` disables it; `1` explicitly enables it.
The worker requests
one successor of a complete Bootstrap page before that page commits. At most
the current page and one successor are retained; only the Store advances B/C.
Future pages cannot apply across a gap or failed predecessor. Reconnect and
apply failure retire speculative work, including requests waiting for refreshed
credentials. Those waiters still occupy the single cursor HTTP request slot.
Capacity decline preserves existing
queue/repair budgets and disables lookahead for that client, reverting to serial
requests from committed B. Core callers can explicitly select serial scheduling
with `Control::with_bootstrap_prefetch(status, false)`.

The local [prefetch performance entry point](../../../../../../integration/performance/prefetch-on.sh)
sets that existing switch before starting clients. The current-build
[short comparison](../../../../../../integration/performance/baselines/2026-10-28-prefetch-current/README.md)
retains complete initial-sync off/on observations and measured request/apply overlap.

The [prefetch regressions](../../../../../../crates/sqlite/tests/downlink/prefetch.rs)
cover both HTTP/apply arrival orders, multi-unit pages, auth refresh, failure,
reconnect and close against the real SQLite Store worker.

The [receipt regressions](../../../../../../crates/sqlite/tests/downlink/receipts.rs)
exercise both arrival orders, partial progress below the settlement fence and
reconnect repair through the real SQLite Store worker.

## 5. Building Block View

[Implementation](../../../../../../crates/client/src/sync/delta_applier.rs) owns this component. [Protocol 5](../../../protocols/wire.md) owns shared context, delivery and settlement rules.
