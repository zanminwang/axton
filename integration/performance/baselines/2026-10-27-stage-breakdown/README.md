# Detailed sync phases — 2026-10-27

At `02bdaf0bb9ce8c0ceb290414444f5ad1557a6ef7` (dirty: false), **45/45 samples passed**.
This run measures the newly separated server and Model-watch phases. It changes
observation only; invalidation, transaction, batching and notification policies
are unchanged. It establishes a new instrumentation baseline, not a speedup.

[Workflow](https://github.com/zanminwang/axton/actions/runs/40630070383),
[raw samples](results.json.gz), [phase evidence](evidence.json),
[artifact fingerprints](build.json), [archive checksums](sha256.json).
Run `node integration/performance/baselines/2026-10-27-stage-breakdown/audit.mjs`
to reproduce the audit and evidence from the archive.

## Conditions and timing

Five serial repetitions per case; N = initial Entry count, M = new creates.
Each text is 512 bytes. Release native artifacts and Dart AOT run on Linux Azure,
AMD EPYC 7763 (4 vCPU), Node 26.4, Dart 3.12.1 and PostgreSQL 16.15.
Full machine/tool/database/cache metadata is in evidence.json and raw output.

Each sample has a new process, SQLite files and PostgreSQL database. Fixtures
were just written; OS/PostgreSQL caches are not flushed. Compilation, process
startup, server readiness and fixture setup are outside timing. This is loopback
on shared Actions hardware, not phone, UI, WAN or cold-cache performance.

Times are milliseconds, median [minimum, maximum]. There are only five independent
runs per case; no independent-run p95 is reported. Raw per-write statistics pool
correlated observations within each run and must not be interpreted as 500
independent experiments. Two live clients use one Dart process/clock. Server
transaction duration uses only the server clock; source identities correlate
transactions without subtracting timestamps across clock domains.

The prior startup-layout archive is intentionally refused by strict comparison:
`stageInstrumentation` changed from the earlier version to 2. Additional tracing
can change timing. No regression or improvement percentage follows from that
pair; a disabled-diagnostics control is still needed to estimate trace overhead.

## End-to-end results

Initial milestones start before Store open. The first expected nonempty watch
snapshot counts; an initial empty callback does not. Bootstrap is an independent
native milestone, so it can slightly precede Dart watch visibility.

| N | Store open | First visible | All visible | Bootstrap |
| --- | --- | --- | --- | --- |
| 100 | 5.38 [5.12, 5.68] | 49.86 [48.71, 60.54] | 49.86 [48.71, 60.54] | 49.77 [48.63, 60.45] |
| 1000 | 5.33 [5.15, 7.30] | 136.24 [130.47, 137.83] | 247.30 [236.33, 249.99] | 247.11 [236.22, 249.79] |
| 10000 | 5.19 [5.08, 5.60] | 136.69 [134.06, 142.22] | 2891.37 [2816.91, 2915.59] | 2889.59 [2815.03, 2913.63] |

Single creates: A settlement is accepted-outcome visibility on A, including its
return path, rather than a server timestamp.

| N | A local commit | Server transaction, including COMMIT | A settlement | B visible |
| --- | --- | --- | --- | --- |
| 100 | 1.34 [1.29, 1.36] | 11.57 [11.10, 12.23] | 28.52 [27.90, 28.82] | 29.16 [27.99, 31.61] |
| 1000 | 3.18 [1.50, 3.30] | 14.70 [14.52, 16.18] | 32.67 [31.95, 34.48] | 42.37 [42.04, 42.90] |
| 10000 | 1.58 [1.47, 1.62] | 17.68 [16.70, 18.01] | 32.86 [32.74, 34.48] | 126.63 [111.57, 154.84] |

Continuous creates await each A local commit and immediately begin the next;
they do not wait for settlement. Completion below is from the first write start.

| N | M | All accepted on A | All visible on B |
| --- | --- | --- | --- |
| 100 | 100 | 2034.96 [2028.08, 2128.69] | 2014.65 [2010.86, 2113.59] |
| 1000 | 100 | 2796.83 [2761.35, 2840.39] | 2782.08 [2750.16, 2820.35] |
| 10000 | 100 | 3501.49 [3291.61, 3601.40] | 3641.75 [3434.21, 3753.63] |

## Upload work on the server

The following values include only operation IDs with Node `host.handleAction`
for `CreateEntry`, excluding delivery preparation. Values are the median summed
completed span time per run for all 100 writes. Spans nest and overlap; these
columns are not additive wall-time components. SQL includes driver/transport
round trips and PostgreSQL execution, not pure PostgreSQL CPU time.

| N | Handler | Framework SQL | Pool acquire | BEGIN | COMMIT | Rust callback wait |
| --- | --- | --- | --- | --- | --- | --- |
| 100 | 37.01 [34.34, 40.36] | 883.73 [871.14, 904.14] | 5.84 [5.44, 11.97] | 23.27 [20.35, 28.15] | 97.86 [86.76, 105.97] | 1292.49 [1270.85, 1350.27] |
| 1000 | 34.82 [33.79, 38.95] | 1534.74 [1528.96, 1572.01] | 8.62 [5.72, 10.79] | 31.30 [23.59, 32.53] | 102.54 [97.39, 111.18] | 1992.92 [1953.78, 2021.62] |
| 10000 | 48.50 [43.68, 54.61] | 1866.63 [1747.19, 1886.79] | 9.73 [6.07, 10.60] | 34.82 [32.41, 36.69] | 120.65 [110.81, 131.62] | 2368.89 [2229.41, 2449.18] |

At N=10,000, 100 writes produce 1,506 framework SQL calls and 1,603 Rust host
callback waits. The fixture handler itself totals about 48 ms, pool acquisition
10 ms and COMMIT 121 ms, compared with 1,867 ms of framework SQL and 2,369 ms of
inclusive callback wait. Handler time and pool waiting are smaller measured
contributors in this fixture. Application SQL inside a handler is not separately
instrumented; these figures do not establish business-handler cost for Most Days.

The enclosing upload transaction sums to 3,134 ms [2,938, 3,205], with
2,965 ms [2,764, 3,028] in its body. Nested native Mutation execution sums to
2,450 ms [2,313, 2,536]. Rust callback encode/decode sum to about 10/7 ms;
Node host decode/encode sum to 23/10 ms. These small serialization spans do not
account for the inclusive callback waits and cannot be subtracted to claim an
isolated callback scheduling cost.

Framework SQL ranked by its direct Node host parent at N=10,000:

| Host operation | SQL call count | Summed SQL ms per run |
| --- | --- | --- |
| host.readTracking | 200 | 623.75 [602.53, 642.67] |
| host.applyStreamMembers | 300 | 574.88 [528.54, 583.84] |
| host.targetPositions | 100 | 242.85 [222.94, 257.21] |
| host.claimStore | 200 | 118.23 [115.47, 123.76] |
| host.saveResult | 200 | 83.33 [81.37, 88.33] |
| host.lockStreams | 100 | 70.39 [62.27, 80.37] |
| host.guardRecords | 100 | 53.88 [48.68, 59.48] |
| host.savepoint | 100 | 34.18 [32.36, 41.10] |
| host.head | 100 | 25.54 [22.74, 26.70] |
| host.release | 100 | 15.55 [13.93, 24.28] |
| host.beginBatch | 6 | 2.35 [2.23, 2.77] |

`readTracking`, stream-member publication and target-position work are the first
server paths to inspect. This run proves their observed call multiplicity and
inclusive duration; it does not prove a full scan, lock wait or a particular query
plan. Rust `host.await` also includes Node scheduling, JavaScript/SQL execution
and response wait. No remaining difference is labeled queue-only or network-only.

## B watch work

Values below are B-only, M=100, summed milliseconds per run. `watch.read` includes
SQL plus native row assembly; convert builds JSON Values; compare checks equality;
snapshot builds/copies changed snapshots. Native drain covers FFI plus native
serialization/copy/free; decode measures Dart JSON parsing. Different stages have
different count boundaries, and all totals remain non-additive.

| N | Query count | Changed snapshots | Read | Convert | Compare | Snapshot | Dart JSON decode | Native drain |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 100 | 600 | 100 | 153.10 [150.42, 155.58] | 44.19 [42.96, 44.72] | 8.27 [8.00, 8.53] | 7.45 [7.42, 7.76] | 60.83 [55.25, 62.71] | 96.01 [93.69, 107.24] |
| 1000 | 600 | 100 | 888.61 [873.17, 900.54] | 298.97 [294.67, 304.01] | 60.11 [56.19, 63.65] | 39.76 [38.65, 41.78] | 266.91 [251.38, 273.46] | 477.12 [468.44, 484.71] |
| 10000 | 93 | 30 | 1763.13 [1666.81, 1807.30] | 731.14 [660.99, 850.44] | 109.03 [105.38, 116.71] | 108.43 [99.62, 111.93] | 698.67 [664.58, 741.98] | 1269.48 [1218.93, 1327.00] |

At N=1,000, the median is **600 queries [599, 600] for 100 changed snapshots**. All these
invalidation events advance exactly one generation, so the recorded last commit
is sufficient to classify their written tables:

| Cause | Invalidations per run | Written tables |
| --- | --- | --- |
| `store.apply` | 100 | Entry and sync metadata |
| `store.freeze` | 200 [199, 200] | `axton_store` only |
| `store.needs` | 300 | `axton_store` only |

Thus a median 500 [499, 500] full rereads produce unchanged results while only
metadata changed.
Read plus conversion totals approximately 1,188 ms, compared with 60 ms for
equality. Optimizing equality alone would miss the larger measured work. Actual
B complete-unit apply totals about 117 ms; watch rereads/conversion are a separate
cost from applying incoming rows.

At N=10,000 the median is 93 queries and 30 changed snapshots: arrivals coalesce
into fewer applied groups while each query reads a larger result. Read plus
conversion totals approximately 2,494 ms, compared with 109 ms for equality and
54 ms for complete-unit apply. Dart JSON decoding contributes another observed
699 ms and native drain 1,269 ms. These nested/overlapping figures do not divide
the 3,642 ms completion into exclusive shares.

Individual cause-count medians at N=10,000 need not sum to the median query
count. The event contains only the last commit's written tables, not the union
across a multi-generation jump; it cannot alone justify skipping such a watch.
No invalidation optimization is implemented by this change.

## Correctness and limits

The runner's 45 samples passed final identity, count and content assertions.
The archive audit independently confirms 45 passing server checks, 1,515 accepted
writes and 1,515 correlated successful server transactions, valid accepted-report
criteria, and zero backend/connection errors, trace loss, cleanup or admin errors.
It retains 1,453 known verified nonterminal owned-create reports; they are not
silently discarded failures. Raw output retains failures/timeouts when present;
none occurred in this matrix. The archive stores assertions and traces rather
than every returned row, so this audit does not re-query the original databases.

Temporary Stores, processes, connections, files and PostgreSQL databases use the
existing E2E cleanup path. The performance workflow and full Linux/macOS
[Verify](https://github.com/zanminwang/axton/actions/runs/40630024072) passed for
the measured runtime source. Local native Dart test execution was blocked by the
nested build-hook telemetry permission; real boundaries were instead exercised
by Verify and the release/AOT matrix. This archive adds no runtime changes.

Next investigations are metadata-only watch invalidation and framework SQL
round-trip multiplicity. Query plans, handler-internal SQL, isolated ORM COMMIT,
disabled-tracing overhead, generated Dart model conversion and UI rendering need
additional measurements. This baseline introduces no performance threshold.
