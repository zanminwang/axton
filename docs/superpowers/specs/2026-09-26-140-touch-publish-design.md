# Generated touch and explicit publication APIs

> **SUPERSEDED — do not implement this revision.** On 2026-09-26 the user approved persistent record-to-Channel membership: `publish` enrolls a record and publishes current state; later inferred or explicitly touched changes automatically distribute to all member Channels. Each changed record gets one new stamp, with separate publication cursors in its Channels, atomically with the business transaction. Client subscription cursors remain client-owned. The [updated #140](https://github.com/zanminwang/axton/issues/140) is authoritative. This document's exclusions of membership and automatic distribution, its unchanged-engine assumptions, and its implementation readiness no longer apply. Rewrite and review the spec/plan to cover membership persistence, removal, concurrency, deletion/retention, repeated publication and Bootstrap compatibility before execution. The text below preserves the previous proposal for reference.

Status: agreed API direction for [#140](https://github.com/zanminwang/axton/issues/140), with engineering details specified here for review. This is a design document, not shipped behavior. Baseline inspected: main `9cfb0b8`.

## 1. Goal

Keep three backend responsibilities distinct without requiring nested reference helpers for routine Model operations:

- `touch` declares additional records changed by the handler.
- `publish` explicitly selects records and their destination Channel.
- The handler's return value supplies its schema-declared outputs.

Mutation Model operands already identify their changed targets. They remain automatic; the developer only touches additional records. No server content comparison, return-value change inference, schema annotation, `changed(...)` return wrapper, or automatic Channel membership is introduced.

Preserve the [guarantees](../../engineering/guarantees.md), [backend interface](../../engineering/architecture/server/backend-interface.md), and [server readback/stamping responsibilities](../../engineering/architecture/server/engine/README.md). The implementation must update the public API documentation to distinguish new developer spelling from unchanged engine semantics.

## 2. Public interface

Mutation handlers keep `{ ctx, args }`. Their context keeps `tx`, `userId` and `callId`; replace `changes` with generated `touch` methods. `publish` is callable for mixed records and also carries generated per-Model methods.

```ts
async function addTodo({ ctx, args }) {
  // Application-owned database writes happen through ctx.tx.
  // args.todo is already a declared Todo.create operand.

  ctx.touch.project({ id: args.todo.projectId });

  ctx.publish({
    channel: `project:${args.todo.projectId}`,
    records: [args.todo],
  });
  ctx.publish.project({
    channel: `project:${args.todo.projectId}`,
    identity: { id: args.todo.projectId },
  });

  return { project: { id: args.todo.projectId } };
}
```

The example assumes the schema declares that explicit Project output and the handler actually updated Project before calling touch. Touch does not write business fields or `updatedAt`.

Generated type shape for an illustrative schema:

```ts
export interface Touch {
  todo(identity: TodoIdentity): void;
  project(identity: ProjectIdentity): void;
  projectMember(identity: ProjectMemberIdentity): void;
}

export interface Publish {
  (args: { channel: string; records: readonly PublishRecord[] }): void;
  todo(args: { channel: string; identity: TodoIdentity }): void;
  project(args: { channel: string; identity: ProjectIdentity }): void;
  projectMember(args: {
    channel: string;
    identity: ProjectMemberIdentity;
  }): void;
}

export interface MutationContext<Tx> {
  tx: Tx;
  userId: string;
  callId: string;
  touch: Touch;
  publish: Publish;
}
```

`PublishRecord` is the runtime's accepted reference input: a tagged generated Model operand or an explicit `{ model, identity }` reference. Generated backend files should describe explicit references with a discriminated Model/identity union. Preserve the existing tagged operand representation and its compile-time assignability; raw structural objects must still be validated at runtime. An arbitrary untagged `{id}` does not identify a Model and is refused by mixed publication.

Routine generated methods require no `ref` helper. For a heterogeneous set created independently of inputs, retain the existing generated Model-named reference constructors, such as `Todo({ id })` and `Project({ id })`; do not add a new `ref` namespace or pretend that a plain identity alone identifies its Model.

```ts
const records = [Todo({ id: todoId }), Project({ id: projectId })];
ctx.publish({ channel: "shared", records });
```

Touch methods return void; do not make touching a requirement to construct a publication reference. A record may be published without having changed. Arrays of existing tagged operands can be passed directly as records. The mixed publication API takes a readonly array in v1, not an arbitrary Iterable/Set; a JavaScript Set can be spread by the application. Multiple records of a single Model use the same mixed form or repeated per-Model calls; no extra batch overload is needed.

Use the same generated Model accessor spelling as existing APIs (e.g. `projectMember`). Reject duplicate generated keys clearly rather than silently overwriting methods. Install callable publication properties safely, including schema names that map to function/prototype keys; `Object.assign` onto a function or a normal-object dictionary is not sufficient. Do not reserve otherwise valid Model names merely to avoid testing the property installation.

## 3. Touch semantics

`ctx.touch.project(identity)` reports a write that business code performed. It adds the encoded `(Model, identity)` to the settlement's changed-record set. It performs no immediate database operation, output registration, Channel publication or network I/O.

The existing Rust executor remains authoritative for the final set: inferred Model operand targets plus explicit touches, deduplicated by canonical identity. Optional absent operands add nothing; list operands add their members. A repeated touch or a touch of an inferred target allocates only one stamp for that record within that successful Mutation. Separate successful Mutations are separate write declarations, even within one batch.

A touch asserts modification; the framework does not compare old/new business content. Declaring a write whose final content happens to be identical can still advance the record stamp. Returned-only and published-only records keep their existing stamp (or initialize one under the existing ensureStamp rule). An authoritative Loader null supplies deletion evidence; a Loader refusal/error is not deletion. Store options do not suppress required touched/input authority.

A touch is independent of declared outputs: an extra changed record can reconcile through receipt/direct authority without becoming a named result field. Returning an identity without touching it does not declare a write. Touching a record does not modify the handler's result shape.

Mutation contexts expose generated touch and publish at runtime and in types. Query contexts expose neither, and retain the existing Rust `query.effects_forbidden` enforcement for forged host settlements.

## 4. Explicit publication and snapshot boundary

Both entry points register the same internal intent:

```ts
ctx.publish({ channel: "shared", records: [args.todo] });
ctx.publish.todo({ channel: "shared", identity: { id: todoId } });
```

`records` is required. Missing, undefined or null records fails validation; an explicit empty array publishes nothing. The generated single-Model form requires one identity. There is no implicit "all changed records", live view of the collector, `touch.records`, or public replacement for `changes.records`.

At each call, normalize and take an owned snapshot of the Channel and each Model identity. Snapshot array membership AND identity values, including mutable Date instances and nested objects; a shallow array copy is not sufficient. Validate/encode temporal and composite identities through the established identity codec. Later mutations to an input, identity, reference or source array cannot retarget a prior publication or touch. Later touches never expand an earlier publication.

This snapshot fixes which records are published, not a copy of their business content. Loader readback and current stamps are determined during normal settlement after the handler returns. Thus publishing a reference before later touching the same record still publishes at that transaction's final allocated stamp. Publishing never changes the record stamp merely for distribution; Channel cursors advance under the existing rules. Preserve existing ordering and repeated-publication behavior; this is not a new cross-call publication deduplication feature.

A publish call does not establish persistent membership or automatic future publication. It does not imply record eviction, a client subscription, read permission, or a backend write. No publications occur on an aborted enclosing transaction. After-commit wakes and multi-process limitations stay unchanged.

## 5. External transactions and supported backend surfaces

The generated backend's external transaction callback receives the same typed touch and publish capabilities:

```ts
await backend.transaction(async ({ tx, touch, publish }) => {
  // Perform the application-owned database write using tx.
  touch.project({ id: projectId });
  publish.project({ channel: "shared", identity: { id: projectId } });
  return applicationResult;
});
```

There are no inferred operands outside a Mutation. Its return value stays application-owned; it is not interpreted as a Mutation output or change list. Keep rollback, stamp allocation and after-commit notifications in the existing external settlement path.

Update the still-supported legacy slot-handler context to the same touch/publish names as well, preserving its existing inferred slot targets. Do not retain a second public `changes.add`/`changes.records` interface there. Loader interfaces are unchanged.

The runnable backend is TypeScript. Update its runtime, generated backend contexts and external transaction inference, tests, demos and documentation. Dart-generated abstract handler contracts retain their existing generic context parameter; do not invent a Dart server or touch the client Rust executor for this issue. Run shared generation checks to catch incidental impacts.

## 6. Implementation ownership and compatibility

Keep the public spelling change in the server SDK and compiler. Retain the private settlement key `changes` used between the host and Rust; it remains a set of reported record identities and is not the retired public collector.

For explicitness to hold across supported hosts, make Rust `PublicationIntent.records` a required vector rather than an optional final-change-set shortcut, and remove the implicit branch from readback. This intentionally tightens the internal host contract, not the client/backend HTTP schema. Update native host fixtures and all handwritten producers together. A malformed host publication must fail at the existing handler/external-transaction boundary, with existing operation-level isolation; it must not partially publish or become a rejection of an unrelated batch item.

Generate strongly typed context aliases in backend.ts rather than re-exporting an untyped runtime MutationContext as the final application type. Raw runtime contexts may derive their method dictionaries from the schema descriptor; generated context types supply compile-time identity types. Ensure retained Mutation versions, current identity codecs, temporal identities and startup registration all agree. The executor, not a new SDK algorithm, still infers target writes from retained descriptors.

No backend operation version bump, schema grammar/history migration, persistent membership table or deprecation facade is needed solely for these developer API changes. Regenerate and update all in-repository callers atomically. A server SDK/native pair must be rebuilt together for the tightened host contract; do not advertise mixed old/new host bindings as supported.

Registration capabilities are valid only while their handler/external callback is active. Close the collector when the callback settles and reject later touch/publish calls rather than allowing unawaited work to mutate a finalized settlement. This is a local lifecycle guard, not a promise to cancel arbitrary application side effects.

## 7. Decisions, exclusions and evidence

This supersedes #140's `changes.push` proposal and its automatic Channel membership/enter-leave design. Membership, retention and automatic distribution are excluded; #139 remains deferred. The purpose of the issue is now generated touch and explicit publication ergonomics, not reimplementing existing stamping/readback.

Current evidence inspected: `packages/server/index.mts` collector and contexts; `crates/compiler/src/emit.rs::backend_typescript`; `crates/server/src/host.rs::PublicationIntent`; `crates/server/src/readback.rs`; `crates/server/src/actions.rs`; generated action-runtime and PostgreSQL fixtures. Current runtime supports `changes.add`, optional publish records and Model-named reference constructors. The new API is not implemented by this document.

## 8. Acceptance and verification

- Generated TS contexts infer all Model identity types, including temporal/composite identities. Missing records, invalid identities, unknown Model helpers and Query effects fail type checks; untyped calls fail runtime validation.
- Touching an inferred input, touching twice and key-order variants still advance one stamp. Extra touches appear in authority even without explicit output/publication. Ordinary output and publication-only records do not advance existing stamps.
- Mixed-record and per-Model publications produce equivalent explicit intents; empty arrays publish nothing; missing records is refused for SDK and handcrafted Rust-host settlements.
- Publication/touch snapshots survive later source-array, identity-object and Date mutation. A later touch does not expand an earlier publish. A later touch of an explicitly published identity still supplies its final stamp.
- Handler and external transaction rollback leaves business writes, stamps and publications unchanged. One invalid handler intent does not reject unrelated calls. Durable retries replay saved outcomes without re-running declarations.
- Direct and durable Mutation paths, generated external transactions and legacy slot handlers use the same API; Query contexts remain effect-free.
- Generated fixtures, examples and active docs contain no implicit publish or public changes collector. Historical task documents may retain old terminology. Internal settlement JSON remains named changes.

Follow the [testing strategy](../../engineering/testing/strategy.md) and [running guide](../../engineering/testing/running.md). Preparation is source inspection and documentation review only; no implementation tests have run.
