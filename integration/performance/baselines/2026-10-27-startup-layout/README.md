# Startup layout validation — 2026-10-27

The release/Dart AOT matrix ran at `cf0e68c8e78e5f5a65b8aedde6f634820034ccf3` (dirty: false).
**45/45 samples passed**, five serial repetitions per case.
[Workflow](https://github.com/zanminwang/axton/actions/runs/40627204284), [raw samples](results.json.gz),
[evidence and comparison](evidence.json), [previous raw samples](../2026-10-27-live-coalescing-v2/results.json.gz).

## Change and conditions

PostgreSQL adapter layout validation moves from every ordinary host operation
to shared startup readiness. The backend waits before binding its port; direct
operations retain first-use protection. Standalone low-level answer remains
guarded. Mutation transaction policy, queue bounds and Model-watch invalidation
are unchanged. Startup/fixtures/compilation remain outside timing.

The strict comparison refused these runs: different machine/tool/database conditions. Timing arrows below are descriptive observations, not a controlled percentage improvement. Full metadata and refusal evidence are in evidence.json. A separate strict comparison with the first coalescing archive (ecf5a21a2d661cf38bf9ab8bf61a60c61f9b8067) did pass. That longer comparison includes both live-prefix progress during HTTP waits and startup layout validation; it does not isolate either change. Its complete admitted values are preserved in evidence.json.

Both runs use 512-byte text, fresh Stores/processes/databases, loopback,
release native artifacts and Dart AOT. OS/PostgreSQL caches are not flushed.
This is shared Actions hardware, not phone/WAN performance. Arrows are before →
after; cells show median [min, max]. Per-write observations are correlated within
a run. Five repetitions do not support an independent-run p95.

## Continuous writes

Seconds; whole-run completion for all 100 writes.

| Existing N | All A local commits | All A accepted locally | All B visible |
| --- | --- | --- | --- |
| 100 | 0.62 [0.37, 0.88] → 0.49 [0.46, 0.52] | 3.91 [3.64, 6.70] → 1.88 [1.84, 1.91] | 3.91 [3.64, 6.70] → 1.86 [1.82, 1.89] |
| 1000 | 0.38 [0.36, 1.22] → 0.59 [0.58, 0.62] | 4.28 [4.22, 7.15] → 2.61 [2.53, 2.64] | 4.31 [4.22, 7.16] → 2.60 [2.52, 2.62] |
| 10000 | 0.88 [0.60, 1.44] → 0.99 [0.83, 1.09] | 5.48 [5.02, 6.25] → 3.39 [3.18, 3.44] | 5.61 [5.13, 6.38] → 3.47 [3.31, 3.59] |

## Single creates

Milliseconds from each write start, except server transaction uses only the
server clock and includes COMMIT. A settlement waits for local accepted outcome;
it is not a server timestamp.

| Existing N | A local commit | Server transaction | A local settlement | B visible |
| --- | --- | --- | --- | --- |
| 100 | 1.21 [1.08, 1.30] → 1.29 [1.19, 1.53] | 23.63 [21.85, 24.53] → 10.87 [10.24, 11.08] | 52.67 [50.66, 57.10] → 25.52 [24.56, 27.57] | 55.27 [51.93, 60.25] → 27.43 [26.54, 28.54] |
| 1000 | 1.19 [1.09, 1.99] → 3.00 [1.32, 3.46] | 27.19 [25.73, 28.03] → 14.45 [14.09, 15.50] | 59.48 [57.21, 185.96] → 32.21 [29.27, 37.81] | 67.02 [54.86, 69.39] → 41.95 [40.76, 44.63] |
| 10000 | 1.34 [1.25, 1.38] → 1.58 [1.45, 3.82] | 28.57 [28.42, 29.13] → 16.16 [16.01, 17.02] | 59.03 [55.09, 63.46] → 31.59 [28.98, 33.26] | 135.76 [131.44, 141.45] → 117.83 [115.92, 126.47] |

## Initial sync

Milliseconds. First/all/Bootstrap begin before Store open; empty watch
notifications do not count. Backend setup/fixture work is outside timing.

| Existing N | Store open | First visible | All visible | Bootstrap |
| --- | --- | --- | --- | --- |
| 100 | 4.66 [4.20, 4.69] → 5.22 [4.97, 5.79] | 57.51 [56.89, 95.62] → 47.65 [46.58, 57.43] | 57.51 [56.89, 95.62] → 47.65 [46.58, 57.43] | 57.45 [56.80, 95.55] → 47.57 [46.49, 57.32] |
| 1000 | 4.34 [4.30, 10.93] → 5.23 [5.00, 5.42] | 128.46 [124.70, 153.53] → 135.63 [130.31, 143.05] | 250.94 [219.83, 334.02] → 241.38 [236.38, 254.65] | 250.82 [219.72, 333.88] → 241.16 [236.16, 254.42] |
| 10000 | 4.58 [4.29, 29.50] → 5.09 [5.03, 5.72] | 124.67 [123.23, 153.59] → 131.01 [130.40, 133.69] | 2571.26 [2443.54, 2702.61] → 2739.34 [2705.89, 2769.42] | 2569.90 [2442.79, 2701.29] → 2738.42 [2704.98, 2768.23] |

## Remaining work

Per-run summed span seconds and median completed counts, B-only where labeled.
These spans overlap/nest: do not add them or treat them as exclusive wall-time
shares. Watch time includes query/conversion/comparison, not SQL alone.

| Existing N | Server Mutation execute, seconds | B watch query, seconds | B watch query count | B complete apply count |
| --- | --- | --- | --- | --- |
| 100 | 2.91 [2.71, 3.35] → 1.25 [1.20, 1.27] | 0.16 [0.15, 0.17] → 0.22 [0.21, 0.23] | 572 → 600 | 98 → 100 |
| 1000 | 3.42 [3.24, 4.40] → 1.93 [1.81, 1.95] | 1.06 [0.94, 1.10] → 1.35 [1.33, 1.37] | 583 → 600 | 100 → 100 |
| 10000 | 4.25 [4.14, 4.69] → 2.30 [2.25, 2.46] | 3.97 [3.71, 4.29] → 2.92 [2.73, 3.14] | 141 → 90 | 45 → 29 |

The immediate prior run used EPYC 9V74; this run used EPYC 7763. Strict comparison rejected that pair. The 10k burst observation is 5.61 → 3.47 seconds, but its full difference cannot be assigned to startup validation. A separate comparison with the first coalescing baseline passed all compatibility checks: 6.61 → 3.47 seconds (47.5% lower). That comparison includes both live-prefix progress during HTTP waits and startup validation.

The main remaining client hotspot is Model-watch work. At N=1,000, B performed 600 queries for 100 changed snapshots; at N=10,000 it performed a median 90 queries for 29 changed snapshots. Its query/conversion/comparison sum was 2.917 seconds and bridge drain/decode 1.862 seconds in the 10k burst. Complete-unit apply on B summed to 0.050 seconds. These are overlapping source spans, not exclusive wall-time portions. Query invalidation has not changed: different arrival/coalescing patterns changed the number of applied groups. Server Mutation execution still summed to 2.299 seconds; delivery planning to 1.916 seconds.

Initial sync did not uniformly improve against the immediate prior run: 10k all-visible was observed at 2.57 → 2.74 seconds on different CPU models. Against the compatible earlier baseline it changed 2.94 → 2.74 seconds. No cold-cache, phone/UI or WAN performance claim follows from these results.

## Correctness and evidence

All 45 samples passed exact final identity/count/content assertions in the real
runner. The archive audit independently confirmed passing status/server checks,
1,515 accepted writes and 1,515 correlated successful server transactions,
accepted-report criteria, zero backend/connection errors, zero trace loss, zero
cleanup/admin errors. Known verified nonterminal owned-create reports:
1445; these remain visible and are not discarded failures.

The runner reuses generated SDK, real backend/HTTP/WebSocket, PostgreSQL and
SQLite, and closes clients/processes/connections/files and temporary databases.
Failures/timeouts are retained in raw output; none occurred in this matrix.
Native/Node clocks are correlated by identity, never subtracted across domains.

The runtime source passed full Linux/macOS Verify before measurement. This
report adds archived data and does not change runtime behavior. No performance
threshold is introduced.
