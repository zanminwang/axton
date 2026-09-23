# Generated client

This page covers the TypeScript and Dart APIs emitted by the schema compiler. Follow [getting started](../getting-started.md) for a running backend, or [generate your interfaces](../schema/define.md) first.

Choose TypeScript or Flutter above a code example to switch languages throughout the page. Flutter examples use Dart.

Examples below assume an `Entry` model with `id`, `text` and nullable `note`, and an `Edit` mutation whose `entry` slot updates `text` and `note`. TypeScript imports come from `generated/client.ts`; Dart imports come from `generated/generated.dart`.

## Open a client

=== "TypeScript"

    ```ts
    import { GeneratedClient } from './generated/client.ts';

    const client = await GeneratedClient.open({
      path: 'local.sqlite',
    });
    ```

=== "Flutter"

    ```dart
    import 'generated/generated.dart';

    final client = await GeneratedClient.open(
      path: 'local.sqlite',
      libraryPath: '/absolute/path/to/libaxton_dart.dylib',
    );
    ```

Both examples open local storage. To start background sync, supply `server` as shown in [client setup](setup.md#connect-to-your-backend).

| Option | Required | Behavior |
| --- | --- | --- |
| `path` | Yes | SQLite file to create or reopen. The application selects a writable directory. Use a separate file per signed-in user. |
| `server` | No | Backend URL and credentials: `ServerOptions` in TypeScript, `SyncServer` in Dart. AXTON manages HTTP mutation submission, HTTP catch-up and WebSocket updates. |
| `connection` (TypeScript) | No | `onError` and `refreshAuth` callbacks for the background connection. `onError` also receives an `AxtonReport` for each record AXTON could not apply ([Sync](sync.md#recover-from-connection-failures)). |
| `onError`, `refreshAuth` (Dart) | No | The same callbacks, passed directly to `open`. |
| `libraryPath` (Dart) | Outside iOS | Absolute native library path; iOS can use symbols linked into the process. |
| `migration` | No | Defaults and optional cursor rewind for an explicitly changed schema. See [runtime migration](runtime.md#opening-and-schema-changes). |

Returns `Promise<GeneratedClient>` / `Future<GeneratedClient>`. Opening can fail on native library loading, an unwritable or incompatible database, or an invalid schema. Completion means local storage is open, not that initial server data has arrived. Omitting `server` keeps the client local-only.

## Model APIs

### Get a record

=== "TypeScript"

    ```ts
    const entry = await client.models.entry.get({ id: 'entry-1' });
    console.log(entry?.text);
    ```

=== "Flutter"

    ```dart
    final entry = await client.models.entry.get(
      const EntryIdentity(id: 'entry-1'),
    );
    print(entry?.text);
    ```

`get(identity)` returns the complete typed record or `null` when it is absent from local storage. It does not call the backend loader. A newly opened cache can return `null` until channel synchronization supplies the record.

### Query records

=== "TypeScript"

    ```ts
    const entries = await client.models.entry.query({
      where: { note: null },
      orderBy: [{ field: 'text', direction: 'ascending' }],
      limit: 20,
    });
    ```

=== "Flutter"

    ```dart
    final entries = await client.models.entry.query(
      where: const EntryFilter(note: Present(null)),
      orderBy: const [EntryOrder(EntryOrderField.byText)],
      limit: 20,
    );
    ```

Returns a typed list. `where` is an equality filter; supplied fields must all match. An omitted filter selects all local records of that model. `orderBy` is a list of fields and ascending/descending directions; Dart expresses descending order with `descending: true`. `limit` caps the result. Do not rely on an unspecified row order. These generated filters are not a general SQL expression language.

### Watch records

=== "TypeScript"

    ```ts
    const stop = client.models.entry.watch(
      { where: { note: null } },
      entries => console.log(entries),
      error => console.error(error),
    );
    // When the view is disposed:
    stop();
    ```

=== "Flutter"

    ```dart
    final subscription = client.models.entry
        .watch(where: const EntryFilter(note: Present(null)))
        .listen(print, onError: (Object error) => print(error));
    // When the view is disposed:
    await subscription.cancel();
    ```

TypeScript returns an unsubscribe function; Dart returns `Stream<List<Entry>>`. A listener receives an initial query result and distinct results after committed local changes, including sync changes. Identical query results are suppressed. `watch` accepts equality filters, not `query`'s ordering or limit options. It reports current query results, not a log of every intermediate write.

### Follow a relation

The compiler emits relation methods only for relationships declared in the schema. A forward relation returns the related record or `null`; an inverse collection returns a list. Methods take the source model's identity, and read the local database.

For a `Comment.book` relationship, `client.models.comment.book(commentIdentity)` follows the forward reference. The [relations fixture](https://github.com/zanminwang/axton/blob/main/fixtures/compiler/relations.model) defines `Book.comments` and `Comment.book`; the [generated API checks](https://github.com/zanminwang/axton/blob/main/integration/generated-api/verify.sh) exercise those accessors. A singular inverse needs a unique foreign key; an ambiguous inverse is rejected during compilation.

## Transactions

=== "TypeScript"

    ```ts
    await client.transaction(async tx => {
      await tx.models.entry.update({ id: 'entry-1' }, { text: 'Draft' });
      const draft = await tx.models.entry.get({ id: 'entry-1' });
      if (draft?.text !== 'Draft') throw Error('local update missing');
    });
    ```

=== "Flutter"

    ```dart
    await client.transaction((tx) async {
      const id = EntryIdentity(id: 'entry-1');
      await tx.models.entry.update(id, const EntryPatch(text: Present('Draft')));
      final draft = await tx.models.entry.get(id);
      if (draft?.text != 'Draft') throw StateError('local update missing');
    });
    ```

`transaction<T>(callback)` returns the callback's result after local commit. Throwing or a failed operation rolls it back. Await each operation, including nested callbacks; unfinished work is rejected. Inside the callback, use `tx.models` for reads that must see earlier writes in the same transaction. Calling the outer `client` for a read from inside its transaction can wait behind that transaction. A captured `client.mutate` call inside its own transaction callback fails promptly with `transaction_active` instead of waiting on itself.

`GeneratedTransaction` exposes `models` and the underlying `transaction`; it has no mutation or watch method. For nested savepoints, see [transactions and savepoints](runtime.md#transactions-and-savepoints).

A single mutation does not need an explicit transaction: `client.mutate.edit(args)` runs in its own local transaction and returns the ordinal.

## Action contract (execution pending #142)

The compiler emits `ActionClientContract` and backend handler types for the [Action schema](../schema/reference.md#action-contracts-execution-pending-142). These are type-level examples; today's runnable `GeneratedClient` still uses the mutation methods below. This example uses the [integration Action schema](https://github.com/zanminwang/axton/blob/main/integration/action-contract/schema.model) and its generated `ActionClientContract` and `TodoCreate` types. The [standalone declaration example](../schema/define.md#declare-an-action-contract) defines a different, smaller schema.

```typescript title="action-contract"
const todo: TodoCreate = { id: 'todo-1', title: 'Done', state: 'open', note: null };
const call = await client.actions.addTodo({ todo, gone: [], status: null, tags: [] });
const outcome = await call.wait();
if (outcome.error === null) console.log(outcome.result.todo.title);
const todos = await client.actions.call.getTodos({});
console.log(todos.todos);
await client.actions.call.deleteTodo({ todo: { id: 'todo-1' } });
await client.actions.call.sendEmail({ to: 'team@example.test', subject: 'Todo update', body: 'Done' });
```

The default route accepts work locally and returns a handle with only `status` and `wait()`. Initial local validation or commit failure rejects before a handle exists. A successful `wait()` outcome contains the final output in `result`; a terminal business failure contains `ActionError` in the outcome, while a pending network retry remains pending. `actions.call` is a separate request-response route: it has no queue or automatic optimism and rejects with `ActionError` on execution failure. Neither route belongs inside an application-owned local transaction. Standalone `client.models` create/update/delete are local-only, and a transaction exposes local reads and CRUD without Actions or watch.

An implicit create/update Model result is bound to its input identity; a delete result confirms an identity. An explicit Model result comes from an identity object chosen by the handler. Optional Model operands produce null when absent, lists preserve order and duplicates, nullable fields remain present with null, and an Action without outputs returns void. #142 will resolve Model results through a shared Loader path and preserve each call's snapshot, distinct from the batch-final records used to settle local state and from the current local view after later optimism. #116 will define ephemeral output policy. These behaviors are specified by the generated contracts and descriptors; runtime verification belongs to those follow-up issues.

## Mutations

The backend result of your own mutation arrives in its receipt: the records the handler changed are read back by your loader and replace the optimistic values, with or without a subscription. Subscribe to a channel to receive changes made elsewhere. See [receiving mutation results](sync.md#receive-mutation-results).

`client.mutate.edit(args)` runs one framework-owned local transaction and returns the mutation's local ordinal (`number` / `int`). This identifies queued work; it is not a backend result or confirmation. Its declared changes apply in local storage immediately, and the backend later runs the matching handler.

| Schema slot | TypeScript argument | Backend input |
| --- | --- | --- |
| `Entry.create` | Complete `Entry` record | Complete record |
| `Entry.update<text,note>` | `{ identity, values }` with permitted patch fields | `{ identity, patch }` |
| `Entry.delete` | `EntryIdentity` | Identity |
| Optional slot (`?`) | Optional slot value | Optional value |
| List slot (`[]`) | Array of slot values | Array of decoded values |

Dart generates typed slot classes such as `EditEntryUpdate`. Use `Present` for fields you intend to change. The compiler gives each named mutation a method with a lower-case first letter: `Edit` becomes `edit`. Several slots can participate in one mutation. Separate `client.mutate` calls have separate local commits and backend acceptance/rejection outcomes; use one multi-slot mutation when the changes must form one business operation.

The backend handler's implementation can differ from the declared local operation. It may normalize input, enforce permissions, or write several business tables. [Notifications and loaders](../backend/api.md) tell the client the resulting server state.

## Local-only writes

=== "TypeScript"

    ```ts
    await client.transaction(async tx => {
      await tx.models.entry.create({ id: 'draft', text: 'Only here', note: null });
      await tx.models.entry.update({ id: 'draft' }, { note: 'Remember this' });
      await tx.models.entry.delete({ id: 'draft' });
    });
    ```

=== "Flutter"

    ```dart
    await client.transaction((tx) async {
      const id = EntryIdentity(id: 'draft');
      await tx.models.entry.create(
        const Entry(id: 'draft', text: 'Only here', note: null),
      );
      await tx.models.entry.update(id, const EntryPatch(note: Present('Remember this')));
      await tx.models.entry.delete(id);
    });
    ```

`create(record)`, `update(identity, patch)` and `delete(identity)` return `Promise<void>` / `Future<void>`. They change local storage without enqueueing a backend mutation. Use `client.mutate` outside the callback for changes that must reach your backend. Invalid identities, field values, references or uniqueness constraints can reject a local write and roll back the transaction.

## Channels

=== "TypeScript"

    ```ts
    await client.channels.subscribe('book:demo');
    await client.channels.unsubscribe('book:demo');
    ```

=== "Flutter"

    ```dart
    await client.channels.subscribe('book:demo');
    await client.channels.unsubscribe('book:demo');
    ```

These operations persist the desired subscription and wake a running connection. Subscribing does not wait for all records to arrive. Use `watch` to observe the initial pull and later changes.

A channel name must match what your backend publishes to. A subscription is a request for data; loaders must still enforce read permissions. Unsubscribing stops that channel's synchronization and removes nothing: records, their stamps and pending edits stay. See [sync and recovery](sync.md) for cache and account-change behavior.

## Status and lifecycle

- `client.syncState()` returns the client's pending count, cursors, channels and rejections; `client.models.<name>.syncState(identity)` returns one record's pending mutations and rejections, typed by the model. Neither sends network requests. See [pending work and recovery](runtime.md#pending-work-and-recovery).
- `client.clientId` is this database's durable client identity.
- `client.connection` is the connection created by `open` or `client.connect`. It is `undefined` / `null` when there is none. See [connection controls](runtime.md#connection-controls).
- Recovery, prerequisite and escape-hatch members (`dismissRejection`, `drop`, `pendingTasks`, `setReadiness`, `runPrerequisites`, `querySpec`, `readSql`) are on the same object; see the [client runtime reference](runtime.md).
- `await client.close()` stops the connection and releases the local database handle. Close the client when its owning application scope ends; cancel individual watchers when their views end. Calls after close fail.

## Generated data types

| Type or helper | Meaning |
| --- | --- |
| `Entry` | Complete state, including its identity fields. A nullable field is still present in a complete record. |
| `EntryIdentity` | Only the fields declared in `@@id`; composite identities contain every key field. |
| `EntryPatch` | Only editable non-identity fields. Omission leaves a field unchanged; explicit `null` clears a nullable field. |
| `EditArgs` (TypeScript) | Typed argument object for `Edit`; update slots use `values`. |
| `EditEntryUpdate` (Dart) | Typed update slot, with identity and `Present`-wrapped changed fields. |
| `EntryFilter` (Dart) | Typed equality filter. `Present(null)` explicitly filters for null. |
| `EntryOrderField`, `EntryOrder` (Dart) | Typed ordering field and direction. |
| `Present<T>` (Dart) | Distinguishes omission from an explicitly supplied value, including null. |

UUID fields are strings; DateTime fields use language date/time values and encode to UTC strings. Avoid integers outside the JSON/JavaScript safe range. See the [schema compiler reference](../schema/reference.md) for the supported field types.

## Extension points

`ReadPort` declares `read`, `querySpec`, `related`, and `referencing`. `WritePort` adds `direct` and `mutate`. TypeScript `LivePort` adds `watch`; Dart live models use the runtime `Client`. These are forwarding contracts, not alternate storage engines supplied automatically by the generator.

TypeScript exports model classes (`EntryModel`, `EntryLiveModel`, `EntryTxModel`), `Mutate`, `LiveModels`, `TxModels`, `liveModels(port)`, `txModels(port)` and `GeneratedTransaction`. Dart exposes corresponding facade classes. Construct these only when adapting an existing compatible port; normal applications obtain them through `GeneratedClient`.

TypeScript's `encodeEntry`, `decodeEntry`, `encodeEntryIdentity`, `encodeEntryPatch` and `encodeEntryWhere`, and Dart's `toRecord`/`fromRecord`, perform wire conversions. They assume schema-compatible data; casts in generated decoders are not a substitute for validating arbitrary untrusted input. Standalone mutation builders (`Edit(args)` in TypeScript, `edit(...)` in Dart) build operation descriptors; they do not enqueue them until a runtime receives them.
