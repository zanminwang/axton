# Action Execution Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Make the generated durable and direct Action entry points execute reliably with typed per-call results, transactional server replay and memory-only client observers.

**Architecture:** Reuse the existing Rust queue, optimistic operations, receipt settlement and authority applier. Add canonical Action intent and per-call outcomes to this spine, plus a separately scheduled direct request that shares server execution and result validation. PostgreSQL stores final call responses atomically with business writes; SDK observers retain only live in-memory outcomes.

**Tech Stack:** Rust core/client/server, SQLite, PostgreSQL adapters, Node/React Native TypeScript, Dart, compiler-generated contracts and native bindings.

## Global Constraints

- Implement only after reviewed #145 and #141 are incorporated into this worktree. Rebase/merge their completed branch commits into codex/142-action-engine, inspect descriptors and generated interfaces, and update interface adapters in this plan to those actual names before code changes.
- Preserve atomic optimism/enqueue, one frozen durable batch per client, contiguous sequence validation, receipt-only completion, savepoint failure isolation, stamps and independent live downlink.
- Durable default and direct request-response use one handler, input and output contract. Direct calls do not enqueue, apply optimism or drain the durable queue.
- Results are invocation-specific Loader snapshots, distinct from batch-final authority. Explicit Model handler outputs are identity objects, not bare scalars or complete Models. Implicit identities come from inputs.
- Client successful business results stay in live observer memory; backend replay responses are durable. Pending work/completion metadata and rejection visibility stay durable. No new public history, cancellation or progress API.
- Retain internal legacy fixtures while migrating runnable examples, then remove public legacy wiring only when replacement paths actually work. Do not expose throwing placeholder APIs. No legacy user/database migration guarantee is required.
- #116 owns full ephemeral/materialization feature acceptance. Implement the common identity/result execution needed by the approved ordinary and mutation Action examples; record shared work, leaving #116 open for remaining policy/integration coverage.
- Use tests at each changed boundary. Do not infer PostgreSQL transaction correctness from an in-memory mock or arbitrary crash safety from a reopen test.

## Preparation and evidence ledger

Read the sibling spec, current #141 Action descriptors/history, docs/engineering/guarantees.md, client settlement, server push/persistence, SDK typed API and test-running documents. Record the starting commit in .superpowers/sdd/progress.md. Install documented dependencies and build artifacts. Run the relevant baseline tests; preserve logs of pre-existing/environment failures.

Tests below are expected to fail first because the named Action boundary is absent, then pass after the implementing step. Keep each regression meaningful; assertions should check persisted state, results and invocation counts, not merely mirror implementation structure.

## Shared contracts between tasks

Create crates/core/src/actions.rs, exported by crates/core/src/lib.rs, for the runtime view of #141 descriptors and these transport-independent shapes:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionIntent {
    pub call_id: String,
    pub name: String,
    pub version: u64,
    pub args: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ActionOutcome {
    Succeeded { result: serde_json::Value },
    Failed { code: String, execution: ExecutionState },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionState { Rejected, Unknown }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallCompletion {
    pub call_id: String,
    pub outcome: ActionOutcome,
}
```

ExecutionState::Rejected means a confirmed non-success outcome of the framework's transactional execution; it does not claim nontransactional application effects rolled back. Unknown is an observation/transport failure, not a stored success or a safe new invocation. Validate nonempty normalized UUID call IDs, supported versions and exact argument/result shapes through descriptor helpers. Use repository-style typed errors, not unchecked string matching in adapters.

Action descriptor helpers consume the #141 input/output/source/cardinality metadata. Put the same Action descriptors in the client schema and server configuration so validation is shared; do not keep two diverging normalization implementations. Public methods and their responsibility:

```rust
// crates/core/src/actions.rs
normalize_action_args(schema, action, args) -> Result<Value>
validate_action_result(schema, action, result) -> Result<Value>
// crates/client/src/actions.rs, through Client's existing transaction wrapper
submit_action(name, version, args) -> Result<SubmittedCall>
prepare_action(name, version, args) -> Result<PreparedCall>
apply_action_response(request, response) -> Result<ApplyReport>
```

SubmittedCall contains internal callId and ordinal; PreparedCall contains callId and canonical request bytes. ApplyReport gains transient completions; it must never serialize completed business results into the client database. Existing Mutation may carry optional call_id/args solely to reuse queue mechanics during transition; new Action submission always fills them and derives optimistic operations internally.

The backend call replay response includes the request's callId, typed outcome and required authority records. A durable receipt contains per-call completions plus collapsed final authority. Store the full individual response before aggregating its records into batch authority.

### Task 1: Runtime Action descriptors and protocol contracts

**Files:** Create crates/core/src/actions.rs; modify crates/core/src/{lib,schema,protocol}.rs, crates/compiler/src/generate.rs as required to place descriptors in client schema; test crates/core/tests/contracts.rs and compiler descriptor fixtures; add fixtures/protocol/action-results.json.

**Consumes:** #141 Action input/output descriptors and Model read versions. **Produces:** normalized ActionIntent, CallCompletion, direct request/response encoding and durable completion decoding.

- [ ] Add fixtures for ordinary-only args, nullable named output versus void, explicit identity objects, unknown/missing/duplicate call IDs, mismatched name/version, and empty models map for a scalar-only Action. Assert malformed input/result shapes fail before user code or local writes.
- [ ] Carry retained Model read definitions into client schema/result validation; current-only schema.json cannot validate an older Action result from modelReadVersion alone. Add a fixture with Action v1 returning Model read v1 while local authority uses Model v2, asserting independent validation and correct generated result shape. Result assembly must join canonical identity fields to normalized state because authority state omits identity.
- [ ] Add a round-trip fixture where calls c1/c2 have results A/B while receipt records contain only B. Assert decoded completions retain both ordered associations and do not use the authority list as result storage.
- [ ] Run cargo test -p ahead-core --test contracts --locked; verify new tests fail at the missing Action contract.
- [ ] Implement descriptor normalization and typed envelopes using existing scalar/enum/identity helpers. Extend PushReceipt with completions; preserve old internal fixtures using an explicitly isolated legacy decode path during migration, not by accepting incomplete outcomes for new Action entries. Allow an empty Model declaration only when the Action contract needs no Model reads.
- [ ] Add exact receipt correlation checks for every new Action entry. Reject an unexpected completion even if all record stamps are valid. Keep duplicate envelope/authority checks and byte/size limits.
- [ ] Run core and compiler contract suites, inspect fixture diff, and commit.

Representative semantic assertion:

```rust
assert_eq!(receipt.completions[0].call_id, first_id);
assert_eq!(success_result(&receipt.completions[0])["todo"]["title"], "A");
assert_eq!(success_result(&receipt.completions[1])["todo"]["title"], "B");
assert_eq!(receipt.records.len(), 1);
assert_eq!(receipt.records[0].state.as_ref().unwrap()["title"], "B");
```

Implement success_result as a small test helper that pattern-matches Succeeded and fails on Failed; do not add production convenience methods solely for tests.

### Task 2: Persist Action intent and derive optimistic operations

**Files:** Create crates/client/src/actions.rs; modify crates/client/src/{lib,ddl,queue,mutate,policies,push,settlement,transport,schema_store}.rs; test crates/sqlite/tests/{push,settlement}.rs and new crates/sqlite/tests/actions.rs.

**Consumes:** Task 1 descriptors and normalized intent. **Produces:** atomic submitted call ID/ordinal, queued args and replay-compatible Model operations.

- [ ] Add real SQLite tests for scalar-only enqueue with no Model operations; failed argument validation leaving no queue/row; create+queue atomic rollback; reopening queued args/call ID unchanged; optional/list Model inputs; rejection/lifecycle dependency preservation. Inspect the physical queue with the existing harness.
- [ ] Run cargo test -p ahead-sqlite --test actions --locked and demonstrate expected red failures.
- [ ] Add call_id and args columns to pending rows, with a unique non-null call ID for every Action row and serialized canonical args. Keep ordering/batch metadata and existing child operations. Existing legacy test rows may be distinguished explicitly while transitional code remains; public Action calls cannot omit identity/args.
- [ ] Generate a UUID once in Rust submit_action. Normalize args, derive Model operations/references/prerequisites from descriptors, then enqueue all state using the existing framework transaction. Relax the no-operations rejection only for a validated ordinary-only Action, not arbitrary malformed legacy mutation input.
- [ ] Extend queue reads and freeze encoding to preserve identity/args. Model replay continues to use derived child operations. Never reconstruct ordinary args from those child rows. Frozen retries must produce byte-identical requests after reopen.
- [ ] Extend acknowledge to validate/correlate completions, apply authority, retire operations/update completion metadata, then return transient CallCompletion events. Preserve durable failure inbox and local rollback if acknowledgement cannot commit. No local result table.
- [ ] Emit terminal completion events for call IDs of all transitively rejected unsent lifecycle dependents removed by mark_rejected, not just frozen receipt entries. Test a dependent handle wait terminates with its persisted rejection reason. Cover explicit pending-work discard/rebuild observation termination without inventing confirmed rollback for frozen work.
- [ ] Run SQLite action/push/settlement targets and existing simulation push scenarios; commit.

### Task 3: Transactional backend call claims and result persistence

**Files:** Modify crates/server/src/host.rs, packages/server/host-contract.mts, fixtures/protocol/host-operations.json, packages/postgres/{migration.sql,src/sql.mts,src/persistence.mts}; test crates/server/tests/host_contract.rs and integration/persistence/server/driver-conformance.test.mjs.

**Consumes:** stable intent and canonical response. **Produces:** claimCall/saveCall host operations on the caller's database transaction.

- [ ] Extend host fixture tests with ClaimCall {owner,call_id,request} -> {fresh,request,response} and SaveCall {owner,call_id,response} -> null. Keep Rust/TS request unions and exhaustive adapter switches aligned.
- [ ] Add PostgreSQL conformance tests: two transactions racing on one owner/call ID execute the claimed body once; rollback removes first claim and business insert; commit preserves both; another owner cannot read that response; different intent under same ID is refused. Run through all existing pg/Prisma/Drizzle adapters with their real transaction runners.
- [ ] Add ahead_call with (owner_id,call_id) primary key, canonical request text and response text. Use INSERT ON CONFLICT DO NOTHING RETURNING followed by SELECT FOR UPDATE in the same transaction. The insert result supplies fresh, so an unexpectedly committed null-response row is treated as corrupt/incomplete storage rather than blindly executing again.
- [ ] Add SQL constants and persistence union dispatch. saveCall verifies an owned claimed row and stores a complete canonical response. No autonomous connection/transaction is opened by persistence, no TTL cleanup is added.
- [ ] Run cargo test -p ahead-server --test host_contract --locked, then bash integration/persistence/server/run.sh after prerequisites. Verify real concurrent/rollback cases, inspect migration schema and commit.

### Task 4: Shared server Action execution and per-call snapshots

**Files:** Create crates/server/src/actions.rs; modify crates/server/src/{lib,readback,host}.rs and packages/server/index.mts; add crates/server/tests/actions.rs and integration/persistence/server/actions.test.mjs.

**Consumes:** Action descriptor, transactional call claim, existing handler/Loader host bridge. **Produces:** execute_action shared by durable push and direct calls, with replayable result/authority.

- [ ] Add fake-host Rust behavioral tests for one handler/Loader run across duplicate committed calls, failure isolation, ordinary/void/nullable/list outputs, input-bound create/update/delete results, identity-object explicit outputs, and result snapshots A/B preserved before batch authority collapses to B.
- [ ] Add real-backend tests that update a business row and save a call outcome in one transaction, lose the HTTP response after commit, and replay the original result without incrementing handler/Loader counters. A forced persistence fault must roll back both data and confirmation.
- [ ] Run focused Rust Action tests to establish red. Add HandleAction host request and typed response {outputs,changes,publications}, preserving legacy Handle until runnable fixtures migrate. TS adapter dispatches generated versioned handler with {ctx:{tx,userId,callId,changes,publish},args}; return explicit handler outputs rather than discarding them.
- [ ] execute_action claims intent, verifies exact canonical request match, replays stored response when present, otherwise opens a business savepoint and runs the handler. Validate ordinary fields and identity objects, reuse changed-record readback, load additional identity-selected outputs using consistent stamp evidence, and build named result. Save successful result after release; on per-call rejection roll back/release and save the typed failure outside that savepoint. Infrastructure failure aborts the outer transaction.
- [ ] Share/expose the adapters' existing retryable-transaction classifier to the server handler/Loader bridge. Preserve original retryable errors across native dispatch and rethrow to the transaction runner before generic application-failure conversion. Test two distinct call IDs concurrently updating the same business record, including pg/Drizzle SQLSTATE 40001/40P01 and Prisma wrapped errors; assert retry succeeds without a permanently saved transient rejection.
- [ ] Capture result values at that call's readback, before executing later calls. Aggregate authority by greatest stamp per identity, rejecting unequal content at equal stamps; persist each individual response before saving the batch receipt. Reject call ID reused with different intent without replacing original data or saved result.
- [ ] Add the saved-replay aggregation regression: fresh cNew loads B@2 followed by replayed cOld A@1. Outcomes remain B/A while final authority is B@2; equal-stamp conflicting states fault. Acquire EnsureStamp before loading additional read-only identities and retain its lock through commit; verify concurrent changed-record writes cannot pair stale content with new metadata.
- [ ] Use existing readback/authority normalization for implicit outputs. Implement the shared explicit identity resolver needed for these examples; do not invent Channel cursors, advance stamps for read-only outputs, or implement deferred ephemeral syntax/policy here. Record shared-core work in #116.
- [ ] Run server Action/readback/host-contract targets and real persistence integration; commit.

Backend handler test shape:

```ts
handlers: {
  addTodo: async ({ctx, args}) => {
    await ctx.tx.query('INSERT INTO todo(id,title) VALUES($1,$2)', [args.todo.id,args.todo.title]);
    return {relatedTodo: null, message: 'Created'};
  },
}
```

Use the existing adapter-specific transaction in each test; the generated handler type is generic in that tx type. Loader returns the row through its existing versioned read contract.

### Task 5: Direct endpoint, native commands and local completion

**Files:** Modify bindings/{common/src/lib.rs,node/src/lib.rs,node/src/server.rs,dart/src/lib.rs}, crates/client/src/actions.rs, crates/server/src/actions.rs, packages/server/index.mts, packages/client-js/{transport,connection,live}.mts and Dart transport counterparts; test bindings/common/tests/session.rs and integration/persistence/server HTTP tests.

**Consumes:** shared execute_action, prepare_action/apply_action_response and transient completion events. **Produces:** /sync/actions and native submit/prepare/apply commands with independently scheduled direct transport.

- [ ] Add tests proving direct invocation never creates a pending queue row or optimistic write, can complete while durable work is blocked/offline, does not advance batch/channel cursors, and applies successful authoritative mutation data before exposing completion.
- [ ] Export processAction through the Node server native bridge using the same transaction runner as push. Authenticate first, run one execute_action, commit, then send the canonical response. Direct claims only the call row, not the durable client's sequence row.
- [ ] Expose submitAction, prepareAction and applyActionResponse through common bindings, and validate arguments/unknown op behavior. Core creates IDs and correlation, SDK executes HTTP bytes outside its exclusive local database section, then applies response under a short local transaction.
- [ ] Extend SyncCycle::complete and every native/SDK response consumer to carry completions separately from diagnostic reports. The current report.reports extraction must not drop completion events. Test a full durable pump resolving a real waiting handle, rather than only injecting observer events.
- [ ] Extend the shared transport carrier with the action endpoint. Store/configure the existing connection's transport for direct calls; when no transport is configured fail immediately with a typed availability error rather than enqueue. Direct callers use the same configured authentication source; auth refresh retries reuse the same prepared request and ID.
- [ ] Respect existing timeouts/abort behavior; otherwise add a documented finite direct-attempt timeout option defaulting to 30 seconds. Lost response/timeout is execution unknown. Durable retry/backoff remains owned by existing Rust/controller semantics.
- [ ] Run common binding session tests, native JS integration and endpoint tests; commit.

### Task 6: Memory-only observers and generated runtime bindings

**Files:** Create packages/client-js/actions.mts and packages/dart/lib/src/actions.dart; modify packages/client-js/{runtime,index}.mts, packages/client-react-native/index.ts, packages/dart/lib/src/{client,port}.dart, crates/compiler/src/emit.rs; add SDK lifecycle tests in integration/bindings/client-js/actions.test.mjs, integration/bindings/client-react-native/actions.test.mjs and packages/dart/test/actions_test.dart.

**Consumes:** native submitted identity/completion events and generated Action interfaces. **Produces:** working client.actions.name and client.actions.call.name with status/wait handles and typed output decoders.

- [ ] Add tests for one initial local await, delayed final result, terminal error outcome, direct thrown error, repeated waits, void mapping, no-wait execution, close with pending waiter, and a wait that remains live after the original handle reference is dropped. Verify result snapshots are not mutable live Model views.
- [ ] Register observation state before the work wakeup in the serialized submit completion, with no response-before-registration await gap. Add an immediate-response transport race test and a close-versus-submit test. Closing must fail all live pending states even when wait() has never been called, so a later wait returns client.closed.
- [ ] Implement a shared CallState with status and cached completion. Registry maps IDs to weak states; invoking wait retains its state in an active-waiter map until completion. Complete updates status/result, resolves all waiters, and removes routing/active retention. A handle still held by application code continues to own the settled state. Do not depend on wrapper reachability to retain a pending wait.
- [ ] Sweep dead weak registrations on registration/completion and clear routing on close. Use injected weak-reference access in unit tests so cleanup assertions are deterministic; include integration tests without forcing GC. Preserve backend work when observers disappear.
- [ ] Wire generated Action namespace through concrete raw client submit/direct ports. Generated code converts DateTime/enums and Model values with existing serializers; Rust remains authoritative for validation and completion. Remove the compile-only limitation from public Action examples only when their runtime test passes.
- [ ] Node/RN share observer logic where supported; Dart uses WeakReference and strong waiter state with equivalent semantics. Validate runtime feature support on supported RN/Hermes/toolchain; probe the actual supported runtime capability rather than assuming Node coverage proves Hermes support. A fallback must preserve live-handle and abandoned-state semantics; do not silently substitute permanent strong retention or evict live pending calls. If WeakRef is unavailable, surface a precise unsupported-runtime error before submission and document the supported runtime requirement. The current device fixture pins React Native 0.86.3; host tests alone are not device evidence.
- [ ] Run SDK lifecycle tests, Dart analyze/test from packages/dart, compiler/type contract checks and RN host runtime tests. Commit.

Representative lifecycle assertions:

```js
const call = await client.actions.addTodo({todo});
assert.equal(call.status, 'pending');
const waiting = call.wait();
await deliverServerResponse();
const first = await waiting;
assert.equal(first.error, null);
assert.equal(first.result.todo.title, 'A');
assert.deepEqual(await call.wait(), first);
assert.equal(handlerCalls, 1);
assert.equal(await queueSize(), 0);
```

Implement deliverServerResponse and queueSize using the fixture's real transport/SQLite harness. In separate observer-unit tests inject completion events; do not confuse them with engine atomicity evidence.

### Task 7: Integrate Action fixtures, histories and application examples

**Files:** Modify integration/action-contract and integration/generated-api fixtures/runners, fixtures/schema sources, affected examples, docs/engineering/architecture/{protocol,client,server,sdks}, website/docs/frontend/client-api.md and API index; adjust scripts/test.sh to include new Action coverage.

**Consumes:** working compiler and runtime boundaries. **Produces:** runnable documented Action API and removal of obsolete public transaction/mutation examples.

- [ ] Convert representative AddTodo, UpdateTodo, DeleteTodo, SendEmail, SearchTodos and nullable/list result examples to Action schema and the two generated entry points. Generated backend tests verify one registered handler per retained Action version and explicit Model identity object types.
- [ ] Run the new fixtures end to end against SQLite + PostgreSQL, including ordinary-only offline enqueue, restart, direct request-response, result A versus later local B, receipt/channel arrival order and replay of stored snapshots after subsequent backend updates.
- [ ] Remove intermediate compile-only ActionClientContract wiring and obsolete public mutation facade only after migrated examples pass. Legacy test helpers may remain internal where they test unchanged lower-level guarantees; do not claim a legacy migration promise or leave public examples using both names without explanation.
- [ ] Update owning docs with actual payload/table/host changes, responsibilities, no-pruning backend retention, memory-only client results and direct transport configuration. Keep planned #116 ephemeral behavior clearly separate from implemented common resolver behavior.
- [ ] Run generated API runner, protocol/host fixtures, website example checker and link checks. Inspect generated history files for intentional changes; commit.

### Task 8: Final verification and integration review

- [ ] Run cargo fmt --all --check, cargo clippy --workspace --all-targets --locked -- -D warnings and cargo test --workspace --locked.
- [ ] Run npm ci and bash scripts/build.sh as required; run bash integration/generated-api/verify.sh, bash integration/persistence/server/run.sh, relevant Node/RN binding suites and packages/dart analyze/test with the correct native library.
- [ ] Run bash scripts/test.sh for the full host gate after component tests pass. Investigate failures by actual owning component; do not alter unrelated tests merely to reach green.
- [ ] Review the whole diff against specs/issue acceptance, include proof of transaction atomicity/concurrent replay, per-call snapshots, offline recovery, direct non-enqueue and observer cleanup. Explicitly report any unexecuted device smoke tests or #116 deferred policy acceptance.
- [ ] Update the three issues with branch/PR links, evidence and outstanding scope. Create reviewable PRs in dependency order and attach them to this task. The user authorized merging all three issues in dependency order after successful verification and PR acceptance. Merge each reviewed PR, update dependent branches to the merged base, and continue without another approval gate.

## Verified runners

Existing runners are `integration/generated-api/verify.sh`, `integration/persistence/server/run.sh`, `integration/persistence/transaction-probe/run.sh` and `scripts/test.sh`. Use their documented prerequisites. New test files are created by their owning task; mock tests do not replace PostgreSQL transaction evidence.
