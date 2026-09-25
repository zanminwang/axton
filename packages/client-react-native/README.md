# AXTON for React Native

React Native host integration for the existing Rust/SQLite client. The generated TypeScript models and mutation API are shared with Node. Native calls, transaction scope, and HTTP/WebSocket transport use the mobile environment.

The package is currently used from an AXTON repository checkout. It is not a published standalone npm distribution. The initial native target is an arm64 iOS simulator with an Expo native build. Android, browser/WASM, physical-device execution and app-store packaging are not verified by this integration.

## Install and build

Use the [integration app](../../integration/platform/react-native/README.md) for a complete, locked example. From an Expo app in this repository, add file dependencies on this package and [the native module](native-module/README.md), then rebuild the native app. Expo Go does not contain the AXTON module.

Build the Rust library before installing CocoaPods for a local file dependency:

```sh
rustup target add aarch64-apple-ios-sim
bash packages/client-react-native/native-module/scripts/build-ios.sh simulator
```

The integration app's Metro configuration includes the repository source and `.mts` modules, and resolves React Native/Expo dependencies from the app. Its Babel configuration applies the TypeScript transform to `.mts` files, which the Expo preset otherwise treats as plain JavaScript. Reuse both configurations when consuming these local packages. Do not substitute Node polyfills or import `packages/client-js/index.mts` in a mobile bundle.

Generate the application's client with `--client-runtime` pointing to this package's `index.ts`, or to `@axton/client-react-native` when package resolution is configured. The compiler owns generated files.

```ts
import { databasePath } from '@axton/client-react-native';
import { GeneratedClient } from './generated/client';

const client = await GeneratedClient.open({
  path: await databasePath('app.sqlite'),
  server: { url: 'http://127.0.0.1:4242', token: 'demo-user' },
  connection: { onError: error => console.error(error) },
});
await client.scopes.subscribe('book:demo');
```

The URL above is for a simulator using a backend on its host. Configure a reachable address and the application's real authentication for other environments. Local HTTP permission belongs to the development app configuration.

`databasePath(name = 'axton.sqlite'): Promise<string>` resolves a basename in Application Support and creates the parent directory. It rejects paths and invalid basenames. Keep the same filename when reopening a client; the engine persists client identity and queued work there. Different installations have separate app containers.

## Client behavior

Generated reads, queries, watches, direct writes, named mutations, transactions, and channel subscriptions use the same ports as Node. A `client.mutate` call commits its optimistic changes and queue entry together before returning its local ordinal; network delivery proceeds independently. Public transactions are for local model reads and direct writes only. See [client API](../../website/docs/frontend/client-api.md) for the shared generated API.

Always await each operation inside a transaction. A failed command poisons that transaction even if the callback catches its error. Unawaited operations prevent commit; queued work drains before rollback, and escaped transaction objects reject further calls. The mobile raw transaction does **not** expose nested `savepoint`; Node's existing savepoint API remains available on Node. A captured `client.mutate` call during an active public transaction fails promptly with `transaction_active`. React Native applies this guard to unrelated concurrent mutation calls too; retry those after the public transaction settles.

`Client` also exposes inspection/control methods used by the generated facade: `status`, `recordStatus`, `pendingTasks`, `setReadiness`, `runPrerequisites`, `drop`, `dismissRejection`, `readSql`, connection lifecycle, and `close`. These retain the current engine contracts, including the existing limitations of migration options. Do not create a new client on every React render. Dispose watches and call `close` when the owning session ends.

The transport authenticates HTTP and native WebSocket requests with bearer tokens, forwards raw frames to the shared Rust `LiveSession`, honors cancellation, and bounds pending frame delivery to 64. Rust validates acknowledgements and owns catch-up, page application, gap recovery and overflow recovery. Node and React Native share the same action executor; ordinary live pages do not cause polling. Native WebSocket errors lack a structured HTTP status; automatic `refreshAuth` based on status 401 is available for HTTP errors, while applications must also manage credentials for native socket failures. The adapter does not infer status from error message wording. The transport derives the WebSocket address with the global `URL` class, which Expo polyfills; a bare React Native app without Expo needs its own `URL` polyfill.

Foreground execution on an arm64 iOS simulator is verified by the integration harness (see its recorded evidence); iOS background execution and delivery while the process is suspended are not promised. Offline relaunch testing must use an embedded JS bundle rather than depend on Metro.

## Verification

Host tests cover the mobile transaction adapter, the shared runtime using real SQLite through the Node carrier, and the native-style transport over real HTTP/WebSocket. They do not substitute for iOS runtime validation:

```sh
node --test integration/bindings/client-react-native/*.test.mjs
```

The separate [simulator harness](../../integration/platform/react-native/README.md) checks the actual Expo/Swift/Rust boundary and two-client offline/restart/reconnect behavior. See its recorded evidence for tested versions and results.
