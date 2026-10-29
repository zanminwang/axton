# Cursor response generation: canonical JSON allocation

A short local experiment isolated response preparation after Bootstrap prefetch
was enabled. The shared canonical encoder now writes recursively into one byte
buffer instead of allocating, formatting and joining a String for each JSON
child. UTF-16 object-key ordering, key escaping and JCS scalar encoding are
preserved. Protocol checks, digest calculation and delivery boundaries are unchanged.

## Measurement

Four fresh N=10,000 initial syncs ran before/after/after/before, two observations
per condition. Every sample used 20 pages of 500 rows, prefetch enabled, commit
group size 8 and the same release Dart native library and AOT executable. Only
the Node backend addon changed. Both addons contain identical stage spans.
The baseline addon was saved before the encoder change and restored for the
last sample; that sample's source fingerprint therefore includes the optimized
source while its addon hash identifies the captured baseline binary.

| Milliseconds, median [min, max] | Before | After |
| --- | ---: | ---: |
| All 10,000 visible | 671.087 [662.271, 679.902] | 583.360 [582.505, 584.214] |
| First nonempty result | 62.924 [61.865, 63.983] | 63.372 [62.862, 63.881] |
| Server delivery planning, summed | 395.643 [390.856, 400.429] | 263.895 [263.503, 264.287] |
| Manifest/digest construction, summed | 129.639 [128.895, 130.382] | 74.814 [73.988, 75.639] |
| Response validation/encoding, summed | 67.119 [66.710, 67.528] | 33.662 [33.306, 34.017] |
| Metadata page selection, summed | 51.766 [50.976, 52.555] | 29.689 [28.884, 30.494] |
| Loaded-record grouping, summed | 38.117 [37.746, 38.488] | 20.635 [20.325, 20.945] |

All-visible time fell 13.1%; server planning fell 33.3%. First-visible time did
not improve. Each corresponding before/after pair favored the new implementation.
These are elapsed spans, not CPU samples. The stage totals overlap the enclosing
plan span and must not be added to it. Planning includes the concurrent empty
live check as well as the 20 HTTP pages. The baseline's largest measured phase
was manifest/digest construction, followed by response encoding; both repeatedly
use canonical JSON. No allocation profiler was run.

All four runs verified final client/server identities and content, snapshots
0/500/.../10000, exactly 20 complete page applies, all 19 successor HTTP requests
overlapping the previous apply, no backend/transport/cleanup errors, and complete
aligned traces. Client artifacts were deliberately held at their prior build to
isolate the server change; no client-side benefit is measured here. The optimized
local apply spans still total approximately 267–269 ms, close to server planning.

Apple M5 Max, macOS arm64, PostgreSQL 16.14, Node 24.19.0, Dart 3.12.1, Rust 1.98.1.
Fresh PostgreSQL databases, client processes and SQLite files; fixture setup,
process startup and compilation excluded. OS/database caches were not flushed,
and no WAN latency was simulated. This small comparison establishes direction
for this fixture, not a production or live-mutation performance guarantee.

## Evidence and verification

The four `*.json.gz` files retain raw reports, source/build identities, machine
settings and spans. `evidence.json` is derived by `audit.mjs`, which checks output,
artifact identities, runner hash, page boundaries and prefetch overlap:

```sh
node integration/performance/baselines/2026-10-28-page-response/audit.mjs
```

`runner.mjs` freezes the exact local runner used by each process, including its
absolute workspace imports. It derives from the previous prefetch runner; its
inherited off/on experiment description is historical metadata. Its actual
sequence is `[true]`, one prefetch-enabled sample per invocation. The four named
reports and this audit define the encoder comparison. Relocate its workspace
paths before replaying elsewhere. For a rerun, build both backend variants,
retain each addon, keep the Dart artifacts fixed, select the addon between fresh
runner processes, record build identity with `run.mjs --record-build`, and use
`--output` to retain each report. Use a disposable localhost `DATABASE_URL`.
`instrumentation.patch` applies to the parent delivery planner;
`optimization.patch` contains the independent encoder and regression changes.
Earlier mutation/group/prefetch work remains in the dirty parent workspace.

Executed correctness checks:

- `cargo test --locked -p axton-core -p axton-protocols -p axton-server` passed,
  including differential old/new encoding for nested values, numeric boundaries,
  escaping, UTF-16 key order and existing protocol wire/digest fixtures.
- All 15 real PostgreSQL `protocol-v05-delivery.test.mjs` tests passed with the
  optimized release addon, including constrained groups, removals, retained
  schemas, refusal, Bootstrap, WebSocket delivery and capacity cases. The attempted
  name exclusion did not exclude the capacity test; it also ran successfully.
  This invocation did not run the optional PostgreSQL-to-SQLite capacity replay.
- `cargo clippy --locked -p axton-core -p axton-protocols -p axton-server
  --all-targets -- -D warnings` and `git diff --check` passed.
