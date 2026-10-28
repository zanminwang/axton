# Incremental Model-watch delivery

This archive compares Rust's completed incremental Model-query implementation
with the same implementation plus incremental Rust-to-SDK delivery and generated
Model decoding. It does not compare against the older instrumentation-2 archive.

## Results

All 27 before samples and 45 after samples passed their content, identity,
settlement and trace checks. All 72 common-axis traces aligned; maximum position
uncertainty was ±39.0 µs.

The runner retained 869 before and 1455 after admissible transient
own-create divergence reports. Each names a verified accepted/settled Entry under
the existing measurement contract. Unexpected reports, transport errors and trace
loss fail a sample; none was accepted here.

The N = 10,000 single-write B-visible median fell from **64.462 ms to 28.644 ms
(55.6% lower)**. These are actual B visibility measurements; A settlement is
reported separately.

| Case | Metric | Before median (ms) | After median (ms) | Change |
| --- | --- | ---: | ---: | ---: |
| initial-sync/100/0 | All visible | 37.087 | 36.124 | -2.6% |
| initial-sync/1000/0 | All visible | 206.139 | 194.328 | -5.7% |
| initial-sync/10000/0 | All visible | 2201.035 | 1923.484 | -12.6% |
| live-sync/100/1 | B visible per write | 26.705 | 24.179 | -9.5% |
| live-sync/100/100 | B visible per write | 589.850 | 533.767 | -9.5% |
| live-sync/1000/1 | B visible per write | 29.569 | 27.232 | -7.9% |
| live-sync/1000/100 | B visible per write | 683.519 | 755.115 | 10.5% |
| live-sync/10000/1 | B visible per write | 64.462 | 28.644 | -55.6% |
| live-sync/10000/100 | B visible per write | 992.807 | 786.133 | -20.8% |

Live per-write observations in each run are correlated. Burst completion medians
(all 100 new Entries visible) were:

| Existing N | Before (ms) | After (ms) | Change |
| ---: | ---: | ---: | ---: |
| 100 | 1194.726 | 1126.336 | -5.7% |
| 1000 | 1413.742 | 1583.575 | 12.0% |
| 10000 | 1936.321 | 1591.541 | -17.8% |

The N = 1,000 burst case regressed in this run: per-write B visibility increased
10.5%, and all-visible completion increased 12.0%. The data does not establish
a cause, and the serial before/after runs do not control backend/scheduling
variation. This result is retained, not removed or described as an improvement.

## One actual sample from each matrix

Each column below is the sample at the median B latency, not independently
selected phase medians. Milestones are relative to that write's start.

| Milestone | Before (ms) | After (ms) |
| --- | ---: | ---: |
| A local commit | 1.348 | 1.737 |
| A settlement | 31.854 | 28.358 |
| B application callback | 64.462 | 28.644 |

The same samples' B-only longest drain/JSON-decode spans were:

| Source span | Before (ms) | After (ms) |
| --- | ---: | ---: |
| bridge.nativeDrain | 14.830 | 0.052 |
| bridge.decode | 12.393 | 0.035 |
| watch.snapshot | 2.060 | 0.002 |
| watch.enqueue | 0.003 | 0.135 |

These spans overlap/nest with other work and are not an additive end-to-end
breakdown. The after `watch.enqueue` includes patch application, typed decoding
and snapshot copying; the before span did not include later generated decoding.

The full-result bridge work is no longer the dominant observed cost in this
selected sample. The named Mutation's server transaction spans about 4.673–20.199
ms (15.526 ms elapsed); B's callback runs at 28.644 ms. Other transactions can
overlap and continue after B is visible. The trace preserves their real positions
and leaves unrecorded intervals unattributed rather than labeling the remainder
network, queue or CPU time.

## Implementations and method

- Before: `9895d13c`, incremental Rust queries, full observer snapshots and full
  generated Dart Model decoding. Three repetitions per case.
- After: `74b8967c`, opt-in versioned ordered splices, private SDK caches and
  generated changed-row decoding. Five repetitions per case.
- Both: instrumentation 3, bounded monotonic clock correlation, release native
  artifacts, Dart AOT, real HTTP/WebSocket, PostgreSQL and SQLite, 512-byte text,
  N = 100 / 1,000 / 10,000, initial sync and live M = 1 / 100 writes.
- Host: Apple M1 Pro, macOS arm64, Node 26.4.0, Dart 3.12.1, Rust 1.98.1.
  Complete machine, database settings, source fingerprints and artifact hashes
  are retained in each raw file.

The baseline was captured before editing the delivery implementation. Both
matrices ran serially on this host with fresh databases and client files per
sample. They were not randomized/interleaved, and OS/PostgreSQL caches were not
flushed. The small per-case repetition counts describe this run, not a phone
performance claim or a statistically established universal speedup.

## Evidence

`before.json.gz` and `after.json.gz` retain every raw sample and source span.
`evidence.json` is derived by `audit.mjs`, which checks matrix completion and
correctness, identical timing/fixture/machine conditions, absence of trace loss,
and valid clock correlation for every sample. Run from any directory:

```sh
node integration/performance/baselines/2026-10-27-model-watch-delivery/audit.mjs
```

`before-selected.trace.json` and `after-selected.trace.json` are the median-B
single-write sample in each matrix at N = 10,000. Open them in Perfetto. Each
file uses one shared time axis for client A/B, native and backend work, preserves
actual span parents and gives concurrent work separate lanes. They are source
elapsed-time traces, not CPU profiles; unrecorded intervals remain unattributed.
The raw results allow every other sample's trace to be regenerated with
`integration/performance/timeline.mjs`.

## Interpretation

The optimized generated API still emits complete result Lists. SDKs decode only
incoming rows, then construct caller-owned snapshots: immutable scalar-only Dart
Models can be shared, while mutable Lists and JavaScript objects/Dates are copied.
Public snapshot creation remains O(N), and inserts/deletes may shift retained
rows. Full initial results, reset/bulk fallbacks and error recovery retain their
full-result cost. `watchSql` retains its existing dependency-table requery path.

Applications must regenerate their typed client to obtain changed-row Model
decoding. Existing generated code remains compatible through generic `watch`,
but still performs its existing full-list Model conversion.

`watch.enqueue` covers synchronous observer dispatch. After this change it also
contains SDK patch application, changed-row decoding and copying; before, the
generated Model mapping ran later in the Dart stream. Do not treat its duration
as an unchanged isolated phase. Native drain and JSON decode keep their existing
boundaries. Nested/overlapping source durations must not be summed into an
end-to-end latency partition.


## Remaining measured work

The selected optimized sample's named Mutation transaction makes 17 framework SQL
calls between 4.673 and 20.198 ms on the shared scenario
axis. Its elapsed duration is 15.526 ms, while its business handler takes
0.413 ms. The larger serial host phases include:

| Host phase | Elapsed ms |
| --- | ---: |
| host.beginBatch | 1.166 |
| host.readTracking | 1.620 |
| host.readTracking | 1.451 |
| host.applyStreamMembers | 3.387 |
| host.targetPositions | 1.277 |

Connection acquisition is 0.028 ms and COMMIT is
0.822 ms. The phases contain their SQL calls, so SQL/host/transaction
durations cannot be added together. B's complete-unit apply is
0.842 ms in this sample. These observations make
backend tracking/member/position query count and post-commit delivery reasonable
next investigation targets. Query plans and dependency/lock requirements will
determine which round trips can be combined or avoided.
