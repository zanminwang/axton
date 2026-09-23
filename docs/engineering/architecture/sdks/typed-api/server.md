# Server

## 1. Introduction and Goals

A backend author writes handlers (one per mutation) and loaders (one per model) against their own database transaction; the server typed API gives them typed inputs, `changes` and `publish` to report and distribute what a handler changed, and an HTTP and WebSocket server, while the Rust engine decides what runs, reads the results back and tells the client.

## 3. Context and Scope

`createBackend({database, authenticate, handlers, loaders, loaderHooks?, translateRejection?, onError?})` returns `{listen, transaction, …}`. Generated `backend.ts` supplies `Handlers<Tx>` and `Loaders<Tx>` so a missing or misnamed handler is a type error, and wraps `createBackend` with the compiled config ([Compiler / Generate](../../compiler/generate.md)). The behavior behind each option is owned by the [backend interface](../../server/backend-interface.md); this page is about the shape an application sees.

| Piece | Shape |
| --- | --- |
| Handler | `({input, tx, userId, changes, publish}: HandlerCall) => Promise<void>`; `input` is one typed value per slot; the return value is ignored |
| Change set | `changes.records` (the records the uploaded operations target) and `changes.add(record)` for a record the handler changed beyond them; every member is stamped, read back and returned in the receipt |
| Loader | `({ids, tx, userId}: LoaderCall) => Promise<(Row \| null)[]>`, aligned with `ids`; one per retained model version, `Row` being that version's record type; no channel |
| Rejecting one mutation, or refusing a read | throw `MutationRejected(code)`, or throw anything `translateRejection` maps to a code; from a loader in a push this rejects the mutation, in a pull it fails the page |
| Publishing | `publish({channel})` or `publish({channel, records})` (`PublishArgs`) inside a handler; outside one, `backend.transaction(async ({tx, changes, publish}) => …)` hands the body the same `changes` and `publish` and settles them when it returns ([Publish](../../server/engine/publish.md)) |
| Serving | `backend.listen({port, host?})` → `{url, close}` |
| Development auth | `devAuth()` treats the bearer token as the user id; documented as development only |

## 5. Building Block View

The runtime package holds `createBackend`, the HTTP and WebSocket servers and the SQL-free `Database<T>` contract; `@ahead/postgres` builds that object from a two-method driver and ships the `pg`, `prisma` and `drizzle` shims ([Persistence](../../server/persistence.md)). Slot arguments handed to a handler are tagged with a hidden record reference, which is why `changes.add(input.entry)` and `publish({channel, records: [input.entry]})` work without spelling out model and identity.

Code: [server/index.mts](../../../../../packages/server/index.mts); generated signatures from `backend_typescript` in [compiler/emit.rs](../../../../../crates/compiler/src/emit.rs).

## 9. Architecture Decisions

**Handler and loader registration by version ([#91](https://github.com/zanminwang/ahead/issues/91)).** Group versions under the mutation or model name. For an initial v1-only contract, a function is shorthand for `{v1: implementation}`. Once multiple versions are supported, register each explicitly:

```ts
handlers: {
  edit: {
    v1: handleOriginalEdit,
    v2: handleNewEdit,
  },
},
loaders: {
  task: {
    v1: loadOriginalTask,
    v2: loadNewTask,
  },
}
```

Handlers receive generated input types for their mutation version; loaders return generated record types for their independent [model version](../../schema/models.md#9-architecture-decisions). Registration keys use `v1`, `v2`; wire versions remain numbers. Shorthand always means v1, never the latest version. Client calls use `client.mutate.edit(...)`, with their generated version fixed in the request.

Handler registration implements this decision. Generated `Handlers<Tx>` holds one key per mutation, `lowerFirst(name)`, whose value carries a `v<n>` member for every retained version; a mutation retaining only v1 also accepts the bare function. The runtime refuses at startup: a bare function whenever the retained versions are not exactly v1, a missing version, an unknown `v<n>` key and a non-function value, each naming the mutation and version. Dispatch stays keyed by name and version, so a request never falls back to another version.

Loader registration implements the same decision. Generated `Loaders<Tx>` holds one key per model, `lowerFirst(name)`, with a `v<n>` member per retained [model version](../../schema/models.md#9-architecture-decisions), each returning that version's record type: the schema's own version keeps the plain name (`Entry`), an older retained contract is its own type (`EntryV1`) with the enum values of its time inline. A model retaining only v1 also accepts the bare function. The runtime applies the handler rules to loaders at startup, naming the model and version. Dispatch is by model name and contract version: the engine names the version in every `load`, and the runtime never routes one version's request to another's loader. Which version a pull is served at is the one the client declared ([Server / Engine / Pull](../../server/engine/pull.md#6-runtime-view)).

## 10. Quality Requirements

- **Startup fails on an invalid config or a missing handler or loader.** Evidence: [runtime.test.mjs](../../../../../integration/persistence/server/runtime.test.mjs) `backend validates config and complete registrations at startup`.
- **Registration names every retained version; a bare function registers v1 only.** Evidence: `handler registration names every retained version and a function means v1 only`.
- **A version reaches only its own handler, whichever way v1 was registered.** Evidence: `a version dispatches only to its own handler and a function registers v1`.
- **Slot arguments can be passed to `changes.add` and `publish` directly.** Evidence: `slot arguments are tagged so changes.add and publish accept them directly`.
- **A handler returns nothing; the framework reads the change set back and publishes what was asked, with or without a `publish` call; loaders receive no channel.** Evidence: `a handler that publishes nothing still returns readback records and touches no channel`, `publish({channel}) publishes the final change set, an addition made after the call included`, `loaders receive no channel` ([Backend interface §10](../../server/backend-interface.md#10-quality-requirements)).
- **Generated handlers group the retained versions of a mutation.** Evidence: [compiler/tests/compiler.rs](../../../../../crates/compiler/tests/compiler.rs) `backend_emitter_groups_handler_versions_under_the_mutation_name`, `backend_emitter_accepts_a_bare_function_only_for_a_v1_only_mutation`.
- **Loader registration names every retained model version, a bare function registers v1 only, and a pull reaches only the served version's loader.** Evidence: [runtime.test.mjs](../../../../../integration/persistence/server/runtime.test.mjs) `loader registration names every retained model version and a function means v1 only`, `a pull reaches the loader of the declared model version and normalizes rows with that contract`; [compiler/tests/compiler.rs](../../../../../crates/compiler/tests/compiler.rs) `backend_emitter_groups_loader_versions_under_the_model_name`; the `@ts-expect-error` loader negatives (bare function, missing and unknown versions, a value outside the v1 contract) in [test.ts](../../../../../integration/generated-api/test.ts).

Executed 2026-09-15 with the `changes`/`publish` handler API: `cargo test -p ahead-compiler --locked`, `bash integration/generated-api/verify.sh`, and `bash integration/persistence/server/run.sh` (61 passed).

## 11. Risks and Technical Debt

**Accepted limitation.** The backend runtime exists for TypeScript only; the compiler emits no server signatures for other languages.
