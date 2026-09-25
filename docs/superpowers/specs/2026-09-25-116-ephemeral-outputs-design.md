# Per-call storage of Action Model results

Status: approved design, documentation only. Issue: [#116](https://github.com/zanminwang/axton/issues/116). Companion: [implementation plan](../plans/2026-09-25-116-ephemeral-outputs.md) and [implementation handoff](../plans/2026-09-25-116-cloud-handoff.md).

This revision supersedes the schema `@ephemeral` proposal and the proposal for the backend's deployed descriptor to select output storage policy. The public option is **`store`**, chosen per invocation. Historical filenames remain stable for existing links.

## Scope

Merged #141/#142 already provide typed Action results, handler identity objects, versioned Loader snapshots, authority application, durable/direct delivery and saved outcomes. This issue adds invocation-level control over additional Model-output authority, plus dedicated acceptance coverage. It does not add a fetch primitive or replace the Action executor.

Follow the existing [Action schema](../../engineering/architecture/schema/actions.md), [Action protocol](../../engineering/architecture/protocol/actions.md) and [guarantees](../../engineering/guarantees.md). Downlink scheduling, subscriptions and Bootstrap remain #150/#151. Retention, membership, explicit loading and tools remain separately scoped.

## Public API

The schema continues to describe inputs and result shape only:

```text
action SearchTodos(query String) {
  todos Todo[]
}
```

```ts
// Default: return Loader snapshots and apply additional Model authority.
const result = await client.actions.call.searchTodos({ query });

// Same Action and same result type; do not store its output-only records.
const suggestions = await client.actions.call.searchTodos(
  { query },
  { store: false },
);

// The durable entry point accepts the same options.
const call = await client.actions.searchTodos({ query }, { store: false });
const outcome = await call.wait();

// For an Action with mainTodo and suggestions Model outputs:
const page = await client.actions.call.openTodo(
  { id },
  { store: { suggestions: false } },
);
```

Generated TypeScript options use this shape, with K restricted to the Action's explicit Model-output names:

```ts
type ActionOptions<K extends string> = {
  store?: boolean | Partial<Record<K, boolean>>;
};
```

Omission and `true` store all eligible outputs. `false` stores none of those outputs. A map overrides named outputs; unmentioned outputs default to true. Single, nullable and list Model outputs are eligible. Scalar/value outputs, identity-only Delete confirmations and inferred input-bound outputs are not map keys. Invalid keys or non-boolean values fail validation before local optimism/enqueue or direct dispatch. Boolean options on Actions without eligible outputs are accepted and have no output-storage effect. For an Action without eligible keys, generate a boolean-only option type rather than an unrestricted empty-object map.

Dart exposes the same semantics idiomatically: each generated Action gets a typed store selector with `all`, `none`, and `outputs(...)` constructors; output arguments are nullable booleans, where null means unspecified. For example `searchTodos(query: query, store: const SearchTodosStore.none())` and `openTodo(id: id, store: const OpenTodoStore.outputs(suggestions: false))`. Reserve the generated option parameter from business-argument collisions using the existing codegen naming convention; if no convention exists, package options in a generated Action options argument and test collision handling. Do not silently overwrite an input named `store`. Both Dart entry points share the selector type. Actions with no eligible outputs omit the outputs constructor.

`actions.call` does not imply `store: false`. The entry point determines delivery and completion semantics; `store` determines only output-driven local storage. Handler inputs, identity outputs, result types and `call.wait()` remain unchanged.

## Meaning of store

The option controls each output position's contribution to response authority, not whether the call is durable or whether the backend persists its outcome.

| Authority source | Effect of store: false |
| --- | --- |
| Explicit Model output alone | Return its Loader snapshot, omit its additional authority |
| Another enabled output containing the same identity | Keep that output's authority |
| Mutation input or handler-reported changed record | Keep required reconciliation authority |
| Independent subscription, Bootstrap or another Action | No effect |

Compute a positive union of required authority and enabled output identities. Never collect all records and subtract disabled identities. Required write acknowledgment, stamps, before-images and pending-edit replay must survive even when an output selects the same record with storage disabled.

An output-only disabled read does not insert, update or delete local Model rows or stamps, or emit Model-change notifications solely for that read. An already cached row stays unchanged, including when the result is newer or authoritatively absent. Durable queue/completion metadata and Action lifecycle notifications still follow existing rules. No retention holders, eviction, cursor progress or live query membership are implied.

The option does not change authentication, authorization, side effects, result persistence on the backend, timeout/cancellation or result object lifetime. A returned Model is always this invocation's Loader snapshot, never a read of the reconciled local optimistic view. Subsequent local changes do not mutate that snapshot. Enabled/required authority must be applied before successful completion is exposed.

## Invocation persistence and protocol

Add an optional top-level `store` field to each Action intent beside `callId`, `name`, `version` and `args`. It accepts a boolean or an output-name-to-boolean object. Keep it outside business args and out of Handler invocation. Both direct requests and durable push mutations carry the field; response envelopes are unchanged.

Use a typed core policy, not unchecked generic descriptor metadata. Missing policy defaults to all. Canonical serialization omits explicit true; object keys have deterministic ordering. Preserve explicit map entries, including true, for semantic key validation. Different explicit representations (for example false versus a map naming every output false) need not be interchangeable for an existing call ID. Omission and true are interchangeable, and map key ordering is immaterial.

Store the normalized policy with each durable mutation in the same local transaction as its call ID, args and optimism. Add nullable `store` TEXT to `axton_mutation`; NULL denotes omitted/default-all. Existing rows keep their queue state and default behavior. Freeze and reopen retain the policy without consulting later caller options or backend defaults. Old frozen requests without the field retain their existing bytes and all-output behavior.

Include the normalized policy in the server's canonical call identity, consistently for both routes. A changed policy on an existing call ID is `call.identity_conflict`, not a new execution. Preserve the existing distinction between structurally invalid envelopes and per-call semantic rejection. Validate unknown/scalar/implicit output keys against the retained Action descriptor before invoking the Handler; a semantic failure must not reject unrelated valid batch calls. Claim and saved failure handling remain transactional.

Replay returns the committed saved result and records without invoking Handler/Loader or recomputing policy. Do not extend persistent response lifetime or add a TTL. Direct response validation currently synthesizes a durable request internally: carry store there too, and through every raw/typed request conversion.

No schema annotation, Action-output descriptor flag, retained-history policy field or policy-specific relaxation of compatibility checks is needed. Choosing store does not bump Action versions. Actual incompatible backend input/output/read contracts retain their existing version requirements. Generated clients and backend protocol support must be deployed together for this new option; supporting old servers that silently ignore it is outside this issue.

## Server resolution

Refine existing result assembly into `crates/server/src/action_results.rs` if needed to isolate its responsibility. The transactional executor still owns claim, Handler invocation, required mutation readback, save and commit. Result assembly accepts the validated invocation policy alongside the descriptor, args, selected identities, declared read versions and required readback.

1. Validate all output selections and canonical identities; preserve names, list order and multiplicity separately from deduplicated work.
2. Determine enabled explicit-output identities and union them with required mutation/extra-change authority before resolving reads.
3. Ensure stamp evidence for additional enabled identities through existing EnsureStamp rules. Do not allocate stamps or publish solely for disabled output-only reads.
4. Resolve every Model result through its retained result read version. Deduplicate reads by canonical identity and read version within this invocation. Reuse required readback only at a matching read version.
5. Resolve enabled additional authority at the client's declared read version. Different result/authority versions may require separate Loader calls. Disabled output-only records need no extra authority-version Loader call.
6. Validate the named result, combine additional and required authority, and save the response atomically with backend business writes. Client application continues to use the shared authority path and response records, never a second output-to-database path.

Keep existing Model declaration/read-contract requirements even when store is false. This feature does not introduce Models absent from the local schema.

## Absence and failures

Preserve existing shapes: single, nullable single, or lists with non-null elements. Handler-selected identities are typed key objects, including composite keys; implicit create/update identities come from inputs. Delete confirmations remain identity-shaped.

A nullable selection can return null; a selected identity resolving to authoritative absence can yield null only for a nullable result. Required missing Models and missing list elements are resolution errors; do not compact lists. Loader exceptions and refusals remain errors, never null/deletion. Preserve call-level rollback and failure isolation; external side-effect rollback is not promised.

A handler null or empty list does not itself delete cached data. A Loader-confirmed absence contributes a tombstone only when storage is enabled and valid authority evidence exists, or independently required mutation reconciliation supplies it. Output-only store:false cannot delete cached content. No partial successful business result is introduced.

## Acceptance

- Both routes and generated TypeScript/Dart support default, boolean and typed per-output selection without changing result types.
- Same Action can load a page into local Models or return suggestions without output storage.
- Disabled-only reads preserve existing local content/stamps and avoid output-only stamp allocation; enabled and required overlaps still apply in either declaration order.
- Results preserve Loader snapshot A while pending local edits show B, under both policies and both entry points.
- Different result/authority read versions use the correct Loaders; nullable/missing/error/list semantics are unchanged.
- Policy survives enqueue, reopen, freeze and retry. Saved replay performs no Handler/Loader work; changed policy conflicts without re-execution.
- Invalid policy keys fail before local side effects and remain isolated per call on server semantic validation.
- Actual PostgreSQL transaction/replay evidence, SQLite reconciliation, generated language type tests and updated runtime/docs examples cover the feature.
