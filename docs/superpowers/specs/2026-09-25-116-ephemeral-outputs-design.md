# Action Model outputs with @ephemeral

Status: specification prepared from the user's confirmed direction; documentation only, no implementation claimed. Issue: [#116](https://github.com/zanminwang/axton/issues/116). Companion: [implementation plan](../plans/2026-09-25-116-ephemeral-outputs.md).

## 1. Scope and existing behavior

The merged #141/#142 implementation already supports typed Action results, handler-selected identity objects, versioned Loader snapshots, authority materialization, durable/direct delivery and saved call outcomes. This issue adds per-output materialization policy and completes its acceptance coverage. It does not replace the resolver or introduce a fetch API.

Use the existing [Action schema](../../engineering/architecture/schema/actions.md), [Action protocol](../../engineering/architecture/protocol/actions.md) and [guarantees](../../engineering/guarantees.md), especially Q1–Q6 and A3–A5. Scope loading and the Downlink worker remain #150/#151; retention, membership and tools remain #139/#140/#143.

## 2. Public contract

```text
model Todo {
  id String
  title String
  @@id(id)
}

action SearchTodos(query String) {
  todos Todo[] @ephemeral
}

action OpenTodo(id String) {
  todo Todo
  suggestions Todo[] @ephemeral
}
```

An explicit Model output without the annotation returns its Loader snapshot and contributes local authority. An annotated output returns the same kind of Loader snapshot but contributes no local authority. The Handler still returns identity objects, for example `{ todos: [{ id: "a" }] }`; the caller still receives complete Todo values, not identity wrappers. Both delivery routes retain their existing signatures:

```ts
const result = await client.actions.call.searchTodos({ query: "meeting" });
const call = await client.actions.searchTodos({ query: "meeting" });
const outcome = await call.wait();
```

No output policy argument is added at call time. No separate ephemeral invocation method is generated. Dart uses the equivalent generated methods and existing outcome/error types.

V1 accepts a single bare `@ephemeral` on explicit Model outputs: `Todo`, `Todo?`, and `Todo[]`. Reject arguments, repeated annotations, scalar/enum/value outputs, declaration-level use, Model fields, inputs, and identity-only Delete confirmations. Create/update outputs inferred from Model operands remain materialized; do not add syntax to override or redeclare them. Existing duplicate-output diagnostics continue to apply.

## 3. Meaning of ephemeral

The policy belongs to an output position, not to a Model, record identity, Action transport or database-wide prohibition.

| Source of a record | Contributes local authority? |
| --- | --- |
| Only an ephemeral explicit output | No |
| Any ordinary Model output | Yes |
| A changed record required by input or handler-reported changes | Yes |
| A later independent subscription page or Action | Its own existing policy applies |

If the same identity appears in both ordinary and ephemeral outputs, materialize the ordinary contribution once. Output declaration order must not change that decision. An ephemeral output overlapping a mutation target cannot suppress acknowledgment, stamp updates, before-image handling or pending-operation replay required to settle the mutation.

An already cached record is left untouched by an ephemeral-only read, even if the returned snapshot is newer, older or authoritatively missing. No row, local stamp, retained membership or Model-change notification is created solely for that output. A durable call still writes queue/completion metadata and may emit existing Action lifecycle notifications; this feature does not suppress those commits or redesign global watchers.

`@ephemeral` does not make a call read-only, remove authentication/authorization, avoid backend call-result persistence, choose direct delivery, change timeout/cancellation semantics, or guarantee that a record will never arrive by another path. Backend saved outcomes still contain the result for replay. Client result objects retain the existing in-memory lifetime.

## 4. Schema descriptor and versioning

Add a typed boolean `ephemeral` to the compiler's validated Action output and core `ActionOutputDescriptor`. Missing means false. Generated descriptors omit false and encode true explicitly. Descriptor decoding rejects non-boolean values, and semantic validation permits true only for `kind: "model"` with explicit `source: "handlerIdentity"`.

```json
{
  "name": "todos",
  "kind": "model",
  "cardinality": "list",
  "source": "handlerIdentity",
  "model": "Todo",
  "modelReadVersion": 1,
  "ephemeral": true,
  "handlerType": {
    "kind": "identity",
    "model": "Todo",
    "fields": [{"name": "id", "type": {"kind": "scalar", "name": "string"}}]
  }
}
```

The user explicitly clarified that changing materialization policy does not require an Action version bump. Adding or removing @ephemeral does not change Handler arguments, identity return shape or Loader result shape. Exclude this policy field from incompatible-output comparison while retaining all existing checks for actual output shape/read-contract changes. Normalize explicit false and omission as equivalent. Regeneration may update this policy for the current Action version without inventing a new Handler version; unrelated retained versions are not rewritten merely because the latest policy changed. This issue does not otherwise redesign Action versioning.

The backend chooses the policy from its deployed Action descriptor when a call is first executed. A queued but not yet executed call therefore uses that deployed policy. Once a call has a committed saved outcome, replay returns its original result and authority unchanged even after the policy is edited; do not reinterpret old receipts or re-run Loaders. The client applies response records using the existing authority contract, rather than rejecting an older saved response based on the currently generated output policy. Changing policy is not retroactive cache cleanup.

The generated Handler identity type and caller result type are unchanged. Preserve the field in runtime schema descriptors, backend descriptors, retained history and client reopen serialization. Keep the current Model/read-contract declaration requirements; allowing ephemeral-only Models absent from the local schema is outside this issue.

## 5. Server execution and authority selection

The existing response already separates `completion.outcome.result` from `records`. Reuse that envelope. Construct `records` from positive authority contributors; do not construct an all-output list and then subtract ephemeral identities, which would remove overlapping required records.

Refine `assemble_result` into a focused `crates/server/src/action_results.rs` module, called by the existing transactional executor. Preserve its input/result contract: Config, owner, Action descriptor, normalized args, Handler outputs, already-required readback records, declared Model versions and Host; return named result plus additional authority. Keep the public `process_action`, durable batch execution and shared `Engine::apply_records` interfaces unchanged.

Execution sequence:

1. Keep Handler execution and mutation/extra-change readback in the existing savepoint and outer application transaction. Required records, stamp allocation and publication follow current rules.
2. Validate and normalize all output selections, including cardinality and canonical identities. Determine the union of identities required by ordinary outputs before using ephemeral policy. Preserve output names/order/multiplicity separately from deduplicated work.
3. Establish stamp evidence for ordinary additional authority through the existing `EnsureStamp` behavior before resolving its authority content. Pure ephemeral-only identities require no stamp allocation or publication solely for their result. They still use the canonical versioned Loader under the same transaction.
4. Resolve result values using each output's retained `modelReadVersion`. Cache read results by canonical Model identity plus read version within the invocation. Existing mutation readback can seed the cache only at its actual read version; never substitute the current local/authority version for an older result contract.
5. Resolve ordinary additional authority at the client's declared read version, reusing matching cached reads. Different result and authority read versions may require different Loader calls. For an ephemeral-only output, do not run an additional current-authority-version Loader merely to populate `records` that will not be sent.
6. Assemble/validate the complete named result and combine required mutation authority with ordinary-output authority. Save the unchanged response envelope atomically with the business transaction. Replay returns that stored result and authority without invoking Handler or Loader again.

The transaction/readback consistency established by #142 remains necessary. Cached reads must not mix identities/read versions or assemble old content with newly allocated stamp evidence from a different transaction. A selected null optional output invokes no Loader and emits no authority.

## 6. Missing records, errors and ordering

Retain the current result-shape rules:

- Lists preserve ordering and repeated identities; no silent omission or list compaction.
- A selected identity whose Loader returns authoritative absence becomes null only for a nullable explicit Model output. A required Model or list element missing is an Action output error.
- Loader exceptions/refusals are errors even for nullable or ephemeral outputs. An invalid required output does not yield a partial successful Action result. Existing per-Action savepoint rejection and other-Action isolation stay intact.
- Empty lists and handler-returned null do not mean deletion. Ordinary selected identities resolving to stamped absence follow existing tombstone behavior. Ephemeral absence contributes no deletion authority.
- Results are invocation snapshots. Later local edits, subscription updates or replays never mutate that completed result. Ordinary authority lands through existing stamp rules before success is exposed; ephemeral values are never substituted with local Model reads.
- Client timeout/close and backend idempotency retain #142's contract; this issue adds no cancellation command or stronger rollback promise for external effects.

## 7. Client and SDK integration

The client continues validating result shapes and applying only response `records`. Do not materialize by walking result objects in SDK code. Do not add an identity denylist or skip shared records because one output references them ephemerally. Required mutation authority remains mandatory when validating receipts. Test both the durable receipt and direct response paths.

The new descriptor field must survive native schema serialization and reopening. A pure ephemeral direct response with an empty `records` array can use the current no-authority fast path. Durable completion still commits queue/settlement state. No cursor changes or active subscription are required for either path.

Public Model watches must not emit a changed Model value solely because a pure ephemeral result arrived. Existing general commit/status notification behavior remains unchanged; tests distinguish queue bookkeeping from business Model changes.

## 8. Verification matrix and parallel work

Cover single, nullable, list, duplicate/composite identities; invalid annotation placement/arguments; missing versus false policy; same-version policy changes, genuine incompatible-shape rejection and retained replay; pure ephemeral result on an empty/cached database; mixed output identities in both declaration orders; overlap with inferred mutations and handler extra changes; different result/authority read versions; authoritative missing records, errors and rollback; pending optimism, stale authority, independent later live delivery; repeated call ID and reopen. Test TypeScript and Dart generated APIs, native JS/React Native host behavior, Rust core/server/SQLite contracts, and real PostgreSQL transaction behavior. Host tests do not establish mobile-device behavior.

This work can proceed alongside #150/#151. Keep runtime changes concentrated in Action descriptors/compiler history and Action result assembly. Do not change `live.rs`, subscription storage, Downlink worker scheduling, Bootstrap messages or the shared authority-applier signature. Shared files such as `crates/compiler/src/emit.rs`, native schema plumbing, examples and guarantee docs may still conflict; reconcile narrowly against latest main and rerun affected checks before any implementation merge.

Deliver this preparation as a spec and plan only. Runtime implementation, its PR review and merge are separate execution work.
