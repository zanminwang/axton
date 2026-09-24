# Direct Actions

## 1. Introduction and Goals

Direct Actions use request/response delivery for a typed final result. They share the Action executor, per-call identity and Model result rules with [durable Action pushes](push.md), but have no durable client queue or inferred local optimism.

## 3. Context and Scope

`POST /sync/actions` carries `{call: {callId, name, version, args}, models}`. `callId` is a UUID generated once by the client; `models` declares the Model read contracts needed for results and authority. Ordinary-only Actions may use an empty `models` map. The response carries one correlated `completion` and `records` of authority. The client validates correlation and applies authority after the server commits.

## 5. Building Block View

Wire envelopes and normalization: [core/actions.rs](../../../../crates/core/src/actions.rs). Shared execution: [server/actions.rs](../../../../crates/server/src/actions.rs). HTTP route: [server/index.mts](../../../../packages/server/index.mts). Persistent call claim/response: [Persistence](../server/persistence.md).

## 6. Runtime View

The server authenticates the request, claims its call ID in the application transaction, then either returns the saved response or invokes the retained handler version. The same transaction contains business writes, Model Loader snapshots, stamps, publications and the saved response. A repeated call ID replays the stored outcome without re-running the handler or Loader. Direct request/response has a finite transport timeout; a client timeout may leave execution status unknown because the transaction could have committed.

A successful Model output is the Loader snapshot at that invocation. Its record authority may differ from the client's optimistic view after later local work is replayed. The direct route applies committed authority without draining an independent durable queue. The backend keeps saved call outcomes without TTL or automatic pruning; client SDKs hold result objects only in memory.

## 10. Quality Requirements

- **One call ID has one committed outcome, and replay does not re-execute application code.** Evidence: [server Action tests](../../../../crates/server/tests/actions.rs) and [PostgreSQL conformance](../../../../integration/persistence/server/driver-conformance.test.mjs).
- **The direct response is validated against the requested Action and Model contracts.** Evidence: [core Action contracts](../../../../crates/core/tests/contracts.rs).
