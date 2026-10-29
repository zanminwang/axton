# Persistence

## 1. Introduction and Goals

Persistence implements the strict Rust host operation contract inside the application transaction. Store binds principal/Stream and Batch progress; MutationResult retains member outcomes; StreamRecord retains explicit interest and current positions. Delivery responses are not persisted. Publication locks and reserved cursors are transaction-owned. The fresh namespace contains only canonical protocol-5 tables; old framework layouts are refused intact.

## 5. Building Block View

[Implementation](../../../../packages/backend/postgres/migration.sql) owns this component. [Protocol 5](../protocols/sync.md) owns shared context, delivery and settlement rules.

PostgreSQL adapters validate known incompatible layouts once during database
readiness, before `backend.listen` opens its port. Headless operations retain
first-use validation; caller-owned publication can validate using its transaction.
The adapter is bound to a stable database/role/schema; normal dispatch does not
re-scan catalog tables. Standalone `answer` retains its guard for callers without
an adapter lifecycle. [Database readiness](../../../../website/docs/backend/database.md#database-readiness)
owns startup/retry usage. [Startup regressions](../../../../integration/persistence/server/database-startup.test.mjs)
cover concurrent initialization, refusal before serving/business work, retained
transactions and absence of repeated scans; the real PostgreSQL namespace suite
checks refusal without data changes and 100 reads after one validation.


Bulk identity guards, member application and positional reads use bounded SQL batches inside the same application transaction. Guards retain canonical order across mode changes; positional reads retain duplicate inputs and missing targets. [PostgreSQL batching tests](../../../../integration/persistence/server/persistence-batching.test.mjs) check query growth at 2,055 identities, result correlation, cursor reuse, savepoint rollback and safe counters. Alternating guard modes require separate runs to preserve lock order; general cursor reservation uses one statement per published Stream. A bounded
single-Stream publish-only run reserves and applies membership together through
CTE `RETURNING`; its temporary-table setup remains separate.

Existing Store claims take their progress lock with one query. Missing Stores
retain insert/conflict recovery and re-read the binding under the transaction's
snapshot; serialization failures retry the whole transaction. Batch admission
and old-outcome pruning share one statement, with pruning conditional on
successful admission. Result insertion and guarded progress advancement also
share one statement: a failed guard inserts nothing, and an insertion error
rolls back the progress update. Caller savepoints and outer transactions retain
their rollback boundaries. [Mutation round-trip tests](../../../../integration/persistence/server/mutation-roundtrips.test.mjs)
exercise query counts, concurrent first claims, final Batch promotion and failed
guards, insertions, pruning and outer transactions against PostgreSQL.

The adapter advertises `mutationCommitGroups` support. Publication reservations
include a scope in their connection-local temporary key: ordinary transactions
use the empty scope, and grouped Mutations use their stable call identity. This
preserves distinct member cursors while reusing one cursor for that member's
nested preparation/publication. Reservations still roll back with savepoints and
clear on COMMIT; durable framework tables require no migration.

Grouped execution locks Store progress once per outer transaction. PostgreSQL
advertises `mutationResultGroups`: `saveResults` inserts a bounded contiguous
outcome suffix and advances/promotes Batch progress in one statement. Its guard
checks Batch ID, starting ordinal, total count and digest. Failed guards insert
nothing; any insertion failure rolls back the entire statement, including its
progress update. Business savepoints and group COMMIT/rollback boundaries remain
unchanged. Group-capable adapters without this capability receive sequential
`saveResult` calls from the SDK in the same outer transaction. The
[grouped SQL experiment](../../../../integration/performance/baselines/2026-10-28-group-sql/README.md)
retains the controlled comparison and query counts.

The single-Stream reserve/apply statement preserves scoped cursor reuse, input
order and overflow refusal without member writes. Multi-Stream, mixed and large
runs retain the general path. [Publication SQL evidence](../../../../integration/performance/baselines/2026-10-28-publication-sql/README.md)
records the remaining-query breakdown, short adapter comparison and savepoint/
namespace regression coverage.

Tracking discovery and its post-lock recheck always execute independently. A
singleton explicit pair uses exact model/identity/Stream predicates; a singleton
global identity uses the record identity and live-holder indexes. Multi-key or
mixed input retains the general deduplicating JSON/UNION path. Empty input
needs no tracking statement. No tracking result is cached between the two
phases. [Tracking SQL evidence](../../../../integration/performance/baselines/2026-10-28-tracking-sql/README.md)
records indexed/general equivalence, real phase ordering and the short comparison.

Singleton target/position reads use exact identity and Stream predicates with a
one-row LEFT-join input. Missing metadata or membership still yields one nullable
slot: `targetPositions` returns null and `readPositions` refuses missing positions.
Remove kinds/cursors, duplicate slots and ordering across batch boundaries remain
unchanged. [Position-query evidence](../../../../integration/performance/baselines/2026-10-28-position-sql/README.md)
records the regression coverage and short comparison, including wall-time variance.
