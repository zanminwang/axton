# Query and Mutation contracts with delivery defaults

Status: design/specification prepared from the user's approved direction; no implementation claimed. Issue: [#157](https://github.com/zanminwang/axton/issues/157). Companion: [implementation plan](../plans/2026-09-25-157-query-mutation-contracts.md).

## 1. Decision and scope

The application schema distinguishes backend business intent: a Query reads without business side effects; a Mutation may change business state or perform external effects. Delivery is a separate axis. Mutations default to durable local acceptance; Queries default to direct online completion. Explicit overrides remain available on both types. Model outputs from both default to store:true.

Build on merged #141/#142, per-call store (#116), and the #150 runtime. Follow [architecture](../../engineering/architecture.md), [guarantees](../../engineering/guarantees.md) and [testing strategy](../../engineering/testing/strategy.md). Preserve existing Loader snapshots, authority application, optimistic settlement and saved call outcomes. This is a schema/generated API/capability change, not a replacement sync engine.

Out of scope: automatic pagination or list membership, new load declarations, Query caching/refetch scheduling, backfill jobs, Bootstrap changes, tool execution, database sandboxing, generic cancellation or result retention redesign. A Query may express ordinary business pagination using its declared values; the framework does not infer pagination from field names.

## 2. Schema contract

```text
model Todo {
  id String
  title String
  @@id(id)
}

mutation AddTodo(todo Todo.create) {
}

mutation SendEmail(recipient String, message String) {
  messageId String
}

query SearchTodos(text String, cursor String?) {
  todos Todo[]
  nextCursor String?
}
```

Use the existing parenthesized Action grammar and named output shapes for both keywords. Mutation supports existing ordinary inputs and Model create/update/delete operands, bindings, prerequisites and sequence policy. Inferred create/update results remain input-bound Models; Delete remains an identity confirmation. Queries accept existing ordinary scalar/enum/list inputs and explicit ordinary/Model outputs; they reject all mutation operands and @sequence. Do not add input Model identity syntax, arbitrary objects or new output shapes in this issue.

Query and Mutation declaration names share one operation namespace, including generated lower-camel-name collision checks. A name may not be declared twice across kinds. Reserve namespace members call on Mutations and enqueue on Queries after codegen normalization, and reserve the new public type names.

@version applies to both. Queries cannot acquire changes/publication capabilities through another delivery route. Schema validation repeats these constraints for hand-written descriptors, not just parsed source.

The maintained application surface moves from action Name(...) to mutation/query Name(...). Update current source fixtures/examples and generated callers together; no client.actions alias or parallel public Action vocabulary is required. The parser already has an older mutation Name { slots } branch: the new parenthesized declaration must not accidentally take that legacy path. Retain the legacy block form only for existing low-level fixture/descriptor coverage during this change, without expanding its public API or presenting it in new guides. Removing that older kernel representation is not a prerequisite. Source action declarations can remain accepted only while migrating internal test fixtures in the implementation branch; final maintained application examples and public schema docs use the new kinds.

## 3. Client API and completion

| Public call | Delivery | Return type | Await boundary |
| --- | --- | --- | --- |
| client.mutations.name(args, options?) | Durable | Promise<Call<Output>> | Local acceptance, queue and declared optimism committed |
| client.mutations.call.name(args, options?) | Direct | Promise<Output> | Backend outcome received and authority applied |
| client.queries.name(args, options?) | Direct | Promise<Output> | Backend outcome received and authority applied |
| client.queries.enqueue.name(args, options?) | Durable | Promise<Call<Output>> | Local acceptance and queue committed; no inferred optimism |

```ts
const call = await client.mutations.addTodo({ todo });
const outcome = await call.wait();

const result = await client.queries.searchTodos({ text: "design", cursor: null });
const confirmed = await client.mutations.call.addTodo({ todo });
const queued = await client.queries.enqueue.searchTodos({ text: "design", cursor: null });
```

Call<T> retains exactly status and wait(); wait yields the existing result/error outcome, never a separate result property or polling API. Public neutral names are Call, CallStatus, CallOutcome, CallError and CallOptions. Dart uses Call, CallStatus, CallOutcome, CallSuccess, CallFailure and CallError with the existing outcome semantics. These are shared across both durable kinds. Keep result lifetime, weak routing and repeated-wait behavior; rename public types without rewriting registry internals solely for terminology.

Generated Dart uses the same namespaces and route names with existing named input arguments and typed store selectors. Direct methods return Future<Output>, durable methods Future<Call<Output>>. A nullable business input remains a required argument with a nullable value, as today.

Delivery is selected by the method, not an options flag that changes the return type. There is no automatic offline fallback from a direct call to enqueueing. New direct invocations each obtain a fresh call ID. Queued Queries read when executed, not at enqueue time, and receive stable saved outcomes on delivery retry. Ordinary values are captured at enqueue; no speculative Query result is derived from local data.

Mutations are not promised instant final execution. Inferred Model optimism applies only to durable Mutations with operands; a durable plain-value Mutation may produce no local Model change. Direct Mutations derive no local optimism but retain authoritative mutation readback. Both types and all routes remain forbidden inside an application-owned local transaction. Standalone and transaction client.models remain local-only.

## 4. Store and result semantics

Preserve #116's option exactly on all four paths: omitted/true stores all eligible explicit Model outputs; false disables output-only authority; a typed per-output boolean map overrides named explicit Model outputs. Existing generated per-operation Options and Dart Store selectors remain strongly typed, including the outputStore collision fallback for a business input named store.

Returned Models always come from invocation Loader snapshots at the retained result read version. Store controls additional authority, not result shape, backend business effects, queue persistence or saved outcome persistence. Mutation-required and handler-extra-change authority cannot be suppressed; select a positive union when identities overlap. Preserve Loader refusal/null/missing/list semantics and stamp-based application of stale records.

A Query returning a Model may initialize framework stamp evidence and update the local database. Those framework writes are allowed. A Query with store:false does not create/update/delete local Model rows or stamps solely from its outputs. No Scope subscription is required and no Scope cursor is moved by either kind.

## 5. Backend contract and enforceable boundaries

Expose generated Mutations<Tx> and Queries<Tx> registration maps alongside existing Loaders<Tx>:

```ts
createBackend({
  database,
  authenticate,
  mutations: {
    addTodo: async ({ ctx, args }) => {
      // Application business write using ctx.tx; existing changes/publish available.
    },
  },
  queries: {
    searchTodos: async ({ ctx, args }) => {
      // Application read using ctx.tx and trusted ctx.userId.
      return { todos: [{ id: "todo-1" }], nextCursor: null };
    },
  },
  loaders,
});
```

MutationContext<Tx> carries tx, userId, callId, changes and publish. QueryContext<Tx> carries tx, userId and callId only. Type Query handler outputs from the same identity/value rules as Mutation explicit outputs. Runtime Query contexts must also omit changes/publish, not only hide them in TypeScript. Use CallRejected as the neutral application rejection class; keep structured rejection behavior.

Registration is kind- and retained-version-aware. Reject missing, extra or wrong-kind registrations at startup. Function shorthand still means the only retained version is v1; otherwise use vN registrations. An empty kind map may be omitted when no retained contracts of that kind exist. A history containing Mutation Foo v1 and Query Foo v2 registers each version only under its matching kind; current clients expose Foo only under its current kind.

The Rust shared executor rejects a Query host settlement containing any nonempty changes or publications with query.effects_forbidden before stamping, readback or publication. Treat this as a call-level semantic rejection, roll back its savepoint and preserve other calls in a durable batch. Perform the check for both direct and queued Query execution and for non-TypeScript hosts. Invalid Query descriptors with Model operands must fail before invoking application code.

This is not a security sandbox around application code. The framework cannot inspect arbitrary SQL or prevent a handler from using an independently captured database/network client. Tx remains the application's generic type; documenting it as globally read-only would be false. Developers must honor Query's business read-only contract. Do not use a read-only SQL transaction around the entire execution: stamp evidence and saved-call bookkeeping legitimately write framework metadata. Read-only database adapters or effect isolation would be separate work.

## 6. Shared descriptors, execution and storage

Add a typed CallKind enum (Mutation, Query) and a required kind in newly emitted operation descriptors. Keep the existing internal actions collection, ActionDescriptor/ActionIntent structures, actions history file and kernel mutation queue terminology where retaining them avoids unrelated rewrites. These are implementation storage/protocol details, not a second generated application namespace. Missing kind in older descriptors/history normalizes to Mutation; reject unknown kinds. Validate even descriptors loaded directly from persisted schema.

Kind belongs to the retained (name, version) contract. Reclassifying a published contract requires a new version; changing delivery route or store does not. Normalize omitted legacy kind and explicit mutation as equal during history comparison. Keep existing input/output/read-contract checks. Server selects kind from its retained descriptor, not a client-supplied capability flag, and kind must not change in place under a saved call identity. If kind changes at a new version, generate historical context/registration types by each retained version's kind, not the latest kind.

Reuse the two existing delivery paths:

- Durable default Mutation and explicit queued Query use the existing queue, Uplink worker, frozen request, saved call outcome and push receipt. A Query has zero derived local operations; this must remain a valid queued intent.
- Direct default Query and explicit direct Mutation use the existing immediate request path and /sync/actions transport. No new HTTP endpoint, running worker or native ABI entry is necessary solely to rename the public contract.
- Both use the shared resolver in crates/server/src/action_results.rs and the shared Rust authority applier. Preserve current business-transaction/savepoint ownership and commit-before-success behavior.

Ordinary Queries bypass the client persistent queue, but this milestone deliberately retains existing backend claim/save behavior for both kinds. A fresh invocation reads again; retrying the same call ID replays its saved result. Removing durable server receipts for direct Queries is not required for this split and would change execution/storage guarantees independently. Query traffic therefore still incurs existing framework metadata storage; retention remains #61. Do not claim a cheaper or stateless Query server path.

No new client queue column is required just to identify kind: name/version and retained schema already determine it. Preserve frozen bytes, store persistence and queue reopen. No automatic dropping/reclassification of pending intents. New source code and generated artifacts can change together because the project has no released-user migration requirement; this does not permit weakening retained-version correctness or resetting queues in tests to avoid it.

## 7. Parallel implementation with Bootstrap

As checked on 2026-09-25, #150 is merged at 544e3d0; #151 has an active implementation claim, a separate worktree and implementation commits, but no open PR at the check. #116 is merged. This worktree is based on main 544e3d0, not on the unmerged Bootstrap branch.

This feature has no semantic dependency on #151. Primary owned areas are compiler kind/codegen, operation schema validation, Query handler contexts and generated client dispatch. Do not edit subscription/Bootstrap state machines, request modes, S/B/L/H semantics, page limits, checkpoints, recovery or notification scheduling.

Observed overlap candidates with #151: bindings/common/src/lib.rs, crates/core/src/protocol.rs and tests, crates/server/src/lib.rs, crates/client/src/lib.rs, shared SDK exports, integration/persistence/server/runtime.test.mjs and generated connection fixtures. Avoid general file moves and broad Action-to-Mutation search/replace in shared files. In particular legacy queue Mutation and new semantic Mutation are not interchangeable types.

Either feature may merge first. The second integrates current main, resolves shared-file changes while preserving both contracts and reruns relevant tests plus the full host gate. Do not block or modify Bootstrap's active task merely to prepare this feature. No agent has been dispatched for #157 by this preparation task.

## 8. Acceptance and review

- Both keywords generate correct names, output/identity types, kind descriptors and version history. Invalid Query operands/sequence and generated-name collisions have source-located diagnostics.
- All four routes have fixed return types in TS and Dart. Local-only models/transaction APIs remain unchanged. Direct offline calls never silently enqueue.
- Default Mutation accepts offline and retains declared optimism; default Query does not write queue metadata; queued Query survives reopen without optimism; direct Mutation waits for final outcome.
- Query contexts omit business effect capabilities in types and runtime; forged Query settlements fail before any framework business-change handling, isolated from adjacent valid batch calls.
- Both types default store:true. All four routes exercise store:false and selective maps, including Mutation-required overlaps, snapshot-versus-local-view and read-version correctness.
- Call ID replay, policy conflicts, retained-kind dispatch, errors and weak handle lifetime remain correct. Changing kind at the same version fails; route/store changes require no version bump.
- Current examples/docs adopt the new application vocabulary. Historical plans remain historical. Query is documented as a trusted application contract, not a SQL sandbox.
- After integration with #151, subscription initialization, Bootstrap recovery/barriers and shared authority application still pass their actual tests; no claimed completion from source inspection alone.
