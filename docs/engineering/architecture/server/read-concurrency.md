# Read concurrency

## 1. Introduction and Goals

Reading immutable Store ownership must not reserve Mutation progress. Requests
still run in Serializable transactions and revalidate authenticated ownership
and Stream admission. Snapshot reads may retry for real dependencies; a read
endpoint can also perform explicit tracking, Bootstrap preparation or plan writes.

## 5. Building Block View

| Path | Store access | Other work and guards |
| --- | --- | --- |
| Local SQLite Model/SQL query and watch | No server Store operation | Committed query-only WAL reader; ordinary views do not begin a write transaction. Runtime tasks still queue behind an open application transaction, as described below. Explicit user transactions read their own writer state. |
| Server Query/Fetch | Snapshot inspection of an existing immutable binding | A missing binding is atomically claimed after application awaits. Explicit tracking/preparation writes use record/Stream guards. Client `store:true` response installation is a real local write. |
| Reconnect to completed Store | Snapshot binding and Bootstrap-state reads | No Mutation progress lock. |
| New Store / unfinished Bootstrap | Atomic claim and progress guard | Identity uniqueness and one committed Bootstrap initialization remain protected; failed transactions retry completely. |
| HTTP delta, live delta, continuation | Snapshot binding inspection | Fresh plans persist temporary payloads; continuations read frozen payloads. Missing Stores use the claim path. |
| Owned materialization | Snapshot binding inspection | Saved owner/target evidence is validated in the same transaction. Fresh payloads are staged; no Mutation progress update is performed. |
| Mutation execution/replay | `FOR NO KEY UPDATE` on Store progress | Competing uploads remain serialized. Store identity/principal/Stream are immutable. |

The plan table references Store identity through a foreign key. Its `KEY SHARE`
check must not wait behind a progress writer that never changes that key. `FOR
NO KEY UPDATE` keeps writers exclusive with each other while permitting that
reference check. The PostgreSQL [row-lock compatibility rules](https://www.postgresql.org/docs/16/explicit-locking.html#LOCKING-ROWS)
define this distinction. Store deletion/key changes still conflict with references.

Owning sources:

- [Server binding/Bootstrap/delivery](../../../../crates/server/src/delivery_plan.rs)
- [Mutation progress](../../../../crates/server/src/mutation_batch.rs)
- [PostgreSQL guards](../../../../packages/backend/postgres/src/sql.mts)
- [Plan persistence and foreign-key schema](../../../../packages/backend/postgres/migration.sql)
- [Local committed views](../../../../crates/client/src/frontend_interface.rs) and
  [SQLite reader/writer separation](../../../../crates/sqlite/src/lib.rs)

## 6. Remaining read-side work

`saveDelivery` still performs shared expiry cleanup and inserts a plan in the
request transaction. That path has measured Serializable conflicts and remains a
separate investigation; this Store change does not alter its cleanup policy.
Expiry reads can also delete their own expired plan. Neither is a pure SELECT.

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
HTTP delta, live delta, owned materialization and continuation to finish. Fresh
plan cases include the real foreign-key check. Owner/Stream mismatches and denied
admission fail before Loader execution. A concurrent duplicate upload must still
block, then receive the same saved accepted outcome without rerunning business
work. Existing protocol tests retain first-claim, Bootstrap, replay and settlement
coverage. Code inspection of local reader paths is distinct from device evidence.
