# TypeScript backend SDK

This TypeScript SDK embeds the shared Rust server runtime in your Node application. Business Handlers and Loaders are implemented in TypeScript, against the `Handlers`/`Loaders` interfaces the compiler generates from your `.model` file.

`index.mts` runs on Node with TypeScript support (Node 22.18+), or can be compiled with TypeScript. Build the local native module with `node bindings/node/build.mjs`. Supply an injected `native` implementation when packaging the native artifact elsewhere.

Given `models/book.model` describing an `Entry` Model and an `Edit` Mutation, the compiler emits `generated/backend.ts`, which already binds the schema:

```ts
// handlers.ts
import type { Handlers } from './generated/backend.ts';
import { MutationRejected } from './generated/backend.ts';

export const handlers: Handlers<Tx> = {
  async edit({ input, tx, userId, publish }) {
    const { identity, patch } = input.entry;
    if (!await canEdit(tx, userId, identity)) throw new MutationRejected('entry.forbidden');
    await tx.entry.update({ where: identity, data: patch });
    publish({ channel: 'book:demo' });
  },
};

// loaders.ts
import type { Loaders } from './generated/backend.ts';

export const loaders: Loaders<Tx> = {
  async entry({ ids, tx, userId }) {
    return Promise.all(ids.map(identity => loadVisibleEntry(tx, userId, identity)));
  },
};

// main.ts
import { createBackend, devAuth } from './generated/backend.ts';
import { prisma } from '../../packages/postgres/index.mts';
import { handlers } from './handlers.ts';
import { loaders } from './loaders.ts';

const backend = createBackend<Tx>({ database: prisma(db), authenticate: devAuth(), handlers, loaders });
const server = await backend.listen({ port: 4242 });
console.log(server.url);
```

The generated `createBackend` needs no `config` option: the schema is already bound. The runtime's own `createBackend` (`packages/server/index.mts`) still takes `config` explicitly, for callers that build the schema themselves.

`db` is your Prisma client and `Tx` is `Prisma.TransactionClient`. The application supplies `canEdit` and `loadVisibleEntry` to enforce its write and read permissions. Authorization, unique constraints, child deletion and client identity are the application's responsibility; the runtime does not enforce them ([What your backend owns](api.md#what-your-backend-owns)).

## Call objects

A Handler receives a `HandlerCall<Tx, Input>`: `{ input, tx, userId, changes, publish }`. A Loader receives a `LoaderCall<Tx, Identity>`: `{ ids, tx, userId }`. `input` and `ids` come from `EditInput`-style generated types; `tx` is the application's own transaction object; `userId` is the authenticated owner, so a loader can decide what this user sees and return null for rows they must not see. A loader is never told a channel: the row it returns is the row every delivery path carries for that record.

## changes and publish

`changes` is the set of records this mutation changed. It starts as the records the uploaded operations name; `changes.add(record)` (a slot argument or a `{ model, identity }` ref, such as those returned by the generated `Entry({ id })` constructor) reports a record the Handler wrote beyond them. When the Handler returns, the framework allocates a new **stamp** for every record in the set, reads them all back through the Loaders, and returns their content in the receipt, so the client that sent the mutation completes it without waiting for any channel.

`publish({ channel })` distributes that final change set on `channel`, a non-empty string; `publish({ channel, records })` distributes exactly `records` instead, and `[]` distributes nothing. Publishing is optional and may be called several times; it allocates channel cursors and carries the records' stamps, never a new stamp. A record published without being changed keeps its current stamp.

A stamp is a per-record counter that every delivery path carries with the record's content: the receipt, catch-up pages and the live stream. The client applies content strictly by stamp, so a receipt and a page for the same change agree, and older content arriving later on any channel cannot overwrite newer content.

Publish to every channel that provides a record whenever that record changes, including when a loader starts returning `null` for it. A channel that is not published to keeps delivering its old position, and its subscribers will not pick up the change through it. The framework does not detect a missing publication.

## Authentication

`authenticate` is `(request) => userId | null | undefined`, called per HTTP/WebSocket request; returning `null` or `undefined` rejects the request. `devAuth()` is a development-only implementation that trusts the `Authorization: Bearer <userId>` header verbatim — never use it in production.

## Errors

`onError?: (error) => void` on `BackendOptions` is called for server-side failures that clients only see as `{ code: "server" }` over HTTP: `authenticate` throws, persistence faults, publication errors, loader refusals while serving a page, and live drain failures. A failure raised by the native engine arrives as an `EngineError` with a stable `code` and a readable `message`; branch on the code, never on the message. See [Errors](api.md#errors) for the codes that map to HTTP statuses.

## Background jobs

Outside a Handler there is no readback and no receipt, so a change must be published to reach clients. Use `backend.transaction`; its body gets the same `changes` and `publish` as a Handler, the framework stamps and publishes what it collected inside the same transaction as your writes, and wakes live subscribers after commit:

```ts
await backend.transaction(async ({ tx, changes, publish }) => {
  await tx.entry.update({ where: { id: 'entry-1' }, data: { text: 'From a job' } });
  changes.add(Entry({ id: 'entry-1' }));
  publish({ channel: 'book:demo' });
});
```

See [background writes](api.md#background-writes).

## Transaction ownership

The outer transaction belongs to the application. Persistence, Handler, and Loader callbacks all receive that same transaction. The runner must provide a coherent snapshot (Repeatable Read or stronger), roll back on rejected promises, and retry serialization conflicts. Every shim of [`@axton/postgres`](database.md) supplies this contract.

## Mutation results

A successful Handler returns nothing; its return value is ignored. The receipt carries the content and stamp of every record in `changes`, read back by the Loaders in the Handler's transaction, once per record with the last successful mutation's result. An explicit `MutationRejected` or registered `translateRejection` code, thrown by the Handler or by a Loader during the readback, rolls back that mutation's savepoint, including business effects, stamps and publications; any other exception rejects that same mutation with `handler.failed`/`loader.failed`, reported to `onError`, and the rest of the batch still commits ([#95](https://github.com/zanminwang/axton/issues/95)). Translation must produce a stable machine code. A mutation naming a known but unsupported version is rejected the same isolated way (`mutation_version_unsupported`), without calling any handler. Only the request envelope, client identity/order and infrastructure failures (a failed `rollback`, a persistence fault) still abort the whole batch. The `handlers` key for a mutation is its lowerFirst name (e.g. `editTask`), and its value registers every retained version under `v1`, `v2`, .... A mutation that retains only v1 also accepts the plain function shown above; see [handlers](api.md#handlers).

## Loaders and Pull

Loaders return one state object or null for every identity, in precisely the supplied order. A missing or unauthorized row is null. One pull covers every channel the client follows and delivers a record published to several of them once. Each channel scans at most 50 compacted invalidations, and the pull materializes their current state with each record's current stamp.

A record that cannot be read fails alone ([#95](https://github.com/zanminwang/axton/issues/95)). When a Loader throws or refuses a batch, AXTON retries each identity on its own. The record that still fails is delivered as an error change carrying `loader.failed` or the refusal code, and the rest of the page is served. The failure is reported to `onError`, which defaults to `console.error`. The client keeps its copy of that record and reports it. The record is corrected the next time you publish it. A Loader that returns the wrong number of entries is retried the same way, and a row that does not match the model type fails only its record with `loader.invalid`.

Run `integration/persistence/server/run.sh` for the disposable PostgreSQL/Prisma integration suite. Its database is created, used, and destroyed by the runner.

`backend.listen({ port, host? })` starts a Node HTTP+WebSocket server that serves `/sync/mutations`, `/sync/pull`, and `/sync/live` on one port, and returns `{ url, close() }`. It resolves once the listener is bound.

For every option, callback, return value and failure mode, see the [backend interface reference](api.md). For process placement, the reverse-proxy configuration and trust boundaries, see [Deploy the backend](deployment.md).
