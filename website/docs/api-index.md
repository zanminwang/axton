# API reference

Use this index to find the interface you call or implement. The examples use the `Entry` model and `Edit` mutation of the [round-trip fixture](https://github.com/zanminwang/ahead/blob/main/integration/e2e/fixtures/round-trip/models/entry.model); the [To-do example](getting-started.md) exposes the same interfaces as `Todo`, `addTodo` and `setTodoDone`. Generated names change with your schema: `Entry` becomes your model name, and `edit` becomes your mutation name.

## Application interfaces

| Interface | Use it to | Reference |
| --- | --- | --- |
| `GeneratedClient.open` | Open a local database and optionally start background sync | [Generated client](frontend/client-api.md#open-a-client) |
| `client.models.<model>` | Read, query, watch and follow relations in local data | [Model APIs](frontend/client-api.md#model-apis) |
| `client.transaction` | Commit local reads and direct writes together | [Transactions](frontend/client-api.md#transactions) |
| `tx.models.<model>` | Create, update or delete local-only records | [Local-only writes](frontend/client-api.md#local-only-writes) |
| `client.mutate.<mutation>` | Apply a declared local change and queue its backend operation atomically | [Mutations](frontend/client-api.md#mutations) |
| `client.channels` | Subscribe or unsubscribe to a named channel | [Channels](frontend/client-api.md#channels) |
| `client.connection` | Pause, resume or wake background sync | [Connections](frontend/runtime.md#connection-controls) |
| `client.syncState`, `client.close` | Inspect pending work and release resources | [Status and lifecycle](frontend/client-api.md#status-and-lifecycle) |
| Model, Identity, Patch, Filter and Order types | Pass typed data to generated methods | [Generated data types](frontend/client-api.md#generated-data-types) |
| `Handlers<Tx>`, `HandlerCall` | Implement each mutation's authoritative business logic | [Handlers](backend/api.md#handlers) |
| `Loaders<Tx>`, `LoaderCall` | Return current records for synchronization | [Loaders](backend/api.md#loaders) |
| `changes`, `Changes` | Report a record a handler changed beyond the uploaded operations, so it is stamped, read back and returned in the receipt | [Handlers](backend/api.md#handlers) |
| `publish`, `PublishArgs`, model reference functions | Distribute a mutation's changed records, or chosen records, to a channel | [Publishing](backend/api.md#publishing) |
| `createBackend`, `Options<Tx>` | Connect your implementations to the backend runtime | [Backend setup](backend/api.md#createbackend), [What your backend owns](backend/api.md#what-your-backend-owns) |
| `backend.listen` | Serve sync requests and close the listener | [Listener](backend/api.md#listener), [Deploy the backend](backend/deployment.md) |
| `Authenticate`, `devAuth` | Identify the caller | [Authentication](backend/api.md#authentication) |
| `MutationRejected`, `translateRejection`, `onError`, `EngineError` | Reject business operations and diagnose failures | [Errors](backend/api.md#errors) |
| `backend.transaction`, `TransactionCall` | Write outside a handler with the same `changes` and `publish`; subscribers wake after commit | [Background writes](backend/api.md#background-writes) |
| `pg`, `prisma`, `drizzle`, `PostgresDriver`, `persistence` | Run business and sync storage in one PostgreSQL transaction through your own access tool | [Database](backend/database.md) |

## Advanced interfaces

| Interface | Use it to | Reference |
| --- | --- | --- |
| React Native `databasePath` | Resolve a persistent local database path | [React Native setup](frontend/platforms.md#react-native) |
| `Client`, `Transaction`, `QuerySpec`, `RecordValue` | Access the generic runtime beneath generated APIs | [Client runtime](frontend/runtime.md) |
| `ServerOptions`, `SyncServer`, `ConnectionOptions` | Configure the backend connection and refresh credentials | [Server connection](frontend/runtime.md#server-connection) |
| `RuntimeConnection`, `AuthenticationExpired` | Control Dart sync and identify authentication failures | [Connections](frontend/runtime.md#connection-controls) |
| `syncState`, `models.<name>.syncState`, `dismissRejection`, `drop` | Inspect a record's pending work and handle rejected or unsent mutations | [Recovery APIs](frontend/runtime.md#pending-work-and-recovery) |
| `pendingTasks`, `runPrerequisites`, `setReadiness` | Complete prerequisite I/O before a mutation can be sent | [Prerequisites](frontend/runtime.md#prerequisites) |
| `freeze`, `acknowledge`, `applyPull` | Exercise the engine protocol in tests and tooling | [Protocol primitives](frontend/runtime.md#protocol-primitives) |
| `ReadPort`, `WritePort`, `LivePort`, model factories and codecs | Bind generated facades to a compatible runtime | [Generated extension points](frontend/client-api.md#extension-points) |
| `loaderHooks`, `Native` | Prepare a loader call or supply the native backend binding | [Backend extension points](backend/api.md#extension-points) |
| Compiler command and `.model` declarations | Generate and evolve the interface contract | [Schema compiler](schema/reference.md) |

The generated application API is the normal entry point. Raw backend protocol methods marked `@internal` in the implementation are not a supported application integration surface; use `listen`, handlers, loaders and transaction-bound notifications.

## Generated Action contracts (execution pending #142)

| Contract | Intended use | Reference |
| --- | --- | --- |
| `ActionClientContract.models`, `ActionTransactionContract.models` | Local Model reads and CRUD; only the standalone client also has watch | [Action contract](frontend/client-api.md#action-contract-execution-pending-142) |
| `client.actions.<name>` / `ActionCall<Output>` | Queue an Action locally, then inspect `status` or await its final outcome | [Action contract](frontend/client-api.md#action-contract-execution-pending-142) |
| `client.actions.call.<name>` | Request a direct final result | [Action contract](frontend/client-api.md#action-contract-execution-pending-142) |
| `Handlers<Ctx>`, `Loaders<Ctx>`, `ActionBackendContract<Ctx>` | Type retained Action handlers and Model Loader versions; callable factory arrives with #142 | [Action schema](schema/reference.md#action-contracts-execution-pending-142) |

The existing `GeneratedClient` and `client.mutate` entries above describe the runnable legacy mutation API. The Action entries describe generated interfaces only; #142 supplies both execution routes and the shared Loader result path. Ephemeral outputs remain #116 work.
