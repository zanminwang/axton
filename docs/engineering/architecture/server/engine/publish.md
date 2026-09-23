# Publish

## 1. Introduction and Goals

Publish is how a change becomes visible to pull. Publishing a record to a channel gives it a new position in that channel, at the record's current version, inside the same transaction as the change, and wakes live subscribers once that transaction commits. Publishing distributes a version; it does not create one.

## 3. Context and Scope

Three ways in, one path:

| Caller | Call | What it publishes | Stamp |
| --- | --- | --- | --- |
| a handler | `publish({channel})` | the mutation's final change set, resolved after the handler returns | the stamp the mutation allocated for each record |
| a handler | `publish({channel, records})` | exactly those records, changed or not; `[]` publishes nothing | the mutation's stamp for changed records, the existing stamp for others, initialized at 1 when a record has none |
| application code | `backend.transaction(async ({tx, changes, publish}) => …)` | the same two forms, with the change set starting empty: `changes.add` registers a business change made outside a handler | one new stamp per record in `changes`, shared by every channel it is published to; the existing stamp for a record published without being changed |

A handler reports changes beyond its uploaded operations with `changes.add({model, identity})`; that registers a change (a stamp and a readback) without publishing it. Each publication asks [Persistence](../persistence.md) to allocate the next *cursor* for the channel and to upsert the invalidation row at the given stamp ([Pull](pull.md)). The set of channels published in a transaction feeds the wake after commit ([Server / Connection / Controller](../connection/controller.md)).

## 5. Building Block View

- **Stamps and cursors are independent counters.** A stamp is per record and orders content; a cursor is per channel and orders pages. A change allocates one stamp; publishing it to channels A and B allocates one new cursor in each and carries that one stamp to both (guarantee D3). The simulation and the persistence tests hold this by construction: `publish` refuses a stamp that is not the record's current one.
- **Publication order** within a mutation is the order of `publish` calls, with records in canonical key order inside each; an external transaction publishes in the order of its `publish` calls once its body returns.
- **Wake set.** The TypeScript session records every channel a `publish` host request passed through in the transaction, snapshots that set at each mutation's savepoint and restores it on rollback, so a rejected mutation's publications neither remain nor wake anyone. After the transaction commits, an in-process hub calls the wake callbacks registered by live sockets for those channels.

Code: publication resolution in [server/readback.rs](../../../../../crates/server/src/readback.rs) (`read_back`, `allocate_stamps`, `publish_intents`, `publish_one`); the external path `settle_external` in [server/lib.rs](../../../../../crates/server/src/lib.rs); `collect`, `transaction`, `Session.touched` and `WakeHub` in [server/index.mts](../../../../../packages/server/index.mts).

## 6. Runtime View

Inside a push: handler runs, collecting `changes.add` and `publish` intents → stamps allocated for the change set → loaders read it back → publications go out at those stamps → receipt → commit → wakes. Outside a push with `backend.transaction`: the framework opens the application transaction → the body writes, collecting `changes.add` and `publish` intents → the body returns → `settle_external` allocates the stamps and carries out the publications, with no readback → completion check → commit → the framework wakes the touched channels.

## 9. Architecture Decisions

**External writes go through `backend.transaction` (decided in [#50](https://github.com/zanminwang/axton/issues/50), implemented 2026-09-15).** The framework owns the transaction, the completion check and the after-commit wake, so an application cannot publish without waking. Business writes and publications share one transaction; a failure rolls back both and wakes nobody. Cross-process wakes stay with [#62](https://github.com/zanminwang/axton/issues/62). Evidence: [runtime.test.mjs](../../../../../integration/persistence/server/runtime.test.mjs) `backend.transaction publishes in the application transaction and wakes after commit`, `backend.transaction rolls back a failing body and wakes nobody`, `backend.transaction wakes a connected live subscriber without reconnect`.

**External writes speak the handler vocabulary ([#132](https://github.com/zanminwang/axton/issues/132), 2026-09-16).** The body of `backend.transaction` receives `changes` and `publish`, the objects a handler receives, and the engine settles them after the body returns through `settle_external`, which takes the same `{changes, publications}` a handler answers with and reuses readback's stamp allocation and publication resolution without the load step. The separate `notify` vocabulary, its bound-session variant `bindTransaction(tx).notify` and the `publish(changes, channels)` engine entry are gone: one way to report a change, whether or not a client uploaded it. Evidence: [server/tests/stamp.rs](../../../../../crates/server/tests/stamp.rs) `an_external_settlement_advances_one_stamp_per_record_and_distributes_it_at_that_stamp`; [runtime.test.mjs](../../../../../integration/persistence/server/runtime.test.mjs) `publishing an unchanged record does not advance its stamp; a body without publish still stamps its changes`, `backend.transaction rolls the business write back when the settlement is rejected`, `notify and bindTransaction are gone; backend.transaction is the only external write path`.

## 10. Quality Requirements

- **A change allocates one stamp and every channel it is published to carries that stamp; publishing an unchanged record initializes a missing stamp and otherwise reuses it; a publication naming a stale stamp is refused; concurrent first publications agree on stamp 1** (guarantee D3). Evidence: [server/tests/stamp.rs](../../../../../crates/server/tests/stamp.rs) `an_external_settlement_advances_one_stamp_per_record_and_distributes_it_at_that_stamp`; [server/tests/readback.rs](../../../../../crates/server/tests/readback.rs) `default_publication_covers_the_final_change_set`, `handler_changes_are_read_back_and_publication_only_records_are_not`, `explicit_empty_records_publish_nothing`, `a_publish_that_echoes_another_stamp_is_host_invalid`; [runtime.test.mjs](../../../../../integration/persistence/server/runtime.test.mjs), the stamp regressions listed under [Persistence](../persistence.md#10-quality-requirements).
- **A rejected mutation publishes nothing, and a rolled-back transaction publishes nothing.** Evidence: `rejected mutation publishes nothing even though it asked to`, `publication rollback uses user transaction and rejects unregistered models`.
- **Subscribers are woken only after commit, and never by a duplicate receipt.** Evidence: `live transport negotiates, wakes only after commit, reconnects, and cleans up`.

Rust evidence executed 2026-09-15 (`cargo test -p axton-server --locked`); the PostgreSQL rows are named after the tests in `runtime.test.mjs` — see the pull request for that run.

## 11. Risks and Technical Debt

**Accepted limitation.** Wakes are in-process: a second server instance, or a publication from another process, does not wake this process's sockets; those clients catch up on reconnect. Cross-process notification delivery is [#62](https://github.com/zanminwang/axton/issues/62).

**Potential risk.** Every publication to a channel updates the same channel row under a row lock, so handlers touching one hot channel serialize and may retry on serialization failure; every change to a record locks its stamp row the same way. Not measured ([#12](https://github.com/zanminwang/axton/issues/12)).
