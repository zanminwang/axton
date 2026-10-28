# Remaining sync bottlenecks

Source audited at `adc9b95a2b9e6c54c55179a0676e5c0f9c5e7e7f` (runtime `7e495814de0cfce65da3ae04e387f40944608790`).
This is an inspection report, not a new benchmark or an optimization. The
[evidence index](bottleneck-evidence.json) records provenance and limits.

## 1. Repeated legacy-layout catalog queries

Every [persistence answer](https://github.com/zanminwang/axton/blob/adc9b95a2b9e6c54c55179a0676e5c0f9c5e7e7f/packages/backend/postgres/src/persistence.mts#L433)
awaits `requireFreshLayout` before dispatch, including savepoint/release,
metadata reads, locks and saving results. Its
[SQL](https://github.com/zanminwang/axton/blob/adc9b95a2b9e6c54c55179a0676e5c0f9c5e7e7f/packages/backend/postgres/src/sql.mts#L102) queries `pg_class`
and `information_schema.columns` for retired layouts. It protects against opening
an incompatible namespace, but runs repeatedly within the same transaction.

The [first coalescing archive](../2026-10-27-live-coalescing/evidence.json)
records fixed counts across five N=10,000/M=100 runs: 100 each of claimStore,
savepoint, lockStreams, guardRecords, applyStreamMembers, targetPositions, head,
release and saveResult, 200 readTracking, and three beginBatch calls. These
persistence calls imply **1,103 catalog checks in the upload path alone**, from
source plus recorded host counts. This is not a separately instrumented SQL count.
Read/live preparation adds more checks.

The same archive measured the 100 application handlers at a summed median
**74.014 ms** versus
**5917.108 ms**
for the containing native member operations. These are first-round numbers,
not the final run. The evidence points to framework/persistence/coordination work,
rather than the fixture's single INSERT handler, as the larger cost.

Existing host spans include layout checking, actual persistence SQL and async
waits. They do not isolate catalog time, SQL execution, network round trips,
lock waits or pool acquisition. Do not assign a claimed saving to layout checks.

The first server experiment should count/time this statement separately and
compare checking once within the retained transaction/namespace context against
the current per-operation policy. Preserve refusal before business writes, retry
behavior, connection/search-path/namespace identity, and caller-owned transactions;
a process-global success flag is not an adequate replacement.

## 2. Model watches re-query on every Store generation

[Client writes](https://github.com/zanminwang/axton/blob/adc9b95a2b9e6c54c55179a0676e5c0f9c5e7e7f/crates/client/src/frontend_interface.rs#L202) advance
generation after every successful write transaction, even if the visible Model
projection did not change. Runtime
[committed_since](https://github.com/zanminwang/axton/blob/adc9b95a2b9e6c54c55179a0676e5c0f9c5e7e7f/crates/client/src/runtime/mod.rs#L187) turns that
into one global stale flag. [publish](https://github.com/zanminwang/axton/blob/adc9b95a2b9e6c54c55179a0676e5c0f9c5e7e7f/crates/client/src/runtime/observers.rs#L394)
then queries every ordinary Model watch and converts the complete result before
comparing with the prior snapshot. Unchanged results avoid a notification, but
have already paid the query/conversion/comparison cost.

A concrete trigger is the Store's `Needs` command: it invokes
[`settle_ready05`](https://github.com/zanminwang/axton/blob/adc9b95a2b9e6c54c55179a0676e5c0f9c5e7e7f/crates/client/src/settlement05.rs#L382), which
unconditionally enters `write` even when no outstanding settlement completes.
The next publication sees the new generation. This establishes a source path
for metadata/no-op activity to refresh a Model watch; traces do not yet label each
query with its exact initiating Store command.

| First coalescing case | B watch queries, median count | B changed-result enqueues, median count | B complete-unit applies, median count |
| --- | ---: | ---: | ---: |
| N=1,000 / M=100 | 598 | 100 | 100 |
| N=10,000 / M=100 | 154 | 49 | 49 |

Counts come from the already committed first-round evidence. Each column is a
median of five runs; these are not reconstructed final-run counts. The fixture
registers one unfiltered Entry watch on B. The compiler
[generates Model watch calls](https://github.com/zanminwang/axton/blob/adc9b95a2b9e6c54c55179a0676e5c0f9c5e7e7f/crates/compiler/src/emit.rs#L2029)
through the [Dart ordinary watch API](https://github.com/zanminwang/axton/blob/adc9b95a2b9e6c54c55179a0676e5c0f9c5e7e7f/packages/frontend/dart/lib/src/api/client.dart#L488).
The table demonstrates substantial query work beyond changed snapshots.

SQL watches already use
[dependency table signals](https://github.com/zanminwang/axton/blob/adc9b95a2b9e6c54c55179a0676e5c0f9c5e7e7f/crates/client/src/runtime/sql_watches.rs#L85),
and the Client retains written-table sets. Model-watch invalidation can adopt
dependency-based signals rather than a global generation bit, without changing
the generation's separate stale-writer protection. Dependencies must cover all
tables affecting the visible projection, including pending replay, inheritance
and relevant relation/filter paths. Do not treat sync status, pending-call and
Model-data observers as interchangeable.

The first client experiment should count skipped queries and prove that
metadata-only or unrelated-table commits do not refresh Entry, while local
optimism, rejection/replay, canonical apply, removals, cascades, transaction
rollback and commit ordering still publish correct committed snapshots.
Avoid replacing this with a debounce that delays real content visibility.

## 3. HTTP batching does not batch transaction execution

[The push loop](https://github.com/zanminwang/axton/blob/adc9b95a2b9e6c54c55179a0676e5c0f9c5e7e7f/packages/backend/server/api/backend.mts#L684)
awaits one `run` per member. The PostgreSQL adapter wraps each run in its own
Serializable BEGIN/COMMIT. A transport Batch can carry many writes while
the server still executes and commits 100 member transactions in order.

The final matrix measured a 4.253 s median summed Mutation-execute duration at
N=10,000/M=100; this is not isolated PostgreSQL COMMIT time. Server planning
also continues for live clients. The first-round trace records 200 live response
operations for the burst, so receiver coalescing has not removed all server
response preparation or payload transmission.

Independent outcome commits/replay are existing guarantees. Do not parallelize
members on the same Store or merge their transactions as an incidental speed fix.
First remove repeated checks and measure SQL/host/persistence costs; consider
larger transaction changes only with explicit failure-isolation/replay design.

## Final measurement and comparison limits

The final 45/45-passing matrix's stage totals remain: Mutation execution 4.253 s,
watch query/conversion/comparison 3.967 s, and B's whole-burst completion 5.608 s
at N=10,000/M=100. Stages overlap/nest; live totals may include both clients.
They cannot be added or interpreted as exclusive latency shares.

Metadata inspection now confirms a CPU mismatch:
the previous cursor/first-coalescing runs used **AMD EPYC 7763 64-Core Processor**;
the final run used **AMD EPYC 9V74 80-Core Processor**.
The existing strict machine comparison would reject this. The observed
8.40 → 5.61 s change is not a controlled 33% code improvement.

The workspace briefly recovered: the branch fast-forwarded and final JSON
download/metadata inspection succeeded. The subsequent aggregate profiling
command was interrupted by execution-environment transport loss. It did not
complete a strict comparison, a full raw-data audit or a new benchmark.
The final raw JSON is still not archived into Git; see the main report's
artifact link/expiry. All detailed first-round figures above are explicitly
sourced to its committed evidence rather than substituted for missing final
aggregates.

Recommended order: measure/deduplicate layout checks; narrow Model-watch
invalidation; rerun the same matrix with matched machine metadata. Neither
change should claim a gain before exact-state regressions and a compatible
performance comparison pass.
