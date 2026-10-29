# Current default performance snapshot

Collected on 2026-10-28 from the current dirty worktree at parent `294f356`.
The same release Node/Dart artifacts and Dart AOT clients ran every case. Both
optimization environment overrides were unset: native prefetch and supported
backend group size 8 used their new defaults. Canonical encoding, SQL admission
reductions and SQLite statement caching are included.

Each case ran twice with 10,000 existing Entries, 512-byte text, a fresh
PostgreSQL database, client process and SQLite files. Native library preparation,
fixture setup and process startup are excluded. OS/database caches are not
flushed. Initial visibility starts before application configuration/Store open.
Live timing starts after both clients initialize and B registers its watch.
This is SDK watch visibility on local HTTP/WebSocket, not UI rendering, mobile
or WAN latency. Apple M5 Max/macOS arm64, PostgreSQL 16.14, Node 24.19.0,
Dart 3.12.1, Rust 1.98.1; full settings and identities are retained.

## Results

Milliseconds, median [minimum, maximum], two independent runs per case:

| Boundary | Time |
| --- | ---: |
| Initial first 500 visible | 43.424 [42.274, 44.573] |
| Initial all 10k visible | 449.738 [445.351, 454.124] |
| One Mutation local commit | 0.543 [0.517, 0.570] |
| One Mutation A settlement | 14.679 [12.796, 16.563] |
| One Mutation B visibility | 14.080 [12.203, 15.957] |
| 100 continuous Mutations, all A settled | 516.654 [515.092, 518.216] |
| 100 continuous Mutations, all B visible | 513.007 [510.884, 515.130] |

Live A calls named CreateEntry Mutations; B independently watches the Stream.
A settlement waits for accepted outcome and its local authoritative obligations.
The 100-write case awaits each local commit but issues the next write without
waiting for settlement; it is continuous traffic through several frozen HTTP
Batches. It is not one application bulk-upload request or one SQL transaction.
The two burst runs each used 14 Mutation group transactions and 1,543 framework
SQL calls across measured backend work. Within Mutation transactions themselves,
each used 1,303 framework calls, totaling 380.654 and 382.386 ms. SQL span times
include execution and waiting. Business Handler/Loader SQL is outside these
framework query counts.

Initial server delivery planning totals 254.048 ms (median), versus 144.171 ms
of client apply. These phases overlap under prefetch and are not additive wall
time. The HTTP-completion-to-apply-start interval remains about 9.484 ms per page
(pooled median across the two runs); its validation/assembly/scheduling components
have not been separately measured. Initial sync remains 20 complete pages.

Single-write time is one independent observation per run. Burst per-write
observations are correlated within each run; the raw report's pooled p95 is not
an independent-run tail estimate. Its per-member server-transaction values repeat
the enclosing group duration and are not individual service times. Use all-A/
all-B completion for the burst. Two repetitions do not establish a production
SLA, tail latency or a stable improvement over a separate archive.

## PowerSync reference boundary

The [official January 2025 Flutter benchmark](https://powersync.com/blog/how-fast-is-powersync-performance-benchmarks-for-flutter)
provides historical desktop reference numbers. Its hardware, row schema, timing
boundaries, averaging and bulk transaction shape differ. No PowerSync instance
was run here. The [October 2026 SDK release](https://releases.powersync.com/announcements/flutter-client-sdk)
is newer than that benchmark. Numerical differences are not a matched-engine
comparison or a measurement of today's PowerSync version.

## Evidence and reproduction

All 6/6 runs passed final client/server identities, text and accepted outcomes;
there were no backend/transport/cleanup errors or trace loss. Every trace aligned.
Initial runs produced 0/500/.../10000 snapshots and all 19 successor requests
overlapped the predecessor's apply. `results.json.gz` retains the full output;
`evidence.json` is derived by the audit:

```sh
node integration/performance/baselines/2026-10-28-performance-summary/audit.mjs
```

`runner.mjs` freezes the exact standard runner used. It has repository-relative
imports for its original `integration/performance/run.mjs` location. With the
normal release/AOT artifacts built and a disposable localhost `DATABASE_URL`:

```sh
unset AXTON_EXPERIMENTAL_BOOTSTRAP_PREFETCH AXTON_EXPERIMENTAL_MUTATION_GROUP_SIZE
node integration/performance/run.mjs --record-build
node integration/performance/run.mjs --samples 2 --sizes 10000 --writes 100 \
  --timeout-ms 60000 --fail-fast on --output artifacts/performance/current-summary.json
```

The disposable local database was stopped after collection. No runtime source
was changed for this snapshot.
