# Sync, offline work and recovery

Local reads and writes go through the Rust engine and SQLite. A connection handles network work in the background. Your app can continue using its local data while the connection is paused or a response is delayed.

## Subscribe and observe

=== "TypeScript"

    ```ts
    await client.channels.subscribe('book:demo');
    const stop = client.models.entry.watch({}, entries => render(entries), console.error);
    ```

=== "Flutter"

    ```dart
    await client.channels.subscribe('book:demo');
    final subscription = client.models.entry.watch().listen(
      render,
      onError: (Object error) => print(error),
    );
    ```

Here `render` is your UI's update function. Subscribing records the desired channel and wakes the connection; it does not wait for the initial data. Expect an empty initial result on a new database. `watch` emits again when synchronization commits records.

Use channel names that your backend publishes to, and subscribe when the client needs to receive changes other clients make. A channel is not a database query or an authorization token. Loaders decide which requested records the authenticated user may see.

## Receive mutation results

**A subscription is not required to see your own result.** A mutation's local changes are an optimistic prediction. Its receipt confirms that the backend accepted the operation and carries the final content of every record the handler changed, read back by your loader in the handler's transaction. Ahead replaces the prediction with that content as soon as the receipt arrives: a normalized value shows up, and a record the backend refused to create disappears with the rejection. Your loader must return the resulting record for that user, exactly as it would for a page.

Subscribe with `client.channels.subscribe(channel)` as above when the client needs changes made elsewhere: by other users, by background jobs, or by handlers that touch records without reporting them. Subscription starts synchronization; it does not wait for initial data. Use `watch` to observe the records, and wait for an existing record to be available locally before updating it.

You can send mutations without subscribing to any channel. The receipt still corrects the local row to the server's result; what you do not receive is later changes to that record from elsewhere. If you subscribe to a channel the handler publishes to, the page for your own change carries the same stamp as the receipt and rewrites nothing, whichever arrives first.

## Work offline

=== "TypeScript"

    ```ts
    await client.connection!.pause();
    await client.mutate.edit({
      entry: { identity: { id: 'entry-1' }, values: { text: '  Draft  ' } },
    });
    console.log((await client.models.entry.get({ id: 'entry-1' }))?.text);
    await client.connection!.resume();
    ```

=== "Flutter"

    ```dart
    await client.connection!.pause();
    await client.mutate.edit(
      entry: const EditEntryUpdate(
        identity: EntryIdentity(id: 'entry-1'),
        text: Present('  Draft  '),
      ),
    );
    print((await client.models.entry.get(const EntryIdentity(id: 'entry-1')))?.text);
    await client.connection!.resume();
    ```

This assumes `GeneratedClient.open` was given `server` and the record has already arrived locally. Without it the client is local-only until you call `client.connect`. Dart uses the same `pause`/`resume` methods with its typed mutation arguments.

A `client.mutate` call's completion confirms its local commit. It does not mean the server has accepted the operation. Display pending and rejected state using the record's [syncState](runtime.md#pending-work-and-recovery) when that distinction matters to the UI.

You can close and reopen the same local database without losing queued changes. Keep the same backend database as well: replacing a backend's receipt/cursor history with an empty database is a reset, not a temporary network interruption. The tutorial's `offline` / `online` commands preserve both databases.

## Understand acceptance and rejection

After a local mutation, the connection pushes its frozen request. A successful receipt completes the batch at once: the runtime applies the server's content for every record the accepted mutations changed, removes the completed mutations and replays remaining local changes, in one local transaction. This lets a handler's normalized result replace the optimistic value without waiting for any channel. One batch is in flight at a time, so later mutations complete after earlier ones.

If a handler rejects the mutation, Ahead removes that mutation's optimistic contribution and retains its rejection code locally. Later valid pending work may still affect the displayed record, so rollback is not necessarily a return to the value the user saw before all edits.

=== "TypeScript"

    ```ts
    const { rejections } = await client.models.entry.syncState({ id: 'entry-1' });
    console.log(rejections);
    // After handling the rejection in your UI:
    await client.dismissRejection(rejectionOrdinal);
    ```

=== "Flutter"

    ```dart
    final state = await client.models.entry.syncState(
      const EntryIdentity(id: 'entry-1'),
    );
    print(state.rejections);
    // After handling the rejection in your UI:
    await client.dismissRejection(rejectionOrdinal);
    ```

`rejectionOrdinal` is taken from the rejection you handled. Dismissing only clears the inbox entry. Retrying the business action means creating a new mutation after resolving its cause. `drop(ordinal)` is for eligible unsent mutations; it cannot cancel a request whose server outcome is unknown.

## Recover from connection failures

Provide `onError` to record background failures, and `refreshAuth` if your credentials can expire. Records that could not be applied also reach `onError`, as an `AheadReport` with a `kind`: `readFailed` when the server could not read the record, `skipped` when the local schema refused it, `conflict`, or `diverged` when a pending edit no longer applies to newer server state. A diverged edit is still sent, and `models.<name>.syncState(identity)` marks it `diverged` until the server answers it. Let the runtime retry frozen work; do not generate a new mutation merely because the original request timed out. The backend may already have committed it and retained its receipt.

Use `wake()` after an application event that should prompt another scheduling check. Use `resume()` after explicitly pausing. A closed connection cannot resume; create a new one with `client.connect` or reopen the client.

On connection or reconnection, Ahead establishes the WebSocket subscription and receives the current position of each channel. If every saved cursor is already there, it streams at once. Otherwise it sends one HTTP pull for all channels, holding changes that arrive meanwhile, then continues with WebSocket updates. Both sources use the same Rust page processing: each page applies as one transaction, covered pages are discarded, overlapping pages apply their unseen changes, and gaps trigger HTTP recovery from saved progress. Subscription changes replace the session; pages from replaced or canceled sessions cannot update local data.

## Authentication and account changes

Authenticate requests on the backend and check business permissions in handlers and loaders. `devAuth` is only for the local example. In production, your `authenticate` callback should verify your existing application's credentials and return its user ID.

Use a separate local database per signed-in user. On an account change, stop and close the old client before opening the other user's database. Changing only the transport token leaves the old user's cached records and client identity in place.

When permissions change, publish the affected records to the channels that deliver them. A loader can then return null to withdraw a record. Unsubscribing removes nothing: it stops that channel's delivery and keeps the records, their stamps and any pending edits in place. It is not a cache wipe or an authorization mechanism.

## Diagnose pending work

| Observation | Check |
| --- | --- |
| Empty local query after opening | Desired channel, running connection, loader output and read permission |
| `queued` with failed prerequisites | Host callback failure; reset its readiness to pending and run it again |
| `frozen` after a network failure | Connectivity/authentication; retain the frozen bytes for retry |
| `frozen` long after the network recovered | Either the server refused the batch on identity or order grounds (401/403/409 `client.owner_mismatch`/`gap`/`overlap`) — the code reaches `onError` and the batch is resent as is because the server never ran it — or a received receipt was refused locally (it named another client or batch, or omitted an accepted record): check `onError` and the backend's loaders |
| Server values do not update | Whether every affected channel was published to, and whether the handler reported every record it changed with `changes.add` |
| Local client fails after another process wrote | One active client per SQLite file; close/reopen the stale instance |
| Empty local data after an app update | `syncState().schema.rebuilt`: the schema was incompatible and a fresh database is synchronising from the beginning; `syncState().schema.pending` means the old file is still sending its last changes, call `rebuild()` when it reaches 0 ([local storage](storage.md#change-the-schema)) |

See [runtime APIs](runtime.md) for controls and [compatibility and recovery](storage.md) for storage constraints.
