# Direct Actions

## 1. Introduction and Goals

Direct Actions use request/response delivery for a typed final result. They share the Action executor, per-call identity and Model result rules with [durable Action pushes](push.md), but have no durable client queue or inferred local optimism.

## 3. Context and Scope

`POST /sync/actions` carries `{call: {callId, name, version, args, store?}, models}`. `callId` is a UUID generated once by the client; `models` declares the Model read contracts needed for results and authority. Ordinary-only Actions may use an empty `models` map. The response carries one correlated `completion` and `records` of authority. The client validates correlation and applies authority after the server commits.

## 5. Building Block View

Wire envelopes, normalization and `ActionStore`: [core/actions.rs](../../../../crates/core/src/actions.rs). Shared execution: [server/actions.rs](../../../../crates/server/src/actions.rs); result and output authority assembly: [server/action_results.rs](../../../../crates/server/src/action_results.rs). HTTP route: [server/index.mts](../../../../packages/server/index.mts). Persistent call claim/response: [Persistence](../server/persistence.md).

## 6. Runtime View

The server authenticates the request, claims its call ID in the application transaction, then either returns the saved response or invokes the retained handler version. The same transaction contains business writes, Model Loader snapshots, stamps, publications and the saved response. A repeated call ID replays the stored outcome without re-running the handler or Loader. Direct request/response has a finite transport timeout; a client timeout may leave execution status unknown because the transaction could have committed.

### Store policy

`store` is an optional per-invocation field beside `args`, never passed to the Handler. Omitted or `true` stores every eligible output, `false` stores none, and an object maps explicit Model output names (handler-selected, single, nullable or list) to booleans; unnamed outputs default to `true`. Scalar outputs, input-bound Model outputs and Delete confirmations are not keys. A non-boolean value or non-object is a structural envelope error; an unknown or ineligible key is a per-call `action.invalid` rejection before the Handler runs. The same field appears on each durable push entry.

The policy selects only the additional authority that explicit outputs contribute. The response's authority is the positive union of required authority (mutation inputs and handler-reported changes) and the identities chosen by enabled outputs, so an identity also selected by an enabled output or required by a write is always included. A disabled output-only read gets no stamp allocation and no authority-version read; its result is still the Loader snapshot at the output's read version. Loader reads are deduplicated per record and read version within an invocation. The policy does not change result types, backend persistence of the outcome, delivery or errors.

Keys are validated against the explicit map, so an unknown key is refused even with value `true`. The canonical policy then drops `true` entries (an empty map is the default); clients persist and send it, and the call identity includes it only when it is not the default. Omitted, `true` and `{a: true}` share one identity, `{a: false, b: true}` equals `{a: false}`, and key order is immaterial. Reusing a call ID with a different policy is `call.identity_conflict`; a replay returns the saved result and authority without running the Handler or Loader.

A successful Model output is the Loader snapshot at that invocation. Its record authority may differ from the client's optimistic view after later local work is replayed. The direct route applies committed authority without draining an independent durable queue. The backend keeps saved call outcomes without TTL or automatic pruning; client SDKs hold result objects only in memory.

## 10. Quality Requirements

- **One call ID has one committed outcome, and replay does not re-execute application code.** Evidence: [server Action tests](../../../../crates/server/tests/actions.rs) and [PostgreSQL conformance](../../../../integration/persistence/server/driver-conformance.test.mjs).
- **`store` removes only output-only authority, joins the call identity and replays unchanged.** Evidence: [server store tests](../../../../crates/server/tests/action_store.rs), [PostgreSQL persistence](../../../../integration/persistence/server/actions.test.mjs) and [end-to-end](../../../../integration/action-e2e/action.test.mts).
- **The direct response is validated against the requested Action and Model contracts.** Evidence: [core Action contracts](../../../../crates/core/tests/contracts.rs).
