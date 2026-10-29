# Grouped Store admission and outcome persistence

One Store claim/lock is now shared by a commit group. Rust keeps provisional
progress in memory while each member still authorizes, executes sequentially
and owns its business savepoint and publication cursor. New outcomes are saved
as one contiguous suffix with one guarded progress update before group COMMIT.
Replayed saved outcomes remain unchanged; a retry can mix a saved prefix with
new members after a group-size change. PostgreSQL advertises the new
`mutationResultGroups` capability. The SDK translates `saveResults` to sequential
`saveResult` calls for older group-capable adapters. Size 1 remains direct.

## Short controlled comparison

Four fresh N=10,000/M=100 live samples ran before/after/after/before. The control
is the exact pre-change release Node addon, whose hash also appears in the
[previous current-default snapshot](../2026-10-28-performance-summary/README.md).
Only the Node addon changed between conditions. Updated SDK/adapter sources were
identical in both conditions: the old addon uses the unchanged single-result
operation, while the optimized addon invokes the new bulk operation. Dart native
and AOT artifacts, group size 8, prefetch on, encoders and SQLite statement cache
were held constant. Both conditions produced 14 commit groups per run.

Median [minimum, maximum], two runs per condition; times in milliseconds:

| Metric | Before | After |
| --- | ---: | ---: |
| All 100 settled on A | 514.662 [506.361, 522.963] | 497.291 [493.701, 500.880] |
| All 100 visible on B | 510.883 [503.272, 518.494] | 494.580 [491.415, 497.744] |
| Framework SQL, all backend work | 1543.000 [1543.000, 1543.000] | 1371.000 [1371.000, 1371.000] |
| Framework SQL, Mutation transactions | 1303.000 [1303.000, 1303.000] | 1131.000 [1131.000, 1131.000] |
| Store claims | 100.000 [100.000, 100.000] | 14.000 [14.000, 14.000] |
| Result writes | 100.000 [100.000, 100.000] | 14.000 [14.000, 14.000] |
| Mutation framework SQL elapsed ms | 380.750 [375.008, 386.491] | 365.455 [362.448, 368.462] |

All-A completion improved 3.4% (17.372 ms), and all-B completion 3.2%
(16.304 ms). Both adjacent pairs favored the optimized implementation.
Total framework SQL count fell 11.1%, and Mutation-transaction SQL count fell
13.2%. Removing 172 calls saves much less time proportionally: the removed
Store/result statements were relatively cheap, and most SQL work remains.
This is a small directional experiment, not a production speedup guarantee.
Do not combine its control with a different earlier run's latency.

Group boundaries and frozen HTTP Batch membership were unchanged. This does not
batch business Handler SQL, publication guards, Stream writes or target reads.
The group still executes all 100 logical Mutations in order. Framework SQL
counts exclude direct business Handler/Loader SQL; SQL elapsed spans include
execution and wait. Group/SQL/member spans overlap and must not be added.
`mutation.execute` now starts after the shared Store claim and excludes final
bulk persistence; compare outer `server.processBatchGroup` or end-to-end time,
not old/new per-member span sums. Burst per-write observations share a run and
do not establish an independent-run p95.

All four samples verified all 100 accepted results, final client/server identities
and content, drained local work, no backend/transport/cleanup errors, zero trace
loss and complete aligned timelines. The audit checks the frozen runner hash,
source identity, unchanged client artifacts, control/addon hashes, 14 groups,
exact claim/write counts and the 1,543/1,371 framework query budgets:

```sh
node integration/performance/baselines/2026-10-28-group-sql/audit.mjs
```

Raw reports are the four `*.json.gz` files; `evidence.json` is derived by the
audit. `runner.mjs` freezes the exact one-sample local runner and contains absolute
workspace imports to relocate elsewhere. Select the captured old or optimized
Node addon between fresh runner processes, set `AXTON_GROUP_SQL_VARIANT=before`
or `after`, run `run.mjs --record-build`, and retain output with `--output`.
`runtime.patch` is a partial dirty-worktree snapshot relative to parent `294f356`,
including preceding group/SQL changes; it is not a standalone clean-parent patch.
Source fingerprints describe the shared current sources, while the captured
control binary retains its pre-optimization implementation.

Apple M5 Max, macOS arm64, PostgreSQL 16.14, Node 24.19.0, Dart 3.12.1, Rust 1.98.1.
Fresh database, client process and SQLite files per run. Fixture setup, process
startup and compilation are excluded; OS/database caches are not flushed.
This is desktop loopback SDK visibility, not UI, mobile, WAN or isolated
single-Mutation latency. Clients issue 100 continuous calls while awaiting each
local commit, without waiting for settlement between calls.

## Executed verification

- All 30 native/PostgreSQL Batch tests passed, including refusal, group rollback,
  exact replay, concurrent duplicate, retained prefix, mixed saved/fresh retry,
  group-size changes, separate cursors and older-adapter result-write fallback.
- All 11 real PostgreSQL round-trip tests passed, including three new grouped
  tests for one-statement writes/promotion, digest/progress guard refusal and
  atomic rollback after a duplicate-result insertion failure.
- All three pg/Prisma/Drizzle Batch tests passed. Rust protocol/server tests
  passed (42); default/host/performance-harness tests passed (27).
- Strict server/protocol Rust clippy, TypeScript checking, formatting and
  whitespace checks passed. No mobile/full-host gate was run.

The optimized addon is retained locally; no publish/deploy was performed.
The disposable local database was stopped after verification.
