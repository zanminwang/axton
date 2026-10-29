# Client rebuild: canonical encoding and apply timing

Two release Dart native libraries were rebuilt with identical apply spans and
old/new core canonical encoders. The control uses `crates/core/src/lib.rs` from
parent `294f356`; the variant uses the allocation optimization from the
[server experiment](../2026-10-28-page-response/README.md). The current optimized
Node backend, Dart AOT executables, prefetch setting (on), commit-group size (8)
and fixture were held constant. Four fresh N=10,000 initial syncs ran
before/after/after/before. Only the Dart native library hash changes between
conditions. The workspace was restored to the optimized client afterward.

## Results

| Milliseconds, median [min, max] | Old encoder | New encoder |
| --- | ---: | ---: |
| All 10,000 visible | 617.071 [615.950, 618.192] | 505.346 [503.475, 507.216] |
| First nonempty result | 87.163 [84.907, 89.418] | 72.266 [70.094, 74.438] |
| Client apply, summed | 271.259 [270.146, 272.372] | 232.921 [232.188, 233.654] |
| Server planning, summed | 272.227 [270.080, 274.373] | 274.114 [273.558, 274.669] |

The client-only change reduced all-visible time by 18.1%, first-visible time by
17.1%, and apply time by 14.1%. Both adjacent comparison pairs favor the new
encoder. Use this run's control for these percentages: the previous experiment
had a different native build/instrumentation and observed 583 ms. Cross-run
absolute differences are not an isolated encoder effect. Two samples per
condition provide directional local evidence, not a stable production estimate.

## Where apply spends time

Summed elapsed time across the optimized client's 20 complete unit applies:

| Phase | Median ms | Meaning |
| --- | ---: | --- |
| Authority installation | 152.511 | Read/version-check evidence, save authority/base, clear local layer, inspect pending ownership and upsert the projected row. |
| Authority pre-clear pass | 28.812 | Validate keys, read admission evidence and clear prior projection for atomic uniqueness handling. |
| Apply transaction COMMIT | 29.187 | Only the 20 COMMIT spans contained within apply, not every client commit. |
| Delivery identity bookkeeping | 15.418 | Insert the unit's record identities into the delivery-key table. |
| Pending settlement reconciliation | 0.189 | No mutations are queued in this initial-load fixture. |
| Held projection rebuild | 0.152 | This fixture has no dirty records requiring optimistic replay. |

These are children of the 232.921 ms apply total. The authority parent
(181.427 ms) already includes its clear/install children and must not be added
to them. Approximately 6.655 ms remains in other apply work. Server planning
and client apply overlap; their totals do not form a serial wall-time equation.
The apply span starts after complete-unit selection, so response validation and
assembly before selection are not attributed to apply. Full-load gains therefore
need not equal the apply reduction. Watch-query spans separately fell from
51.926 to 31.593 ms; this run does not isolate their suboperations.

Authority installation accounts for about 65.5% of apply. Inspection of
[settlement05.rs](../../../../crates/client/src/settlement05.rs),
[authority.rs](../../../../crates/client/src/authority.rs) and
[SQLite storage](../../../../crates/sqlite/src/lib.rs) shows per-record evidence
reads/writes and projection operations. SQLite query helpers call `prepare`
each time, and writes call `Connection::execute`, which also prepares the SQL.
Reusing prepared statements is a concrete next experiment. This run does not
separate prepare cost from execution, JSON/schema work or parameter conversion,
and does not measure a statement-cache speedup. No apply algorithm was changed.

## Evidence and reproduction

All four samples passed final client/server identity and content checks,
0/500/.../10000 watch snapshots, 20 complete page applies, all 19 successor HTTP
requests overlapping the previous apply, no transport/backend/cleanup errors,
and complete aligned traces. The audit checks fixed backend/AOT hashes, changed
client-library hashes, old/new/old artifact restoration, runner fingerprints,
phase counts and exactly 20 apply-contained COMMIT spans:

```sh
node integration/performance/baselines/2026-10-28-client-encoding/audit.mjs
```

Raw reports are in the four `*.json.gz` files, derived summaries in
`evidence.json`, and the new coarse spans in `instrumentation.patch`. The frozen
`runner.mjs` is the exact runner used, including absolute workspace imports;
relocate those paths before running elsewhere. Each invocation runs one fresh
prefetch-enabled initial sync. Set `AXTON_CLIENT_ENCODING_VARIANT=before` or
`after`, select the corresponding release `libaxton_dart` between fresh runner
processes, record identity with `run.mjs --record-build`, and retain each report
with `--output`. Source fingerprints describe the current workspace; the control
library was explicitly rebuilt from the old core encoder and is identified by
its own binary hash. Its other sources and spans match the new library.

Apple M5 Max, macOS arm64, Node 24.19.0, Dart 3.12.1, Rust 1.98.1 and PostgreSQL
16.14. Fresh client process, SQLite files and PostgreSQL database per sample;
fixture setup, process startup and compilation excluded. OS/database caches were
not flushed, and no WAN delay was simulated. This measures SDK watch visibility,
not UI rendering or live-mutation latency.

Executed verification: targeted real SQLite downlink, authority, progress and
binding tests passed; optional ignored tests were not run. Client clippy with
`--all-targets -- -D warnings`, workspace formatting, diff whitespace and this
archive audit passed. The prior canonical differential/wire tests are recorded
in the server experiment. The local database was stopped after verification.
