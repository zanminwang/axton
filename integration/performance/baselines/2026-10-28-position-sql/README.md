# Singleton target-position lookup

After tracking optimization, the largest measured SQL categories were member
publication (about 85 ms/100 Mutations) and target-position reads (about 59 ms).
This experiment chooses the smaller, read-only position operation because its
single-key JSON-array join has avoidable planning/scanning work. Publication
writes and all locking/savepoint/recheck rules remain unchanged.

`targetPositions` and `readPositions` now use exact model/identity/Stream index
predicates for a singleton batch. A one-row input with LEFT joins retains one
nullable slot for absent metadata or an untracked pair. Target reads return
null; strict reads still refuse missing positions. Remove cursors retain their
kind. Multi-row batches retain the existing ordered query, and a singleton final
batch preserves positional cardinality and duplicate inputs. No result cache,
new index, migration, native or client change was introduced.

`plans.json` retains a separate synthetic 10k-metadata EXPLAIN ANALYZE/BUFFERS
profile: the general query's execution was 0.588 ms, versus 0.008 ms for direct
predicates. This one invocation diagnoses plan shape; it is not the application's
round-trip timing or a production speedup claim.

## Short controlled comparison

Four fresh N=10,000/M=100 live runs executed before/after/after/before. All native
libraries and AOT executables were identical. The control is the frozen adapter
and SQL under `control/`; the runner records their hashes and the audit checks
them. Both variants retain group size 8, prefetch, canonical encoding, SQLite
caching, grouped results, combined publication and indexed tracking.

Median [minimum, maximum], two runs per condition; times in milliseconds:

| Metric | Before | After |
| --- | ---: | ---: |
| Target-position SQL calls | 100.000 [100.000, 100.000] | 100.000 [100.000, 100.000] |
| Target-position SQL elapsed ms | 57.507 [55.549, 59.466] | 14.142 [12.205, 16.080] |
| All 100 settled on A | 351.151 [335.334, 366.968] | 327.452 [289.037, 365.867] |
| All 100 visible on B | 346.712 [330.226, 363.197] | 323.675 [285.300, 362.050] |

Target-query time fell 75.4%, saving 43.344 and 43.386 ms in the adjacent pairs.
All-A completion's median fell 6.7% (23.699 ms), but its pairwise savings were
46.297 and only 1.101 ms. Both pairs favored the new query; end-to-end gains were
variable. The optimized runs span 289–366 ms and must not be reported as a stable
289 ms result. Recorded load averages rose during the run; this observation does
not establish the cause of the slower second optimized sample. Its other SQL
and group spans also grew. Do not combine these controls with earlier archives.

Target SQL remains 100 calls, tracking remains 200, and each sample retains the
actual `readTracking -> lockStreams -> guardRecords -> readTracking` sequence for
all 100 Mutations. Publication remains 200 SQL calls and is the largest remaining
SQL category (optimized median 86.517 ms). Group count and subscriber work vary
with live HTTP freezing; the audit checks the resulting query-budget formula.
Two repetitions provide directional local evidence, not a production guarantee
or an independent-run tail estimate. Burst per-write values are correlated.

## Verification and reproduction

All four samples verified 100 accepted outcomes, complete client/server identity
and content, drained local work, no transport/backend/cleanup errors, no trace
loss and aligned timelines. Raw reports are the four `*.json.gz` files;
`evidence.json` is derived by the audit:

```sh
node integration/performance/baselines/2026-10-28-position-sql/audit.mjs
```

`runner.mjs` freezes the exact local runner and includes absolute workspace
imports to relocate elsewhere. Copy `control/persistence.mts` and `control/sql.mts`
to `artifacts/performance/positions-control/`, select
`AXTON_POSITION_VARIANT=before` or `after` in fresh runner processes, record build
identity with `run.mjs --record-build`, and retain each output. `runtime.patch`
is a partial dirty-worktree snapshot relative to parent `294f356`, including
preceding persistence changes, not a standalone clean-parent patch.

Executed checks: all 16 PostgreSQL batching tests passed, including live/remove,
missing/untracked, strict-read refusal, duplicate input and a singleton final
batch after 1,000 records. All 30 native Batch, four publication semantics and
three pg/Prisma/Drizzle Batch tests passed. All 20 performance-harness tests,
TypeScript build/check, formatting and whitespace checks passed. Rust/native/
client sources were unchanged; no mobile or full-host gate was run.

Apple M5 Max, macOS arm64, PostgreSQL 16.14, Node 24.19.0, Dart 3.12.1, Rust 1.98.1.
Fresh PostgreSQL database, client process and SQLite files per sample. Fixture
setup, process startup and compilation excluded; caches not flushed. This is
local SDK watch visibility, not UI rendering, mobile or WAN latency. The clients
issue 100 continuous calls, awaiting each local commit but not settlement.
The optimized adapter is retained locally. No publish/deploy was performed;
the disposable local database was stopped after verification.
