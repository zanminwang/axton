# Generated client

This page covers the TypeScript and Dart APIs emitted by the schema compiler. Follow [getting started](../getting-started.md) for a running backend, or [generate your interfaces](../schema/define.md) first.

Choose TypeScript or Flutter above a code example to switch languages throughout the page. Flutter examples use Dart.

Local Model examples below use an `Entry` model with `id`, `text` and nullable `note`. Action examples use the [generated Action fixture](https://github.com/zanminwang/axton/blob/main/integration/action-contract/schema.model), whose `Todo` model has `id`, `title`, `state` and nullable `note`. TypeScript imports come from `generated/client.ts`; Dart imports come from `generated/generated.dart`.

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
| `server` | No | Backend URL and credentials: `ServerOptions` in TypeScript, `SyncServer` in Dart. AXTON manages durable and direct Action requests, HTTP catch-up and WebSocket updates. |
| `connection` (TypeScript) | No | `onError`, `refreshAuth` and `directTimeoutMs` for the connection. `onError` also receives an `AxtonReport` for each record AXTON could not apply ([Sync](sync.md#recover-from-connection-failures)). |
| `onError`, `refreshAuth`, `directTimeout` (Dart) | No | Callbacks and a `Duration` for direct requests, passed to `open`. |
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

`transaction<T>(callback)` returns the callback's result after local commit. Throwing or a failed operation rolls it back. Await each operation, including nested callbacks; unfinished work is rejected. Inside the callback, use `tx.models` for reads that must see earlier writes in the same transaction. Calling the outer `client` for a read from inside its transaction can wait behind that transaction. Calling an Action from the transaction callback fails with `transaction_active` instead of waiting on itself.

`GeneratedTransaction` exposes `models` and the underlying `transaction`; it has no Action or watch method. For nested savepoints, see [transactions and savepoints](runtime.md#transactions-and-savepoints).

## Actions

An Action is a typed backend operation. Use `client.actions.<name>(args)` when work should be accepted locally and delivered after an offline period. It returns `Promise<ActionCall<Output>>` / `Future<ActionCall<Output>>` after the local intent and inferred Model changes commit. Use `client.actions.call.<name>(args)` when the caller needs a direct response now; it returns the final `Output` and requires a connection. Both routes execute the same registered backend handler and resolve Model results through the versioned Loader. Neither route runs inside a local `client.transaction` callback.

These examples use the [Action fixture](https://github.com/zanminwang/axton/blob/main/integration/action-contract/schema.model). Its `AddTodo` inputs include a create operand, optional update, delete list, nullable enum and string list.

=== "TypeScript"

    ```typescript title="action-contract"
    const todo: TodoCreate = { id: 'todo-1', title: 'Done', state: 'open', note: null };
    const call = await client.actions.addTodo({ todo, gone: [], status: null, tags: [] });
    console.log(call.status);
    const outcome = await call.wait();
    if (outcome.error === null) console.log(outcome.result.todo.title);
    else console.error(outcome.error.code, outcome.error.execution);
    const todos = await client.actions.call.getTodos({});
    console.log(todos.todos);
    ```

=== "Flutter"

    ```dart title="action-contract"
    final call = await client.actions.addTodo(
      todo: const TodoCreate(id: 'todo-1', title: 'Done', state: Status.open, note: null),
      gone: const [],
      status: null,
      tags: const [],
    );
    final outcome = await call.wait();
    if (outcome is ActionSuccess<AddTodoOutput>) {
      print(outcome.result.todo.title);
    } else if (outcome is ActionFailure<AddTodoOutput>) {
      print(outcome.error.code);
    }
    final todos = await client.actions.call.getTodos();
    print(todos.todos);
    ```

`ActionCall` exposes `status` (`pending`, `succeeded` or `failed`) and `wait()`. The handle's initial return confirms local acceptance, not backend success. Initial validation or local commit failure rejects before a handle exists. `wait()` resolves to an outcome with either `result` or `ActionError`; a pending network retry keeps waiting. Direct calls do not enter the durable queue or apply automatic local optimism. A direct execution error rejects with `ActionError`, and its request has a finite timeout. `ActionError.code` is a stable machine code and `execution` is `rejected` for a known rejection or `unknown` when the outcome cannot be observed. A timeout with unknown execution does not prove the backend did nothing.

TypeScript sets `connection.directTimeoutMs` in milliseconds (an integer from 1 to 2,147,483,647); Dart sets `directTimeout` on `open` or `connect` to a positive `Duration`. Both default to 30 seconds. See [server connection](runtime.md#server-connection) for option placement. Queued Actions use background retry instead of this direct timeout.

The created `todo` result is the Loader snapshot for that Action invocation. A later Action in the same batch can change the batch-final authority, and a pending local edit can change what `client.models.todo.get(...)` shows. The result retains its own snapshot; applying server authority replays pending edits over the new base. Model creates/updates imply full Model results bound to the input identity; deletes confirm identities. A handler selects an explicit Model output by returning an identity object with every `@@id` field. Optional outputs can be null, lists preserve order and duplicates, and no outputs means void. Results held by live calls are kept in client memory; reopening preserves pending work and completion state, but does not restore a past business result to a new handle. The backend retains committed outcomes for replay without a TTL or automatic pruning.

### Storing Model results

By default, Model records returned by an Action's explicit Model outputs also update the matching local Models. Pass `store` when calling to return results without storing them, for example search suggestions. `false` stores none of those outputs; a map names outputs, and unnamed ones stay stored. Both routes accept it and the result type is unchanged.

=== "TypeScript"

    ```typescript title="action-contract"
    const suggestions = await client.actions.call.getTodos({}, { store: false });
    const page = await client.actions.call.openTodo({ store: null }, { store: { suggestions: false } });
    const call = await client.actions.getTodos({}, { store: false });
    const outcome = await call.wait();
    console.log(suggestions.todos, page.mainTodo, outcome.error);
    ```

=== "Flutter"

    ```dart title="action-contract"
    final suggestions = await client.actions.call.getTodos(store: const GetTodosStore.none());
    final page = await client.actions.call.openTodo(
      store: null,
      outputStore: const OpenTodoStore.outputs(suggestions: false),
    );
    print([suggestions.todos, page.mainTodo]);
    ```

`store` only controls these output records. Records the Action writes (its Model operands and changes the handler reports) are always reconciled, a record also returned by a stored output is stored, and an already stored row is left unchanged by an unstored read. It does not make the call read-only or change backend persistence: the outcome is still saved for replay, and a retry keeps the original choice. A call ID replayed with a different `store` is rejected with `call.identity_conflict`. In Dart, the selector parameter is `store` unless the Action has a business input named `store`, as `OpenTodo` does; it is then `outputStore`.

The TypeScript/React Native Action observer requires a working `WeakRef`; a runtime without it rejects durable invocation with `ActionError` code `action.unsupported_runtime`. Dart uses `WeakReference`. SDKs keep active waits strongly until they settle, while otherwise allowing unobserved handles to be collected. Exceptions from a diagnostic `onError` callback after authority commits are reported through the runtime's uncaught-error channel (`reportError` or an asynchronous throw in JavaScript; the current Zone in Dart). They do not replace the Action result, retry the handler or become transport errors.

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

`create(record)`, `update(identity, patch)` and `delete(identity)` return `Promise<void>` / `Future<void>`. They change local storage without uploading a backend Action. Use `client.actions.<name>` for backend work. A later server update for the same identity may replace the cached local record; the application owns any conflict policy. Invalid identities, field values, references or uniqueness constraints can reject a local write and roll back the transaction.

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

- `client.syncState()` returns the client's pending count, cursors, channels and rejections; `client.models.<name>.syncState(identity)` returns one record's pending Actions and rejections, typed by the Model. Neither sends network requests. See [pending work and recovery](runtime.md#pending-work-and-recovery).
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
| `AddTodoInput` (TypeScript) | Typed arguments for `AddTodo`; its update operand uses `values`. |
| `AddTodoPatchUpdate` (Dart) | Typed update operand, with identity and `Present`-wrapped changed fields. |
| `EntryFilter` (Dart) | Typed equality filter. `Present(null)` explicitly filters for null. |
| `EntryOrderField`, `EntryOrder` (Dart) | Typed ordering field and direction. |
| `Present<T>` (Dart) | Distinguishes omission from an explicitly supplied value, including null. |

UUID fields are strings; DateTime fields use language date/time values and encode to UTC strings. Avoid integers outside the JSON/JavaScript safe range. See the [schema compiler reference](../schema/reference.md) for the supported field types.

## Extension points

`ReadPort` declares `read`, `querySpec`, `related`, and `referencing`. `WritePort` adds local direct writes; TypeScript `LivePort` adds `watch`. These are forwarding contracts for generated Model facades, not alternate storage engines supplied automatically by the generator.

TypeScript exports Model classes (`EntryModel`, `EntryLiveModel`, `EntryTxModel`), `LiveModels`, `TxModels`, `liveModels(port)`, `txModels(port)` and `GeneratedTransaction`. Dart exposes corresponding facades. Construct these only when adapting a compatible port; normal applications obtain them through `GeneratedClient`.

TypeScript's `encodeEntry`, `decodeEntry`, `encodeEntryIdentity`, `encodeEntryPatch` and `encodeEntryWhere`, and Dart's `toRecord`/`fromRecord`, perform wire conversions. They assume schema-compatible data; casts in generated decoders are not a substitute for validating arbitrary untrusted input. Generated Action methods encode arguments, invoke the shared runtime and decode typed results.
