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
# Small tracing-disabled control; not directly comparable to instrumented data.
gh workflow run verify.yml --ref YOUR_BRANCH -f performance=true \
  -f performance_samples=2 -f performance_sizes=100 -f performance_diagnostics=off
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
| Live | Server transaction | Server transaction entry until the adapter returns after COMMIT, including retries. Its end is a server commit confirmation in the Node clock domain. |
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
With diagnostics enabled, each CreateEntry's host call ID/ordinal correlates its
successful transaction span to the write. `serverSettlements` retains that source
start, duration and commit-confirmed timestamp. Older raw files retain the same
evidence in spans; the report reconstructs it. This server duration is not an
A-to-server latency. No clock synchronization is claimed between Dart and Node.

Each sample verifies exact identities, counts and every text value in the final
watch and local queries. Live verifies both clients, accepted settlement outputs,
A's empty queue, handler executions and PostgreSQL content. Backend/connection
errors and trace loss fail the sample. A fast but incorrect sample never passes.

The v2 measurement contract distinguishes a nonterminal `AxtonReport` from a
transport failure. An owned create may emit `diverged` when its canonical
materialization arrives before full Stream coverage, then settle successfully.
Live samples retain and count these reports only when they come from A during
measurement, name that exact Entry/ordinal, and the Entry's content and accepted
settlement are independently verified. Other reports fail. No reported sample is
silently discarded; summaries/comparisons show report counts alongside timings.

## Results

The first [Dart measurements and findings](baselines/2026-10-27-dart/README.md)
include a full 45-sample matrix and a tracing-disabled control. Initial sync and
single writes passed; continuous writes exposed backend serialization/connection
failures and are retained as failed diagnostic evidence.

Those original measurements used the former global publication fence. Current
code removes its database writes and native call sites while retaining
Serializable isolation, record/Stream guards, Store ordering and bounded retries.
Raw results identify the implementation/commit; reruns are separate evidence,
never replacements for the original samples. The old bridge request and public
helper are removed. Fresh namespaces omit the old table; reapplying the migration
retires it from current protocol-5 namespaces. The one-off historical lock probe
and its runner/workflow entry points are removed; archived evidence stays at its
recorded source commit.
The [fence-removal results](baselines/2026-10-27-fence-removal/README.md) retain
the second full matrix, comparisons and remaining Store/delivery-plan conflicts.
The [Store snapshot-read results](baselines/2026-10-27-store-reads/README.md)
retain the third matrix. Initialized reads no longer claim Mutation progress;
uploads retained ordering with a foreign-key-compatible lock. In that Store-only
experiment all burst cases still failed, and backend errors rose from 228 to 270. The
[read concurrency audit](../../docs/engineering/architecture/server/read-concurrency.md)
also records local callback scheduling limits. Current code removes server
payload persistence and old-plan continuations, returning complete cursor pages
from current state. The timing boundaries and fixture parameters remain the same;
the [cursor-page measurements](baselines/2026-10-27-cursor-pages/README.md)
retain the fourth matrix: 40/45 passing, zero backend errors, and five N=10,000
burst failures from B's bounded delivery queue. Initial-only comparison passes;
the full failed matrix remains diagnostic evidence.

Current clients coalesce newly observed live responses into validated head hints
while ordinary delivery is staged. An HTTP wait alone does not block an eligible
live response. Admitted fragments finish, then fresh repair reads from committed progress. The queue
limits and watch workload are unchanged. `delivery.coalesce` spans count these
offers. The [initial coalescing experiment](baselines/2026-10-27-live-coalescing/README.md)
passed 45/45 cases and removed the previous queue errors. It also exposed an
HTTP-wait liveness boundary; current code admits a live response that can advance
the committed prefix even while HTTP waits. That archive predates this refinement
and is not a speed claim for it.
The [final live-prefix measurements](baselines/2026-10-27-live-coalescing-v2/README.md)
passed 45/45, with full Linux/macOS Verify at the measured runtime. Its recovered
raw samples are now archived and audited. The [startup-layout measurements](baselines/2026-10-27-startup-layout/README.md)
passed another 45/45: the 10k burst completed on B in 3.47 s [3.31–3.59 s].
The immediate prior 5.61 s run used a different CPU, so that pair is descriptive.
A strict compatible comparison with the earlier 6.61 s baseline passed, covering
both the live-prefix refinement and startup layout validation. Model-watch
invalidation remains unchanged; its internal costs are the next investigation.


JSON retains raw samples, partial progress, failures, timeouts and traces, plus
commit/dirty state, source/artifact hashes, tool versions, machine information,
PostgreSQL settings and scenario parameters. It checkpoints after each sample,
continues through failures to collect the matrix, and exits nonzero if any sample
fails. `matrixCompleted` distinguishes an attempted full matrix from a partial run;
only a fully passing matrix has `completed: true`. `--fail-fast on` stops after the
first failure. Backend errors retain their message, SQLSTATE and stack separately
from final-state verification, so recovered writes do not hide connection defects.

```sh
node integration/performance/report.mjs artifacts/performance/baseline.json
node integration/performance/report.mjs compare before.json after.json
node integration/performance/report.mjs compare before.json after.json --scenario initial-sync
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
The report command also reads `.json.gz` archives without extracting them.
An explicit `--scenario` can compare a fully passing scenario from two fully
attempted matrices even if another scenario failed. It never drops failed
repetitions within the selected scenario or accepts a partially attempted matrix.

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

Applications opt in through their existing entry points:

```dart
class MyDiagnostics implements AxtonDiagnostics {
  @override
  void record(AxtonTraceRecord record) {
    // Buffer or forward to an application-owned exporter; return promptly.
  }
}
final client = await GeneratedClient.open(
  path: storePath, stream: stream, diagnostics: MyDiagnostics(),
);
```

```ts
const backend = createBackend({
  ...options,
  diagnostics: { record(record) { exporter.enqueue(record); } },
});
```

The receiver is optional for customers. AXTON supplies stage observations; the
application chooses retention and export. This does not add a new sync entry point
or require a telemetry service.

Schema-1 records contain kind, name, source, clockDomain, startUs, optional durationUs,
span/parent identifiers and attributes. Source execution measures monotonic time;
bridge receipt is not the source timestamp. A native span's identity is the pair
(clockDomain, spanId). Stage names/attributes are experimental. Receivers return
promptly; exceptions are ignored. Receivers/exporters own file or network I/O.

Stages cover Store open, SQLite COMMIT, complete-unit apply, watch publication,
Dart bridge drain/decode, watch enqueue/HTTP intervals, server native operations, preparation,
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

### Detailed phase diagnostics

Stage instrumentation version 2 separates `handler.invoke`, framework
`postgres.query`, pg pool acquisition, BEGIN/COMMIT/ROLLBACK and transaction-body
work. Node host JSON encode/decode and Rust callback encode/wait/decode remain in
separate source clock domains. `host.await` includes Node scheduling, handler/SQL
execution and response waiting; it is not a measured queue-only delay. Framework
SQL spans record statement kind, not SQL text or parameter values. Application
SQL inside handlers is included in `handler.invoke` but is not separately traced
by the framework. Prisma/Drizzle own their commit internals: only the pg adapter
reports isolated transaction commands; no residual is labeled ORM COMMIT time.

Ordinary Model-watch reruns contain `watch.read` (SQL plus native row assembly),
`watch.convert` (JSON Value conversion), `watch.compare` (equality) and
`watch.snapshot` (snapshot construction/copy). `bridge.nativeDrain` includes the
FFI drain, native serialization, copying and freeing; `bridge.decode` isolates
Dart JSON parsing. Existing `watch.enqueue` measures listener dispatch/enqueue,
not later asynchronous UI work or generated Model-object conversion.

`watch.invalidate` records the runtime cause, generations and last commit's
written tables. A generation jump greater than one can contain several commits:
`lastCommitTables` is not their cumulative written set. It must not be used alone
to skip watches across that jump. These records investigate metadata-only
refreshes without changing invalidation, transaction or notification semantics.
SQL-watch internals are not split by these Model-watch spans.

The raw compatibility contract records the instrumentation version. New detailed
runs are deliberately incompatible with earlier instrumented timings; retain a
separate disabled-diagnostics control when estimating instrumentation overhead.
Summed stages overlap and cannot be added into an end-to-end latency breakdown.
