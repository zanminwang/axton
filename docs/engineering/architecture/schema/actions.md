# Actions

## 1. Introduction and Goals

An Action is a versioned named backend operation with typed inputs and outputs. Both generated client routes use the same Action contract: durable `actions.name` and direct `actions.call.name`. The declaration determines input normalization, inferred local Model optimism on the durable route, and result validation; it does not implement business logic.

## 3. Context and Scope

`action Name(inputs) { outputs }` accepts ordinary scalar/enum values and Model create, update or delete operands. A Model operand may be optional or a list. Required nullable ordinary values still require the argument key. Create/update operands imply full Model outputs bound to input identities; deletes imply identity confirmations. Explicit ordinary outputs come from the handler; explicit Model outputs come from identity objects returned by the handler, then are resolved through a retained Model Loader read contract. Optional outputs may be null and lists preserve order and duplicates. An Action with no outputs returns void.

`@version(n)` selects the Action contract. `history/actions.json` retains input and output snapshots for every published version. Incompatible input or output changes require a new version; a Model read version selected by an output is retained with it. Action history is independent of Model read history. The compiler emits one versioned handler registration per retained Action version. See [compiler generation](../compiler/generate.md) for descriptors and [website schema reference](https://zanminwang.github.io/axton/schema/reference/#actions) for syntax.

## 5. Building Block View

Parsing and validation: [compiler/parse.rs](../../../../crates/compiler/src/parse.rs), [compiler/validate.rs](../../../../crates/compiler/src/validate.rs), [compiler/history.rs](../../../../crates/compiler/src/history.rs). Runtime normalization: [core/actions.rs](../../../../crates/core/src/actions.rs). Generated TypeScript and Dart bindings: [compiler/emit.rs](../../../../crates/compiler/src/emit.rs).

## 9. Architecture Decisions

Actions do not define a separate persistence or sync engine. The durable route derives optimistic Model operations from the declared operands and stores canonical args under the original Action version and call ID. The direct route skips local queueing and optimism. Both share backend execution and Loader result assembly. Per-output ephemeral behavior remains [#116](https://github.com/zanminwang/axton/issues/116); tool behavior remains [#143](https://github.com/zanminwang/axton/issues/143).

## 10. Quality Requirements

- **Changing a published Action output shape requires a version bump; retained versions preserve their snapshots.** Evidence: [compiler history tests](../../../../crates/compiler/tests/history.rs) and [Action contract fixture](../../../../integration/action-contract/schema.model).
- **Canonical input normalization rejects malformed arguments before application code runs.** Evidence: [core contract tests](../../../../crates/core/tests/contracts.rs).
