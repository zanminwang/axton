# Backend interfaces

Your backend implements the write path through handlers and the read/sync path through loaders. The compiler generates their TypeScript interfaces from your schema. AXTON supplies protocol processing; your application supplies business logic, authorization and a database transaction.

Action signatures below follow the [generated Action fixture](https://github.com/zanminwang/axton/blob/main/integration/action-contract/schema.model). The background-write example uses an independent `Entry` Model fixture. The working To-do backend is [examples/todo/server.mts](https://github.com/zanminwang/axton/blob/main/examples/todo/server.mts).

## createBackend

```ts
import { PrismaClient, type Prisma } from '@prisma/client';
import { createBackend, devAuth } from './generated/backend.ts';
import { prisma } from '../../packages/postgres/index.mts';
import { handlers } from './handlers.ts';
import { loaders } from './loaders.ts';

const db = new PrismaClient();
const backend = createBackend<Prisma.TransactionClient>({
  database: prisma(db),
  authenticate: devAuth(),
  handlers,
  loaders,
  onError: error => console.error(error),
});
const server = await backend.listen({ port: 4242 });
console.log(server.url);
```

The imports assume the same directory depth as the repository example. `handlers.ts` and `loaders.ts` contain the implementations below. Generate the Prisma application client and apply AXTON's metadata migration first; see [Database](database.md).

The generated `Options<Tx>` requires:

| Option | Responsibility |
| --- | --- |
| `database: Database<Tx>` | A PostgreSQL shim, `pg(pool)`, `prisma(client)` or `drizzle(db)`, or `persistence(driver)` over your own driver ([Database](database.md)) |
| `authenticate: Authenticate` | Resolve the caller's user identity or reject the request |
| `handlers: Handlers<Tx>` | Implement each retained Action version |
| `loaders: Loaders<Tx>` | Implement the read function for each supported model version |

Optional options are `translateRejection`, `onError`, `loaderHooks` and `native`, described below. The generated function binds the schema and returns the backend synchronously. The generic function in `packages/server/index.mts` additionally requires `config`; normal generated integrations do not pass it.

## What your backend owns

The Rust runtime processes the sync protocol and nothing else. The rules below are yours to implement; the runtime neither enforces nor checks them, and the schema does not make it do so.

| Rule | Who owns it | What the runtime does |
| --- | --- | --- |
| Authorization | Handlers decide what `userId` may write; loaders decide what `userId` may see and return `null` for the rest, whatever channel asked. | Authenticates the request and passes `userId` through. There is no channel-level policy. |
| Unique constraints and identities | Your database schema. `@@unique` and `@@id` are enforced on the client only; the client's local database refuses a violating write, but nothing checks the server. | Decodes identities and patches by shape. A duplicate that your database allows is stored. |
| Child deletion | Your handler. `onTargetDelete: delete` is a client-side cascade: the client deletes the children locally, and those deletes never reach the server. A handler that deletes a parent must delete its children itself, report them with `changes.add` and publish them to each channel that delivered them. | Reads the parent back as deleted and delivers it; a child the handler did not report stays on other clients until a channel delivers it. |
| Client identity | Each signed-in user gets their own local client database. A client id is bound to the first user that pushed with it; a push from another user with the same client id answers `403 client.owner_mismatch`, and there is no reassignment. | Stores the owner with the client row. |
| Backend language | TypeScript on Node, through the generated `createBackend`. The Dart package is a client SDK; there is no Dart or Rust-hosted backend. | Runs the same Rust engine inside the Node addon. |
| Prerequisite expressions | `@requires(Name(field: self))` is the only supported form: every argument is `self`, the value of the annotated field. The runner that satisfies prerequisites is client code. | Never sees prerequisites; they gate when the client sends a durable Action, not what the backend receives. |

These are accepted limits of the current runtime, not planned features. See [deployment](deployment.md) for the process and network boundaries.

## Handlers

A handler receives `{ ctx, args }`: `ctx` holds the trusted transaction, authenticated user, call ID, changes and publisher; `args` holds decoded caller inputs. In the snippets, `Tx` stands for the transaction type supplied by your database adapter. The handler writes to your database and returns explicit output values. AXTON resolves Model outputs through the corresponding versioned Loader in the same transaction. For a durable Action, it also reads final changed records into the receipt, independently of each invocation's result snapshot. Database work and framework metadata share the transaction; external effects such as sending email do not become atomic with it. Use an application outbox or equivalent design where that distinction matters.

```ts title="action-contract"
import { ActionRejected, type Handlers } from './generated/backend.ts';

// saveTodo is application code that writes to the business database.
const handleAddTodoV2: Handlers<Tx>['addTodo']['v2'] =
  async ({ ctx, args }) => {
    if (!args.todo.title.trim()) throw new ActionRejected('todo.title_empty');
    await saveTodo(ctx.tx, args.todo);
    ctx.changes.add(args.todo);
    ctx.publish({ channel: 'todos' });
    return { relatedTodo: null, matches: [], count: 1, state: null };
  };
```

The implicit `todo` result resolves through the `Todo` Loader at this invocation. Explicit `relatedTodo` and `matches` are identity-selected Model outputs; `count` and `state` are ordinary outputs. The client that queued this Action receives its result and batch-final record authority without a subscription. Other clients learn of the change through a published channel.

```ts title="action-contract"
import { type Handlers } from './generated/backend.ts';

const handleGetTodos: Handlers<Tx>['getTodos'] =
  async ({ ctx }) => {
    const ids = await visibleTodoIds(ctx.tx, ctx.userId); // application function
    return { todos: ids.map(id => ({ id })) };
  };
```

Returning identity objects lets the Loader resolve the visible records in order, including repeated identities. Authorization remains the application's responsibility.

`ActionContext<Tx>` contains:

| Field | Meaning |
| --- | --- |
| `tx` | Your database transaction object |
| `userId` | Authenticated caller; use it for business authorization |
| `callId` | Stable identity of this Action invocation, including retries |
| `changes` | The records this Action changed; `changes.add(record)` reports an additional record |
| `publish` | Synchronous function for publishing records to a channel; see [Publishing](#publishing) |

A handler returns the generated explicit output shape, or no value when the Action has no explicit outputs. On durable delivery, AXTON allocates a **stamp** for each changed record and reads its batch-final content through the Loader into the receipt. Report every business record changed beyond the inferred Model operands with `changes.add`. Reporting is not publishing; other clients receive records only through channels the handler publishes.

A loader is channel-independent: the row it returns for a record is the row every client receives for it, in the receipt, in a catch-up page and on the live stream, at the same stamp. What a loader may vary by is `userId`.

One Action can have several operands and perform several business writes in one savepoint. The schema's Model operands describe local optimism; the backend can normalize values or use different tables.

`handlers.addTodo` holds every retained version of `AddTodo`. The fixture retains v1 and v2, so register both:

```text
handlers.addTodo = {
  v1: handleOriginalAddTodo, // receives AddTodoV1Input
  v2: handleAddTodoV2,       // receives AddTodoInput
};
```

A bare function means v1 only. A missing retained version, unknown version key or non-function value is refused at startup. Dispatch uses the requested Action version and never falls back to another. An unsupported version is a per-call rejection.

An error that is neither `ActionRejected` nor translated to a business code rejects that Action with `handler.failed` and reaches `onError`. Independent valid calls in the batch can still commit. A retryable database transaction error instead retries the transaction; it is not saved as a permanent business rejection.

## Loaders

```ts title="action-contract"
import type { Loaders } from './generated/backend.ts';

const loadTodoV2: Loaders<Tx>['todo']['v2'] =
  async ({ ids, tx, userId }) =>
    Promise.all(ids.map(id => loadVisibleTodo(tx, userId, id)));
```

`LoaderCall<Tx, Identity>` contains:

| Field | Meaning |
| --- | --- |
| `ids` | Read-only list of typed record identities |
| `tx` | Your transaction, shared with sync persistence for this request |
| `userId` | Caller whose visibility must be checked |

A Loader is not told which channel, if any, asked: it serves Action Model outputs, durable authority readback, catch-up pages and the live stream. It sees the same application transaction during Action execution.

A loader returns `Promise<readonly (Record | null)[]>`. Return exactly one item per identity, in the same order. Do not filter out missing rows or return a differently ordered database result directly.

`loaders.todo` holds every retained version of the `Todo` read contract. The Action fixture retains v1 and v2, so register both while older clients or Action results use v1:

```text
loaders.todo = {
  v1: loadTodoV1, // returns TodoV1 rows
  v2: loadTodoV2, // returns Todo rows
};
```

The generated `TodoV1` type is the record shape published for v1, so a v1 Loader maps current rows into it; AXTON does not convert between versions. Registration is checked at startup like handlers: a bare function means v1 only, and missing/unknown versions or non-function values are refused. A load reaches only the version it names.

What each item may be:

| Item | Meaning | Result |
| --- | --- | --- |
| A row object | The record's current state for this user | Delivered with the record's current stamp |
| `null` | The record does not exist, or this user must not see it | Delivered as a deletion. A newer stamp clears the authoritative row, whichever channel delivered it; the client keeps the stamp so older content cannot bring the record back; pending local operations are replayed on that state. |
| a thrown `ActionRejected` (or an error `translateRejection` maps to a code) | A refused read | During Action execution, that call is rejected with the code and rolled back. In a pull, that record is delivered as an error with that code: the client keeps its local copy and reports it, and the rest of the page applies |
| any other thrown error | A failure | Reported to `onError` (default `console.error`). During Action execution, the call is rejected with `loader.failed`; in a pull, that record is delivered as a `loader.failed` error and the rest of the page applies |
| `undefined`, a missing entry, a non-array result, a nonfinite number | A defect | Reported to `onError` and treated like a thrown error: only the records it affects fail. It is never read as `null` |

A row object must match the generated model type exactly. Include every non-identity field: a nullable field that is absent reads as `null`, but an absent non-nullable field is a defect. The identity fields may be present. Any other property, such as an extra database column or a relation object, is a defect. Map your rows to the model type rather than returning a wider database row.

Loaders run during synchronization and Action execution, not when the app calls local `get`, `query` or `watch`. A malformed result is never skipped silently: the affected record arrives as an error change, or the Action being read back is rejected with `loader.invalid`, and `onError` hears about it.

## Publishing

`ctx.publish({ channel })` distributes the Action's final change set, including records added with `ctx.changes.add` after the call. `ctx.publish({ channel, records })` distributes exactly `records`: a subset, or records the Action did not change (an empty array publishes nothing). Publishing does not broadcast the supplied object's field values; subscribers receive what the Loader returns.

```ts title="action-contract"
import { Todo, type ActionContext } from './generated/backend.ts';

function announce(ctx: ActionContext<unknown>) {
  ctx.publish({ channel: 'todos' });
  ctx.publish({ channel: 'archive', records: [Todo({ id: 'todo-1' })] });
}
```

| Interface | Shape |
| --- | --- |
| `RecordRef` | `{ model: string, identity: object }` |
| `PublishArgs` | `{ channel: string, records?: readonly (RecordRef | object)[] }` |
| `Changes` | `{ records: readonly RecordRef[], add(record: RecordRef | object): void }` |
| Generated model reference function | `Todo(identity: TodoIdentity): RecordRef` |
| Handler `publish` | `(args: PublishArgs) => void` |

The channel must be nonblank. Decoded Model operands such as `args.todo` carry record-reference metadata and can be passed to `ctx.changes.add` and `ctx.publish` directly. Spreading or cloning an operand can lose this metadata; use the generated Model reference function when constructing a reference yourself.

Publish to every channel that distributes a changed record, including when its Loader should now return null. AXTON does not infer publications from writes to your database. Several calls are allowed; none is required, and a handler that publishes nothing can still complete its Action result and durable receipt.

A change allocates one **stamp** per record; publishing allocates a **cursor** in each channel and carries that same stamp to all of them. Publishing an unchanged record reuses its current stamp (a record that has never been stamped gets its first one). Stamps prevent older content delivered later, on any channel, from overwriting newer content. See [concepts](../concepts.md).

## Authentication

`Authenticate` receives Node's `IncomingMessage` and returns a user ID string, null, undefined, or a promise of those values. Null/undefined or a blank user ID rejects authentication. The SDK calls it for HTTP requests and WebSocket connections. Verify your application's session/token here; enforce read permissions in loaders and write permissions in handlers.

`devAuth(): Authenticate` treats `Authorization: Bearer <userId>` as the identity without verification. It is provided for local development, not production authentication. See [authentication and account changes](../frontend/sync.md#authentication-and-account-changes).

## Errors

| Interface | Use |
| --- | --- |
| `new ActionRejected(code)` | Reject one Action with a stable machine-readable code |
| `translateRejection(error)` | Return a stable rejection code for a known application error; return null/undefined for other errors |
| `onError(error)` | Log server failures that are returned to the client as a generic server error |
| `EngineError` | A failure from the native engine: `code` (stable), `message` (readable, may change), `details` (fields the code promises) |

Codes must match `^[a-z][a-z0-9]*(?:[._-][a-z0-9]+)*$`, such as `todo.title_empty`. A recognized business rejection rolls back that Action's business writes, stamps and publications. For a queued call, `wait()` returns an `ActionError` outcome and the optimistic Model change rolls back; the durable rejection remains inspectable until dismissed. For a direct call, the promise rejects with `ActionError`. An unknown transport outcome can be retried with the same call identity; it is not evidence that the handler did nothing.

Business codes come from `ActionRejected` or `translateRejection`; `action_version_unsupported`, `handler.failed`, `loader.failed` and `model_version_unsupported` identify framework failures attributable to one call. A durable receipt records each call's outcome. Independent valid calls in the batch can commit. A direct response carries the same final outcome for that call.

Infrastructure errors that make the transaction unusable abort delivery for retry. `onError` receives diagnostic failures, including handler and Loader exceptions. Diagnostic callback exceptions after a committed outcome cannot replace its result, repeat its handler or become a transport error; SDKs report those callback exceptions through their runtime uncaught-error channel.

Protocol refusals use a status and JSON body chosen by the engine error's `code`; message text may change. Per-Action content errors are outcomes, while a pull or live subscription with an unsupported Model declaration receives a whole-request `409`.

| Code | HTTP status | Meaning |
| --- | --- | --- |
| `request.invalid` | 400 | Malformed body, or a pull cursor ahead of the channel head |
| `client.owner_mismatch` | 403 | The client identity belongs to another user |
| `gap`, `overlap` | 409 | The batch sequence is not the next one and not a retry of the last |
| `model_version_unsupported` | 409 | Pull and live subscribe: a Model read contract this backend does not serve. During Action execution it is a per-call failure. |
| `handler.invalid` | 500 `{ code: "server" }` | The handler's settlement could not be used: an invalid rejection code, or a change or publication naming a record without a model or an object identity |
| anything else | 500 `{ code: "server" }` | A server-side failure; the `EngineError` or thrown error goes to `onError` |

## Listener

`await backend.listen({ port, host? })` binds a Node HTTP and WebSocket server. Host defaults to `127.0.0.1`; port zero selects an available port. The result is `{ url, close(): Promise<void> }`.

| Route | Purpose |
| --- | --- |
| `POST /sync/mutations` | Receive durable Action batches |
| `POST /sync/actions` | Execute one direct Action and return its result |
| `POST /sync/pull` | Materialize changed records through loaders for catch-up and gap recovery |
| `/sync/live` (WebSocket) | Subscribe to channels and stream ongoing record changes |

The listener has no TLS, CORS or proxy-header handling and binds to loopback by default; run it behind a reverse proxy as described in [Deploy the backend](deployment.md).

Generated clients use all three routes automatically from one `server` configuration. The WebSocket subscription acknowledgement confirms that channel listeners are installed before HTTP catch-up starts, so changes during catch-up can be queued and reconciled. Listener errors reject. `await server.close()` releases the listener and its live connections; your application must separately close its database pool. The supported listener owns its server; mounting into an application-owned HTTP server is not currently exposed.

## Background writes

Writes outside handlers have no readback and no receipt; they reach clients only through channels. Run them through `backend.transaction`: the framework opens the application transaction and hands the body the same `changes` and `publish` a handler receives. When the body returns, the framework allocates one new stamp per record in `changes` and carries out the publications inside that same transaction; once it commits, the live subscribers of the published channels are woken.

```ts
await backend.transaction(async ({ tx, changes, publish }) => {
  await tx.entry.update({ where: { id: 'entry-1' }, data: { text: 'From a job' } });
  changes.add(Entry({ id: 'entry-1' }));
  publish({ channel: 'book:demo' });
});
```

`tx` is the transaction of the shim passed as `database`, and `Entry` is the generated reference function. The body's return value is returned. If the body throws, the transaction rolls back and nobody is woken; the error propagates so the driver can retry serialization failures, which run the whole body again. Do not call `backend.transaction` from a handler: a handler already has a transaction.

| `TransactionCall<Tx>` member | Contract |
| --- | --- |
| `tx` | The application transaction; write business data through it |
| `changes` | The records this body changed; `changes.add(record)` registers one, and each gets a new stamp when the body returns |
| `publish(args)` | `{ channel }` publishes the final change set; `{ channel, records }` exactly those records, a record outside `changes` at its current stamp |

Same objects and rules as a handler's, with two differences: the change set starts empty, because nothing was uploaded, and nothing is read back, because no client is waiting for a receipt. A body that registers changes without publishing still advances their stamps; a body that publishes an unchanged record does not. Wakeups are process-local; distributed wake delivery needs additional application infrastructure.

## Extension points

`loaderHooks` maps model names to `{ prepareForViewer(call): Promise<void> }`. The hook runs before that model's loader in the same request context. Its failure fails the load. Use it only if viewer-specific preparation is needed; a loader already receives the user.

`native?: Native` injects the native bridge when packaging it elsewhere. It implements `validateConfig`, `processPush`, `processPull`, `settleExternal`, `negotiateLive` and `pullLive` with the string/JSON callback contracts in the [SDK source](https://github.com/zanminwang/axton/blob/main/packages/server/index.mts). The default binding comes from this repository's Node addon. This is a packaging seam; the generated handlers and loaders remain the application contract.

Backend methods marked `@internal` (`push`, `pull`, `negotiateLive`, `pullLive`, `onCommitted`, `notifyCommitted`, `closeLive`) are used by the listener and tests. They are not the supported application-facing HTTP integration surface.
