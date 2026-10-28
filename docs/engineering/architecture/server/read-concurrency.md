# Read concurrency

## 1. Introduction and Goals

Reading immutable Store ownership must not reserve Mutation progress. Requests
still run in Serializable transactions and revalidate authenticated ownership
and Stream admission. Snapshot reads may retry for real dependencies; a read
endpoint can also perform explicit tracking, Bootstrap preparation.

## 5. Building Block View

| Path | Store access | Other work and guards |
| --- | --- | --- |
| Local SQLite Model/SQL query and watch | No server Store operation | Committed query-only WAL reader; ordinary views do not begin a write transaction. Runtime tasks still queue behind an open application transaction, as described below. Explicit user transactions read their own writer state. |
| Server Query/Fetch | Snapshot inspection of an existing immutable binding | A missing binding is atomically claimed after application awaits. Explicit tracking/preparation writes use record/Stream guards. Client `store:true` response installation is a real local write. |
| Reconnect to completed Store | Snapshot binding and Bootstrap-state reads | No Mutation progress lock. |
| New Store / unfinished Bootstrap | Atomic claim and progress guard | Identity uniqueness and one committed Bootstrap initialization remain protected; failed transactions retry completely. |
| HTTP/live delta and cursor resume | Snapshot binding inspection | Current-state complete pages; no persisted response, shared cleanup or old-plan continuation. Missing Stores use the claim path. |
| Owned materialization | Snapshot binding inspection | Saved owner/target evidence is validated in the same transaction. The complete response is returned without server staging or Mutation progress updates. |
| Mutation execution/replay | `FOR NO KEY UPDATE` on Store progress | Competing uploads remain serialized. Store identity/principal/Stream are immutable. |

Progress writers remain mutually exclusive through `FOR NO KEY UPDATE`.
References to immutable Store identity can use `KEY SHARE` without reserving its
progress. See PostgreSQL's [row-lock compatibility rules](https://www.postgresql.org/docs/16/explicit-locking.html#LOCKING-ROWS).
The delivery-plan foreign key and payload tables have been removed.

Owning sources:

- [Server binding/Bootstrap/delivery](../../../../crates/server/src/delivery_plan.rs)
- [Mutation progress](../../../../crates/server/src/mutation_batch.rs)
- [PostgreSQL guards](../../../../packages/backend/postgres/src/sql.mts)
- [Retained PostgreSQL schema](../../../../packages/backend/postgres/migration.sql)
- [Local committed views](../../../../crates/client/src/frontend_interface.rs) and
  [SQLite reader/writer separation](../../../../crates/sqlite/src/lib.rs)

## 6. Remaining read-side work

Delivery no longer inserts payloads, scans shared expiry predicates or deletes
expired plans. All initialized delta, cursor-resume and owned reads in the test
fixture run successfully in PostgreSQL `READ ONLY` transactions. Application
preparation hooks can still intentionally perform writes.

Loader preparation or explicit Query tracking can change membership/content.
Their record/Stream guards remain necessary to enforce real dependencies and
were not removed by the Store binding change. Local cache installation, apply
and observer publication likewise retain their atomic write boundaries.

The persistence adapter also repeats read-only legacy-layout catalog checks.
Those are a potential query-cost target, not a Store progress lock; no change to
them is included in this experiment.

The [Node live-session registry](../../../../bindings/node/src/server.rs) has a
process-wide mutex for controller transitions, released before host/database
awaits. The [client runtime registry](../../../../bindings/common/src/actor.rs)
likewise releases its map lock before database work or callbacks. These short
in-memory critical sections do not reserve a Store's database progress for the
duration of a read request; their existence alone is not evidence of the measured
database contention.

The local runtime has a separate scheduling limitation: while an application
transaction callback owns the writer, ordinary tasks, including committed reads,
wait for that callback to finish. SQLite's separate reader can read the old
committed state, but the public task scheduler does not dispatch that read yet.
A long callback can therefore delay local reads without a database read/write
lock conflict. This audit does not change that scheduling contract or attribute
the measured server serialization failures to it. The owning
[runtime](../client/runtime.md) documents the behavior and existing regression.

## 10. Quality Requirements

[Read liveness tests](../../../../integration/persistence/server/read-liveness.test.mjs)
pause an actual same-Store Mutation handler, then require initialized handshake,
HTTP delta, live delta, owned materialization and cursor resume to finish under
`SET TRANSACTION READ ONLY`. Owner/Stream mismatches and denied
admission fail before Loader execution. A concurrent duplicate upload must still
block, then receive the same saved accepted outcome without rerunning business
work. Existing protocol tests retain first-claim, Bootstrap, replay and settlement
coverage. Code inspection of local reader paths is distinct from device evidence.
