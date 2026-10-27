# Global publication fence removal — 2026-10-27

The requested removal experiment ran the same 45-case matrix at commit
`b5663850d67b5f51d132b22f7668e679d41ffdcc` (dirty: `false`).
**30/45 samples passed.**
[Workflow](https://github.com/zanminwang/axton/actions/runs/40589695896),
[lossless raw samples](results.json.gz), [evidence index](evidence.json), and
[original baseline](../2026-10-27-dart/README.md).

## Implementation

Native Mutation, Bootstrap, delivery, materialization and Query-tracking paths
no longer request a namespace-wide publication fence. The backend no longer
acquires it before callbacks/preparation. Fresh DDL installs seven canonical
tables and omits the fence table; existing tables are left unused, not dropped.
The old bridge operation and `acquirePublicationFence()` remain compatibility
no-ops. No replacement global admission lock was added. Serializable isolation,
Store/record/Stream guards, atomic outcomes and the default three driver retries
are unchanged. Traces recorded **0** publication-fence callback spans.

## Case outcomes

Counts below include every attempted sample. Final data and accepted settlement
are independent checks; transport/backend errors continue to fail a sample.

| Existing N | Writes M | Before passed | After passed | Backend errors | Connection errors | Record reports |
| --- | --- | --- | --- | --- | --- | --- |
| 100 | 1 | 5/5 | 5/5 | 0 → 0 | 0 → 0 | 0 → 0 |
| 100 | 100 | 0/5 | 0/5 | 110 → 66 | 64 → 49 | 431 → 473 |
| 1000 | 1 | 5/5 | 5/5 | 0 → 0 | 0 → 0 | 0 → 0 |
| 1000 | 100 | 0/5 | 0/5 | 83 → 69 | 50 → 29 | 451 → 465 |
| 10000 | 1 | 5/5 | 5/5 | 0 → 0 | 0 → 0 | 1 → 0 |
| 10000 | 100 | 0/5 | 0/5 | 116 → 93 | 66 → 105 | 471 → 495 |

## Continuous-write timings

All cells are **before → after**, seconds, median [minimum, maximum] across
client-completed runs. The original continuous-write runs failed despite eventual
correctness; their timings are diagnostic observations, not a passing baseline.
Any failed after-runs are included here too, not silently discarded. Raw failures
and the table above determine whether a case is a clean baseline.

| Existing N | All local writes | All B visible | All A settled |
| --- | --- | --- | --- |
| 100 | 0.42 [0.40, 0.42] → 0.44 [0.40, 0.48] | 10.37 [8.46, 12.34] → 4.91 [4.16, 6.42] | 9.84 [9.26, 11.77] → 5.83 [5.05, 6.57] |
| 1000 | 0.43 [0.42, 0.46] → 0.45 [0.44, 0.48] | 10.67 [8.80, 11.49] → 6.05 [5.71, 6.23] | 10.69 [9.91, 11.80] → 6.76 [6.12, 7.25] |
| 10000 | 0.67 [0.51, 0.78] → 0.81 [0.55, 0.88] | 10.86 [8.73, 16.40] → 7.66 [7.18, 8.84] | 11.17 [9.70, 15.69] → 8.54 [7.41, 9.71] |

## Initial-sync timings

Before → after, milliseconds, median [minimum, maximum]. Store open is a separate
duration; other milestones start at initialization. Empty watch callbacks do not
count as first visibility.

| Existing N | Store open | First visible | All visible | Bootstrap |
| --- | --- | --- | --- | --- |
| 100 | 5.40 [5.01, 6.70] → 6.48 [5.13, 7.74] | 127.29 [103.64, 167.23] → 86.66 [79.90, 120.61] | 127.29 [103.64, 167.23] → 86.66 [79.90, 120.61] | 127.20 [103.54, 167.11] → 86.52 [79.80, 120.52] |
| 1000 | 5.32 [5.11, 5.57] → 5.19 [5.00, 5.42] | 255.51 [236.43, 261.91] → 295.01 [203.10, 311.07] | 344.95 [322.30, 349.60] → 376.73 [288.46, 401.88] | 344.77 [322.07, 349.46] → 376.52 [288.19, 401.71] |
| 10000 | 5.63 [5.25, 8.55] → 5.17 [5.03, 5.21] | 929.03 [873.26, 1026.35] → 1491.81 [862.19, 1569.14] | 3093.08 [3059.73, 3298.91] → 3676.34 [3091.07, 3779.82] | 3091.58 [3057.94, 3297.06] → 3674.49 [3089.54, 3778.32] |

## Verification and limits

All 45 cases were attempted; 45
clients completed exact local/watch validation, 45 passed PostgreSQL
identity/count/content verification, and 1515 writes were accepted.
The after-run retained 228 backend errors,
183 connection errors and 1433
nonterminal record reports. Expected owned-create reports retain the same v2
validation rules; backend/connection errors are never excused.

Removing the global fence improves the observed burst end times in this run,
but **does not produce a clean continuous-write baseline**: all 15 burst samples
still fail error criteria. Backend errors fall from 309 to 228; connection errors
are 180 before and 183 after, with the N=10,000 case increasing substantially.
All final-state checks still pass. The 228 backend failures are SQLSTATE `40001`:
123 report concurrent updates and 105 report read/write dependencies.

The remaining stacks identify 123 failures in Store claim/locking, 104 in delivery
plan insertion/expiry cleanup, and one at COMMIT. These are the next investigation
targets, not reasons to remove Store ordering, identity guards or Stream cursor
protection. Delivery's binding path currently uses the same `claimStore`/`FOR
UPDATE` operation that Mutation progress uses. `saveDelivery` runs namespace-wide
expiry cleanup before inserting a plan; whether its predicate reads cause avoidable
Serializable conflicts needs a separate controlled experiment.

Initial sync does not show a uniform improvement. N=10,000 first-visible median
increases from 929 to 1,492 ms, and all-visible from 3,093 to 3,676 ms. Every one
of its five after-runs has a failed `saveDelivery` attempt recovered by the
existing transaction retry loop. When the HTTP planning path retries, the first
HTTP pull lasts about 1.4–1.5 seconds; when the competing live planning path
retries, the first pull is about 0.76–0.84 seconds. These source-observed retries
are visible even though the overall initial samples pass.

| N=10,000 repetition | Retried native path | First HTTP pull ms | First visible ms |
| --- | --- | --- | --- |
| 1 | Live | 841.61 | 943.99 |
| 2 | HTTP delivery | 1,478.51 | 1,569.14 |
| 3 | Live | 757.87 | 862.19 |
| 4 | HTTP delivery | 1,416.81 | 1,505.41 |
| 5 | HTTP delivery | 1,403.81 | 1,491.81 |

No trace loss, cleanup failure or admin-pool error was recorded. Each of the
1,515 accepted writes has a correlated source-clock server commit confirmation.
The measured commit's complete Linux and macOS Verify jobs passed, including the
new unrelated-publication liveness test, existing tracking/invalidation races,
real PostgreSQL adapters, native E2E and optimized artifacts. This is bounded
test evidence, not a proof of every possible concurrent schedule.

Both matrices use five repetitions, N=100/1,000/10,000, live M=1/100, 512-byte
text, full Entry watches, release native builds, Dart AOT and loopback. Stores,
processes and databases are fresh; compilation and fixture work are untimed; OS
and database caches are not flushed. Samples run serially. Separate shared GitHub
VMs are not dedicated hardware; environment differences and ranges matter.
No phone/WAN or production latency claim follows from these measurements.

| Condition | Before | After |
| --- | --- | --- |
| cpu | AMD EPYC 7763 64-Core Processor | AMD EPYC 7763 64-Core Processor |
| cpus | 4 | 4 |
| memoryBytes | 16766414848 | 16766410752 |
| release | 6.17.0-1022-azure | 6.17.0-1022-azure |
| node | v26.4.0 | v26.4.0 |
| dart | Dart SDK version: 3.12.1 (stable) (Tue May 26 01:02:21 2026 -0700) on "linux_x64" | Dart SDK version: 3.12.1 (stable) (Tue May 26 01:02:21 2026 -0700) on "linux_x64" |
| rustc | rustc 1.98.1 (48a229cea 2026-09-01) | rustc 1.98.1 (48a229cea 2026-09-01) |
| postgres | PostgreSQL 16.15 (Ubuntu 16.15-1.pgdg24.04+2) on x86_64-pc-linux-gnu, compiled by gcc (Ubuntu 13.3.0-6ubuntu2~24.04.1) 13.3.0, 64-bit | PostgreSQL 16.15 (Ubuntu 16.15-1.pgdg24.04+2) on x86_64-pc-linux-gnu, compiled by gcc (Ubuntu 13.3.0-6ubuntu2~24.04.1) 13.3.0, 64-bit |

Default baseline comparison rejects failed matrices. Explicit initial-scenario
comparison is allowed only when all initial repetitions pass and the recorded
environment/fixture/timing conditions match. These two VMs report a 4,096-byte
difference in total RAM, so the strict automatic environment check also rejects
an initial-only comparison. The tables are descriptive comparisons of the two
recorded runs, not an automatically admitted performance-regression verdict.
This report retains failed-run data so correctness and error changes remain visible.
