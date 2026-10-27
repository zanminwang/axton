# Store snapshot-read experiment — 2026-10-27

The same 45-case matrix ran at `cc02af94dc0792d69a325cdf12009cb82fa52a07` (dirty: `false`).
**30/45 samples passed.**
[Workflow](https://github.com/zanminwang/axton/actions/runs/40595291369), [lossless raw samples](results.json.gz),
[evidence index](evidence.json), and [previous fence-removal experiment](../2026-10-27-fence-removal/README.md).

## Change and read-path audit

Initialized handshake, HTTP/live delta, continuation and owned materialization
inspect immutable Store ownership without claiming Mutation progress. Missing
Stores and unfinished Bootstrap retain their atomic claim. Uploads retain
mutual exclusion with `FOR NO KEY UPDATE`; plan foreign-key `KEY SHARE` checks
can proceed while an upload is paused. Identity and owner/Stream validation,
Serializable isolation and the existing retry budget remain unchanged. Plan
insertion and expiry cleanup are unchanged for this isolated experiment.

The [read concurrency audit](../../../../docs/engineering/architecture/server/read-concurrency.md)
also identifies local runtime reads queued behind open application callbacks,
explicit tracking/preparation writes, and repeated read-only layout checks.
Local SQLite readers do not reserve the writer; public runtime scheduling still
waits for a callback transaction. That limitation is separate from measured
server serialization failures and is not changed here.

## Case outcomes

Every attempted sample is included. Final data correctness does not excuse a
backend/connection error; failed samples remain failures.

| Existing N | Writes M | Before passed | After passed | Backend errors | Connection errors |
| --- | --- | --- | --- | --- | --- |
| 100 | 1 | 5/5 | 5/5 | 0 → 0 | 0 → 0 |
| 100 | 100 | 0/5 | 0/5 | 66 → 91 | 49 → 62 |
| 1000 | 1 | 5/5 | 5/5 | 0 → 0 | 0 → 0 |
| 1000 | 100 | 0/5 | 0/5 | 69 → 66 | 29 → 61 |
| 10000 | 1 | 5/5 | 5/5 | 0 → 0 | 0 → 0 |
| 10000 | 100 | 0/5 | 0/5 | 93 → 113 | 105 → 100 |

## Continuous writes

Before → after, seconds, median [minimum, maximum] across all client-completed
runs, including runs that fail error criteria. These are observations, not a
passing baseline when the case fails. Five independent runs per cell; per-write
observations within a burst are correlated.

| Existing N | All local writes | All B visible | All A settled |
| --- | --- | --- | --- |
| 100 | 0.44 [0.40, 0.48] → 0.52 [0.45, 0.53] | 4.91 [4.16, 6.42] → 4.78 [4.49, 5.31] | 5.83 [5.05, 6.57] → 5.07 [4.45, 6.23] |
| 1000 | 0.45 [0.44, 0.48] → 0.50 [0.45, 0.56] | 6.05 [5.71, 6.23] → 5.60 [5.12, 6.42] | 6.76 [6.12, 7.25] → 5.97 [5.26, 6.20] |
| 10000 | 0.81 [0.55, 0.88] → 0.74 [0.63, 1.11] | 7.66 [7.18, 8.84] → 7.58 [5.94, 8.40] | 8.54 [7.41, 9.71] → 7.43 [6.54, 8.39] |

## Initial sync

Before → after, milliseconds, median [minimum, maximum]. Store open is a separate
duration; other milestones start before open. Empty watch snapshots do not count.

| Existing N | Store open | First visible | All visible | Bootstrap |
| --- | --- | --- | --- | --- |
| 100 | 6.48 [5.13, 7.74] → 5.21 [4.99, 7.31] | 86.66 [79.90, 120.61] → 117.00 [114.10, 130.18] | 86.66 [79.90, 120.61] → 117.00 [114.10, 130.18] | 86.52 [79.80, 120.52] → 116.90 [113.93, 130.07] |
| 1000 | 5.19 [5.00, 5.42] → 5.03 [4.96, 5.32] | 295.01 [203.10, 311.07] → 278.08 [269.19, 298.12] | 376.73 [288.46, 401.88] → 360.69 [350.79, 379.44] | 376.52 [288.19, 401.71] → 360.54 [350.61, 379.27] |
| 10000 | 5.17 [5.03, 5.21] → 5.14 [4.92, 5.27] | 1491.81 [862.19, 1569.14] → 1453.33 [1409.19, 1646.72] | 3676.34 [3091.07, 3779.82] → 3573.14 [3544.91, 3783.50] | 3674.49 [3089.54, 3778.32] → 3571.58 [3543.28, 3782.03] |

## Evidence and findings

The read/write liveness change is narrower than a contention fix. Error-site
counts below are exhausted backend failures; source error spans additionally
include attempts recovered by the unchanged retry loop.

| Failure site | Fence removed | Store snapshot reads |
| --- | ---: | ---: |
| Store progress SELECT | 114 | 92 |
| Store claim INSERT | 9 | 7 |
| Plan INSERT | 92 | 119 |
| Shared expiry DELETE | 12 | 43 |
| Plan-unit INSERT | 0 | 3 |
| COMMIT | 1 | 6 |
| Total backend errors | 228 | 270 |
| Connection errors | 183 | 223 |

All 444 failed `host.claimStore` source attempts in this run correlate with
`server.processBatchMember`, rather than delivery/live/materialization operations.
This does not mean Store failures vanished: 99 exhausted failures still occur
in Store claim/progress statements on the upload path. Writer ordering and
idempotent replay remain necessary. Removing the read-side lock does not
eliminate Serializable dependencies from reading the same tuple.

In every burst sample, Node-clock `server.processBatchMember` spans from
distinct operation IDs overlap: the maximum concurrent count is two or three.
Only A writes in this scenario, and the backend iterates each request's members
sequentially. Separately, A's Dart-clock HTTP push intervals do not overlap in
any of the 15 samples. These are same-clock interval checks in each domain, not
cross-clock subtraction. Together they point to overlapping server work across
retried/retired requests; exact HTTP cancellation and request-to-attempt
correlation remain unmeasured. They do not justify removing the guard between
competing uploads. The evidence index preserves per-sample overlap counts.

Plan persistence accounts for 165 exhausted failures, up from 104. The total
backend count rises, so this run does **not** establish improved reliability or a
clean continuous-write baseline. All 15 burst samples still fail; all initial
and single-write samples pass. All 270 exhausted backend errors are SQLSTATE
`40001`: 99 concurrent-update errors and 171 read/write-dependency errors.
The shared expiry DELETE remains an avoidable-write investigation target, but
this experiment does not isolate that statement's causal contribution from the
other plan writes or concurrent uploads.

Burst end times are broadly similar within the observed ranges. B's all-visible
median is 4.78 / 5.60 / 7.58 seconds for N=100 / 1,000 / 10,000, versus 4.91 /
6.05 / 7.66 before. No latency win is inferred from five runs on separate shared
VMs. Single-write B-visible medians are 153 / 113 / 200 ms; the corresponding
server transaction medians are 29 / 34 / 33 ms in the server's own clock domain.
These clocks are not subtracted to invent a transport-only duration.

For N=10,000 initial sync, Store open remains about 5 ms, first visibility
1,453 ms, and all visibility 3,573 ms. Every one of its five runs has an HTTP
delivery-plan attempt fail and recover by retry. The source traces therefore
still show repeated plan work on the first-visible path. Selected **per-run
summed span medians** are below. Spans nest/overlap and include repeated work;
they are not disjoint latency shares and must not be added together.

| N=10,000 initial stage | Sum median ms |
| --- | ---: |
| Server transaction | 1,677.66 |
| Delivery plan | 1,606.11 |
| Preparation | 183.58 |
| Canonical load | 208.50 |
| Plan persistence | 351.53 |
| Local delivery apply | 713.56 |
| Watch query | 583.54 |
| Watch publication | 607.23 |
| Dart bridge drain/decode | 514.23 |
| SQLite COMMIT | 96.91 |

Each of the 1,515 accepted writes has a correlated source-clock server commit
confirmation. Record-report validation admits only transient divergence of an
exact verified accepted own create; unexpected reports, backend errors and
connection errors are not excused.

## Code validation

Complete Linux and macOS Verify jobs passed at `bb871fb5dc799137b525fee28cc921269a79f86a`:
[Linux job](https://github.com/zanminwang/axton/actions/runs/40596029448/job/122512039185)
and [macOS job](https://github.com/zanminwang/axton/actions/runs/40596035590/job/122512241816).
They include actual PostgreSQL adapter tests, all five read-liveness cases with
same-Store writer/replay checks, existing tracking/invalidation races, native E2E
and optimized artifacts. Local affected checks passed: 40 Rust tests, 18 Node
tests, Clippy/format/TypeScript and the existing focused callback-transaction
scheduling regression. These are bounded test evidence, not a proof of every
concurrent schedule.

The measured commit precedes a retained-context correction to the materialization
test fixture. Only that fixture and audit documentation changed between
`cc02af94` and `bb871fb5`; production, native and benchmark runtime sources are
identical. Original failed fixture checks are visible in the workflow history.
Actual PostgreSQL validation and measurements ran remotely because the local
sandbox refuses PostgreSQL shared-memory startup. Local reader scheduling was
verified with real SQLite; no phone or WAN measurements were performed.

All 45 samples were attempted. 45 clients completed
exact watch/local-data validation, 45 passed server
identity/count/content checks, and 1515 writes were accepted.
There were 270 backend errors, 223
connection errors and 1455 nonterminal record reports.
Trace loss: 0; cleanup errors: 0;
admin errors: 0. Raw records retain each failure.

## Conditions and limits

Both runs use release native artifacts and Dart AOT, 512-byte text, full Entry
watches, N=100/1,000/10,000 and M=1/100, serial scenes and five repetitions.
Each sample starts fresh processes, SQLite files and a PostgreSQL database.
Compilation, database/backend setup and fixtures are untimed. Fixtures are just
written; OS/database caches are not flushed. Loopback shared GitHub VMs do not
establish phone, production, WAN or Most Days application latency. Source clocks
are never subtracted across processes; nested stage spans are not additive.

| Condition | Before | After |
| --- | --- | --- |
| cpu | AMD EPYC 7763 64-Core Processor | AMD EPYC 7763 64-Core Processor |
| cpus | 4 | 4 |
| memoryBytes | 16766410752 | 16766414848 |
| release | 6.17.0-1022-azure | 6.17.0-1022-azure |
| node | v26.4.0 | v26.4.0 |
| dart | Dart SDK version: 3.12.1 (stable) (Tue May 26 01:02:21 2026 -0700) on "linux_x64" | Dart SDK version: 3.12.1 (stable) (Tue May 26 01:02:21 2026 -0700) on "linux_x64" |
| rustc | rustc 1.98.1 (48a229cea 2026-09-01) | rustc 1.98.1 (48a229cea 2026-09-01) |
| postgres | PostgreSQL 16.15 (Ubuntu 16.15-1.pgdg24.04+2) on x86_64-pc-linux-gnu, compiled by gcc (Ubuntu 13.3.0-6ubuntu2~24.04.1) 13.3.0, 64-bit | PostgreSQL 16.15 (Ubuntu 16.15-1.pgdg24.04+2) on x86_64-pc-linux-gnu, compiled by gcc (Ubuntu 13.3.0-6ubuntu2~24.04.1) 13.3.0, 64-bit |

Automatic comparison rejects incomplete/failed selected scenarios and requires
exact fixture, boundary and environment compatibility. These tables preserve
descriptive observations; any environment difference is not silently normalized.
The strict initial-only comparison rejects these VMs' 4,096-byte reported RAM
difference. The full comparison also rejects the failed burst cases.
