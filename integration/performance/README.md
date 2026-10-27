# Sync performance

Manual Performance E2E through generated Dart, release native artifacts, the real
backend, HTTP/WebSocket, PostgreSQL and SQLite. It verifies state as well as time.
It is separate from simulation and has no performance threshold.

## Run

From the repository root with Rust, Node, Dart and PostgreSQL tools on PATH:

```sh
bash integration/performance/run.sh
```

This builds the release compiler, Dart library and Node addon, generates the SDK
and AOT-compiles both Dart programs before measuring. Defaults are five repetitions,
N = 100 / 1,000 / 10,000 existing Entries and live M = 1 / 100 writes, all serial.
Entries have 512 ASCII bytes and fixed-width identities. Fixture publication groups
contain 100 Entries: publishing all N at one cursor would force one atomic group
and obscure incremental first-visible behavior.

```sh
bash integration/performance/run.sh --samples 5 --sizes 100,1000,10000 \
  --writes 100 --timeout-ms 120000 --output artifacts/performance/baseline.json
```

The runner creates a temporary PostgreSQL cluster, then a new database, backend,
Dart process and client directory per sample. Compilation, database/fixture setup,
backend startup and final verification are untimed. It closes clients, listeners,
pools and files and drops the sample database. The shell stops its cluster on exit.
Timeouts terminate the child, with a KILL fallback. Database initialization errors
retain their log in `artifacts/performance/setup-failure.json`.

`AXTON_DART` selects a Dart executable. `AXTON_PERFORMANCE_SKIP_BUILD=1` requires
matching source/artifact fingerprints. `--diagnostics off` supports a separate
overhead experiment; its results are not directly compatible with instrumented runs.

The existing Verify workflow also accepts a manual performance invocation:

```sh
gh workflow run verify.yml --ref YOUR_BRANCH -f performance=true
```

It runs on Ubuntu, uploads `artifacts/performance/*.json`, and skips the host gate
for that invocation. Push/PR jobs do not run benchmarks. Shared GitHub runners
describe their recorded environment, not dedicated hardware or phone performance.

## Timing and assertions

| Scenario | Metric | Boundary |
| --- | --- | --- |
| Initial | Store open | Immediately before `GeneratedClient.open` until its Future returns. |
| Initial | First visible | Initialization start until the first watch snapshot containing expected fixture data; empty callbacks do not count. |
| Initial | All visible | Initialization start until the watch contains N Entries. |
| Initial | Bootstrap | Initialization start until `bootstrap()` returns. |
| Live | Local commit | Before submitting each Mutation until its Call handle returns after local commit. |
| Live | Settlement | That write's start until `Call.wait()` reports accepted local settlement, not the server's commit timestamp. |
| Live | B visible | That write's start until B's watch contains its identity. |

Initial timing starts before application-data configuration/native-library loading,
then opens an empty Store, registers a watch, connects and requests Bootstrap.
Process launch and code loading before Dart main are excluded. Raw milestones also
retain application configuration, connect start/return and Bootstrap request times.
Connect return alone does not establish handshake completion.

Live initializes A and B and registers B's watch before a runner start barrier.
Both clients use one Dart Stopwatch. A awaits each local commit, then submits the
next write without awaiting settlement. B visibility can precede A's settlement.
Whole-run completion and throughput are also retained. Watches intentionally read
and decode the full Entry model, so varying N exposes retained-data notification cost.

Each sample verifies exact identities, counts and every text value in the final
watch and local queries. Live verifies both clients, accepted settlement outputs,
A's empty queue, handler executions and PostgreSQL content. Backend/connection
errors and trace loss fail the sample. A fast but incorrect sample never passes.

## Results

JSON retains raw samples, partial progress, failures, timeouts and traces, plus
commit/dirty state, source/artifact hashes, tool versions, machine information,
PostgreSQL settings and scenario parameters. It checkpoints after each sample.

```sh
node integration/performance/report.mjs artifacts/performance/baseline.json
node integration/performance/report.mjs compare before.json after.json
node --test integration/performance/*.test.mjs
```

Summaries give median and min/max. Nearest-rank p95 appears only with at least 100
observations. Live per-write observations are correlated within each run, rather
than independent experiment repetitions. Stage summaries contain summed completed
span duration per run, counts and maximum duration. Nested/overlapping stages cannot
be added to reconstruct wall time.

Comparison requires matching fixture, timing, tracing/cache conditions, scenario
matrix and machine/tool/database metadata, and no failed samples. Commits, artifacts
and sample counts may differ. Curated results belong in `baselines/`; routine
output stays under ignored `artifacts/`.

Stores, processes and databases are fresh; fixtures were just written. OS and
PostgreSQL caches are **not flushed**. This is not a cold-hardware-cache claim.
Loopback has no simulated WAN latency. These text-only cases do not establish
media, Flutter layout, phone storage or Most Days authorization performance.

## Diagnostics and ownership

Dart `Client.open` / generated `GeneratedClient.open` and backend `createBackend`
accept optional `diagnostics` receivers with `record(record)`. Unconfigured instances
install no native collectors or trace bridge traffic. Rust uses standard `tracing`
spans scoped to instances/requests, following native async polls across threads;
AXTON never installs a process-global subscriber.

Schema-1 records contain kind, name, source, clockDomain, startUs, optional durationUs,
span/parent identifiers and attributes. Source execution measures monotonic time;
bridge receipt is not the source timestamp. A native span's identity is the pair
(clockDomain, spanId). Stage names/attributes are experimental. Receivers return
promptly; exceptions are ignored. Receivers/exporters own file or network I/O.

Stages cover Store open, SQLite COMMIT, complete-unit apply, watch publication,
Dart watch dispatch/HTTP intervals, server native operations, preparation,
materialization/persistence, Mutation execution, transactions and host callbacks.
Native buffers cap records at 4,096 per drain/request and explicitly count loss;
test recorders also cap retention and reject incomplete evidence.

Client traces use the existing runtime outbox. Enabled backend native operations
deliver records once through their existing host callback, outside business
declaration handling. Sync state and wire authority never depend on telemetry.
Client milestones share a clock; server/native clocks remain separate. Identifiers
support correlation, never subtraction across clock domains. HTTP duration includes
server and transport work; no residual is labeled measured network-only or CPU time.

Scenario files own sequencing/assertions; `support/recorder.dart` collects records;
`fixture/` owns the shared contract/backend; `run.mjs` owns experiments/metadata;
`report.mjs` owns statistics. Historical [Channel evidence](channel-removal/README.md)
is unchanged. The old Scope removal runner called retired APIs and was removed.
