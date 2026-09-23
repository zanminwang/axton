# Local storage

AXTON stores cached records, queued mutations, channel progress and rejection details in a local SQLite file. This page explains how to manage that file and recover from storage or synchronization failures.

## Choose a database path

Choose a writable directory owned by your application. Use one active client per file and a separate file per signed-in user. Closing the client releases its connection and native resources; reopening the same file preserves local records and pending work.

The database contains a persistent client identity used for request deduplication. Do not let independently writable copies of the same file send mutations to the same backend. Switch accounts by closing the current client and opening the appropriate user's file, rather than changing only its authentication token.

## Change the schema

Each database records the schema it was built for. Opening it with a newer generated schema takes one of three ways:

| Change | What happens |
| --- | --- |
| None | The database opens. |
| A new model, or a new nullable field | Applied in place; cached records, queued mutations and frozen request bytes are preserved. |
| Anything else: a required field, a removed or retyped field, a changed identity, unique constraint, relation, enum or model version, a removed model, or a file from an earlier AXTON runtime | The file is left untouched and a fresh database is opened beside it (`<path>.1`, `<path>.2`, …). A small `<path>.current` file names the one in use. The new database keeps the old subscriptions and synchronises from the beginning. |

Before switching, the runtime looks at the old database's unsent mutations. If there are any, it keeps that database open so they can still be sent; `syncState().schema.pending` reports how many remain and why the schema is incompatible. When they are sent, call `rebuild()`. If they cannot be sent, call `rebuild({ discardPending: true })`: the report tells you how many mutations and local-only records stay in the old file. Nothing is copied between schemas, and the runtime never deletes an old file; delete the numbered files you no longer need. See [opening and schema changes](runtime.md#opening-and-schema-changes) for the calls. Update your backend tables separately through your database's migration process.

## Recover pending work

| Situation | What to do |
| --- | --- |
| A request times out | Let sync retry the persisted frozen request. The backend may already have committed it. |
| Frozen work remains pending | Check connectivity and authentication; a receipt AXTON cannot apply is refused and the batch resent, so check `onError` on both sides. |
| A mutation is rejected | Display its code, inspect the record's `syncState`, then dismiss the handled rejection. |
| A prerequisite fails | Resolve its cause, reset its readiness to `pending`, then run its callback again. |
| Another client wrote to the same file | Close the stale instance and reopen it; keep one active client per file. |

Do not manually delete pending batches, channel cursors or backend receipts to clear an error. These records work together to prevent duplicate execution and complete local changes from their receipts. Preserve the database for diagnosis when an error cannot be resolved through the public APIs.

[Sync and recovery](sync.md) shows the application calls for these cases.

## Manage cached data

Unsubscribing stops that channel's synchronization and removes nothing: cached records, their stamps, before images and pending edits stay, and another subscribed channel can still update them. Retained records are readable but not kept fresh without a channel that delivers them. Permissions are enforced by your backend. When a record is no longer visible, publish it to the affected channels so their loaders can return null. There is no automatic eviction of cached records.

Receipts and pull changes carry a per-record stamp. A newer stamp replaces the record's authoritative state; a delayed lower stamp cannot overwrite it, whichever path delivers it. Deletions apply across channels, and the deleted record's stamp is kept so older content cannot resurrect it. See [how state moves](../concepts.md) for the relationship between records, channels and pending writes.

## Storage size

Cached records, queued mutations, rejection details and backend receipts persist. The runtime does not impose a cache-size limit or automatically expire these entries. Backend invalidations compact by channel/model/identity, but distinct identities still consume space.

Measure database size, pending work and synchronization lag with your application's working set. Local reads, including read-only SQL, use on-disk SQLite tables. They do not copy the full record set into a separate query projection.
