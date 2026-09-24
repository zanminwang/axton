# Server

## 1. Introduction and Goals

A backend author writes retained Action handlers and versioned Model Loaders against their own database transaction. The server typed API gives handlers `{ctx, args}` with trusted transaction context and decoded caller input, plus typed explicit outputs. The Rust engine decides execution, resolves Model outputs through Loaders, and distributes changed records.

## 3. Context and Scope

`createBackend({database, authenticate, handlers, loaders, loaderHooks?, translateRejection?, onError?})` returns `{listen, transaction, …}`. Generated `backend.ts` supplies `Handlers<Tx>`, `Loaders<Tx>`, `ActionContext<Tx>` and `ActionRejected`; missing or misnamed handlers are type errors. It binds the compiled config to `createBackend` ([Compiler / Generate](../../compiler/generate.md)).

| Piece | Shape |
| --- | --- |
| Action handler | `({ctx, args}: ActionHandlerCall<Tx, Args>) => Promise<Outputs>`; `ctx` has `tx`, `userId`, `callId`, `changes`, `publish` |
| Change set | `changes.records` (the records the uploaded operations target) and `changes.add(record)` for a record the handler changed beyond them; every member is stamped, read back and returned in the receipt |
| Loader | `({ids, tx, userId}: LoaderCall) => Promise<(Row \| null)[]>`, aligned with `ids`; one per retained model version, `Row` being that version's record type; no channel |
| Rejecting one Action or refusing a read | throw `ActionRejected(code)`, or throw anything `translateRejection` maps to a code; during Action execution this rejects the call, in a pull it reports the affected record |
| Publishing | `publish({channel})` or `publish({channel, records})` (`PublishArgs`) inside a handler; outside one, `backend.transaction(async ({tx, changes, publish}) => …)` hands the body the same `changes` and `publish` and settles them when it returns ([Publish](../../server/engine/publish.md)) |
| Serving | `backend.listen({port, host?})` → `{url, close}` |
| Development auth | `devAuth()` treats the bearer token as the user id; documented as development only |

## 5. Building Block View

The runtime package holds `createBackend`, the HTTP and WebSocket servers and the SQL-free `Database<T>` contract; `@axton/postgres` builds that object from a two-method driver and ships the `pg`, `prisma` and `drizzle` shims ([Persistence](../../server/persistence.md)). Decoded Model operands carry a hidden record reference, so `ctx.changes.add(args.todo)` and `ctx.publish({channel, records: [args.todo]})` work without spelling out model and identity.

Code: [server/index.mts](../../../../../packages/server/index.mts); generated signatures from `backend_typescript` in [compiler/emit.rs](../../../../../crates/compiler/src/emit.rs).

## 9. Architecture Decisions

**Handler and Loader registration by version ([#91](https://github.com/zanminwang/axton/issues/91)).** Group versions under the Action or Model name. For a v1-only contract, a function is shorthand for `{v1: implementation}`. Once multiple versions are retained, register each explicitly:

```ts
handlers: {
  addTodo: {
    v1: handleOriginalAddTodo,
    v2: handleNewAddTodo,
  },
},
loaders: {
  todo: {
    v1: loadOriginalTodo,
    v2: loadNewTodo,
  },
}
```

Handlers receive generated Action input types for their version and return typed explicit outputs. Loaders return record types for an independent [Model read version](../../schema/models.md#9-architecture-decisions). Registration keys use `v1`, `v2`; wire versions remain numbers. Shorthand always means v1, never the latest version. Client calls use the Action version embedded in their generated method.

Generated `Handlers<Tx>` holds one key per Action, `lowerFirst(name)`, with a `v<n>` member for every retained version; v1-only Actions also accept a bare function. The runtime refuses missing, unknown or non-function registrations at startup. Dispatch is keyed by name and version, so a request never falls back to another version.

Generated `Loaders<Tx>` holds one key per Model and a `v<n>` member per retained read version, each returning that version's record type. The current shape uses `Todo`; an older retained shape uses `TodoV1`. A Model retaining only v1 also accepts a bare function. The engine names the Loader version for each Action output separately from the client's current authority version; a pull uses the client's declared version ([Server / Engine / Pull](../../server/engine/pull.md#6-runtime-view)).

## 10. Quality Requirements

- **Startup fails on an invalid config or a missing handler or loader.** Evidence: [runtime.test.mjs](../../../../../integration/persistence/server/runtime.test.mjs) `backend validates config and complete registrations at startup`.
- **Registration names every retained version; a bare function registers v1 only.** Evidence: `handler registration names every retained version and a function means v1 only`.
- **A version reaches only its own handler, whichever way v1 was registered.** Evidence: `a version dispatches only to its own handler and a function registers v1`.
- **Decoded Model operands can be passed to `changes.add` and `publish` directly.** Evidence: [generated backend test](../../../../../integration/action-runtime-ts/backend.test.mts).
- **Action handlers return explicit outputs and Model identity objects while the framework resolves Model snapshots through a Loader.** Evidence: [Action execution tests](../../../../../crates/server/tests/actions.rs) and [generated backend test](../../../../../integration/action-runtime-ts/backend.test.mts).
- **Generated handlers group retained Action versions and share one `Tx` with Loaders.** Evidence: [Action contract fixture](../../../../../integration/action-contract/backend.ts) and [generated backend test](../../../../../integration/action-runtime-ts/backend.test.mts).
- **Loader registration names every retained model version, a bare function registers v1 only, and a pull reaches only the served version's loader.** Evidence: [runtime.test.mjs](../../../../../integration/persistence/server/runtime.test.mjs) `loader registration names every retained model version and a function means v1 only`, `a pull reaches the loader of the declared model version and normalizes rows with that contract`; [compiler/tests/compiler.rs](../../../../../crates/compiler/tests/compiler.rs) `backend_emitter_groups_loader_versions_under_the_model_name`; the `@ts-expect-error` loader negatives (bare function, missing and unknown versions, a value outside the v1 contract) in [test.ts](../../../../../integration/generated-api/test.ts).

The earlier legacy mutation evidence in the runtime tests remains useful for unchanged lower-level savepoint and Loader rules. Task 7 integration verifies the Action API separately.

## 11. Risks and Technical Debt

**Accepted limitation.** The backend runtime exists for TypeScript only; the compiler emits no server signatures for other languages.
