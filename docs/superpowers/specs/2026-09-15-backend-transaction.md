# `backend.transaction`: framework-managed external writes that wake subscribers

Status: implementation specification. Tracking: [#50](https://github.com/zanminwang/axton/issues/50). Cross-process wake delivery stays with [#62](https://github.com/zanminwang/axton/issues/62); persistence boundaries with [#103](https://github.com/zanminwang/axton/issues/103).

## 1. Problem

Writes outside a handler reach clients only through a publication. Today the SDK offers two entry points in `packages/server/index.mts`:

- `backend.notify(tx, { channel, records })`: publishes inside the caller's transaction through a throwaway `Session`, so the set of touched channels is discarded and no live subscriber is woken. Connected clients see the change only after they reconnect and catch up. The e2e fixture server, the React Native integration server, the To-do seed and several tests use it.
- `backend.bindTransaction(tx)`: returns `{ notify, assertCommittable, afterCommit, close }`; the caller must run the completion check, capture the wake callback and invoke it after its own commit. Correct, but four calls the application can get wrong.

## 2. Decision (approved in #50, confirmed 2026-09-15)

Add `backend.transaction`, remove `backend.notify(tx, …)`, keep `bindTransaction` as the advanced path.

```ts
const result = await backend.transaction(async ({ tx, notify }) => {
  await tx.entry.update({ where: { id: "entry-1" }, data: { text: "From a job" } });
  await notify({ channel: "book:demo", records: [Entry({ id: "entry-1" })] });
  return "done";
});
```

### `backend.transaction(body)`

- Signature: `transaction<R>(body: (call: TransactionCall<Tx>) => Promise<R>): Promise<R>` with `export interface TransactionCall<Tx> { tx: Tx; notify(args: NotifyArgs): Promise<void>; }`. Single object argument, like `HandlerCall` and `LoaderCall`.
- Runs `body` inside `options.database.transaction`, the same adapter push and pull use. `tx` is that adapter's transaction. No second database, no business logic in Rust.
- `notify` is the bound-session notify: every record named gets a new stamp and the channel an invalidation, inside `tx`. It must be awaited; a notify still pending when `body` resolves fails the transaction with `unawaited transaction operations`, exactly as `assertCommittable` does today.
- After `body` resolves, the framework runs the completion check, the adapter commits, and the in-process wake hub wakes every live subscriber of the touched channels. The caller never invokes an after-commit callback.
- If `body` throws, or the completion check fails, the adapter rolls back, nobody is woken, and the original error propagates so the adapter's serialization retry keeps working. Each retry attempt gets a fresh session, so a rolled-back attempt's touched set never leaks into the next.
- `body`'s return value is returned.
- Not for use inside a handler: a handler already has a transaction and uses `publish`. Calling `backend.transaction` from a handler opens a second, independent transaction; the docs say not to.

### `backend.notify(tx, …)` is removed

It cannot wake correctly without the completion and after-commit steps, so it is a trap. Every caller in the repository moves to `backend.transaction`. The internal `publish` helper no longer falls back to a throwaway session; publishing on a transaction that is neither run by the framework nor bound rejects with `transaction not bound`.

### `bindTransaction(tx)` stays, as the advanced path

Unchanged contract (`notify`, `assertCommittable`, `afterCommit`, `close`). Documented as the path for an application whose framework already owns the transaction and cannot let AXTON open it. The user left this open in #50 and chose to keep it on 2026-09-15.

## 3. Implementation shape

`createBackend` already has `run(operation)`: open the adapter transaction, bind a session, run the operation, `assertCommittable`, snapshot the touched channels, close the session, and after the adapter resolves, `wakes.notify(committed)`. `backend.transaction` is `run` with `operation = (tx) => body({ tx, notify: notifyIn(tx) })`, where `notifyIn(tx)` is the same closure `bindTransaction` returns as `notify`, wrapped to `Promise<void>`. The generated backend (`createBackend` in `crates/compiler/src/emit.rs`) spreads the runtime backend, so the method appears on generated backends without an emitter change.

## 4. Callers to migrate

| File | Today | After |
| --- | --- | --- |
| `integration/e2e/fixtures/round-trip/server.mts` `initialize` | `db.$transaction(async tx => { upsert; await backend.notify(tx, …) })` | `backend.transaction(async ({ tx, notify }) => { upsert; await notify(…) })` |
| `integration/platform/react-native/server.mts` seed | same shape | same change |
| `examples/todo/seed.mts` + `examples/todo/server.mts` | `seed(tx, backend)` called from `db.$transaction` | `seed(backend)` opens `backend.transaction` itself |
| `integration/e2e/parity.test.mjs` `reseed` | `app.db.$transaction(… app.backend.notify …)` | `app.backend.transaction(…)` |
| `integration/e2e/round-trip.test.mjs` 55-row seed | same | same |
| `integration/persistence/server/runtime.test.mjs` | every `backend.notify(tx, …)` call | `backend.transaction(…)`; tests that exercise `bindTransaction` on purpose stay as they are |

## 5. Documentation

- `docs/engineering/architecture/server/engine/notify.md`: §3 table rows for the two external entry points, §6 runtime view for `backend.transaction`, new §9 decision, §11 problem paragraph removed.
- `website/docs/backend/api.md` "Background writes", `website/docs/backend/setup.md` "Background jobs", `website/docs/concepts.md`, `website/docs/api-index.md`: `backend.transaction` is the documented way; `bindTransaction` is listed under advanced use with its existing table.
- Testing docs: `docs/engineering/testing/components/server.md` (Notify row), `docs/engineering/testing/integration/persistence.md` (new rows), `docs/engineering/testing/end-to-end.md` (the fixture no longer relies on catch-up), `docs/engineering/testing/review.md` (item 1).

## 6. Done when (from #50)

- [x] A publication made through `backend.transaction` reaches an already connected live subscriber without reconnect (PostgreSQL test with a real WebSocket).
- [x] A body that throws rolls back business writes and publication and wakes nobody; an un-awaited `notify` cannot be committed unnoticed.
- [x] `backend.notify(tx, …)` no longer exists; `bindTransaction` keeps its tested contract.
- [x] Every caller in §4 migrated; e2e and persistence suites pass; the To-do and React Native servers typecheck.
- [x] Docs in §5 updated.
