# Dart sync measurements — 2026-10-27

The full instrumented matrix attempted **45 samples: 30 passed, 15 failed**.
It is not a clean baseline for the entire workload. Failed runs remain in the
raw evidence and never enter the passing latency tables. There are no performance
thresholds. [Full workflow](https://github.com/zanminwang/axton/actions/runs/40580232937)
and [tracing-disabled control](https://github.com/zanminwang/axton/actions/runs/40581124113).

## Initial sync

Times are milliseconds, median [minimum, maximum], five independent process/Store
repetitions per size. First visible excludes the initial empty watch callback.

| Existing N | Passed | Store open | First visible | All visible | Bootstrap |
| --- | --- | --- | --- | --- | --- |
| 100 | 5/5 | 5.40 [5.01, 6.70] | 127.29 [103.64, 167.23] | 127.29 [103.64, 167.23] | 127.20 [103.54, 167.11] |
| 1000 | 5/5 | 5.32 [5.11, 5.57] | 255.51 [236.43, 261.91] | 344.95 [322.30, 349.60] | 344.77 [322.07, 349.46] |
| 10000 | 5/5 | 5.63 [5.25, 8.55] | 929.03 [873.26, 1026.35] | 3093.08 [3059.73, 3298.91] | 3091.58 [3057.94, 3297.06] |

These milestones separate configuration, opening and connection work. A connect
Future returning does not establish that the handshake completed. HTTP spans
include server and transport time; no network-only residual is inferred.

| Existing N | App/native configuration | Connect Future returns | HTTP handshake | First HTTP pull | First snapshot rows |
| --- | --- | --- | --- | --- | --- |
| 100 | 0.41 [0.39, 0.43] | 0.27 [0.26, 0.43] | 14.19 [13.72, 18.95] | 90.95 [68.02, 125.98] | 100 |
| 1000 | 0.42 [0.40, 0.45] | 0.30 [0.27, 0.33] | 13.40 [12.82, 13.76] | 163.47 [144.76, 172.63] | 500 |
| 10000 | 0.42 [0.38, 0.44] | 0.27 [0.27, 0.31] | 13.71 [13.56, 14.97] | 838.61 [782.68, 933.53] | 500 |

## Internal stages

Each cell is the median [minimum, maximum] of the **summed completed span duration
per run**, in milliseconds. Nested/overlapping rows cannot be added as percentages
of wall time. Raw records also retain span counts, maxima, parentage and source
clock domains.

| Stage | N=100 | N=1000 | N=10000 |
| --- | --- | --- | --- |
| node/server.transaction | 138.18 [113.84, 184.37] | 228.15 [203.90, 231.92] | 1148.75 [1102.53, 1262.29] |
| rust/delivery.plan | 72.68 [65.13, 92.55] | 147.28 [137.60, 164.87] | 1055.99 [1013.42, 1143.89] |
| rust/delivery.prepare | 8.47 [8.36, 9.78] | 18.18 [16.55, 23.71] | 99.30 [89.56, 104.95] |
| rust/delivery.load | 1.55 [1.31, 1.90] | 9.53 [8.40, 10.79] | 104.97 [96.33, 115.55] |
| rust/delivery.persist | 7.90 [7.61, 8.22] | 24.85 [23.04, 28.46] | 238.25 [213.85, 274.08] |
| rust/delivery.apply | 7.37 [7.26, 7.49] | 67.72 [66.66, 70.08] | 711.88 [709.56, 734.32] |
| rust/sqlite.commit | 5.57 [5.11, 7.89] | 11.11 [10.67, 13.90] | 101.02 [98.02, 119.04] |
| rust/watch.query | 0.35 [0.31, 0.39] | 7.51 [6.49, 8.28] | 622.66 [579.11, 670.43] |
| rust/watch.publish | 0.49 [0.45, 0.53] | 8.08 [7.07, 8.88] | 648.84 [604.15, 700.43] |
| dart/bridge.drain | 1.14 [1.07, 1.19] | 10.22 [8.93, 12.43] | 518.30 [505.15, 549.07] |

## Live sync passing samples

Both clients finish Bootstrap before the measured interval. A awaits each local
commit, then submits the next write without waiting for settlement. A and B share
one Dart Stopwatch. Server transaction duration is measured separately from its
Node entry to confirmation after adapter COMMIT, including retries. A settlement
is the later accepted Call outcome; it is not a server commit timestamp.

| Existing N | Writes M | Passed | Local commit | Server transaction | A settlement | B visible | Reports passed / failed |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 100 | 1 | 5/5 | 1.23 [1.22, 4.11] | 32.67 [32.11, 37.63] | 71.82 [67.86, 98.24] | 113.39 [67.41, 144.76] | 0 / 0 |
| 100 | 100 | 0/5 | — | — | — | — | 0 / 431 |
| 1000 | 1 | 5/5 | 3.00 [1.47, 3.35] | 36.53 [36.02, 37.86] | 73.27 [72.75, 74.66] | 115.04 [109.26, 228.72] | 0 / 0 |
| 1000 | 100 | 0/5 | — | — | — | — | 0 / 451 |
| 10000 | 1 | 5/5 | 1.58 [1.54, 1.66] | 39.29 [38.25, 56.58] | 75.19 [74.27, 202.66] | 210.26 [195.34, 329.01] | 1 / 0 |
| 10000 | 100 | 0/5 | — | — | — | — | 0 / 471 |

Per-write medians/ranges pool observations within cases. Writes within one run
are correlated. Five runs cannot establish an independent-run p95; raw summaries
only report a per-write nearest-rank p95 when there are at least 100 observations.

## Continuous-write failure diagnostics

The following table intentionally includes client-completed **failed** samples,
so their waiting time is visible. It is diagnostic evidence, not a passing baseline.
Final-state verification and backend/connection failures are separate assertions.

| Existing N | Clients completed / runs | Exact server state verified | Local commit per write | A settlement per write | B visible per write | All B visible per run | Backend errors | Connection errors |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 100 | 5/5 | 5/5 | 3.77 [1.23, 15.81] | 9188.84 [161.96, 11390.07] | 5470.77 [84.13, 11951.21] | 10370.62 [8463.56, 12335.01] | 110 | 64 |
| 1000 | 5/5 | 5/5 | 3.97 [1.38, 19.39] | 9073.53 [234.65, 11448.56] | 5193.58 [102.71, 11033.78] | 10667.32 [8799.09, 11490.83] | 83 | 50 |
| 10000 | 5/5 | 5/5 | 4.09 [1.50, 94.10] | 7744.72 [190.92, 15397.53] | 6214.26 [210.35, 15902.91] | 10858.17 [8732.98, 16395.77] | 116 | 66 |

Across the full matrix there were 309 backend errors (SQLSTATE counts
`{"40001":309}`), 180 connection errors
(180 during measurement), and
1354 nonterminal record reports. Raw samples retain their exact messages,
stacks, times and phases. An owned-create `diverged` report is admissible only for
the matching accepted Entry/ordinal during the measured interval, after exact
final data verification; other record reports fail. This admission never excuses
backend errors or HTTP/WebSocket failures.

The derived [server confirmations](server-settlements.json.gz) link each completed
write to its source-clock successful transaction span. Their Node timestamps are
not subtracted from Dart timestamps. They are reconstructed from the original
records without rewriting the raw archive.

## Tracing-disabled control

The separate control used N=100, two repetitions, the same fixture/write timing
boundaries and release/AOT builds, with diagnostics disabled. Its matrix attempted
6 samples, of which 4 passed.
The continuous-write results below are also failed samples, not accepted baseline
statistics. End times are milliseconds from that run's shared Dart start.

| Repetition | Status | Accepted / final rows | Local writes complete | All B visible | All A settled | 40001 errors | Connection errors |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | failed | 100 / 200 | 406.74 | 9628.64 | 10679.04 | 11 | 10 |
| 2 | failed | 100 / 200 | 405.16 | 8657.41 | 7811.77 | 26 | 15 |

This control ran on `AMD EPYC 7763 64-Core Processor` (4 vCPU). Separate shared
VMs and changed tracing mode do not support an instrumentation-overhead percentage.
The reproduced serialization/connection failures demonstrate that enabling trace
collection is not required for this failure mode.

## Findings and next work

1. **Store opening does not explain the seconds-long wait in this fixture.** Its
   median stays near 5–6 ms. At N=10,000 the first 500 Entries appear at 929 ms,
   followed by 20 nonempty snapshots until 3,093 ms. The HTTP handshake remains
   about 14 ms while the first pull grows to 839 ms. The longest delivery-plan
   span has a 775 ms median. Together with the finite-plan implementation, this
   points to preparation before the first page as a first-visible investigation
   target. These observations do not establish network-only latency.
2. **Local processing and full-model notifications also grow substantially.** At
   N=10,000 the per-run cumulative apply, watch-query and bridge-drain medians are
   712, 623 and 518 ms. Watch publication includes query work, and COMMIT can sit
   inside apply; these are overlapping measurements, not separate latency shares.
   The full-model watch intentionally exposes retained-data cost. Profile its
   repeated queries, serialization and Dart decoding before generalizing to an
   application's narrower watch shape. Typed model decoding/listener execution
   is included in user-visible timing but is not separately isolated by the
   bridge-drain span.
3. **Single writes settle quickly; bursts expose a distinct contention problem.**
   All 15 single-write samples passed. B visibility rises from 113 ms at N=100
   to 210 ms at N=10,000. Every continuous-write sample failed despite exact
   final state and accepted outcomes: local submission of all 100 writes finished
   in 0.40–0.78 s, while B needed 8.46–16.40 s to observe the entire burst.
   The 309 recorded backend error stacks locate 258 failures at the publication
   fence and 51 in Store claim/locking. The [persistence adapter](../../../../packages/backend/postgres/src/persistence.mts)
   and [SQL fences](../../../../packages/backend/postgres/src/sql.mts) are concrete
   investigation points. Repeated failed HTTP work and reconnects accompany the
   accumulation; the measurements do not identify an exclusive CPU bottleneck.
4. **Stabilize burst transactions before treating burst latency as a baseline.**
   The disabled-tracing control reproduced the same failure class. Next work
   should examine overlapping push, pull and materialization transactions, Store
   claims and the publication fence, preserving Serializable/settlement guarantees.
   No retry limits, isolation levels, pacing or sync algorithms were changed to
   obtain passing numbers. The initial-sync scenario is a usable first reference;
   the full workload needs a subsequent clean baseline after this issue is fixed.
   JS parity and mobile/network variants remain follow-up work.

## Conditions and evidence

| Condition | Value |
| --- | --- |
| Measured commit | 8e972685a67dcd053b999e5a73aef6643b66dfce |
| Dirty | false |
| Machine | AMD EPYC 7763 64-Core Processor; 4 vCPU; 15.61 GiB RAM |
| OS | linux 6.17.0-1022-azure x64 |
| Node / Dart | v26.4.0; Dart SDK version: 3.12.1 (stable) (Tue May 26 01:02:21 2026 -0700) on "linux_x64" |
| Rust / Cargo | rustc 1.98.1 (48a229cea 2026-09-01); cargo 1.98.1 (797e8a9bc 2026-08-05) |
| TypeScript | Version 5.8.3 |
| PostgreSQL | PostgreSQL 16.15 (Ubuntu 16.15-1.pgdg24.04+2) on x86_64-pc-linux-gnu, compiled by gcc (Ubuntu 13.3.0-6ubuntu2~24.04.1) 13.3.0, 64-bit |
| Native / Dart mode | release / AOT executable |
| Fixture | 512-byte ASCII text; fixed-width identity; 100-Entry publication groups |
| Caches | Fresh processes, SQLite files and PostgreSQL databases; fixtures just written; OS/database caches not flushed |
| Network / watch | Loopback HTTP/WebSocket; full Entry model snapshots |
| Diagnostics | Enabled for the full matrix; disabled for the separate control |

PostgreSQL durability/settings are retained in raw JSON. This host-loopback,
text-only benchmark does not measure a phone, Flutter layout, WAN, media or Most
Days authorization. Tracing overhead is included in the main measurements.
Compilation, backend startup, database/fixture preparation and final verification
are excluded from timed intervals. Source/artifact fingerprints identify exactly
what ran; later report/cleanup changes do not relabel that measured commit.

| Artifact | SHA-256 |
| --- | --- |
| bindings/node/axton-node.node | 84f88fa58be10d3076b7d390643f3381480b7af0690606e0eff4bdd81c80093f |
| target/release/libaxton_dart.so | 999dcc62d2c8d9f8620fbe9c3991f791b58854cbf5b7c8cc8a2b9a3402138ddf |
| artifacts/performance/initial-sync | 26cc3babf0d997014a1141f92dc2e666aee015e217846f43c517a2d7f7032f32 |
| artifacts/performance/live-sync | 62726d7bd398b9e3ad9fe1d8b466d54166a8328f9996b8c12c9d2287f823ea83 |

[Evidence index](evidence.json) retains workflow links, commits and SHA-256 of the
uncompressed originals. Archives are lossless. The earlier trial archives preserve
the PostgreSQL physical-socket cleanup failure, v1 nonterminal-report failure,
and first continuous-write failure. They are not pooled with the full matrix.

## Reproduce and inspect

From the repository root:

```sh
bash integration/performance/run.sh
node integration/performance/report.mjs integration/performance/baselines/2026-10-27-dart/full-matrix-8e97268.json.gz
node integration/performance/report.mjs compare before.json after.json --scenario initial-sync
```

Default comparisons reject any failed matrix. Explicit scenario selection requires
two fully attempted matrices and every repetition in that scenario to pass, plus
matching environment, fixture and timing conditions. It never drops failed
repetitions. See the [runner guide](../../README.md) for manual workflow controls,
cleanup, receiver integration and raw timing boundaries.

## Validation limits

Local validation passed: 513 Rust tests (7 ignored), workspace Clippy/format,
backend TypeScript checks, 70 focused Dart tests, and 12 Node diagnostics/host/report
tests. Both performance programs analyzed and AOT-compiled. The real PostgreSQL
measurements ran in the workflows linked above: the local sandbox prevents
PostgreSQL shared-memory startup. PR checks report the broader host E2E gate.
Simulation was unchanged. This PR adds measurement infrastructure, not a fix for
the observed synchronization contention, and does not introduce a latency gate.

All 45 instrumented samples completed their client-side and PostgreSQL identity,
count and content checks; all 1,515 Mutations were accepted. There were no retained
trace losses, database/client cleanup errors or admin-pool errors. This establishes
eventual final-state correctness for these runs, not error-free live delivery.
The real-data comparison smoke test passed for the explicitly selected initial
scenario; the default comparison correctly rejected the failed full matrix.
