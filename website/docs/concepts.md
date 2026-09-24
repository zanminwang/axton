# How state moves

A local-first client reads its local database. A durable Action can make an inferred Model change visible immediately while its backend intent waits for delivery. Its typed result arrives with the Action outcome. Batch-final record authority arrives in the receipt, and Pull delivers changes made elsewhere; the client reconciles both with remaining local work.

## Model, Record and Identity

A **Model** describes client data. A **Record** is one instance, identified by an **Identity** that can contain one field or several fields.

These are client-facing shapes. A Loader may assemble one Record from several backend tables, or expose several Models from the same business data. The Rust runtime receives schema descriptors; generated TypeScript and Dart types provide the language-facing API.

See the [compiler guide](schema/reference.md) for supported declarations and generated types.

## Actions and local state

An **Action** describes a named backend operation with typed inputs and outputs. The durable route, `client.actions.name`, commits its intent and inferred Model optimism locally, then returns an `ActionCall` whose `wait()` observes the final outcome. The direct route, `client.actions.call.name`, returns the final result without queueing or automatic optimism. A dirty record's sparse before image holds the authoritative base used when replaying pending changes.

For example, a durable `SetTodoDone` call can change `done` locally while offline. If a remote update arrives, the client updates the authoritative base and replays pending local operations. Pending local fields may remain visible until their Actions complete or are rejected.

Standalone `client.models` and transactional `tx.models` writes change only local storage; they do not upload. Later backend authority for the same identity can replace a cached local record. The application owns any conflict policy. The framework does not rerun arbitrary application callbacks to reconstruct optimistic state.

## Handler, Loader and Publish

The TypeScript backend SDK connects three operations to your application:

| Primitive | Application responsibility |
| --- | --- |
| **Handler** | Execute a named Action against your business data, including business authorization, and return explicit outputs. |
| **Loader** | Return current, complete, visible state for the requested identities, in their supplied order. Return null for missing or unauthorized rows. |
| **Publish** | Explicitly name the Channels that should receive an Action's changed Records; `changes.add` reports additional changed Records. |

The framework resolves Model outputs through retained Loaders during each Action invocation. The result is that invocation's snapshot. For durable batches it also stamps and reads changed Records for batch-final authority in the receipt. The application provides the transaction runner; business writes, outcomes, stamps, publications and receipts commit together. A business rejection or attributable Handler/Loader failure rolls back that Action's savepoint, while independent calls can commit. An infrastructure fault aborts the delivery transaction for retry. Backend outcomes remain stored without TTL or automatic pruning; client business results live only in memory.

Background jobs publish through `backend.transaction`, which runs the job's writes and its publication in one application transaction, advances the stamps of the records it names, and wakes live subscribers once the transaction commits.

The [backend SDK guide](backend/setup.md) shows registration and background publication. The backend stores its metadata in [PostgreSQL](backend/database.md), through `pg`, Prisma or Drizzle.

## Channel and Cursor

A **Channel** is an explicitly named distribution scope, such as a shared book. Clients subscribe to Channels; the backend publishes invalidations to them. A Channel may contain several Models.

A **Cursor** is the client's receive position within a Channel. Numbers from different Channels are not comparable, and a Cursor says nothing about a Record's content; that is the Stamp's job.

Channels distribute access to current state. They are not event logs that promise delivery of every historical intermediate value. Pull uses Loaders to obtain the current authoritative content for invalidated identities.

## How a receipt replaces optimism

Consider one pending title edit:

1. The client writes inferred optimistic Model state and persists the Action intent.
2. Push sends a frozen request. If the result is unknown, a retry uses the same persisted request bytes and batch sequence.
3. The server commits business changes, each call's result snapshot, batch-final record authority and the receipt. The receipt reports the outcome per Action and the authoritative content and Stamp of changed Records.
4. The client applies that authority beneath its pending work, removes completed queue entries and replays remaining pending work, all in one local transaction. No Channel is awaited to complete an Action.

If the Handler also published the Record, a Pull page carries the same content at the same Stamp, before or after the receipt. Whichever arrives second rewrites nothing: an equal Stamp with equal content is a no-op, a newer Stamp wins, an older one is ignored. Both orders leave the same local state.

A server may normalize the title or reject the edit. `wait()` reports the Action outcome; rejections are also retained in the local inbox. See [recovery](frontend/storage.md) for retry and rejection handling.

## Stamps across channels

A **Stamp** orders content for one record across every path that delivers it: the receipt, catch-up pages and the live stream. Every successful change to a record allocates a newer stamp; publishing the same version to several channels carries that one stamp to all of them. The client applies a newer value and ignores delayed older content, regardless of which path delivers it. Equal stamps are idempotent; inconsistent content for the same stamp is a diagnostic condition.

A newer deletion withdraws the record across channels, and the client keeps the deleted record's stamp so that older content arriving later cannot bring it back. A channel is a delivery path, not an owner: unsubscribing stops its delivery and removes nothing the client already holds. Stamps are required on pull changes; they are separate from each channel's cursor. See the [stamp acceptance tests](https://github.com/zanminwang/axton/blob/main/crates/sqlite/tests/stamp_scenarios.rs) for the ordering cases.

## Local reads and sync reads

`get`, `query`, relation accessors, raw SQL and `watch` read local SQLite through the Rust engine. They do not call a loader. Read-only SQL uses the on-disk tables rather than copying the full record set into a separate projection.

The backend Loader supplies Action Model result snapshots, changed-record authority, and the current authorized content of records a publication identified for a page. It never sees which channel is asking. A record the Loader cannot read in a page fails alone: the rest of the page is delivered, and the client keeps its copy and reports the failure.

AXTON uses one connection: HTTP submits durable Actions and direct requests, and catches up missing records in one pull for all channels; WebSocket delivers ongoing changes. Initial connection, reconnection and gap recovery use saved channel cursors. Received records pass through the Rust engine into SQLite.

## Current limits

- A malformed pull change can be skipped while the cursor advances. A cursor is not an unconditional proof that every malformed change was applied successfully.
- Live wakeups are process-local. Multi-process deployments need an application-provided committed notification mechanism.
- Production-scale cache performance requires measurement with your working set.

See [sync and recovery](frontend/sync.md) for application behavior and [local storage](frontend/storage.md) for storage constraints.
