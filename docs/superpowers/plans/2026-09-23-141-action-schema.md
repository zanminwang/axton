# Model and Action Schema Contracts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Compile unified Model/Action declarations and generate accurate TypeScript, Dart and backend contracts for local Models and both Action delivery modes.

**Architecture:** Parse and validate Actions into one typed representation, then derive runtime metadata and language interfaces from it. Publish generated Action contracts as type-level interfaces while existing mutation execution stays operational; #142 later binds those interfaces to real durable/direct runtime implementations, including the shared Loader result path; #116 adds ephemeral policy and dedicated mixed-output acceptance.

**Tech Stack:** Rust compiler (`parse`, `validate`, `generate`, `emit`, `history`), TypeScript, Dart, generated API fixtures.

## Global Constraints

- Before code changes, update/rebase `codex/141-action-schema` onto the completed, reviewed #145 branch (implementation may proceed while its CI runs; merge #145 before the #141 PR), inspect its new public transaction contract, then adjust file paths and tests below to that baseline. Resolve conflicts in this worktree only.
- #141 owns compiler, descriptors, generated interfaces, type tests, diagnostics, fixtures and docs; #142 owns execution and the shared Loader/materialization path; #116 owns ephemeral policy and dedicated output acceptance. Do not add a callable stub or make #141's gate depend on unfinished #142/#116.
- Preserve a working existing optimistic mutation path during the staged compiler transition. Internal compatibility metadata is temporary development scaffolding, not a user migration contract.
- No `@tool`, `@local`, `@synced`, new invocation dependencies, cross-Action atomic groups, nested or nullable lists.
- Follow red/green cycles, inspect each task's diff, and commit coherent changes. Read compiler architecture and testing guides before edits.

## File map and staging

`crates/compiler/src/parse.rs` owns grammar/positions; `validate.rs` owns Action semantic types and diagnostics; `generate.rs` owns descriptors; `emit.rs` owns TS/Dart/backend type generation; `history.rs` and CLI own version/history behavior: add `history/actions.json`, `--action-history FILE` and `--initialize-action-history` without changing the legacy `history/mutations.json` path. Tests live in `crates/compiler/tests/{parse,compiler,history,cli}.rs`; legacy executable outputs live in `integration/generated-api`; create compile-only Action fixtures in `integration/action-contract/{schema.model,generated.ts,generated.dart,backend.ts,positive.ts,negative.ts,positive.dart,negative.dart,check-negative.sh,tsconfig.json,pubspec.yaml}`. The new fixture has no runtime invocation and must not be included in the old executable runner. `docs/engineering/architecture/compiler/{parse,validate,generate}.md` and typed API/schema website docs own published contracts.

The existing `generate::descriptors` emits `schema.clientPolicies` consumed by the Rust client. Keep that working old policy path internally while adding Action metadata; ensure no ordinary-input Action is coerced into it. Concretely, retain `Declarations.mutations`/`Validated.mutations` and the existing `mutation` parser/emitter branch for old executable fixtures, add parallel `actions` fields, and emit new Action type contracts from `actions` only. The existing `clientPolicies` remains derived solely from `mutations`; new `actions` never populate it. No compatibility adapter or fake execution conversion is needed. Do not put `actions` on a concrete generated client until #142 provides the binding. Export a generated `ActionClientContract` with `models`, `transaction` and `actions` signatures; use it in compile-only tests. The concrete existing client retains its executable mutation path during this phase. Mark the generated contract/API examples as planned runtime until #142 lands.

### Concrete intermediate representation and descriptor

Add these Rust shapes in `parse.rs`/`validate.rs` (use the existing `Pos`, `Operation`, `Cardinality` and scalar/enum model types):

```rust
pub struct ActionDecl { pub name: String, pub version: u64,
    pub inputs: Vec<ActionInputDecl>, pub outputs: Vec<ActionOutputDecl>,
    pub sequence: Option<SequenceDecl>, pub pos: Pos }
pub enum ActionInputDecl { Value(FieldDecl), Model(SlotDecl) }
pub struct ActionOutputDecl { pub field: FieldDecl }
pub struct Action { pub name: String, pub version: u64,
    pub inputs: Vec<ActionInput>, pub outputs: Vec<ActionOutput>,
    pub sequence: Option<Sequence> }
pub enum ActionInput { Value { name: String, ty: FieldType, nullable: bool, list: bool },
    Model { slot: Slot } }
pub enum ActionOutputSource { InputIdentity { input: String },
    HandlerValue, HandlerModelIdentity }
pub enum ActionOutputType { Value(FieldType), Model(String), DeleteIdentity(String) }
pub struct ActionOutput { pub name: String, pub ty: ActionOutputType,
    pub cardinality: Cardinality, pub source: ActionOutputSource,
    pub model_read_version: Option<u64> }
```

Use `Cardinality::Single/Optional/List` for Action outputs, with List always non-null and non-null elements. For `action Search(query String?)`, `ActionInput::Value { nullable: true, list: false }` has a required argument key: TypeScript `{ query: string | null }`, Dart `required String? query`, backend `args.query` always present as a string or null, and descriptor `{"name":"query","kind":"value","type":"String","required":true,"nullable":true,"list":false}`. Omitted `query` is a generated-type error and runtime validation error; explicit null and non-null values are accepted. There is no ordinary optional/absent input syntax in #141. `list: true` is a non-null list of non-null scalar/enum elements. Only `ActionInput::Model` uses `Cardinality::Optional`: omitted and null both normalize to absent, and its implicit output becomes null. Keep omitted patch fields absent rather than writing null. If an existing `FieldDecl` cannot carry Action output list/nullability without accepting unsupported persisted Model shapes, use the same fields in a separate `ActionFieldDecl`; do not weaken Model field validation.

For `action AddTodo(todo Todo.create) { relatedTodo Todo? }`, emit one descriptor record under top-level `actions`, not under `schema.clientPolicies`:

```json
{"name":"AddTodo","version":1,
 "inputs":[{"name":"todo","kind":"model","model":"Todo","operation":"create","cardinality":"single"}],
 "outputs":[{"name":"todo","kind":"model","model":"Todo","cardinality":"single","source":{"inputIdentity":"todo"},"modelReadVersion":1},
            {"name":"relatedTodo","kind":"model","model":"Todo","cardinality":"optional","source":"handlerIdentity","modelReadVersion":1}]}
```

The descriptor is one contract for durable and direct delivery. #142/#116 may add execution-only fields, but must preserve these identity-source semantics. For a delete output use `kind: "deleteIdentity"` with `source.inputIdentity`; for plain values use `source: "handlerValue"`. Explicit Model handler values are typed identity objects (`TodoIdentity`), not bare key scalars or full Model rows.

Representative parser/validator test in `crates/compiler/tests/compiler.rs`:

```rust
let descriptor = ahead_compiler::compile("model Todo { id String @@id(id) } action AddTodo(todo Todo.create) { relatedTodo Todo? }").unwrap();
let outputs = descriptor["actions"][0]["outputs"].as_array().unwrap();
assert_eq!(outputs[0]["source"]["inputIdentity"], "todo");
assert_eq!(outputs[1]["source"], "handlerIdentity");
assert_eq!(outputs[1]["cardinality"], "optional");
```

Use the repository's actual schema formatting when embedding `@@id`; keep the assertions on descriptor output, not merely a generated string substring.

### Task 1: Parse unified declarations and precise syntax errors

**Files:** Modify `crates/compiler/src/parse.rs`; test `crates/compiler/tests/parse.rs`.

- [ ] Add parser tests for `action AddTodo(todo Todo.create)`, `Search(query String?)`, `SendEmail(to String, body String) { messageId String }`, `GetTodos(projectId String) { todos Todo[] }`, declaration `@version`/`@sequence`, optional/list Model operands and restricted updates with relation bindings. Assert source positions and syntax errors for missing delimiters, nested lists, nullable list elements while retaining old `mutation` parsing only for the internal executable compatibility path until #142 switches it.
- [ ] Run `cargo test -p ahead-compiler --test parse --locked`; the new cases must fail before the grammar change.
- [ ] Introduce `ActionDecl`, `InputDecl` (`Value` or Model operation) and `OutputDecl` with cardinality/position in `Declarations`; parse parentheses and optional result braces. Preserve reusable field/member annotation parsing and prerequisite declarations. Keep the old grammar as an internal transition because existing integration fixtures still execute it; remove it with #142 when the runtime path moves.
- [ ] Re-run the parser test target. Commit.

### Task 2: Validate names, types and implied outputs

**Files:** Modify `crates/compiler/src/validate.rs`; test `crates/compiler/tests/compiler.rs` and `parse.rs`.

- [ ] Add red tests for ordinary scalar/enum inputs including `Search(query String?)` with required-present nullable semantics; Model create/update/delete single, optional and list; restrictions/bindings/prerequisites/sequence; required/nullable/list scalar, enum and Model outputs; output omission; duplicate implicit/explicit names; `action Call(...)` collision with reserved `call`; unsupported nested/nullable lists and object shapes. Assert diagnostics identify the source location and offending name.
- [ ] Run `cargo test -p ahead-compiler --test compiler --locked`; expect new cases to fail.
- [ ] Add typed `Validated.actions` alongside the temporary `Validated.mutations` compatibility path, carrying `ActionInput`, `ActionOutput`, `IdentitySource` (input-bound or handler-selected), `Cardinality`, and existing mutation-operation metadata. Reuse current Model identity, scalar/enum, patch restriction, binding and sequence validators. For create/update, infer full Model output; for delete, infer identity confirmation. Reject an explicit duplicate even if the type matches. Require explicit outputs to have exactly single, nullable-single or non-null list shape.
- [ ] Assert the implied output of an absent optional Model operand maps to null; descriptor metadata retains list cardinality and input identity binding. Runtime order, duplicate and absent-value behavior is exercised in #142/#116, not proven by compiler metadata alone. Re-run compiler tests and commit.

### Task 3: Emit Action descriptors without breaking current engine policies

**Interfaces and retention:** Action history implementation in this task is concrete: `reconcile_action_history(current, previous)` writes `{"formatVersion":1,"actions":{"AddTodo":{"1":snapshot}}}` to `INPUT_DIR/history/actions.json`. A snapshot captures the versioned ordinary input types/nullability, Model operand operation/cardinality/restrictions/bindings/prerequisites/sequence, named implicit/explicit output kind/cardinality/identity source, referenced enums, and each Model output's `modelReadVersion` selected from that version's retained Model read contract. The shared #142/#116 resolver must use that retained read version, not whatever current Model version exists at execution time. Preserve retained snapshots and reject same-version incompatible input changes and any nonidentical output shape (including name, type, nullability, list, identity source or Model read version); version decrease/removal is refused. The CLI adds `--action-history` and `--initialize-action-history`, checks initial versions start at 1, reconciles this history alongside existing mutation/model histories before staging any output, and emits retained Action versions to backend descriptors. Task 3 retains the complete metadata needed by Task 4; Task 4 emits the versioned `Handlers<Tx>` types. Add CLI tests: v1 `SendEmail(to String) { messageId String }` → same-version `messageId Int` fails without touching any history/output; v2 succeeds while v1 descriptor/handler output type remains; same-version nullable/list/identity-source edits fail; regeneration from retained `history/actions.json` preserves both versions and their Model read versions; a rejected edit leaves all histories and emitted files untouched. Old mutation history/CLI options continue only for executable legacy fixtures until #142 replaces them. A legacy schema with no Actions must not require a new Action history initialization or acquire unrelated empty history files; the new flags govern schemas declaring Actions. This is a new-version compiler history, not a migration of existing user databases.

**Files:** Modify `crates/compiler/src/generate.rs`, `history.rs`, `main.rs` as required by inspected callers; test `crates/compiler/tests/{compiler,history,cli}.rs`.

- [ ] Add descriptor tests that distinguish Action ordinary inputs from Model operands, include explicit output kind/cardinality/identity source and implicit input binding, preserve version/restriction/binding/prerequisite/sequence metadata, and provide one Action record for both delivery modes. Assert descriptor cardinality and source binding for empty/list-capable and optional Model inputs, required-present nullable ordinary input metadata, identity object type, and void output. Test invalid omission versus explicit null/non-null ordinary values in generated TS/Dart type fixtures. Runtime list order and duplicate preservation belong to #142/#116. For explicit `relatedTodo Todo?`, assert the handler-side descriptor requires a key-field identity object (`{id: String}` for Todo), while the client-side result is `Todo | null`; composite keys require an object of all key fields, and Model list output descriptors preserve list cardinality; #142 checks identity-object order at runtime.
- [ ] Run focused compiler/CLI/history tests to establish red.
- [ ] Add a new `actions` descriptor section for #142/#116, with input/output metadata and explicit identity source. Keep existing `clientPolicies` valid only for legacy executable mutation fixtures; do not fabricate policies for ordinary Action inputs. Preserve the old executable fixture by leaving its `mutation` parser branch, `Validated.mutations`, `clientPolicies` derivation and concrete generated `client.mutate` binding unchanged; new `actions` only feed contract artifacts until #142 supplies execution. Document the exact temporary descriptor mapping in `docs/engineering/architecture/compiler/generate.md`.
- [ ] Re-run `cargo test -p ahead-compiler --locked` and verify both Action input/output history fencing plus unchanged legacy mutation/model history tests. Commit.


### Task 4: Generate TypeScript and backend interfaces

**Files:** Modify `crates/compiler/src/emit.rs`; test `crates/compiler/tests/compiler.rs`, `integration/action-contract/{schema.model,generated.ts,backend.ts,positive.ts,negative.ts,tsconfig.json}` plus existing `integration/generated-api/backend-complete.ts` as regression evidence.

- [ ] Add emission/type tests for flattened `TodoCreate`, `TodoUpdate<K>` and `TodoDelete`, ordinary inputs, `ActionCall<T>`, `ActionOutcome<T>`, status/wait only, `ActionError`, default handle versus direct final output, and `{ ctx, args }` handler. Include TS `@ts-expect-error` checks for `tx.actions`, `call.result`, `call.error`, unsupported output shapes and wrong Model identities.
- [ ] Confirm new emission tests are red with `cargo test -p ahead-compiler --test compiler --locked`.
- [ ] Emit a shared generic TypeScript handle/outcome and Action-specific input/output interfaces; emit an `ActionClientContract` whose `actions.foo` and `actions.call.foo` share the same argument/result type and versioned backend handler contracts (`addTodo: { v1(...), v2(...) }` when retained versions exist) whose explicit Model outputs are typed identity objects at each retained Model read version. For example, the handler return type for `relatedTodo Todo?` is `{ relatedTodo: TodoIdentity | null }`, where a single-key Todo identity is `{ id: string }`; reject a bare `string` and a full Todo, while the client result remains `{ relatedTodo: Todo | null }`. Keep concrete client wiring on the existing path until #142. The new ActionClientContract must expose local create/update/delete plus get/query/watch on client.models, and reads/CRUD without watch on tx.models. The current concrete legacy client Models is read/watch-only, so reusing that type would silently lose the approved standalone local writes. Add TS and Dart positive type cases for client.models.<model>.create/update/delete and negative tx watch cases; #142 binds standalone writes to framework-owned transactions.
- [ ] Run compiler tests and `./node_modules/.bin/tsc -p integration/action-contract` with `positive.ts` and `negative.ts` included in that tsconfig; every `@ts-expect-error` must be consumed. Run `./node_modules/.bin/tsc -p integration/generated-api` to confirm old executable fixture types still pass. Confirm its negative backend fixture still fails for a missing handler. Commit.

The generated TypeScript contract for that fixture must have this shape (place in `integration/action-contract`; exact generated symbol prefix may match current naming conventions):

```ts
type ActionStatus = 'pending' | 'succeeded' | 'failed';
type ActionOutcome<T> = {result:T;error:null} | {result:undefined;error:ActionError};
interface ActionCall<T> { readonly status: ActionStatus; wait(): Promise<ActionOutcome<T>>; }
type TodoIdentity = {id:string};
type AddTodoInput = {todo:TodoCreate};
type AddTodoOutput = {todo:Todo;relatedTodo:Todo|null};
type AddTodoHandlerOutput = {relatedTodo:TodoIdentity|null};
interface ActionClientContract {
  actions: {
    addTodo(args:AddTodoInput):Promise<ActionCall<AddTodoOutput>>;
    call:{addTodo(args:AddTodoInput):Promise<AddTodoOutput>};
  };
}
```

Backend handler: `(call: ActionHandlerCall<Ctx, AddTodoInput>) => Promise<AddTodoHandlerOutput>`, with generated `type ActionHandlerCall<Ctx, Args> = {ctx: Ctx; args: Args}`. #142 supplies the concrete trusted `Ctx` implementation. `ctx` is framework-trusted; `args` is caller supplied. Add `@ts-expect-error` tests assigning `{relatedTodo:'id'}` and an inline full object literal `{relatedTodo:{id:'x',title:'extra'}}` to `AddTodoHandlerOutput`; both must fail. TypeScript is structurally typed, so a full `Todo` variable with extra fields can still be assignable to `TodoIdentity`; runtime validation in #142 rejects extra identity fields under the exact identity contract. A composite identity test uses `{tenantId:string,id:string}`. For `Todo[]`, handler outputs `TodoIdentity[]` and client outputs `Todo[]` in the same order. Add a retained `v1`/`v2` type test: `v1` keeps its original output fields and `modelReadVersion`, while `v2` can add a differently typed output only with a version bump; the backend `Handlers<Ctx>` requires both version methods. Omit implicit `todo` from the handler output type because its identity comes from input.

### Task 5: Generate Dart contracts and cross-language fixtures

**Files:** Modify `crates/compiler/src/emit.rs`; test `integration/action-contract/{generated.dart,positive.dart,negative.dart,pubspec.yaml}` and existing `integration/generated-api/negative/check.sh` as regression evidence, plus compiler emission tests.

- [ ] Add Dart analyzer-positive fixtures for `Search(query String?)` using `required String? query` (null and non-null accepted, omission rejected), value/Model/nullable/list/void outputs, including a generated `TodoIdentity` class in `{relatedTodo: TodoIdentity?}` handler output and `Todo?` client output, flattened Model operands, local Model and transaction interfaces, default handle and direct final-output signatures. Add negative analyzer cases for `tx.actions`, duplicate/invalid shapes, and absent framework handle fields.
- [ ] Confirm new analyzer checks fail against old emission; then emit idiomatic `abstract interface class ActionCall<T> { ActionStatus get status; Future<ActionOutcome<T>> wait(); }`, closed/tagged `ActionOutcome<T>` with success result versus failure `ActionError`, and Action namespace contract with `call`. A generated Dart handler return class must require `TodoIdentity? relatedTodo`; analyzer negatives pass `String` and a full `Todo` to that field. Keep the same descriptor semantics as TypeScript and do not add executable new runtime methods.
- [ ] Run `dart analyze integration/action-contract/positive.dart` on the positive fixture; run `dart analyze integration/action-contract/negative.dart` through a new `integration/action-contract/check-negative.sh` that asserts each intended error; also run `bash integration/generated-api/negative/check.sh`, `dart analyze integration/generated-api`, focused generated Dart tests where they exercise existing behavior, and compiler tests. Commit.

### Task 6: Regenerate examples and write owning documentation

**Files:** Modify the new `integration/action-contract` schema and checked-in contract outputs while retaining runnable outputs under `integration/generated-api`; update relevant repository examples found by `rg -n 'mutation |client\.mutate|tx\.mutate' examples integration --glob '!*.svg'`, `docs/engineering/architecture/compiler/{parse,validate,generate}.md`, `docs/engineering/architecture/sdks/typed-api/client.md`, `website/docs/schema/{define,reference}.md`, `website/docs/frontend/client-api.md`, `website/docs/api-index.md`.

- [ ] Add examples for AddTodo, DeleteTodo, distinct explicit Model output, SendEmail, GetTodos, nullable/list and void outputs. Label any new Action invocation as a generated contract example pending #142; label the shared Loader/runtime path as #142 pending and ephemeral policy as #116 pending. Keep the runnable legacy mutation fixtures separate until runtime replacement; add new Action schema/type fixtures without asserting runtime execution.
- [ ] Document input-bound versus handler-selected identity objects, per-invocation Loader snapshot versus batch-final settlement authority and current local view, optional/list/void semantics, initial local acceptance versus final outcome, direct-call error behavior, and local-only transaction boundary. Remove contradictory public API examples without implying new runtime availability.
- [ ] Run `cargo test -p ahead-compiler --locked`, `./node_modules/.bin/tsc -p integration/action-contract`, `dart analyze integration/action-contract/positive.dart`, `bash integration/action-contract/check-negative.sh`, `bash integration/generated-api/verify.sh` for unchanged legacy runtime behavior, and `git diff --check`. Inspect fixture output and website example checker results. Record any deferred execution checks explicitly; commit.

## Acceptance mapping and handoff

Tasks 1–3 cover syntax, validation, diagnostics and descriptors; tasks 4–5 cover both language contracts, backend identity selection and type negatives; task 6 covers fixtures/docs. Check every #141 acceptance item against these tasks and distinguish type-level success from actual runtime behavior. #142 must bind both routes, direct dispatch and backend atomic per-call outcome persistence; client business results stay in memory on live handles while pending/completion state remains durable. #142 implements the shared Loader snapshot/materialization path; #116 retains ephemeral policy and dedicated acceptance. Batch-final settlement records must never be used to reconstruct earlier per-call results. Neither is a gate to mark the compiler contract portion complete. Report exact executed commands, counts/results and remaining runtime limits. The user has authorized implementation, PR acceptance and merge in #145 → #141 → #142 order. Execute this plan after #145 is verified and incorporated; the controller owns final PR acceptance and merge.
