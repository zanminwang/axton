# Single-Stream reserve-and-publish SQL

The previous 1,131-query Mutation path (100 continuous writes, 14 groups) breaks
down as follows. This archive's `before-2` run reproduces that exact budget:

| Purpose | Calls |
| --- | ---: |
| Apply Stream membership: temp-table setup, cursor reservation, member write | 300 |
| Tracking discovery and guarded recheck | 200 |
| Business SAVEPOINT and RELEASE | 200 |
| Stream MVCC locks and record guards | 200 |
| Target positions and final Stream head | 200 |
| Store claims, Batch starts and grouped results | 31 |
| Total | 1,131 |

Tracking and publication were the largest SQL elapsed stages, each around
127–128 ms. This change combines cursor reservation and the member write for
one-Stream, publish-only runs of at most `STREAM_BATCH` (1,000) deltas. A single
CTE carries a newly allocated cursor through `RETURNING` into membership writes;
existing scoped reservations are read from the same temporary table. No new
cursor is allocated when that scope already has one. Temporary table creation
still executes separately, preserving first-creation rollback and namespace/
transaction lifetime. The common publication path uses two statements instead
of three. Multi-Stream, mixed publish/read and larger runs use the existing path.
No framework migration, driver contract or native change is required.

## Short comparison

Four fresh N=10,000/M=100 live runs executed before/after/after/before. All native
libraries and AOT executables were identical. The control is frozen copies of
`persistence.mts` and `sql.mts` from immediately before this change, loaded through
the same pg driver. Their hashes are recorded by the runner and checked against
`control/`. Group size 8, prefetch, canonical encoding, SQLite caching and grouped
result persistence were enabled in both conditions.

Median [min, max], two runs per condition; times in milliseconds:

| Metric | Before | After |
| --- | ---: | ---: |
| All 100 settled on A | 497.877 [492.987, 502.768] | 444.279 [441.379, 447.179] |
| All 100 visible on B | 493.670 [487.733, 499.607] | 440.693 [438.007, 443.379] |
| Publication SQL elapsed ms | 128.057 [126.415, 129.700] | 78.002 [77.161, 78.843] |
| Publication SQL calls | 300.000 [300.000, 300.000] | 200.000 [200.000, 200.000] |
| Mutation framework SQL calls | 1132.000 [1131.000, 1133.000] | 1032.000 [1031.000, 1033.000] |
| All framework SQL calls | 1380.000 [1371.000, 1389.000] | 1280.000 [1271.000, 1289.000] |

All-A completion fell 10.8% (53.599 ms), and all-B visibility fell 10.7%
(52.977 ms). Both adjacent pairs favored the optimized adapter. Publication SQL
elapsed time fell 39.1%, with exactly 100 calls removed per 100 Mutations.
Tracking SQL stayed around 127 ms and is now the largest measured SQL category.
The discovery/recheck and per-member savepoints were retained.

Continuous uploads produced 14 and 15 commit groups in each condition, in
opposite adjacent orders. Additional group bookkeeping adds two Mutation SQL
calls; subscriber delivery also adds work. Hence the total ranges above rather
than one fixed overall budget. The 14-group path changes 1,131 to 1,031 Mutation
SQL calls. Source fingerprints and all artifact hashes match across conditions.
These two observations per condition are directional local evidence, not a
production guarantee or an independent-run tail estimate. SQL times include
execution/wait and overlap enclosing spans; they must not be added together.

## Correctness and evidence

All four samples verified 100 accepted outcomes, complete client/server identity
and content, drained local work, no transport/backend/cleanup errors, no trace
loss and aligned timelines. `audit.mjs` attributes every Mutation query through
its host-span ancestry, verifies per-operation budgets, frozen control hashes,
identical source/build identities, final state and clock evidence:

```sh
node integration/performance/baselines/2026-10-28-publication-sql/audit.mjs
```

The four `*.json.gz` files retain raw data; `evidence.json` is derived by the
audit. `runtime.patch` is a partial dirty-worktree snapshot relative to parent
`294f356`, including earlier SQL/group changes, not a standalone clean-parent
patch. `runner.mjs` is the exact local runner and includes absolute workspace
imports to relocate elsewhere. To replay, copy `control/persistence.mts` and
`control/sql.mts` to `artifacts/performance/publication-control/`, select
`AXTON_PUBLICATION_VARIANT=before` or `after` in fresh runner processes, record
build identity using `run.mjs --record-build`, and retain each output.

Executed regression checks: all 13 PostgreSQL batching tests passed, including
new combined-operation query budgets, scoped reuse and overflow-without-member-
writes; 30 native Batch, 18 caller savepoint/namespace, four publication semantics
and three pg/Prisma/Drizzle Batch tests passed. These cover cached reservation
reuse, first-creation rollback, retained prior reservations, safe counter limits,
concurrency, refusals and replay. All 20 performance-harness tests, TypeScript
build/check, formatting and whitespace checks passed. Rust/native/client sources
were unchanged for this experiment; no mobile or full-host gate was run.

Apple M5 Max, macOS arm64, PostgreSQL 16.14, Node 24.19.0, Dart 3.12.1, Rust 1.98.1.
Fresh PostgreSQL database, client process and SQLite files per sample. Fixture
setup, process startup and compilation excluded; caches not flushed. This is
local SDK watch visibility, not UI rendering, mobile or WAN latency. The clients
make 100 continuous calls, awaiting each local commit but not settlement.
The optimized adapter is retained locally. No publish/deploy was performed;
the disposable local database was stopped after verification.
