# Set up the client

Read, watch and update local data through the generated client. TypeScript and Flutter share the same model and mutation contract, backed by the Rust engine and SQLite. Select a language above each example; Flutter examples use Dart.

Examples use the `Entry` model and `Edit` mutation of the [round-trip fixture](https://github.com/zanminwang/axton/blob/main/integration/e2e/fixtures/round-trip/models/entry.model); the [To-do example](../getting-started.md) uses the same generated API with its own names. [Generate your interfaces](../schema/define.md) before importing them.

## Set up the runtime

Packages are currently used from source. Build the native artifacts from the repository root:

```sh
bash scripts/build.sh
```

=== "TypeScript"

    The TypeScript client currently runs on Node.js 22.18 or newer. Build the native addon before importing generated code. The source package is `packages/client-js`; the generated `client.ts` of the examples already imports it through a relative path.

    Generate with `--client-runtime` pointing to that source package, relative to your generated directory. See the [schema guide](../schema/define.md#generate-from-a-source-checkout) for the working command.

=== "Flutter"

    Flutter uses the Dart client package. Add a local dependency to your application's `pubspec.yaml`, adjusting the path to your checkout:

    ```yaml
    dependencies:
      axton:
        path: /absolute/path/to/axton/packages/dart
    ```

    Run `flutter pub get`, or `dart pub get` in a Dart application. The package currently requires Dart 3.12 or newer. Generated code imports `package:axton/axton.dart`.

    For desktop development, `libraryPath` points to `target/debug/libaxton_dart.dylib` on macOS or `libaxton_dart.so` on Linux. Outside iOS it is required; on iOS, omitting it uses process-linked native symbols. Mobile packaging needs platform-specific native build/link steps; see [platform setup](platforms.md). Choose a writable application directory for the SQLite file.

## Open local storage

=== "TypeScript"

    ```ts
    import { GeneratedClient } from './generated/client.ts';

    const client = await GeneratedClient.open({ path: 'local.sqlite' });
    const entry = await client.models.entry.get({ id: 'entry-1' });
    console.log(entry?.text);
    ```

=== "Flutter"

    ```dart
    import 'generated/generated.dart';

    final client = await GeneratedClient.open(
      path: 'local.sqlite',
      libraryPath: '/absolute/path/to/axton/target/debug/libaxton_dart.dylib',
    );
    final entry = await client.models.entry.get(const EntryIdentity(id: 'entry-1'));
    print(entry?.text);
    ```

These examples open local storage without a connection. A fresh database returns null until you write local data or synchronize a channel. Use one active client per SQLite file and a separate file per signed-in user.

## Connect to your backend

Start the fixture backend with `bash integration/e2e/fixtures/round-trip/run.sh` (or the [To-do backend](../getting-started.md) with its own token and channel), then connect the client:

=== "TypeScript"

    ```ts
    import { GeneratedClient } from './generated/client.ts';

    const client = await GeneratedClient.open({
      path: 'local.sqlite',
      server: {
        url: 'http://127.0.0.1:4242',
        token: 'demo-user',
      },
      connection: { onError: console.error },
    });
    await client.channels.subscribe('book:demo');
    ```

=== "Flutter"

    ```dart
    import 'generated/generated.dart';

    final client = await GeneratedClient.open(
      path: 'local.sqlite',
      libraryPath: '/absolute/path/to/axton/target/debug/libaxton_dart.dylib',
      server: SyncServer(
        url: 'http://127.0.0.1:4242',
        token: () => 'demo-user',
      ),
      onError: (error) => print(error),
    );
    await client.channels.subscribe('book:demo');
    ```

Configure the server once. AXTON submits mutations over HTTP, catches up from saved channel cursors over HTTP, and receives ongoing record changes over WebSocket. Every received page passes through the Rust engine into local SQLite and updates `watch` subscriptions.

Subscribing wakes the connection; it does not wait for initial records. A client with no subscribed channels can still push mutations, but acceptance does not preserve their optimistic records. [Subscribe to receive mutation results](sync.md#receive-mutation-results) when your UI needs to keep displaying them. Subscription changes update live synchronization automatically. Replace the demo URL and token with your application's endpoint and credentials. On a physical device, localhost refers to that device; use a reachable development-server address.

Omit `server` to open local storage without starting a connection.

For expiring credentials, supply a token function and `refreshAuth`. See [server connection options](runtime.md#server-connection).

## Watch and write

=== "TypeScript"

    ```ts
    const stop = client.models.entry.watch({}, entries => console.log(entries), console.error);

    // After entry-1 has arrived locally:
    await client.mutate.edit({
      entry: { identity: { id: 'entry-1' }, values: { text: 'Draft', note: null } },
    });
    ```

=== "Flutter"

    ```dart
    final subscription = client.models.entry.watch().listen(
      (entries) => print(entries),
      onError: (Object error) => print(error),
    );

    // After entry-1 has arrived locally:
    await client.mutate.edit(
      entry: const EditEntryUpdate(
        identity: EntryIdentity(id: 'entry-1'),
        text: Present('Draft'),
        note: Present(null),
      ),
    );
    ```

Watch emits an initial local result and distinct committed results. A mutation applies locally and queues the backend operation. Its return value is a local ordinal, not server confirmation. Direct writes through `tx.models` only change local storage.

In Flutter, use the watch stream with `StreamBuilder<List<Entry>>`; retain it for the view's lifetime rather than reopening a client on every build. Dart's `Present(null)` clears a nullable field; omitting the field leaves it unchanged.

## Connection and cleanup

=== "TypeScript"

    ```ts
    await client.connection!.pause();
    // Local reads and writes remain available.
    await client.connection!.resume();

    // When the owning view/application finishes:
    stop();
    await client.close();
    ```

=== "Flutter"

    ```dart
    await client.connection!.pause();
    // Local reads and writes remain available.
    await client.connection!.resume();

    // When the owner of this client finishes:
    await subscription.cancel();
    await client.close();
    ```

Pausing cancels network activity; resuming reconnects and catches up from saved progress. Database/client lifetime belongs to the application; watch subscriptions belong to their views.

See [Client API](client-api.md) for typed calls, [offline work and sync](sync.md) for connection/recovery behavior, and [advanced client APIs](runtime.md) for SQL, savepoints and prerequisites.
