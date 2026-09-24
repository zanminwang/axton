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

## Receive Action results

**A subscription is not required to see your own result.** A durable Action's inferred local Model changes are optimistic. Its handle's `wait()` returns the final per-invocation result or an error. The receipt also carries batch-final authority for changed records, read through the Loader in the handler transaction. AXTON applies that authority and replays later pending edits over it. Thus the result snapshot and current local Model view can differ. A direct Action has no automatic local optimism or durable queue; its response carries its result and applies authority through the same local state path.

Subscribe with `client.channels.subscribe(channel)` as above when the client needs changes made elsewhere: by other users, by background jobs, or by handlers that touch records without reporting them. Subscription starts synchronization; it does not wait for initial data. Use `watch` to observe the records, and wait for an existing record to be available locally before updating it.

You can send Actions without subscribing to any channel. The receipt still corrects the local row to the server's batch-final state; what you do not receive is later changes from elsewhere. If you subscribe to a channel the handler publishes to, the page for your own change carries the same stamp as the receipt and rewrites nothing, whichever arrives first.

## Work offline

=== "TypeScript"

    ```ts title="action-contract"
    await client.connection!.pause();
    const call = await client.actions.addTodo({
      todo: { id: 'todo-offline', title: 'Draft', state: 'open', note: null },
      gone: [], status: null, tags: [],
    });
    console.log((await client.models.todo.get({ id: 'todo-offline' }))?.title);
    await client.connection!.resume();
    const outcome = await call.wait();
    if (outcome.error) console.error(outcome.error.code);
    ```

=== "Flutter"

    ```dart title="action-contract"
    await client.connection!.pause();
    final call = await client.actions.addTodo(
      todo: const Todo(id: 'todo-offline', title: 'Draft', state: Status.open, note: null),
      gone: const [], status: null, tags: const [],
    );
    print((await client.models.todo.get(const TodoIdentity(id: 'todo-offline')))?.title);
    await client.connection!.resume();
    final outcome = await call.wait();
    if (outcome is ActionFailure<AddTodoOutput>) print(outcome.error.code);
    ```

This assumes `GeneratedClient.open` was given `server`; otherwise call `client.connect` before resuming. The Action's create operand is visible locally while the connection is paused. Dart uses the same `pause`/`resume` methods.

The returned handle confirms the local commit. It does not mean the server has accepted the Action; `wait()` observes the terminal outcome. Display pending and rejected state using the record's [syncState](runtime.md#pending-work-and-recovery) when that distinction matters to the UI.

You can close and reopen the same local database without losing queued intent. A new client handle cannot retrieve a past result from client memory. Keep the same backend database as well: replacing its outcome/receipt/cursor history with an empty database is a reset, not a temporary network interruption. The tutorial's `offline` / `online` commands preserve both databases.

## Understand acceptance and rejection

After a durable Action is accepted locally, the connection pushes its frozen request. A successful receipt completes the batch at once: the runtime applies server authority for changed records, removes completed queue entries and replays remaining local changes in one local transaction. One batch is in flight at a time. The handle's result stays the snapshot of its own invocation, even when later work in the batch changes the record.

If a handler rejects an Action, AXTON removes its optimistic contribution and retains the rejection code locally. Later valid pending work may still affect the displayed record, so rollback is not necessarily a return to the value the user saw before all edits.

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

`rejectionOrdinal` is taken from the rejection you handled. Dismissing only clears the inbox entry. Retrying the business Action means making a new call after resolving its cause. `drop(ordinal)` is for eligible unsent work; it cannot cancel a request whose server outcome is unknown.

## Recover from connection failures

Provide `onError` to record background failures, and `refreshAuth` if your credentials can expire. Records that could not be applied also reach `onError`, as an `AxtonReport` with a `kind`: `readFailed` when the server could not read the record, `skipped` when the local schema refused it, `conflict`, or `diverged` when a pending edit no longer applies to newer server state. A diverged edit is still sent, and `models.<name>.syncState(identity)` marks it `diverged` until the server answers it. Let the runtime retry frozen work; do not create a new Action merely because the original request timed out. The backend may already have committed it and retained its outcome.

Use `wake()` after an application event that should prompt another scheduling check. Use `resume()` after explicitly pausing. A closed connection cannot resume; create a new one with `client.connect` or reopen the client. Direct Action requests use a finite timeout; an `unknown` execution status can mean the backend committed but the client did not observe the response.

On connection or reconnection, AXTON establishes the WebSocket subscription and receives the current position of each channel. If every saved cursor is already there, it streams at once. Otherwise it sends one HTTP pull for all channels, holding changes that arrive meanwhile, then continues with WebSocket updates. Both sources use the same Rust page processing: each page applies as one transaction, covered pages are discarded, overlapping pages apply their unseen changes, and gaps trigger HTTP recovery from saved progress. Subscription changes replace the session; pages from replaced or canceled sessions cannot update local data.

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
