# Frontend interface

## 1. Introduction and Goals

The frontend interface is the one Rust surface every language binding drives: open a database, run transactions, read, queue, sync, observe. It holds almost no state in memory: the open store, the schema, the client id and a generation counter, plus the registered watchers, the open session if any, and the bounded record of issued pulls and subscription epochs that [Pull](engine/pull.md#5-building-block-view) uses to recognize a page from an earlier subscription. None of that is durable, so a client can be dropped and reopened at any commit.

## 3. Context and Scope

Callers are the [bindings](../sdks/bindings.md), the simulation and the crate tests. Dependencies are a store ([Storage](storage/README.md)), the compiled `schema.json` and the [engine](engine/README.md).

The surface, grouped by purpose:

| Purpose | Operations |
| --- | --- |
| Lifecycle | `open(store, schema)` on a store the caller opened; `open_at(path, schema, factory, discard_pending)` chooses the file behind `path` ([Reconciliation](storage/reconciliation.md)); `rebuild(discard_pending)` switches an incompatible database to a fresh file and returns a `RebuildReport` |
| Writes | `transaction(\|tx\| …)` with local `direct`, `submit_action`, `set_channel`, nested `savepoint` and reads inside; legacy internal `enqueue` remains for retained fixtures |
| Subscriptions | `ensure_subscription(scope)` registers durable intent and answers the stored `SubscriptionState` (a repeat writes nothing), `subscription_state(scope)`, `subscription_states`, `remove_subscription(scope, subscription_id)` removes only that identity, `initialize_subscriptions(expected, heads)` commits first boundaries ([client/subscriptions.rs](../../../../crates/client/src/subscriptions.rs)) |
| Session API for hosts that hold a transaction open across calls | `begin_session`, `session(\|tx\| …)`, `session_savepoint`, `session_release`, `session_rollback_savepoint`, `commit_session`, `rollback_session` |
| Reads on the last commit | `read`, `query`, `query_spec`, `related`, `referencing`, `read_sql` |
| Sync | `freeze`, `acknowledge` (returns authority reports and transient call completions; [Settlement](engine/settlement.md)), `prepare_action` and `apply_action_response` for direct calls, `downlink_request`, `apply_page` and `receive_downlink`, plus the `SyncCycle` and `ConnectionDriver` state machines |
| State and control | `pending_count`, `cursor`, `subscriptions`, `subscription_generation` (how many subscribes and unsubscribes committed since open; the [downlink](connection/controller/downlink-worker.md) session restarts when it changes), `rejections`, `record_status` (pending entries carry `diverged` when their replay failed over new authority), `pending_tasks`, `next_task`, `outcome`, `set_readiness`, `drop_mutation`, `dismiss_rejection`, `schema_state` (`rebuilt`, `pending`, `last_rebuild`) |
| Notification | `watch(tables)` → a receiver signalled when a commit touched one of the tables |

## 5. Building Block View

The interface is the `Client` and `ClientTransaction` types in [client/lib.rs](../../../../crates/client/src/lib.rs); each engine call receives an `Engine` handle ([client/engine.rs](../../../../crates/client/src/engine.rs)) bound to the current transaction.

## 6. Runtime View

**Opening through a path** (`open_at`, what every binding does) reads the sidecar to find the current file, checks its layout on the committed reader without writing anything, compares the stored schema descriptor with the incoming one and opens, applies an additive change, keeps the file open for its unsent work, or rebuilds beside it ([Reconciliation](storage/reconciliation.md)). `open` on a caller-opened store has no path to rebuild beside, so it refuses a checkpoint-era layout with "this database was created by an earlier AXTON runtime … open it through a path so it can be rebuilt beside". Either way the open runs the framework DDL and, in one transaction, reconciles the model tables, stores the descriptor and creates or reads the client row (client id, ordinal and push counters, generation, the last completed push and the frozen declaration). Nothing settles on open: a frozen batch waits for its receipt, and a completed one already has its rows. Opening does not change the generation, so two fresh handles are both valid until one of them writes.

**Every write** goes through one path: begin, run the body, bump the generation with `UPDATE … WHERE generation = ?`, commit. A handle whose generation is behind the database's fails that update with `stale client writer` and rolls back; this is how a forgotten handle is fenced out after another one wrote (guarantee R4). A failed commit is followed by a rollback so the store never stays inside an open transaction.

**Sessions** exist for hosts whose transaction spans several native calls. While a session is open, sync commands are refused, and `commit_session` refuses to commit with an unclosed savepoint.

**Reads outside a transaction** use the committed reader connection, so a long session in the same process does not block them and they do not see its uncommitted writes.

**Subscriptions** are durable local state, not a connection. `ensure_subscription` inserts if absent - never an upsert - so registering a Scope while offline commits intent and allocates one never-recycled `subscription_id` without a delivery boundary, and repeating the call reads the stored row untouched. Both cursor fields stay NULL until a session's acknowledgement commits the first boundary through `initialize_subscriptions`, and a NULL boundary is never read as zero: such a row belongs to the desired Scope set (`subscription_states`, `desired_channels`) and to no request (`subscriptions`, `downlink_request`). `remove_subscription` deletes only the identity it names, so a removal that lost a race against a recreation at the same Scope name changes nothing, and it removes no record, stamp, before image or pending call (guarantee D6). A Scope name the wire refuses - empty, or nothing but whitespace - is refused by every registration path (`check_channel` in [core/protocol.rs](../../../../crates/core/src/protocol.rs) is the one rule), because a stored row for it would make every handshake, and so every Scope's delivery, fail. Every write of the ledger marks the transaction, which bumps `subscription_generation` and makes pulls in flight stale. The table layout and the `next_subscription` allocator are in [Reconciliation](storage/reconciliation.md); what the acknowledgement commits is in [Downlink worker](connection/controller/downlink-worker.md); the handle and status the SDKs publish from it are in [Typed API / Client](../sdks/typed-api/client.md).

**Call boundaries.** Both kinds use the same entry points; the SDK's generated method picks the route ([Mutations and Queries](../schema/actions.md#3-context-and-scope)). `submit_action` serves a durable Mutation or a queued Query: it normalizes the retained operation contract and commits its call ID, canonical args and inferred Model operations in one local transaction. `submit_action_with_options` and `prepare_action_with_options` also take an `ActionCallOptions` whose `store` policy is validated first and kept in the queue row's `store` column (NULL for the default) and in the frozen request. It can enqueue an operation with no Model operation, such as a plain-value Mutation or any queued Query. `prepare_action` builds the direct request of a Query or a direct Mutation; network I/O occurs outside the exclusive local database section, and `apply_action_response` validates the response and applies authority in a short transaction. Neither route runs inside an application-owned local transaction. Business results are returned through transient completions and SDK memory, not stored in client SQLite; pending and completion state remains durable.

## 10. Quality Requirements

- **A committed write survives close and reopen; identity, queue and rejections persist** (guarantee L2). Evidence: [sqlite/tests/client.rs](../../../../crates/sqlite/tests/client.rs) `open_creates_tables_persists_identity_and_survives_reopen`.
- **An error anywhere in a transaction rolls back the whole transaction; a savepoint confines its own scope** (guarantee L3). Evidence: `local_transaction_and_mutation_savepoint_have_independent_fate`, `session_reads_own_writes_without_notifying_until_commit_and_blocks_other_writes`.
- **A stale handle cannot commit** (guarantee R4). Evidence: `stale_writer_cannot_overwrite_committed_database`.
- **Registering a Scope offline commits durable intent with no delivery boundary; a repeat writes nothing; a rolled-back registration leaves no row; an uninitialized row is never read as cursor zero; removal by a stale identity changes nothing** (guarantees D6, D9). Evidence: [sqlite/tests/subscriptions.rs](../../../../crates/sqlite/tests/subscriptions.rs) `set_channel_registers_without_a_boundary_and_repeats_without_a_generation_change`, `duplicate_registration_keeps_one_identity_and_recreation_allocates_another`, `a_rolled_back_registration_leaves_no_subscription`, `identity_and_uninitialized_cursors_survive_a_reopen`, `an_uninitialized_subscription_is_never_read_as_cursor_zero`, `removal_by_a_stale_identity_changes_nothing`, `set_channel_removes_the_current_identity_and_recreation_starts_over`, `cursor_advancement_cannot_resurrect_a_removed_subscription`, `a_blank_scope_name_is_refused_and_registers_nothing`.
- **Watchers fire only for the tables they named, and only after commit.** Evidence: `watch_fires_only_for_declared_tables`.
- **A checkpoint-era database opened as a store is refused and left untouched; opened through a path it is rebuilt beside.** Evidence: [sqlite/tests/ddl.rs](../../../../crates/sqlite/tests/ddl.rs) `a_database_from_the_checkpoint_era_is_refused_untouched`; [sqlite/tests/rebuild.rs](../../../../crates/sqlite/tests/rebuild.rs) `an_earlier_framework_layout_is_rebuilt_beside_not_refused`.
- **`rebuild` switches the same handle to the new file and notifies every model table.** Evidence: `unsent_work_keeps_the_old_file_open_until_it_is_sent_then_rebuild_switches`; [bindings/common/tests/session.rs](../../../../bindings/common/tests/session.rs) `incompatible_schema_reports_pending_work_and_rebuild_switches_files`.

Executed 2026-09-16 (2026-09-25 for the subscription bullet): `cargo test -p axton-sqlite -p axton-binding --locked` passed with the tests above.

## 11. Risks and Technical Debt

**Accepted limitation.** `drop_mutation` refuses a mutation that has been frozen, because its outcome is unknown until the receipt arrives. The consequence for a batch the server keeps failing is recorded under [Batching](engine/push/batching.md).

**To confirm.** Applications cannot run code inside the page transaction; [#17](https://github.com/zanminwang/axton/issues/17) proposes such a hook and notes the binding constraint.
