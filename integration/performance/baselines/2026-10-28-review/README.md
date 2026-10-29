# Sync optimization summary and design review

This dated review covers the changes after `294f356` on `codex/sql-roundtrips`.
It records the completed experiments and remaining design work. The current
[server contract](../../../../docs/engineering/architecture/server/protocol5.md),
[persistence contract](../../../../docs/engineering/architecture/server/persistence.md)
and [guarantees](../../../../docs/engineering/guarantees.md) own lasting behavior.

## Implemented changes

The backend runs existing HTTP Batch members sequentially in bounded commit
groups. Current native/PostgreSQL backends default to eight; unsupported adapters
retain one and explicit size one selects per-member commits. Every member still
authorizes, owns its business savepoint and retains a separate publication
cursor. Store admission is shared and new outcomes are persisted as one guarded
contiguous suffix. Business refusals stay local to a savepoint; infrastructure
failure rolls back the current group. Earlier committed groups survive replay.
This changes the default durability boundary and must be included in release
notes. It introduces no wait-to-fill delay or parallel business execution.

PostgreSQL combines admission/pruning, outcome/progress and eligible single-Stream
reservation/publication statements. Singleton tracking and target lookups use
exact indexed predicates. General multi-key/multi-Stream paths remain available.
Tracking discovery and the post-lock recheck are both retained without caching.
No durable schema migration or index was added.

Clients now prefetch one successor Bootstrap page by default. Canonical JSON
writes to one buffer while retaining existing ordering and scalar encoding.
SQLite uses its bounded connection-local prepared-statement cache; authorizer
inspection still prepares freshly and schema changes refresh row metadata.
Stage spans identify delivery encoding, local apply and reconciliation costs.

## Measurements

Each row below is its own controlled local experiment. Rows use different
intermediate builds and must not be combined into one cumulative speedup.
Times are milliseconds. Except where stated, there are two runs per condition.

| Experiment and retained evidence | Before → after | Interpretation |
| --- | --- | --- |
| [Admission/progress SQL](../2026-10-28-mutation-roundtrips/README.md), five runs per condition at 10k | Single-Mutation framework SQL 17 → 14; B visible 13.697 → 13.347 | Small isolated-write benefit. |
| [Commit groups 1 → 8](../2026-10-28-mutation-groups/README.md), three runs per condition at 1k | All 100 settled 664.537 → 473.542 | Burst completion improved 28.7%; pooled per-write settlement median worsened 8.2%. Eight is not established as optimal. |
| [Bootstrap prefetch](../2026-10-28-prefetch-current/README.md), 10k | All visible 1013.753 → 661.436 | Nineteen successor requests overlap predecessor apply. |
| [Server canonical encoding](../2026-10-28-page-response/README.md), 10k | All visible 671.087 → 583.360 | Server planning 395.643 → 263.895; first-visible time did not improve. |
| [Client encoding rebuild](../2026-10-28-client-encoding/README.md), 10k | All visible 617.071 → 505.346 | Apply 271.259 → 232.921. |
| [SQLite statement cache](../2026-10-28-sqlite-statements/README.md), 10k | All visible 493.310 → 480.546 | Apply 228.922 → 144.114; much of apply overlaps downloading. |
| [Grouped admission/results](../2026-10-28-group-sql/README.md), 10k/100 writes | All settled 514.662 → 497.291 | Mutation framework SQL 1303 → 1131; claims/results each 100 → 14. |
| [Combined publication SQL](../2026-10-28-publication-sql/README.md), 10k/100 writes | All settled 497.878 → 444.279 | Publication SQL 300 → 200; publication time 128.058 → 78.002. |
| [Indexed tracking](../2026-10-28-tracking-sql/README.md), 10k/100 writes | All settled 469.033 → 357.272 | Tracking remains 200 calls; elapsed 134.119 → 24.591. |
| [Indexed positions](../2026-10-28-position-sql/README.md), 10k/100 writes | All settled 351.151 → 327.452 | Position reads remain 100; elapsed 57.508 → 14.143. End-to-end pairwise savings vary from 1.101 to 46.297. |

The [same-build snapshot](../2026-10-28-performance-summary/README.md), collected
before the last four SQL optimizations, measured 10k first-visible/all-visible
at 43.424/449.738, and isolated-write local commit/A settlement/B visibility at
0.544/14.680/14.080. Those scenarios were not remeasured after the final SQL
changes. The latest 100-write optimized runs span 289–366 ms; 289 ms is not a
stable result. Publication remains the largest measured SQL category, about
86.5 ms across 100 writes. Overlapping stage spans cannot be added to wall time.

All measurements use Apple M5 Max desktop loopback, 512-byte Entry text, fresh
databases/client files and instrumented SDK watches. Setup, compilation and
process launch are excluded; OS/database caches are not flushed. They establish
neither mobile/WAN/UI performance nor independent-run p95 or a PowerSync ranking.
Every archive retains raw reports, source/artifact identities and an audit.
The [defaults smoke](../2026-10-28-defaults/README.md) separately verifies omitted
configuration, rather than establishing a speedup.

## Maintainability and design debt

The ownership split remains useful: Rust owns protocol admission and settlement,
the SDK owns transaction/Handler integration, and adapters own SQL. The principal
maintenance cost is now keeping the grouped path consistent across those layers.
This review found documentation contradictions about transaction cursor scope;
the owning server and website pages now describe per-member scopes correctly.

| Priority | Observed condition and consequence | Recommended follow-up |
| --- | --- | --- |
| 1 | The default-on group option still has an experimental name; effective size depends on optional native functions and adapter capabilities. The client default also lives in core construction and native environment handling. Configuration is harder to inspect and reason about across versions. | Normalize scheduling configuration once per boundary, expose the resolved mode in diagnostics, and introduce a stable option with a compatibility alias. Preserve explicit size one. |
| 1 | Rust provisional Store progress mirrors guarded SQL promotion rules; single-result and grouped-result paths coexist. The Rust/TypeScript host operation unions are maintained manually. Future changes can drift across implementations. | Add generated or differential operation-contract checks and state-machine tests comparing grouped/sequential outcomes across replay, refusal, partial prefixes and infrastructure aborts. Keep protocol ownership in Rust and SQL guards in persistence. |
| 1 | Eight bounds member count, not transaction duration. Slow application Handlers can extend locks and group retry work; the initial experiment also worsened pooled per-write settlement. | Measure group duration, retry amplification and independent-run tail latency before changing size again. Evaluate a policy for ending groups between members; do not promise a deadline during arbitrary Handler execution. |
| 2 | Singleton/general SQL paths add a bounded selection matrix. They preserve semantics today, but new membership kinds or batching rules must update both. | Extend existing equivalence coverage when adding cases; retain missing/null slots, ordering, overflow, savepoint rollback and post-lock rechecks. Avoid cross-member tracking caches. |
| 2 | Experiment archives copy runners with local absolute imports and sometimes retain partial patches or intentionally older control binaries. Audits preserve collected evidence, but a new machine cannot replay every experiment directly from the current tree. | Build one portable, parameterized experiment harness with explicit variant source/artifact identities. Keep historical raw reports and runner hashes immutable. |
| 2 | Most comparisons have only two repetitions and varying host load. Full-model watches and overlapping spans obscure independent costs. | Use more isolated, interleaved repetitions and separate SQL elapsed, transaction duration, delivery work and SDK/UI visibility. Record tail estimates only from enough independent runs. |

The next performance investigation should profile publication's remaining
temporary-state setup, membership write and native/host crossings. A faster path
must retain rollback-safe scoped cursor reuse and the locked recheck. The next
design cleanup should make effective configuration and cross-language contracts
observable and testable before adding more fast paths. No broad refactor is
included in this performance PR.

## Aggregate verification

Executed for the combined changes: `cargo test --workspace --locked`, workspace
Clippy with `-D warnings`, and `bash integration/v05-sdk/run-host.sh` (21 passing
acceptance tests, including real Dart/HTTP/WebSocket/SQLite/PostgreSQL), and
`bash integration/persistence/server/run.sh` (189 passing tests across pure host
and real PostgreSQL suites, including pg/Prisma/Drizzle). All 20 performance
harness tests, TypeScript checking, formatting and all 12 new evidence audits
passed. Changed Markdown file links and heading fragments were checked; the
website strict build and all 955 built internal links/assets passed, as did its
three snippet-checker unit tests and `python3 website/scripts/check_examples.py`
(maintained TypeScript/Dart snippets and compiled schema examples). The
per-experiment archives record the additional focused SQL, replay, savepoint,
statement-cache, prefetch and harness checks executed during development.
The complete `scripts/test.sh`, mobile/device gates and installed-release matrix
were not run for this final review. Performance runs used release artifacts;
aggregate correctness gates rebuilt debug artifacts and do not constitute new
performance samples.
