# How state moves

A local-first client reads its local database. A user edit can become visible immediately, while a durable Mutation records the intent that still needs backend processing. The authoritative result arrives in the mutation's receipt, and through Pull for changes other clients made, and is reconciled with remaining local work.

## Model, Record and Identity

A **Model** describes client data. A **Record** is one instance, identified by an **Identity** that can contain one field or several fields.

These are client-facing shapes. A Loader may assemble one Record from several backend tables, or expose several Models from the same business data. The Rust runtime receives schema descriptors; generated TypeScript and Dart types provide the language-facing API.

See the [compiler guide](schema/reference.md) for supported declarations and generated types.

## Mutation and local state

A **Mutation** describes a named business operation. Its optimistic operations update the locally visible result, while the queue retains the work needed for backend processing. A dirty record's sparse before image holds the authoritative base used when replaying pending changes.

For example, editing a title while offline makes that title visible locally. If a remote update then arrives, the client updates the authoritative base and replays pending local operations. Pending local fields may therefore continue to be visible until their mutations complete or are rejected.

Direct local writes and local companions have their own roles. A direct write is separate from a server mutation's fate; a companion participates in a mutation's fate without being uploaded. The framework does not rerun arbitrary application callbacks to reconstruct optimistic state.

## Handler, Loader and Publish

The TypeScript backend SDK connects three operations to your application:

| Primitive | Application responsibility |
| --- | --- |
| **Handler** | Execute a named Mutation against your business data, including business authorization. |
| **Loader** | Return current, complete, visible state for the requested identities, in their supplied order. Return null for missing or unauthorized rows. |
| **Publish** | Explicitly name the Channels that should receive a mutation's changed Records; `changes.add` reports a changed Record the uploaded operations did not name. |

After a Handler returns, the framework allocates a stamp for every changed Record, reads those Records back through the Loaders in the same transaction, and returns their content in the receipt. The application provides the transaction runner. Batch processing uses one outer transaction with per-mutation savepoints; business writes, stamps, publications and receipts participate in that transaction. A business rejection, an unsupported mutation version, a handler failure or a loader failure each roll back just that mutation's savepoint; the rest of the batch commits. Only identity/order refusals and infrastructure failures (a broken transaction, a failed rollback) abort the whole batch.

Background jobs publish through `backend.transaction`, which runs the job's writes and its publication in one application transaction, advances the stamps of the records it names, and wakes live subscribers once the transaction commits.

The [backend SDK guide](backend/setup.md) shows registration and background publication. The backend stores its metadata in [PostgreSQL](backend/database.md), through `pg`, Prisma or Drizzle.

## Channel and Cursor

A **Channel** is an explicitly named distribution scope, such as a shared book. Clients subscribe to Channels; the backend publishes invalidations to them. A Channel may contain several Models.

A **Cursor** is the client's receive position within a Channel. Numbers from different Channels are not comparable, and a Cursor says nothing about a Record's content; that is the Stamp's job.

Channels distribute access to current state. They are not event logs that promise delivery of every historical intermediate value. Pull uses Loaders to obtain the current authoritative content for invalidated identities.

## How a receipt replaces optimism

Consider one pending title edit:

1. The client writes the optimistic title and persists the Mutation.
2. Push sends a frozen request. If the result is unknown, a retry uses the same persisted request bytes and batch sequence.
3. The server commits the business change, reads the changed Records back through the Loaders and stores the receipt. The receipt reports acceptance or rejection per Mutation, and the authoritative content and Stamp of every Record the accepted Mutations changed.
4. The client applies that authority beneath its pending work, removes the completed Mutation and replays the remaining pending work, all in one local transaction. No Channel is awaited, and no subscription is needed to complete a Mutation.

If the Handler also published the Record, a Pull page carries the same content at the same Stamp, before or after the receipt. Whichever arrives second rewrites nothing: an equal Stamp with equal content is a no-op, a newer Stamp wins, an older one is ignored. Both orders leave the same local state.

A server may normalize the title or reject the edit. Rejections are retained in the local inbox so the application can explain the result to the user. See [recovery](frontend/storage.md) for retry and rejection handling.

## Stamps across channels

A **Stamp** orders content for one record across every path that delivers it: the receipt, catch-up pages and the live stream. Every successful change to a record allocates a newer stamp; publishing the same version to several channels carries that one stamp to all of them. The client applies a newer value and ignores delayed older content, regardless of which path delivers it. Equal stamps are idempotent; inconsistent content for the same stamp is a diagnostic condition.

A newer deletion withdraws the record across channels, and the client keeps the deleted record's stamp so that older content arriving later cannot bring it back. A channel is a delivery path, not an owner: unsubscribing stops its delivery and removes nothing the client already holds. Stamps are required on pull changes; they are separate from each channel's cursor. See the [stamp acceptance tests](https://github.com/zanminwang/axton/blob/main/crates/sqlite/tests/stamp_scenarios.rs) for the ordering cases.

## Local reads and sync reads

`get`, `query`, relation accessors, raw SQL and `watch` read local SQLite through the Rust engine. They do not call a loader. Read-only SQL uses the on-disk tables rather than copying the full record set into a separate projection.

The backend's loader is the sync read path: it supplies the current authorized content of the records a mutation changed, for the receipt, and of the records a publication identified, for a page. It never sees which path is asking. A record the loader cannot read fails alone: the rest of the page is delivered, and the client keeps its copy and reports the failure. This separation lets your local record schema differ from your backend database layout.

AXTON uses one connection: HTTP submits mutations and catches up missing records in one pull for all channels; WebSocket delivers ongoing changes. Initial connection, reconnection and gap recovery use saved channel cursors. Received records pass through the Rust engine into SQLite.

## Current limits

- A malformed pull change can be skipped while the cursor advances. A cursor is not an unconditional proof that every malformed change was applied successfully.
- Live wakeups are process-local. Multi-process deployments need an application-provided committed notification mechanism.
- Production-scale cache performance requires measurement with your working set.

See [sync and recovery](frontend/sync.md) for application behavior and [local storage](frontend/storage.md) for storage constraints.
