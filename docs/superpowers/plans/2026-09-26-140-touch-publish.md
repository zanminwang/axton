# Generated touch and explicit publication implementation plan

> **SUPERSEDED — do not implement this revision.** On 2026-09-26 the user approved persistent record-to-Channel membership: `publish` enrolls a record and publishes current state; later inferred or explicitly touched changes automatically distribute to all member Channels. Each changed record gets one new stamp, with separate publication cursors in its Channels, atomically with the business transaction. Client subscription cursors remain client-owned. The [updated #140](https://github.com/zanminwang/axton/issues/140) is authoritative. This document's exclusions of membership and automatic distribution, its unchanged-engine assumptions, and its implementation readiness no longer apply. Rewrite and review the spec/plan to cover membership persistence, removal, concurrency, deletion/retention, repeated publication and Bootstrap compatibility before execution. The text below preserves the previous proposal for reference.

> **For agentic workers:** Use superpowers:executing-plans to implement the following tasks and review checkpoints. Use implementation subagents only when authorized; if delegated, the user prefers Sol. Do not start implementation as part of the documentation-only preparation request.

**Goal:** Replace public backend changes collectors with generated touch methods, add generated publication helpers, and require explicit publication records everywhere.

**Architecture:** Reuse the current changed-record settlement/readback engine. A per-callback SDK effect collector normalizes owned identity snapshots; generated contexts type its Model methods. Rust receives the same changes set and only explicit publication vectors. Inputs, outputs and publication remain separate contracts.

**Tech stack:** Rust compiler/server, TypeScript backend SDK, generated TypeScript contracts, Node/native and PostgreSQL integration fixtures.

## Global constraints

- Follow the [spec](../specs/2026-09-26-140-touch-publish-design.md), repository AGENTS.md and issue workflow. Use an isolated codex branch based on current main; include reviewed planning commits.
- Preserve inferred input targets and Rust-side canonical deduplication, stamps, readback, rejection isolation, stored-call replay and receipt/direct authority.
- No public `changes.add`, `changes.records`, implicit publication, return-as-change inference, new schema annotation or automatic Channel membership.
- `ctx.touch.<model>(identity)` returns void. Both `ctx.publish({channel, records})` and `ctx.publish.<model>({channel, identity})` are synchronous intent declarations, not database writes.
- Snapshot identities and membership at each declaration; content and final stamps are resolved at settlement. Publication records are mandatory, and empty arrays remain valid.
- Keep the host's private JSON key `changes`. Tighten `PublicationIntent.records` and update all host producers/fixtures in the same PR. No business operation history/version bump solely for this API change.
- Support generated external transactions and legacy slot handlers; Query contexts have neither capability. Do not add a Dart backend runtime or modify the client executor.
- One implementation PR, no merge until all tasks, final review and required checks pass. Current request produces documents only.

## File ownership and interfaces

| File | Work |
| --- | --- |
| `packages/server/effects.mts` (new) | Owned reference snapshots, per-callback collector, touch and callable publication dictionaries, closed-context guard |
| `packages/server/index.mts` | Re-export shared reference symbol/types; replace public contexts and collector call sites; preserve settlement host keys |
| `crates/server/src/host.rs` | Require records in PublicationIntent |
| `crates/server/src/readback.rs` | Remove final-change-set publication fallback; retain record/version behavior |
| `crates/compiler/src/emit.rs` | Generated Touch/Publish and context aliases, reference union, typed external transaction return surface |
| `crates/compiler/src/validate.rs` | Diagnose generated Model accessor collisions if no existing validation covers them |
| `crates/compiler/tests/compiler.rs` | Generation and naming contracts |
| `crates/server/tests/{host_contract,readback,actions}.rs` | Required-publication host contract, unchanged stamps and failure isolation |
| `integration/persistence/server/effects.test.mjs` (new), `run.sh` | Focused collector behavior and inclusion in the server gate |
| `integration/action-runtime-ts/{types.ts,backend.test.mts}` | Generated positive/negative types and emitted context behavior |
| `integration/persistence/server/{runtime,host-contract,driver-conformance,actions}.test.mjs` | Real adapter and host producer migration/regressions |
| `integration/action-e2e/{backend-fixture.ts,action.test.mts}` | End-to-end inferred/extra authority, explicit publications, returns and retry |
| Other generated fixtures, demo handlers and active documentation | Regenerate/migrate uses discovered by the API search; no edits to old task history |

Internal collector interface (implement and test before wiring handlers):

```ts
interface EffectCollector {
  readonly touch: Record<string, (identity: object) => void>;
  readonly publish: RuntimePublish;
  seed(record: RecordRef): void;
  settlement(): {
    changes: RecordRef[];
    publications: { channel: string; records: RecordRef[] }[];
  };
  close(): void;
}
```

`RuntimePublish` is a callable explicit-record publisher with a dictionary of single-Model helpers. `RecordRef` and the existing `RECORD` symbol have one definition and are re-exported from index.mts, avoiding two symbols or a circular import. `createEffects(models)` returns EffectCollector for the configured schema Model descriptors. The generated application types replace broad runtime dictionaries with concrete Model identity signatures. `settlement()` returns owned data; `close()` prevents further declarations but does not prevent obtaining the already-collected settlement.

## Task 1: Explicit intent collector and context lifecycle

**Inputs:** configured schema Model descriptors, existing tagged operand/reference forms. **Outputs:** createEffects and the unchanged changes/explicit-publications settlement payload.

- [ ] Add `effects.test.mjs` against the new module, using real shape examples rather than only checking method existence. Include this core regression:

```ts
const effects = createEffects([
  { name: "Todo", identity: ["id"], fields: [
    { name: "id", type: { kind: "scalar", name: "string" }, nullable: false },
  ] },
]);
const identity = { id: "before" };
const records = [{ model: "Todo", identity }];
effects.publish({ channel: "shared", records });
identity.id = "after";
records.push({ model: "Todo", identity: { id: "late" } });
effects.touch.todo({ id: "extra" });
assert.deepEqual(effects.settlement(), {
  changes: [{ model: "Todo", identity: { id: "extra" } }],
  publications: [{ channel: "shared", records: [
    { model: "Todo", identity: { id: "before" } },
  ] }],
});
```

- [ ] Add cases for absent/undefined/null records rejected; empty records accepted; helper and mixed publication equivalence; plain untagged identity rejected in mixed calls; duplicate touches canonicalized; missing identity keys; composite key-order normalization; Date encoded and copied before mutation; closed collector refusing touch/publish. Use a descriptor with a DateTime identity for the Date case.
- [ ] Exercise Model method names mapping to `name`, `length`, `call`, `apply`, `bind`, `prototype` and `__proto__`, plus generated-key duplicates. Valid names must invoke helpers without modifying prototypes; ambiguous duplicate keys must fail startup. Use own-property definition on an arrow callable and a null-prototype touch dictionary; do not rely on assignment to Function.name/length.
- [ ] Run `node --experimental-strip-types --test integration/persistence/server/effects.test.mjs`; expected first failure is the missing module/API. Implement the collector, normalization and lifecycle guard, then rerun until all assertions pass.

Implementation shape for the two publication paths:

```ts
const publish = (args: PublishArgs): void => {
  assertOpen();
  if (!Array.isArray(args.records)) throw new Error("publish: records are required");
  const channel = validateChannel(args.channel);
  const records = args.records.map(snapshotRecord);
  publications.push({ channel, records });
};
// For each schema Model, install an own property safely:
Object.defineProperty(publish, methodName, {
  value: ({ channel, identity }: { channel: string; identity: object }) =>
    publish({ channel, records: [{ model: modelName, identity }] }),
  enumerable: true,
});
```

`assertOpen` checks the callback-lifetime flag; `validateChannel` rejects non-string/blank names consistently with Rust; `snapshotRecord` resolves the existing RECORD tag or explicit reference, checks the configured Model, selects and validates its identity fields, normalizes supported scalar identity encodings, and returns an owned JSON-compatible copy. Reject missing/invalid identity values rather than turning undefined or nonfinite values into null. Reuse existing identity encoding rules and test them against generated encoders; do not normalize arbitrary business row fields.

- [ ] Keep target seeding a private collector operation. Preserve current legacy slot seeding; do not infer extra touches from output values. Do not newly duplicate modern input inference in the SDK where Rust already owns it.
- [ ] Review the snapshot and lifecycle tests, then commit this independently tested internal unit.

## Task 2: Wire contexts and enforce explicit publications in Rust

**Inputs:** Task 1 collector. **Outputs:** supported runtime callbacks have touch/publish; native host cannot implicitly publish the change set.

- [ ] Replace `Changes` in MutationContext, TransactionCall and legacy HandlerCall with the new runtime Touch type. Wire every callback through the same createEffects path. Query construction stays limited to tx/userId/callId.
- [ ] Close the collector in a finally block when the callback settles. On success serialize its owned settlement; on callback failure keep the existing refusal/transaction error path. Never close it only on success, and never make a later escaped callback mutate serialized state.
- [ ] Change PublicationIntent to this strict form and remove the optional-records branch from publish_intents:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationIntent {
    pub channel: String,
    pub records: Vec<RecordRef>,
}
```

- [ ] Add native host contract regressions:

```rust
assert!(serde_json::from_value::<PublicationIntent>(
    serde_json::json!({"channel":"shared"})
).is_err());
assert!(serde_json::from_value::<PublicationIntent>(
    serde_json::json!({"channel":"shared","records":null})
).is_err());
assert_eq!(serde_json::from_value::<PublicationIntent>(
    serde_json::json!({"channel":"shared","records":[]})
).unwrap().records.len(), 0);
```

- [ ] Replace `default_publication_covers_the_final_change_set` with rejection of omitted records at the appropriate host-answer decode boundary. Keep explicit-empty, current-stamp publication, rollback, Loader refusal/deletion and per-call rejection tests. Check malformed publication in one call leaves adjacent valid calls executable.
- [ ] Migrate all direct Rust struct constructors and protocol host fixtures from optional records to explicit vectors. Preserve decoded `changes` and `publications` host field names and HTTP envelope fields.
- [ ] Migrate handwritten TypeScript runtime test/legacy handlers: `changes.add(ref)` becomes `touch.<model>(identity)`; omitted-record publishes become an explicit application-chosen list, not a new "all touched" helper. For legacy inputs use their existing identity wrapper; current Model operands are flattened and remain valid tagged publication records.
- [ ] Run `cargo test -p axton-server --test host_contract --test readback --test actions --locked`, the focused effect tests, and after native rebuild `node --test integration/persistence/server/host-contract.test.mjs`. Record the before/after failures and passing commands. Review inference/deduplication and refusal boundaries, then commit.

## Task 3: Generate typed Model methods and external transaction contexts

**Inputs:** Task 2 runtime surfaces. **Outputs:** backend.ts Touch/Publish types, schema-specific MutationContext and typed transaction callback; unchanged handler args/outputs.

- [ ] Generate Touch and callable Publish methods for every current schema Model, with identity types matching generated Model reference constructors. Alias imported raw runtime context types to avoid re-exporting them as application-specific contexts. Use the same lower-first accessor transform and diagnose collisions in compiler/config startup rather than silently replacing methods.
- [ ] Preserve existing named reference constructors for assembling mixed sets without touching records. Generate an explicit-reference discriminated union and allow existing tagged input operands. Do not claim TypeScript structurally proves an object carries the non-enumerable runtime tag; cover untagged rejection at runtime.
- [ ] Give `createBackend(...).transaction` a schema-specific callback signature without changing its runtime behavior or arbitrary return value. Replace broad raw context inference by a generated facade/type declaration that forwards to the same runtime transaction and collector. Cover retained Mutation registrations, legacy Handlers and generated exported context aliases.
- [ ] Update generated contract fixture source only if needed to add an identity shape; do not bump operation versions to accommodate method renames. Regenerate backend output with the existing runner. Add positive and negative calls in `integration/action-runtime-ts/types.ts`:

```ts
context.touch.todo({ id: todo.id });
context.publish.todo({ channel: "todos", identity: { id: todo.id } });
context.publish({ channel: "todos", records: [todo] });
// @ts-expect-error explicit records are mandatory
context.publish({ channel: "todos" });
// @ts-expect-error wrong generated identity
context.touch.todo({ missing: "one" });
// @ts-expect-error public collector was removed
context.changes.add(todo);
// @ts-expect-error Queries cannot touch records
queryContext.touch.todo({ id: todo.id });
```

Also add a temporal `moment` identity and external transaction calls; existing fixture Models Todo and Moment supply these. Add a composite-identity compiler fixture with a generated compile check, not only substring assertions. Query runtime context keys must remain unchanged; Mutation keys become callId/publish/touch/tx/userId.

- [ ] In `backend.test.mts`, replace the extra-reference collector calls with generated touch methods and assert decoded settlement still contains the same normalized references. Keep same-identity Date equivalence and extra-change deduplication assertions. Add per-Model publication parity and external transaction inference coverage.
- [ ] Run `cargo test -p axton-compiler --test compiler --locked`, `bash integration/action-runtime-ts/verify.sh` and `bash integration/generated-api/verify.sh`. Inspect generated diffs, including Dart artifacts, and fix incorrect context exports or unexpected history changes. Review type safety and generated-key handling, then commit.

## Task 4: End-to-end semantics and caller migration

**Inputs:** new generated and runtime APIs. **Outputs:** all active callers migrated, unchanged record authority behavior demonstrated.

- [ ] In `integration/action-e2e/backend-fixture.ts`, change the three Todo operand mutations to `ctx.publish({channel: "todos:demo", records: [args.todo]})`. In `retitleTodos`, call `ctx.touch.todo(identity)` for each returned DB identity and publish each through `ctx.publish.todo`; keep returned `todos`/`first` identities unchanged. Do not make every returned result an automatic touch.
- [ ] Extend action e2e assertions to demonstrate extra touched records reconcile without a Channel or declared Model output, output-only existing records do not advance stamps, repeated/inferred touches advance once, and both publish forms distribute the same current authority. Assert durable replay does not call the business handler or allocate stamps twice.
- [ ] Extend external-transaction PostgreSQL tests with generated touch and publication, no inferred targets, arbitrary return preservation, rolled-back writes/stamps/publications and no after-commit wake on failure. Use the existing pg/prisma/drizzle conformance paths rather than adding another adapter.
- [ ] Search all active consumers and migrate them explicitly:

```sh
rg -n 'changes\.add|changes\.records|\bChanges\b|publish\(' packages integration examples website docs/engineering
```

Inspect multiline calls too. Preserve uses of internal wire `changes`, protocol change arrays and historical task documents; a global text replacement is incorrect. Include demo backend handlers, generated API fixtures and snippets. Do not add compatibility aliases merely to avoid migrations.

- [ ] Run `bash integration/persistence/server/run.sh`, `bash integration/action-e2e/run.sh` and relevant demo/generated checks. Keep meaningful existing assertions; replace tests of implicit publication with rejection/explicit-selection tests. Review the distinction among touched, returned and published records, then commit.

## Task 5: Documentation, combined review and final gate

- [ ] Update `website/docs/backend/api.md`, `setup.md`, `database.md`, `docs/engineering/architecture/server/backend-interface.md`, the owning server engine publication/readback documents, `docs/engineering/architecture/schema/actions.md`, and `docs/engineering/architecture/sdks/typed-api/server.md`. Explain that touch declares a write rather than detecting differences or changing updatedAt; show automatic operand tracking, extra touch, both publication forms and external transactions.
- [ ] Document required records, call-time reference snapshots versus settlement-time content, Query restrictions, closed callback contexts and the tightened native host contract. Keep schema/history compatibility distinct from host SDK/native rebuild requirements. Do not claim membership, retention or #17 hook implementation.
- [ ] Update from current main and review the entire diff. Audit every public context and generated export for leftover changes collectors and every producer for omitted records. Verify no record stamp is advanced merely by returning or publishing it, and snapshot tests protect references rather than accidentally copying whole business records.
- [ ] Run `bash scripts/test.sh` with the documented prerequisites. The gate covers formatting/linting, server/client regression, generated APIs, PostgreSQL and website examples. Record actual results and any environment limits; do not equate inspected tests with execution. Repeat only checks justified by later changes or unresolved failures.
- [ ] Open/update one PR with `Closes #140`, design/plan links, exact behavior changes and validation. Attach the PR to the Codex task if supported. Resolve review findings and required CI before merge when executing under the user's merge authorization; do not merge documentation preparation as implementation completion.

## Review matrix

| Requirement | Primary tasks |
| --- | --- |
| Explicit publication and owned identities | 1, 2 |
| No public changes collector, all callback lifetimes | 1, 2, 3 |
| Automatic input targets, touched extras and single stamps | 2, 4 |
| Generated temporal/composite types and external transactions | 3, 4 |
| Queries/outputs/store policy remain distinct | 2, 3, 4 |
| Failure isolation and rollback across SDK/native boundary | 2, 4 |
| All callers/docs migrated, no membership expansion | 4, 5 |

Preparation has not installed dependencies, executed baseline/runtime tests or changed implementation code. The first implementation session must record its baseline and use the current main, not assume the planning base remains latest.
