# Backend interfaces

Your backend implements the write path through handlers and the read/sync path through loaders. The compiler generates their TypeScript interfaces from your schema. AXTON supplies protocol processing; your application supplies business logic, authorization and a database transaction.

Examples use the `Entry` / `Edit` schema of the [round-trip fixture](https://github.com/zanminwang/axton/blob/main/integration/e2e/fixtures/round-trip/models/entry.model), which keeps a nullable field and an update patch that the [To-do example](../getting-started.md) does not need. The complete working implementation is [server.mts](https://github.com/zanminwang/axton/blob/main/integration/e2e/fixtures/round-trip/server.mts); the To-do backend is [examples/todo/server.mts](https://github.com/zanminwang/axton/blob/main/examples/todo/server.mts).

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
| `handlers: Handlers<Tx>` | Implement each supported mutation version |
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
| Prerequisite expressions | `@requires(Name(field: self))` is the only supported form: every argument is `self`, the value of the annotated field. The runner that satisfies prerequisites is client code. | Never sees prerequisites; they gate when the client sends a mutation, not what the backend receives. |

These are accepted limits of the current runtime, not planned features. See [deployment](deployment.md) for the process and network boundaries.

## Handlers

A handler writes to your database. AXTON then reads every record the mutation changed back through your loader, in the same transaction, and returns that content to the client in the receipt. The simplest handler performs the write and nothing else:

```ts
// handlers.ts
import type { Prisma } from '@prisma/client';
import { MutationRejected, type Handlers } from './generated/backend.ts';

export const handlers: Handlers<Prisma.TransactionClient> = {
  async edit({ input, tx, userId }) {
    // In your application, check userId's write permission here.
    const { identity, patch } = input.entry;
    if (patch.text === 'reject') throw new MutationRejected('entry.denied');
    await tx.entry.update({
      where: identity,
      data: {
        ...patch,
        ...(typeof patch.text === 'string' ? { text: patch.text.trim() } : {}),
      },
    });
  },
};
```

The client that sent the mutation receives the trimmed text from the receipt, with or without a subscription. Other clients learn of the change only if the handler publishes it to a channel they subscribe to:

```ts
// handlers.ts
import type { Prisma } from '@prisma/client';
import { Entry, MutationRejected, type Handlers } from './generated/backend.ts';

export const handlers: Handlers<Prisma.TransactionClient> = {
  async edit({ input, tx, userId, changes, publish }) {
    const { identity, patch } = input.entry;
    if (patch.text === 'reject') throw new MutationRejected('entry.denied');
    await tx.entry.update({ where: identity, data: patch });
    // A write to a record the uploaded operations did not name: report it.
    await tx.entry.update({ where: { id: 'entry-2' }, data: { text: 'also touched' } });
    changes.add(Entry({ id: 'entry-2' }));
    // Distribute this mutation's changes to the channel's subscribers.
    publish({ channel: 'book:demo' });
  },
};
```

These reproduce the demo's normalization and rejection behavior. They are not an application permission policy; the demo trusts its development user.

`HandlerCall<Tx, Input>` contains:

| Field | Meaning |
| --- | --- |
| `input` | Generated mutation input, such as `EditInput`; update slots have `identity` and `patch` |
| `tx` | Your database transaction object |
| `userId` | Authenticated caller; use it for business authorization |
| `changes` | The records this mutation changed: `changes.records` starts as the records the uploaded operations target; `changes.add(record)` reports one more |
| `publish` | Synchronous function for publishing changed records to a channel; see [Publishing](#publishing) |

A handler returns `Promise<void>`; its return value is ignored. When it returns, AXTON allocates a new **stamp** for every record in `changes`, whether or not the values differ from before, reads each of them back through the loader of the model version the client declared, and puts the results in the receipt. A record the handler changed without reporting it is not stamped, not read back and not in the receipt: use `changes.add` for every write beyond the uploaded operations, a related row you update or a child you delete included. Reporting is not publishing; nothing reaches other clients until the handler calls `publish`.

A loader is channel-independent: the row it returns for a record is the row every client receives for it, in the receipt, in a catch-up page and on the live stream, at the same stamp. What a loader may vary by is `userId`.

One mutation can have several slots and perform several business writes. AXTON runs it in a savepoint inside the batch transaction. The schema describes the local operation and typed input; it does not require the backend to replay the same database operations. The backend can normalize values or use different tables.

`handlers.edit` holds every retained version of `Edit`. While only v1 is retained, the function above is shorthand for `{ v1: ... }`. Once a second version is retained, register each one explicitly and keep them all while clients can still send those versions:

```text
handlers.edit = {
  v1: handleOriginalEdit,   // receives EditV1Input
  v2: handleNewEdit,        // receives EditInput
};
```

A bare function always means v1, never the latest version, so a mutation whose retained versions are not exactly v1 refuses it at startup, as does a missing version, an unknown `v<n>` key or a value that is not a function. A request reaches only the handler of the version it names; there is no fallback. A mutation naming a known but unsupported version is rejected `mutation_version_unsupported` without calling any handler; the rest of the batch is unaffected.

Any other error a handler throws — not `MutationRejected`, not a code `translateRejection` maps — rejects that mutation with `handler.failed`, is reported to `onError`, and the rest of the batch commits ([#95](https://github.com/zanminwang/axton/issues/95)).

## Loaders

```ts
// loaders.ts
import type { Prisma } from '@prisma/client';
import type { Loaders } from './generated/backend.ts';

export const loaders: Loaders<Prisma.TransactionClient> = {
  async entry({ ids, tx, userId }) {
    // Replace this demo policy with your application's authorization rules.
    if (userId !== 'demo-user') {
      return ids.map(() => null);
    }
    return Promise.all(ids.map(identity => tx.entry.findUnique({ where: identity })));
  },
};
```

`LoaderCall<Tx, Identity>` contains:

| Field | Meaning |
| --- | --- |
| `ids` | Read-only list of typed record identities |
| `tx` | Your transaction, shared with sync persistence for this request |
| `userId` | Caller whose visibility must be checked |

A loader is not told which channel, if any, asked: it serves the receipt of the client's own mutation, catch-up pages and the live stream alike, so the same identity, model version and stamp always describe the same content.

A loader returns `Promise<readonly (Record | null)[]>`. Return exactly one item per identity, in the same order. Do not filter out missing rows or return a differently ordered database result directly.

`loaders.entry` holds every retained version of the `Entry` read contract, exactly as `handlers.edit` holds mutation versions. While only v1 is retained, the function above is shorthand for `{ v1: ... }`. Once the model has a second version, register each one and keep both while clients of the older version can still read:

```text
loaders.entry = {
  v1: loadOriginalEntry,   // returns EntryV1 rows: the fields and enum values of v1
  v2: loadNewEntry,        // returns Entry rows
};
```

The generated `EntryV1` type is the record shape published for v1, so a v1 loader maps your current rows into it; AXTON does not convert between versions. A row with a field outside the served version's contract fails only that record, with `loader.invalid`, and is reported to `onError`. Registration is checked at startup like handlers: a bare function means v1 only, and a missing version, an unknown `v<n>` key or a non-function value is refused. A load reaches only the loader of the version it names.

What each item may be:

| Item | Meaning | Result |
| --- | --- | --- |
| A row object | The record's current state for this user | Delivered with the record's current stamp |
| `null` | The record does not exist, or this user must not see it | Delivered as a deletion. A newer stamp clears the authoritative row, whichever channel delivered it; the client keeps the stamp so older content cannot bring the record back; pending local operations are replayed on that state. |
| a thrown `MutationRejected` (or an error `translateRejection` maps to a code) | A refused read | In a push, the mutation whose result is being read back is rejected with that code and rolled back. In a pull, that record is delivered as an error with that code: the client keeps its local copy and reports it, and the rest of the page applies |
| any other thrown error | A failure | Reported to `onError` (default `console.error`). In a push, the mutation whose result is being read back is rejected with `loader.failed` and the rest of the batch commits; in a pull, that record is delivered as a `loader.failed` error and the rest of the page applies ([#95](https://github.com/zanminwang/axton/issues/95)) |
| `undefined`, a missing entry, a non-array result, a nonfinite number | A defect | Reported to `onError` and treated like a thrown error: only the records it affects fail. It is never read as `null` |

A row object must match the generated model type exactly. Include every non-identity field: a nullable field that is absent reads as `null`, but an absent non-nullable field is a defect. The identity fields may be present. Any other property, such as an extra database column or a relation object, is a defect. Map your rows to the model type rather than returning a wider database row.

Loaders run during synchronization and during a push's readback, not when the app calls local `get`, `query` or `watch`. A malformed result is never skipped silently: the affected record arrives as an error change, or the mutation being read back is rejected with `loader.invalid`, and `onError` hears about it.

## Publishing

`publish({ channel })` distributes the mutation's final change set, records added with `changes.add` after the call included, to `channel`. `publish({ channel, records })` distributes exactly `records` instead: a subset, or records the mutation did not change (an empty array publishes nothing). Publishing does not broadcast the supplied object's field values; subscribers receive what the loader returns.

```ts
import type { Prisma } from '@prisma/client';
import { Entry, type Handlers } from './generated/backend.ts';

export const handlers: Handlers<Prisma.TransactionClient> = {
  async edit({ input, tx, publish }) {
    await tx.entry.update({ where: input.entry.identity, data: input.entry.patch });
    publish({ channel: 'book:demo' });
    publish({ channel: 'book:archive', records: [Entry({ id: 'entry-1' })] });
  },
};
```

| Interface | Shape |
| --- | --- |
| `RecordRef` | `{ model: string, identity: object }` |
| `PublishArgs` | `{ channel: string, records?: readonly (RecordRef | object)[] }` |
| `Changes` | `{ records: readonly RecordRef[], add(record: RecordRef | object): void }` |
| Generated model reference function | `Entry(identity: EntryIdentity): RecordRef` |
| Handler `publish` | `(args: PublishArgs) => void` |

The channel must be nonblank. Decoded handler slots such as `input.entry` carry record-reference metadata and can be passed to `changes.add` and `publish` directly. Spreading or cloning a slot can lose this metadata; use the generated model reference function when constructing a reference yourself. The low-level `RECORD` symbol marks these decoded references; applications normally do not need to manipulate it.

Publish to every channel that distributes a changed record, including when its loader should now return null. AXTON does not infer publications from writes to your database. Several calls are allowed; none is required, and a handler that publishes nothing still succeeds with its records in the receipt.

A change allocates one **stamp** per record; publishing allocates a **cursor** in each channel and carries that same stamp to all of them. Publishing an unchanged record reuses its current stamp (a record that has never been stamped gets its first one). Stamps prevent older content delivered later, on any channel, from overwriting newer content. See [concepts](../concepts.md).

## Authentication

`Authenticate` receives Node's `IncomingMessage` and returns a user ID string, null, undefined, or a promise of those values. Null/undefined or a blank user ID rejects authentication. The SDK calls it for HTTP requests and WebSocket connections. Verify your application's session/token here; enforce read permissions in loaders and write permissions in handlers.

`devAuth(): Authenticate` treats `Authorization: Bearer <userId>` as the identity without verification. It is provided for local development, not production authentication. See [authentication and account changes](../frontend/sync.md#authentication-and-account-changes).

## Errors

| Interface | Use |
| --- | --- |
| `new MutationRejected(code)` | Reject a business operation with a stable machine-readable code |
| `translateRejection(error)` | Return a stable rejection code for a known application error; return null/undefined for other errors |
| `onError(error)` | Log server failures that are returned to the client as a generic server error |
| `EngineError` | A failure from the native engine: `code` (stable), `message` (readable, may change), `details` (fields the code promises) |

Codes must match `^[a-z][a-z0-9]*(?:[._-][a-z0-9]+)*$`, such as `entry.denied`. An invalid code is itself an error. A recognized business rejection rolls back that mutation's business writes, stamps and publications and is included in the receipt. A loader that throws one while a push reads the mutation's results back rejects that mutation the same way. The client rolls back its optimistic change and retains a rejection entry. A network error is not a business rejection and must not cause a duplicate business action.

Rejection codes appear in a mutation's receipt entry, never in an HTTP status or a thrown request failure: `entry.denied`-style codes you or `translateRejection` produce; `mutation_version_unsupported` for a mutation naming an unregistered version; `model_version_unsupported` for a handler that changed a model the client did not declare or declared at an unretained version; `handler.failed` for any other error a handler threw; `loader.failed` for any other error a loader threw while a push read a mutation's results back. Each rejects only that one mutation; the rest of the batch commits ([#95](https://github.com/zanminwang/axton/issues/95)).

Unexpected exceptions that are not one of the above — a persistence fault, a failed `rollback`, or any host callback failure the engine cannot classify as a mutation outcome — abort the whole delivery transaction. Do not translate every exception into a rejection: a database outage or programming error should remain a retryable request failure. `onError` receives failures including authentication exceptions, persistence faults, publication errors, live-drain failures, and every `handler.failed`/`loader.failed` error (the underlying thrown error, not just the code).

Protocol refusals are answered with a status and a JSON body chosen by the engine error's `code`. Rewording a message never changes a status. `mutation_version_unsupported` and `model_version_unsupported` no longer abort a push at the HTTP level — see the rejection codes above; `model_version_unsupported` stays a whole-request `409` for pull and live subscribe, which still check declarations up front.

| Code | HTTP status | Meaning |
| --- | --- | --- |
| `request.invalid` | 400 | Malformed body, or a pull cursor ahead of the channel head |
| `client.owner_mismatch` | 403 | The client identity belongs to another user |
| `gap`, `overlap` | 409 | The batch sequence is not the next one and not a retry of the last |
| `model_version_unsupported` | 409 | Pull and live subscribe only: a model read contract this backend does not serve — the client declared an unknown model or an unretained version (body adds `model` and `version`), or a page holds a model the client did not declare (body adds `model`). On the WebSocket the handshake closes with `1002` and this code as the reason. Inside a push, this is a per-mutation rejection code instead (above), not an HTTP status. |
| `handler.invalid` | 500 `{ code: "server" }` | The handler's settlement could not be used: an invalid rejection code, or a change or publication naming a record without a model or an object identity |
| anything else | 500 `{ code: "server" }` | A server-side failure; the `EngineError` or thrown error goes to `onError` |

## Listener

`await backend.listen({ port, host? })` binds a Node HTTP and WebSocket server. Host defaults to `127.0.0.1`; port zero selects an available port. The result is `{ url, close(): Promise<void> }`.

| Route | Purpose |
| --- | --- |
| `POST /sync/mutations` | Receive mutation batches |
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
