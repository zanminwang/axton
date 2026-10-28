# Client bridge protocol

## 1. Introduction and Goals

Define the JSON messages between a Frontend SDK and its Rust ClientRuntime: task/transaction commands, callback/effect results, effects, observer changes, diagnostics and terminal outcomes. An ABI admission acknowledgement means the message entered the mailbox; the SDK await completes from the matching terminal event.

## 3. Context and Scope

`requestId` routes a submitted command, `effectId` routes an effect, `transactionId` and `scope` admit transaction work, `companionId` admits Mutation-local callbacks, `callId` identifies a durable Mutation and `observerId` routes watch events. Their allocation, cancellation and completion are runtime responsibilities. Extraction preserves these identities and all null/omitted-field distinctions.

## 5. Building Block View

[Client bridge](../../../../crates/protocols/src/client_bridge/mod.rs) owns messages and decoding, with local operation/report/readiness DTOs and QuerySpec ordering DTOs. The [frontend interface](../../../../crates/client/src/frontend_interface.rs) exposes Client operations; [ClientRuntime](../../../../crates/client/src/runtime) admits and executes commands. [Frontend Bindings](../frontend-sdk/bindings.md) carry messages and run requested platform effects.

Model watches may request `spec.delivery: "splice-v1"`. Their initial result and
full refreshes carry `{kind:"watch", protocol:"splice-v1", version, rows}`.
Subsequent key refreshes carry `{kind:"watch", protocol:"splice-v1", baseVersion,
version, splices:[{index, deleteCount, rows}]}`. Splices apply in message order,
each against the preceding splice's result. Versions increase per publication;
a patch's base is the receiver's last version and its version is base plus one.
Rust alone decides membership and order. `watchSnapshot {observerId}` requests a
newer full cache result for receiver recovery without another SQL query.

Callers without the opt-in retain legacy `{kind:"watch", rows}` snapshots, as do
`watchSql` observers. An older native ignores the opt-in; current SDKs accept its
legacy full results. Unsupported delivery names are refused by current native.
Close ends a negotiated Model watch with `{kind:"watch", closed:true}`; no rows
are needed. Cancellation removes the observer without a terminal snapshot.

## 10. Quality Requirements

An opt-in open request may enable `diagnostics`. Its `trace {records, dropped}`
events carry source-measured performance records through the existing outbox,
independent of task outcomes, error reports or data observers. Default opens emit
none. The [performance guide](../../../../integration/performance/README.md#diagnostics-and-ownership)
defines record/clock boundaries and consumer behavior.

Malformed routable commands must fail their request without leaving an SDK waiter hanging; malformed envelopes report diagnostics. Existing protocol tests move with the definitions. Runtime/SDK tests retain transaction lifetime, nested scope, cancellation, Call completion and observer coverage.
