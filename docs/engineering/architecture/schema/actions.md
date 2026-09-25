# Mutations and Queries

## 1. Introduction and Goals

An operation is a versioned named backend call with typed inputs and outputs. Its kind states the backend business contract: a **Mutation** may change business state or perform external effects; a **Query** reads without business side effects. Delivery is a separate axis chosen per call by the generated method: each kind has a default route and one override. The declaration determines input normalization, inferred local Model optimism (durable Mutations only) and result validation; it does not implement business logic.

## 3. Context and Scope

```text
mutation AddTodo(todo Todo.create)

mutation SendEmail(recipient String, message String) {
  messageId String
}

query SearchTodos(text String, cursor String?) {
  todos Todo[]
  nextCursor String?
}
```

`mutation Name(inputs) { outputs }` and `query Name(inputs) { outputs }` share one grammar; braces are optional for void output. Mutations accept ordinary scalar/enum values and Model create, update or delete operands. A Model operand may be optional or a list. Required nullable ordinary values still require the argument key. Create/update operands imply full Model outputs bound to input identities; deletes imply identity confirmations. Explicit ordinary outputs come from the handler; explicit Model outputs come from identity objects returned by the handler, then are resolved through a retained Model Loader read contract. Optional outputs may be null and lists preserve order and duplicates. An operation with no outputs returns void.

A Query takes only ordinary inputs, and its outputs are explicit ordinary or Model outputs: Model operands and `@sequence` are refused at the member, and a Mutation's `@sequence` may target only Mutations. Mutations and Queries share one lower-camel name namespace, so a name is declared once across both kinds; a Mutation may not be named `call` and a Query may not be named `enqueue`, the members that select the other route. The former `action Name(...)` declaration is a syntax error. The older `mutation Name { slots }` block is a different declaration, selected by the `{` after the name, and remains only for low-level fixtures and descriptors ([slot mutations](mutations.md)).

Choose the kind by business effect and the route by when the caller needs the result:

| Generated call | Delivery | Returns | Awaits until |
| --- | --- | --- | --- |
| `mutations.name` (default) | durable | `Call<Output>` | local acceptance: intent, queue row and inferred Model optimism committed |
| `mutations.call.name` | direct | `Output` | the backend outcome is received and its authority applied |
| `queries.name` (default) | direct | `Output` | the backend outcome is received and its authority applied |
| `queries.enqueue.name` | durable | `Call<Output>` | local acceptance: intent and queue row committed, no optimism |

A durable call's `Call` exposes `status` and `wait()`; `wait()` yields the backend outcome later ([typed client](../sdks/typed-api/client.md)). A direct call never falls back to the queue when offline. A durable Mutation's acceptance is not final backend success, and a plain-value Mutation may produce no local Model change. A queued Query captures its ordinary values at enqueue and reads when the backend executes it, not at enqueue time; no speculative result is derived from local data. A retry of the same call ID replays the saved outcome, while each new invocation has a fresh call ID and reads again.

`@version(n)` selects the contract of either kind. `history/actions.json` retains the kind, inputs and outputs of every published version. Incompatible input or output changes require a new version, and so does a kind change: a snapshot without a kind is a Mutation, and reclassifying a published version is refused. A new version may change kind while older versions keep theirs. A Model read version selected by an output is retained with it. Operation history is independent of Model read history. The compiler emits one versioned handler registration per retained version, grouped by that version's kind. See [compiler generation](../compiler/generate.md) for descriptors and [website schema reference](https://zanminwang.github.io/axton/schema/reference/) for syntax.

## 5. Building Block View

Parsing and validation: [compiler/parse.rs](../../../../crates/compiler/src/parse.rs), [compiler/validate.rs](../../../../crates/compiler/src/validate.rs), [compiler/history.rs](../../../../crates/compiler/src/history.rs), generated names in [compiler/action_names.rs](../../../../crates/compiler/src/action_names.rs). Runtime normalization, `CallKind` and the descriptor-level Query rule (`validate_query`): [core/actions.rs](../../../../crates/core/src/actions.rs). Query settlement enforcement: [server/actions.rs](../../../../crates/server/src/actions.rs). Generated TypeScript and Dart bindings: [compiler/emit.rs](../../../../crates/compiler/src/emit.rs).

## 8. Crosscutting Concepts

**Business effects versus framework storage.** A Query's contract excludes business effects: backend changes and publications. Framework bookkeeping is not a business effect and happens for both kinds: the backend claims each call ID and saves its outcome ([protocol](../protocol/actions.md)); for an explicit Model output that `store` enables (the default), it initializes the record's stamp and the client applies that authority to local Models. A queued Query also writes a client queue row with zero Model operations. `store: false` stops only the output-only authority.

**Enforcement and its limit.** A `QueryContext` has no `changes` or `publish`, in the generated types and at runtime. The shared Rust executor refuses any Query settlement that carries changes or publications with the call-level rejection `query.effects_forbidden` before stamping, readback or publication, and rolls back that call's savepoint; this holds on the direct and durable paths and for any host. It is not a SQL sandbox: `tx` is the application's own transaction and is not read-only, and the framework cannot inspect arbitrary SQL or a separately captured database or network client. A Query handler is trusted application code that must honor its read-only business contract. The execution is not wrapped in a read-only database transaction, because call claims, saved outcomes and stamps legitimately write framework metadata.

## 9. Architecture Decisions

**Kind and delivery are separate axes ([#157](https://github.com/zanminwang/axton/issues/157)).** Kind belongs to the retained `(name, version)` contract; the server selects it from its retained descriptor, never from a client flag. Delivery is chosen by the generated method, so each method has one fixed return type rather than one chosen by an option. Route and `store` never enter the descriptor or history and never require a new version.

**No second engine.** Operations reuse the two existing delivery paths. Durable calls use the queue, Uplink worker, frozen request, saved outcome and push receipt; direct calls use the immediate request path over `/sync/actions`. The durable route derives optimistic Model operations from a Mutation's declared operands and stores canonical args under the original version and call ID; the direct route skips local queueing and optimism. Both share backend execution and Loader result assembly. No endpoint, worker, native ABI entry or queue column was added for Queries: name and version already determine the kind. Internal names are kept to avoid unrelated rewrites: the core `ActionDescriptor`, `ActionIntent` and `ActionStore` types, the `actions` descriptor collection, `history/actions.json`, the `/sync/actions` endpoint, the client queue and the `action.*` error codes.

**Queries keep backend claim and save.** A direct Query is claimed and saved like any call, so a same-ID retry replays and Query traffic stores call metadata; there is no stateless Query server path. Retention of saved outcomes is [#61](https://github.com/zanminwang/axton/issues/61); reusing completed Query loads that Scope synchronization keeps current is planned in [#158](https://github.com/zanminwang/axton/issues/158).

**Store is per call.** Whether explicit Model outputs also update local Models is chosen per call with the `store` option ([protocol](../protocol/actions.md#store-policy)), not declared in the schema, and it applies unchanged to all four routes; both kinds default to `true`. Tool behavior remains [#143](https://github.com/zanminwang/axton/issues/143).

## 10. Quality Requirements

- **Both keywords parse into the same grammar with their kind; `action` is a syntax error and the slot block keeps its own path.** Evidence: [compiler parse tests](../../../../crates/compiler/tests/parse.rs) `operation_keywords_select_kind_and_keep_the_legacy_block_separate`, `retired_action_keyword_and_blockless_queries_are_syntax_errors`.
- **Queries refuse Model operands and `@sequence` at the member, names share one namespace and route members are reserved.** Evidence: [compiler tests](../../../../crates/compiler/tests/compiler.rs) `queries_reject_mutation_operands_and_sequence_at_the_member`, `operation_names_share_one_namespace_and_reserve_route_members`, `mutation_and_query_descriptors_carry_their_kind`.
- **Descriptors carry their kind, an omitted kind is a Mutation, an unknown kind is refused, and hand-written Query descriptors cannot declare effects.** Evidence: [core contract tests](../../../../crates/core/tests/contracts.rs) `operation_kind_defaults_to_mutation_and_round_trips_explicitly`, `unknown_operation_kinds_are_rejected`, `hand_written_query_descriptors_cannot_declare_business_effects`; [server Action tests](../../../../crates/server/tests/actions.rs) `backend_config_refuses_query_descriptors_with_model_operands`.
- **Changing a published output shape or kind requires a version bump; retained versions preserve their snapshots; route and store stay out of history.** Evidence: [compiler history tests](../../../../crates/compiler/tests/history.rs) `retained_operation_kind_is_part_of_each_versioned_contract`, `delivery_route_and_store_never_enter_operation_history`, and the [operation contract fixture](../../../../integration/action-contract/schema.model).
- **A forged Query settlement with effects is rejected for that call only, before framework handling, on both paths.** Evidence: [server Action tests](../../../../crates/server/tests/actions.rs) `forged_query_effects_reject_only_that_call_before_framework_handling`, `forged_query_effects_are_rejected_on_the_direct_path_too`; on PostgreSQL through the prisma, pg and drizzle shims, [actions.test.mjs](../../../../integration/persistence/server/actions.test.mjs) `a forged Query settlement rolls back its own transaction writes and keeps adjacent calls`.
- **Canonical input normalization rejects malformed arguments before application code runs.** Evidence: [core contract tests](../../../../crates/core/tests/contracts.rs).
