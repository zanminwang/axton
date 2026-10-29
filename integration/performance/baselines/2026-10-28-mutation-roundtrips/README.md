# Mutation persistence round trips

The control uses `persistence.mts` and `sql.mts` exactly as retained at
`294f3563dc42ce5c0d875cbf141d425b65399f52`. The optimized condition uses the current
adapter. Both run through the same generated backend, native addon, release Dart
library and Dart AOT clients. The fixture accepts a database factory so only the
adapter changes between conditions. Bootstrap prefetch stays disabled.

## Results

All **30/30 samples passed**, with all 30 common-axis traces aligned. Maximum
clock placement uncertainty was ±28 microseconds. Every single-write control
issued 17 framework SQL calls in the named Mutation; every optimized sample
issued 14. Across all its backend transactions, each single-write sample used
41 control queries versus 38 optimized queries. These counts exclude BEGIN,
COMMIT and business Handler/Loader SQL.

Times are milliseconds, median [minimum, maximum], with five runs per condition:

| N = 10,000 metric | Control | Optimized | Median change |
| --- | ---: | ---: | ---: |
| Initial first visible | 56.901 [55.610, 89.912] | 56.209 [54.952, 58.226] | -1.2% |
| Initial all visible | 1038.328 [1013.122, 1054.653] | 1022.128 [1014.771, 1028.424] | -1.6% |
| Single-write B visible | 13.697 [11.824, 14.628] | 13.347 [11.421, 13.531] | -2.6% |
| Single-write server transaction | 8.824 [7.062, 9.540] | 8.634 [6.656, 8.743] | -2.2% |
| Single-write A settlement | 14.268 [11.656, 14.469] | 13.364 [12.014, 14.124] | -6.3% |
| 100 writes, all B visible | 732.952 [720.227, 736.755] | 720.129 [713.372, 724.477] | -1.7% |

The initial samples used exactly 138 framework queries in both conditions.
The 100-write samples used a median 3,132 [3,132, 3,138] total backend framework
queries in control and 2,935 [2,929, 2,935] optimized. Reduced query counts do
not translate proportionally into elapsed-time savings on this loopback host.
Within the 100 named Mutation transactions themselves, every control run used
1,506 queries and every optimized run used 1,303.

Single-write paired optimized-minus-control B visibility differences had median
-0.318 ms, range [-2.982, +1.461] ms; one of five pairs favored control. The
100-write completion difference was negative in all five pairs: median -11.083
ms, range [-13.321, -6.855] ms. These small samples do not establish a stable
single-write improvement. The pooled per-write A-settlement median during the
burst increased from 412.2245 to 419.7965 ms (+1.8%); that regression is retained
and its correlated write observations are not independent run samples.

Both conditions retain 485 permitted transient own-create divergence reports
during the burst. Final client/server identities, content and accepted outcomes
passed; there were no backend/transport errors, trace loss or cleanup errors.

Host: Apple M5 Max, macOS arm64, Node 24.19.0, Dart 3.12.1, Rust 1.98.1,
PostgreSQL 16.14. Complete toolchain, database settings and artifact fingerprints
are in `evidence.json`. The measured working tree was dirty at parent `294f356`;
its source hash is retained, and `runtime.patch` records the runtime changes.
This machine/toolchain differs from older archives, so these timings are not
compared to their absolute values.

## Changes

- Existing Stores take their progress lock without a preceding no-op INSERT.
  Missing Stores use three queries instead of two: inspect/lock, insert and
  inspect/lock again. Concurrent claims still retry at Serializable isolation.
- Batch admission and pruning older outcomes use one guarded statement.
- Saving an immutable outcome and advancing progress use one guarded statement.
  A failed progress guard inserts no result; insertion failure rolls back the
  progress update. Savepoints and independent member transactions remain.

## Method

Five fresh runs per condition for each N = 10,000 case: initial synchronization,
one live create and 100 continuous creates. Control/optimized and
optimized/control adjacent pairs alternate. Each sample gets a fresh PostgreSQL
database, Dart process and SQLite files. Fixture setup, compilation and clock
calibration are outside the existing measurement boundaries. OS/database caches
are not flushed. The Node fixture process is reused as in the existing runner.

The fixture has one Entry watch, 512-byte text and publication groups of 100.
Initial visibility starts before Store open; live writes await local acceptance
and begin the next write without waiting for settlement. Client/server final
identity and content assertions, accepted outcomes, source-clock settlement
correlation and bounded clock alignment use the existing performance harness.
Tracing is enabled in both conditions. This is a desktop loopback experiment;
it does not measure phones, UI rendering, WAN or remote-database latency.

## Reproduction

From the repository root with Node, Rust, Dart, Python and PostgreSQL on PATH:

```sh
bash integration/performance/baselines/2026-10-28-mutation-roundtrips/run.sh \
  --samples 5 --sizes 10000 --writes 100 --timeout-ms 120000 \
  --fail-fast on --output artifacts/performance/mutation-roundtrips.json
```

The shell extracts the two original adapter sources from the fixed control
commit and copies `runner.mjs` into the ignored artifacts directory, where its
repository-relative imports resolve. It builds release/AOT artifacts and records
their source/artifact hashes before measuring. The runner retains failed or
interrupted samples rather than excluding them. Temporary client files and
databases use the existing cleanup path; the shell stops its disposable cluster.

`results.json.gz` retains the complete measurement output. `audit.mjs` verifies
every sample, frozen runner and control-source hashes, derives the comparison
and per-mutation SQL counts, and regenerates each condition's median single-write
trace from one complete sample:

```sh
node integration/performance/baselines/2026-10-28-mutation-roundtrips/audit.mjs
```

Five observations per case describe this run; they do not establish a universal
speedup or independent-run p95. Per-write burst observations share a run and are
correlated. SQL spans include execution and wait; nested/overlapping spans cannot
be added into exclusive end-to-end components.

## Correctness checks executed

The targeted PostgreSQL regression passed all eight tests for query budgets,
concurrent first claims, progress guards, final promotion, insertion/pruning
failure and caller/outer rollback. Existing namespace/lock (9), bulk persistence
(11), native Batch (22), publication semantics (4), pg/Prisma/Drizzle Batch (3),
caller savepoint (18) and read-liveness (14) tests also passed: **89 database
tests total**. Commands use the disposable local `DATABASE_URL`:

```sh
node --test --test-timeout=120000 integration/persistence/server/mutation-roundtrips.test.mjs
# Run these files serially because their fixtures can share the public namespace:
for file in namespace-and-locks persistence-batching protocol-v05-batch \
  publication-semantics protocol-v05-drivers publication-savepoint read-liveness; do
  node --test --test-timeout=120000 "integration/persistence/server/$file.test.mjs"
done
node node_modules/typescript/bin/tsc -b scripts/release/tsconfig.json
node node_modules/typescript/bin/tsc -p tsconfig.json --noEmit
node --test integration/performance/*.test.mjs
```

The full TypeScript check passed after generating the existing Todo and E2E
Prisma clients; all 20 performance-harness tests passed. The initial missing
Prisma-client setup failures were resolved before the successful checks. Release
native/Dart artifacts were built. The full host/device gate was not run.
