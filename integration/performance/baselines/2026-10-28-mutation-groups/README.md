# Sequential Mutation commit groups: short experiment

One HTTP Batch is partitioned into bounded transactions. Members execute in
order with their own savepoints, outcomes and publication cursor scopes. The
control uses group size 1; the experiment uses size 8. Both use the optimized
SQL adapter, the same release native addon/Dart library and Dart AOT clients.
Bootstrap prefetch is off. Default production execution remains group size 1.

## Results

All six samples passed: three fresh runs per condition, N = 1,000 existing
Entries and M = 100 continuous creates. Conditions alternate 1/8, 8/1, 1/8.
Times below are milliseconds. Complete per-write data, machine metadata,
source/artifact hashes and clock correlation are retained in `results.json.gz`.

| Metric | Group 1 | Group 8 |
| --- | ---: | ---: |
| All 100 visible on B, median [min, max] | 659.588 [586.157, 671.610] | 468.193 [460.297, 468.212] |
| All 100 settled on A, median [min, max] | 664.537 [591.104, 676.567] | 473.542 [467.390, 475.362] |
| Mutation transaction count, every run | 100 | 14 |
| Framework SQL across backend work, every run | 2,929 | 1,543 |
| Live response preparation count, every run | 200 | 28 |
| Pooled per-write B-visible median | 308.7085 | 233.564 |
| Pooled per-write A-settlement median | 369.8195 | 400.031 |

Whole-burst visibility median fell 29.0%; all-settled completion fell 28.7%.
All three adjacent visibility pairs favored group 8, by 117.964, 211.313 and
191.376 ms. The pooled per-write A-settlement median nevertheless increased
8.2%. Individual writes share a run and are correlated; this is not 300
independent experiments per condition. No independent-run p95 is established.

Groups reduce commits, repeated batch parsing/config decoding, and post-commit
subscriber wakes/live preparation. This prototype still checks Store admission
and saves progress for every member; it does not implement bulk business SQL or
bulk result persistence. Transaction duration is now a group duration; the raw
runner attributes that transaction to each member for settlement correlation.
It must not be compared as per-member service time or summed repeatedly.

The fixture published 100 writes through several HTTP requests, hence 14
transactions rather than simply ceil(100/8). It does not wait to fill groups.
The scalar Entry fixture and a three-pair sample do not establish that 8 is the
best size, or that production single-write/tail latency improves. The prototype
has no time budget for slow handlers.

All client/server final identities, content and 100 accepted outcomes passed.
The six traces aligned within ±29 microseconds. There were no backend/transport,
cleanup or trace-loss errors. Permitted transient owned-create divergence reports
were 291 control and 264 grouped; `audit.mjs` validates rather than hides them.

Apple M5 Max, macOS arm64, Node 24.19.0, Dart 3.12.1, Rust 1.98.1 and PostgreSQL
16.14; fresh PostgreSQL database, Dart process and SQLite files per sample.
Fixture setup, compilation and clock calibration are excluded; the Node fixture
process and OS caches are reused. Tracing is on in both conditions. This is
local HTTP/WebSocket loopback, not mobile, UI, WAN or remote-DB evidence.
The measured working tree was dirty at parent `294f356`; source/artifact hashes
are retained. Later documentation/comment edits do not change measured behavior.

## Verification and reproduction

27 native/PostgreSQL Batch regressions passed, including five new group tests for
separate member cursors and nested preparation, typed refusal, whole-group
failure, retained committed prefixes, short final groups and duplicate replay.
All 42 Rust protocol/server tests, 19 focused persistence tests and 22 bridge/
performance-harness tests passed. TypeScript and strict Rust/Node Clippy passed.
No full host/device gate or grouped Prisma/Drizzle matrix was run.

Build release native/Dart artifacts with the normal performance setup. Then,
against a disposable localhost `DATABASE_URL`, from the repository root:

```sh
cp integration/performance/baselines/2026-10-28-mutation-groups/runner.mjs \
  artifacts/performance/group-runner.mjs
node integration/performance/run.mjs --record-build
node artifacts/performance/group-runner.mjs --samples 3 --sizes 1000 \
  --writes 100 --timeout-ms 60000 --fail-fast on \
  --output artifacts/performance/mutation-groups.json
node integration/performance/baselines/2026-10-28-mutation-groups/audit.mjs
```

The frozen runner selects the experiment through
`AXTON_EXPERIMENTAL_MUTATION_GROUP_SIZE` in the fixture; the supported backend
option is `protocol5.experimentalMutationGroupSize`. Reproduction of the
retained audit reads the archived raw output, checks the runner fingerprint and
every final-state/clock/report result, and regenerates `evidence.json`.
