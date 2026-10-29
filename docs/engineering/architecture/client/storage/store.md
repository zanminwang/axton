# Store

## 1. Introduction and Goals

SQLite gives each physical file one exclusive owner across aliases and processes. Unsupported-format admission checks a temporary database/WAL copy before modifying the original. SQL transactions and savepoints commit Model rows, retained input, replay evidence and progress together. Closing releases ownership; normal reopen neither wipes nor abandons pending work.

## 5. Building Block View

[Implementation](../../../../../crates/sqlite/src/lib.rs) owns this component. [Protocol 5](../../protocols/sync.md) owns shared context, delivery and settlement rules.


Fresh admission installs the canonical layout. Reopen requires its supported layout and preserves pending replay and observer filtering; it does not create old-name compatibility views or upgrade a provisional layout in place.

SQLite read and single-write statements use connection-local prepared-statement
caches, bounded by rusqlite's default capacity (16 per writer/reader connection).
Results and parameter bindings are not retained between calls. Schema changes
may trigger SQLite reprepare; column metadata is read after stepping. Watch
dependency discovery prepares afresh under its temporary authorizer. See the
[cache regressions](../../../../../crates/sqlite/tests/statement_cache.rs) and
[short performance comparison](../../../../../integration/performance/baselines/2026-10-28-sqlite-statements/README.md).
