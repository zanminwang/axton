# Mutation receipt cursor scheduling

This run measures `3b8923cc`, which stops Mutation acknowledgments from raising
the head used for ordinary cursor repair. Its control is the preceding
[Model-watch delivery run](../2026-10-27-model-watch-delivery/README.md)'s
`after.json.gz` at `74b8967c`. Both already use incremental Rust queries,
splice delivery and generated changed-row decoding. The intervening archive
commit changes no production runtime behavior.

## Result

All 45 new samples passed state, settlement, backend and clock checks. The
45-sample control also passed. The timing/fixture contract and every recorded
machine, toolchain and PostgreSQL setting match exactly.

The redundant ordinary Pull disappeared in every single-write sample: each of
the 15 control samples had one, while each of the 15 new samples had zero.
For those samples, backend transactions fell from **5 to 4**, and framework SQL
calls from **47 to 41 (12.8% fewer)**. Both live deliveries and the independently
owned settlement Materialize still ran. The named Mutation still issued 17
framework SQL calls.

This establishes less duplicate work, not a general latency speedup. At
N = 10,000, the single-write B-visible median increased from **28.644 to
29.607 ms (+3.4%)**. The 100-write all-visible median increased from
**1591.541 to 1637.025 ms (+2.9%)**. Some other cases improved and some regressed.
All results are retained below. Five serial repetitions per case, collected in
separate campaigns, cannot isolate scheduling, storage or background-load
variation from the code change.

## Complete matrix

Times are milliseconds. Each initial/single-write metric has five observations.
Burst completion has five whole-run observations; its per-write timings in
`evidence.json` are correlated within a run.

| Case | Metric | Before median | After median | Change |
| --- | --- | ---: | ---: | ---: |
| Initial / 100 | All visible | 36.124 | 35.099 | -2.8% |
| Initial / 1,000 | All visible | 194.328 | 195.059 | +0.4% |
| Initial / 10,000 | All visible | 1923.484 | 1926.588 | +0.2% |
| Single write / 100 | B visible | 24.179 | 20.440 | -15.5% |
| Single write / 1,000 | B visible | 27.232 | 32.727 | +20.2% |
| Single write / 10,000 | B visible | 28.644 | 29.607 | +3.4% |
| 100 writes / 100 | All B visible | 1126.336 | 1208.890 | +7.3% |
| 100 writes / 1,000 | All B visible | 1583.575 | 1399.757 | -11.6% |
| 100 writes / 10,000 | All B visible | 1591.541 | 1637.025 | +2.9% |

N = 10,000 ranges provide context for those medians:

| Metric | Before median [min, max] | After median [min, max] |
| --- | ---: | ---: |
| Single-write B visible | 28.644 [24.820, 43.955] | 29.607 [26.562, 35.104] |
| Single-write A settlement | 28.358 [23.999, 42.765] | 30.696 [27.441, 36.060] |
| Single-write server transaction | 15.526 [13.870, 23.924] | 17.076 [14.737, 18.428] |
| 100 writes, all B visible | 1591.541 [1553.896, 1767.554] | 1637.025 [1562.618, 1784.252] |

Burst coalescing can still require ordinary repair. At N = 10,000, its ordinary
Pull count had median 3 both before (range 3–4) and after (range 2–4). This change
removes the receipt-driven trigger; it does not remove validated delivery hints,
reconnect repair or owned target materialization.

## Shared-axis trace

`before-selected.trace.json` and `after-selected.trace.json` each retain the
actual N = 10,000 single-write sample at the median B latency. The control is
repetition 1, sample index 35; the new result is repetition 4, sample index 38.
Each column below therefore belongs to one complete observation, rather than a
combination of independently selected stage medians.

| Selected sample | Before | After |
| --- | ---: | ---: |
| A local commit (ms from write start) | 1.737 | 1.336 |
| A settlement | 28.358 | 30.696 |
| B application callback | 28.644 | 29.607 |
| Named Mutation transaction duration | 15.526 | 18.247 |
| Handler duration | 0.413 | 0.335 |
| B local apply duration | 0.842 | 0.611 |
| Framework SQL across server transactions | 47 | 41 |

The ordinary Pull transaction disappears, while the two live transactions and
settlement Materialize remain on their actual concurrent lanes. The latter can
continue after B becomes visible. Node transaction spans include connection
acquisition and COMMIT; source elapsed spans can overlap and nest. They must not
be added into an end-to-end breakdown or interpreted as CPU profiles. Unrecorded
intervals remain unattributed.

All 90 samples pass bounded clock alignment. Maximum placement uncertainty is
±39 microseconds in the control and ±37 microseconds in the new matrix; the
selected traces have ±33.5 and ±24 microseconds respectively.

## Method and evidence

- Instrumentation 3, release native artifacts, generated Dart AOT clients, real
  HTTP/WebSocket, PostgreSQL and SQLite.
- 512-byte ASCII Entry text, fixed-width identities, fixture publication groups
  of 100; N = 100 / 1,000 / 10,000, initial and live M = 1 / 100.
- Apple M1 Pro, macOS arm64, Node 26.4.0, Dart 3.12.1, Rust 1.98.1,
  PostgreSQL 14.23. Complete versions/settings are retained in `evidence.json`.
- Fresh backend database, Dart process and SQLite files per sample. OS and
  PostgreSQL caches are not flushed. Loopback has no simulated WAN latency.
- The new run measured a clean checkout at `3b8923cc`; compilation, setup and
  clock calibration are outside its timed windows. Source/artifact fingerprints
  are retained in each raw result.

Run:

```sh
CARGO_HOME=/tmp/axton-perf-cargo \
CARGO_TARGET_DIR=/tmp/axton-sync-performance-20261027/target \
AXTON_DART=/tmp/axton-perf-dart \
bash integration/performance/run.sh --samples 5 --sizes 100,1000,10000 \
  --writes 100 --timeout-ms 120000 --output /tmp/axton-receipt-head-after-v3.json
```

`after.json.gz` retains all 45 raw new samples. The control is referenced from
the preceding archive rather than duplicated. `audit.mjs` verifies compatible
conditions, complete passing matrices, backend state, accepted transient record
reports and every clock correlation. It derives the comparisons, operation/SQL
counts and selected traces in `evidence.json`:

```sh
node integration/performance/baselines/2026-10-28-receipt-cursor/audit.mjs
```

Both matrices retained 1,455 permitted own-create transient divergence reports;
there were no backend/transport failures or trace loss. These are local desktop
observations, not phone, UI-rendering or cross-product benchmark claims.
