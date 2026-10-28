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


Bulk identity guards, member application and positional reads use bounded SQL batches inside the same application transaction. Guards retain canonical order across mode changes; positional reads retain duplicate inputs and missing targets. [PostgreSQL batching tests](../../../../integration/persistence/server/persistence-batching.test.mjs) check query growth at 2,055 identities, result correlation, cursor reuse, savepoint rollback and safe counters. Alternating guard modes require separate runs to preserve lock order; cursor reservation remains one statement per published Stream.
