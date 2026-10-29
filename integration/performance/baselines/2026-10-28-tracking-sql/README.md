# Singleton tracking lookups with the post-lock recheck retained

Tracking discovery and post-lock recheck are both retained. This adapter change
specializes one explicit pair into an exact model/identity/Stream index lookup,
and one global identity into an exact record lookup followed by its live holders.
It uses existing record-identity and membership indexes. Empty input returns no
tracking rows without running the tracking statement. Multiple identities/pairs
and mixed inputs retain the general JSON/UNION query and deduplication.
No result cache, migration, native or locking change was introduced.

## Why this query shape

`plans.json` retains a separate synthetic 10k-metadata EXPLAIN ANALYZE/BUFFERS
profile: looking for one absent explicit pair through the general query led to
10,000 repeated identity-index probes. The direct predicate began with one
identity-index lookup. That fresh, un-analyzed fixture reported 6.129 versus
0.008 ms of execution, excluding application round trips. This single plan
example is diagnostic evidence, not the benchmark's timing or a production
speedup claim. The end-to-end comparison below is the performance evidence.

## Short controlled comparison

Four fresh N=10,000/M=100 live runs executed before/after/after/before, with
identical native libraries and AOT executables. Frozen pre-change adapter/SQL
sources are retained under `control/` and their hashes are checked against the
runner's records. Both conditions retain group size 8, prefetch, canonical
encoding, SQLite caching, grouped result persistence and combined publication.

Median [minimum, maximum], two runs per condition; times in milliseconds:

| Metric | Before | After |
| --- | ---: | ---: |
| Tracking SQL calls | 200.000 [200.000, 200.000] | 200.000 [200.000, 200.000] |
| Tracking SQL elapsed ms | 134.119 [133.482, 134.756] | 24.591 [23.728, 25.454] |
| All 100 settled on A | 469.033 [467.173, 470.893] | 357.272 [355.451, 359.093] |
| All 100 visible on B | 465.859 [464.043, 467.675] | 353.318 [351.412, 355.225] |

Tracking SQL time fell 81.7%; all-A completion fell 23.8% (111.761 ms), and all-B
visibility fell 24.2% (112.541 ms). Both adjacent pairs favored the optimized
adapter. Each sample still executed 200 tracking statements for 100 Mutations.
The audit verifies the actual host phase sequence for every Mutation:
`readTracking -> lockStreams -> guardRecords -> readTracking`. The second read
is executed afresh after the guards. Singleton reads switch from the general
WITH/UNION to SELECT predicates; their count is not reduced.

Continuous request freezing produced 14 groups in both controls and 14/15 in
the optimized runs. This can change total query counts and subscriber work;
it does not change the 200 tracking statements. Only two samples per condition
were collected. They establish direction for this local fixture, not production
or independent-run tail latency. Per-write burst observations are correlated.
SQL spans include execution/wait and overlap their parent spans.

## Verification and evidence

All four samples verified 100 accepted outcomes, final client/server identities
and content, drained local work, no transport/backend/cleanup errors, no trace
loss and aligned timelines. `audit.mjs` checks frozen source hashes, identical
artifact/source identities, fresh discovery/lock/guard/recheck ordering, general
versus indexed statement selection, final state and query budgets:

```sh
node integration/performance/baselines/2026-10-28-tracking-sql/audit.mjs
```

Raw reports are the four `*.json.gz` files; `evidence.json` is derived by the
audit. `runner.mjs` freezes the exact local one-sample runner, including absolute
workspace imports to relocate elsewhere. Copy `control/persistence.mts` and
`control/sql.mts` to `artifacts/performance/tracking-control/`, select
`AXTON_TRACKING_VARIANT=before` or `after` in fresh runner processes, record
build identity using `run.mjs --record-build`, and retain output with `--output`.
`runtime.patch` is a partial dirty-worktree snapshot relative to parent `294f356`,
including preceding persistence changes, not a standalone clean-parent patch.

Executed checks: all 15 PostgreSQL batching tests passed, including new indexed/
general result-set equivalence across missing/untracked keys, live/removed
memberships, duplicate mixed inputs, empty input and fresh same-transaction
rechecks. All 30 native Batch tests passed, including concurrent global
invalidation and tracking; all four publication-semantics and three pg/Prisma/
Drizzle Batch tests passed. All 20 performance-harness tests, TypeScript build/
check, formatting and whitespace checks passed. Settlement Rust was unchanged;
no native/client rebuild or mobile/full-host gate was required or run.

Apple M5 Max, macOS arm64, PostgreSQL 16.14, Node 24.19.0, Dart 3.12.1, Rust 1.98.1.
Fresh PostgreSQL database, client process and SQLite files per sample. Fixture
setup, process startup and compilation excluded; caches not flushed. This is
local SDK watch visibility, not UI rendering, mobile or WAN latency. The clients
issue 100 continuous calls, awaiting each local commit but not settlement.
The optimized adapter is retained locally. No publish/deploy was performed;
the disposable local database was stopped after verification.
