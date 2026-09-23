# Store

## 1. Introduction and Goals

The store is the only thing in the client that talks to a database. The engine owns every SQL statement; the store owns connections, transactions and value conversion. Keeping the contract this small lets the same engine run over any SQL store, and keeps every rule about sync out of the database layer.

## 3. Context and Scope

The contract, `ClientStore`:

| Group | Operations | Notes |
| --- | --- | --- |
| Transaction | `begin`, `commit`, `rollback` | one writer transaction at a time |
| Savepoints | `savepoint(name)`, `release(name)`, `rollback_to(name)` | nested scopes inside the transaction |
| Writes | `execute(sql, params)`, `execute_batch(sql)` | rows affected |
| Reads | `query(sql, params)` inside the writer's view; `query_committed(sql, params)` last commit only | columns and rows |

Callers: the engine only ([Engine](../engine/README.md)). The SQLite implementation is the one shipped; the simulation and tests use it too.

## 5. Building Block View

The SQLite store opens two connections to one file: a writer in WAL mode with foreign keys on, and a reader in query-only mode. The reader is what lets the application read the last commit while a long transaction is open on the writer. Transactions begin with `BEGIN IMMEDIATE`, so a second writer waits up to one second and then fails rather than deadlocking.

Values cross as JSON: booleans become integers, arrays and objects become JSON text, numbers become integers or reals. On the way out, blobs and non-UTF-8 text are refused. Both read operations refuse statements that are not read-only or return no columns, which is what makes application SQL safe to expose.

Code: [client/store.rs](../../../../../crates/client/src/store.rs) (contract), [sqlite/lib.rs](../../../../../crates/sqlite/src/lib.rs) (implementation).

## 10. Quality Requirements

- **The reader sees only committed rows; the writer sees its own.** Evidence: [sqlite/tests/store.rs](../../../../../crates/sqlite/tests/store.rs) `reader_sees_only_committed_rows_and_writer_sees_its_own`.
- **Savepoints nest and roll back independently** (basis of guarantee L3). Evidence: `savepoints_nest_and_rollback_independently`.
- **Read paths refuse writes; a second writer fails instead of hanging.** Evidence: `queries_refuse_writes_and_arrays_travel_as_json_text`, `second_writer_waits_then_fails_on_conflicting_immediate_transaction`.

Tests read, not executed.

## 11. Risks and Technical Debt

**Potential risk: two writers on one file.** *Condition:* two handles (two processes, or a stale handle) try to write within the same second. *Consequence:* the second sees `database is locked` after the one-second busy timeout; correctness is protected by the generation fence in [Frontend interface](../frontend-interface.md), availability is not. *Evidence:* the busy-timeout pragma and the test above. Multiple-writer support and the lock timeout are [#57](https://github.com/zanminwang/axton/issues/57).
