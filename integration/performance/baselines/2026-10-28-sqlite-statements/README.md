# SQLite prepared-statement caching

The SQLite adapter now uses rusqlite `prepare_cached` for reads and single
writes. Writer and committed-reader connections each use rusqlite 0.38's bounded
default cache of 16 statements. It caches prepared SQL, not results. Transactions,
parameters, row validation and commit boundaries retain their existing behavior.
`execute_batch` and the authorizer-based watch-dependency inspection still use
fresh preparation. Watch inspection must invoke the authorizer for each request.

A cached statement can reprepare on its first step after a schema change.
The row decoder therefore uses the stepped row's current column count and reads
column names after execution, including empty results. Three new
[SQLite regressions](../../../../crates/sqlite/tests/statement_cache.rs) cover
schema changes on both connections, empty results, bindings, constraint errors,
rollback, warmed watch authorization and read-only enforcement.

## Short comparison

Four fresh N=10,000 initial syncs ran before/after/after/before. Only the release
Dart native library changed; backend and AOT executables remained identical.
Both conditions include the optimized canonical encoder and identical apply
spans. Prefetch is enabled, mutation group size is 8, and pages contain 500 rows.
The baseline library is the saved optimized-client artifact from the previous
[encoding experiment](../2026-10-28-client-encoding/README.md). The cached client
was restored at completion.

| Milliseconds, median [min, max] | Uncached | Cached |
| --- | ---: | ---: |
| All 10,000 visible | 493.310 [489.260, 497.359] | 480.546 [477.819, 483.273] |
| First nonempty result | 72.826 [71.320, 74.331] | 65.421 [63.060, 67.781] |
| Client apply, summed | 228.922 [228.161, 229.682] | 144.114 [142.778, 145.450] |
| Authority installation, summed | 149.905 [149.378, 150.432] | 79.231 [78.238, 80.223] |
| Authority pre-clear, summed | 28.501 [28.432, 28.569] | 18.006 [17.747, 18.265] |
| Identity bookkeeping, summed | 15.061 [15.039, 15.083] | 10.718 [10.619, 10.817] |
| Apply COMMIT, summed | 28.752 [28.462, 29.042] | 28.695 [28.615, 28.776] |
| Server planning, summed | 268.300 [264.120, 272.479] | 267.990 [266.992, 268.988] |

Apply fell 37.0% and installation fell 47.1%, but all-visible time fell only
2.6% (12.764 ms). Both adjacent pairs favor caching. These percentages use this
run's control; do not combine its baseline with earlier runs. Two samples per
condition provide directional local evidence, not a production latency estimate.

## Why full-load savings are smaller

Prefetch already overlaps most apply with HTTP waits. The common-axis trace
measures 212.310 ms of uncached apply overlapping HTTP, versus 137.976 ms with
caching. That overlap shrinks as apply gets shorter; the full apply reduction
cannot be subtracted from wall time. HTTP spans include server processing and
are not a pure network-latency measure. Server planning stays around 268 ms.

The interval from each page's HTTP completion to apply start remains about
9.5 ms per page (cached median 9.520, range 8.971–12.422 across 40 page
observations in two runs). This is a measured interval that can include native
response admission/validation, complete-unit assembly and scheduling. It is not
an isolated measurement of any one of those operations. The next useful profile
should split that interval before choosing another optimization. The 40 page
observations are correlated within the two runs.

## Evidence and verification

All four samples verified final client/server identities and content, snapshots
0/500/.../10000, 20 complete applies, all 19 successor requests overlapping the
previous apply, no backend/transport/cleanup errors, and complete aligned traces.
The raw `*.json.gz` reports retain source/artifact identities and spans.
`evidence.json` is derived by the audit:

```sh
node integration/performance/baselines/2026-10-28-sqlite-statements/audit.mjs
```

The audit checks fixed backend/AOT hashes, changed client-library hashes,
control/variant artifact equality within each pair, runner hash, final state,
phase counts, exactly 20 apply-contained COMMIT spans and prefetch overlap.
`optimization.patch` contains the runtime change. Source fingerprints describe
the current workspace; the control binary was captured before caching and is
identified by its retained hash.

`runner.mjs` freezes the exact one-sample runner, including absolute workspace
imports; relocate those paths before replaying elsewhere. Select the uncached
or cached `libaxton_dart` between fresh runner processes, set
`AXTON_SQLITE_CACHE_VARIANT=before` or `after`, record artifact identity with
`run.mjs --record-build`, and retain each report using `--output`.

Apple M5 Max, macOS arm64, PostgreSQL 16.14, Node 24.19.0, Dart 3.12.1, Rust 1.98.1.
Fresh client process, SQLite files and PostgreSQL database per sample; fixture
setup, process startup and compilation excluded. OS/database caches were not
flushed and no WAN delay was simulated. This measures SDK watch visibility,
not UI rendering, mobile behavior or live-mutation latency. Cache-capacity
sensitivity and memory consumption were not benchmarked.

Executed checks: the full `cargo test --locked -p axton-sqlite` suite passed;
optional ignored tests were not run. The focused cache/query/DDL tests also
passed. SQLite clippy with `--all-targets -- -D warnings`, workspace formatting,
whitespace and archive checks passed. The disposable local PostgreSQL server
was stopped after verification.
