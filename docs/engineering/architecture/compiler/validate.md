# Validate

## 1. Introduction and Goals

Validate refuses schemas the runtimes could not execute consistently and schema changes that would break queued mutations or published records.

## 3. Context and Scope

- Input: the `Declarations` tree from [Parse](parse.md), which carries the source position of every model, field, mutation, slot, `@@unique`, `@@sequence` and prerequisite field; when present, the previous `history/mutations.json`, the previous `history/models.json` and the previous `schema.json` (the fence).
- Output: `Validated`, a typed schema for [Generate](generate.md): enums; models with resolved field types (scalar, enum, list), identity, read-contract version, unique field sets and relations; inverses resolved to the reference they mirror; requirements and prerequisites; mutations with resolved slot bindings, the allowed patch fields of every update slot and the sequence. It contains no JSON and no source positions. The two reconciled histories are a second output, computed from the descriptors (step 6).
- Errors: `line:col: message`, where the position is the declaration the rule is about (a field for relation, type and `@requires` errors; a slot for binding and patch-field errors; the `@@sequence` directive for sequence errors; the model for identity and reserved-name errors). Positions are never written into descriptors.

## 5. Building Block View

Checks run in this order:

1. **Descriptor rules at the declaration.** Duplicate enum or model names, reserved model names (`axton_` and `sqlite_` prefixes, compared case-insensitively because SQLite table names are), duplicate fields, nullable lists, a missing `@@id`, and identity fields that are nullable, lists or unknown ([Models](../schema/models.md)).
2. **Structure.** Relations and inverses ([Relations](../schema/relations.md)), requirements ([Prerequisites](../schema/prerequisites.md)), field types ([Types](../schema/types.md)), slot bindings and default patch fields, inverse uniqueness, `@@sequence` paths ([Mutations](../schema/mutations.md)).
3. **Unique constraints.** Every `@@unique` names existing, distinct fields.
4. **Descriptor.** `Schema::from_value` applies the same rules the runtimes apply at load time, so a schema the compiler accepts is one both runtimes accept. Validate renders the client descriptor through [Generate](generate.md) (`generate::schema`) purely for this check; it assembles no descriptor itself. This pass is a backstop: any rule it refuses that step 1 did not catch reports the end of the input.
5. **Mutations.** Unique names, at least one slot, unique slots, existing models, valid patch fields.
6. **History.** Version and compatibility rules per mutation ([Mutations](../schema/mutations.md)) and per model ([Models](../schema/models.md)); this checks history compatibility at compile time. Each rule compares retained descriptor snapshots with the current descriptors, so both run on the descriptor value from [Generate](generate.md), after `validate` returns, rather than on `Validated`. Both histories are reconciled before any file is written, so a refusal from either leaves every output and both histories untouched.
7. **Fence.** Every previously published model must still exist, and every published field name of a model whose version did not change. A model whose version increased is judged by its history instead.

CLI policy, the same for `--mutation-history` and `--model-history`: initializing a history refuses to overwrite one and requires every declaration at version 1; a named history file that is missing is an error unless initializing.

Code: `validate` and the `Validated` types in [compiler/validate.rs](../../../../crates/compiler/src/validate.rs); `reconcile_history`, `reconcile_model_history` and `check_fence` in [compiler/history.rs](../../../../crates/compiler/src/history.rs), called by [compiler/main.rs](../../../../crates/compiler/src/main.rs) on the descriptors.

## 10. Quality Requirements

- Invalid identities, unknown prerequisites, duplicate versions and non-unique singular inverses are refused. Evidence: [compiler/tests/compiler.rs](../../../../crates/compiler/tests/compiler.rs) `rejects_invalid_identity`, `rejects_dependency_typos`, `singular_inverse_requires_a_unique_foreign_key`.
- A semantic error names the line of the offending declaration, not the end of the input, and the CLI maps that line back to the file that declares it. Evidence: `semantic_errors_report_the_offending_declaration` (relation, inverse, type, binding, sequence, prerequisite, duplicate mutation, patch field, unique constraint, duplicate model, missing identity) and [compiler/tests/cli.rs](../../../../crates/compiler/tests/cli.rs) `cli_relocates_errors_into_the_file_that_declares_them`.
- A model name with a reserved prefix is refused at compile time at its declaration, and at load time by core. Evidence: `rejects_reserved_model_names_at_the_declaration`; [core/tests/contracts.rs](../../../../crates/core/tests/contracts.rs) `field_default_and_record_stamp_round_trip_and_axton_prefix_is_rejected`.
- Identical input produces identical output bytes. Evidence: [compiler/tests/cli.rs](../../../../crates/compiler/tests/cli.rs) `cli_output_is_deterministic`.
- Validate is complete on its own: a semantic error is reported without any descriptor, and `Validated` carries resolved types, relations, inverses, bindings and patch fields as plain data. Evidence: [compiler/tests/parse.rs](../../../../crates/compiler/tests/parse.rs) `semantic_errors_come_from_validate_alone`, `validate_resolves_declarations_into_typed_data_without_descriptors`.
- History misuse is refused before any output is written: a missing explicit history without initialization, initialization above version 1, and initialization over an existing history. Evidence: `cli_refuses_misuse_of_the_mutation_history`.
- Incompatible input changes require a version bump; a fence violation fails without touching existing output. Evidence: [compiler/tests/history.rs](../../../../crates/compiler/tests/history.rs); [compiler/tests/cli.rs](../../../../crates/compiler/tests/cli.rs) `cli_retains_history_and_does_not_overwrite_on_break`.
- A breaking read change without a model version bump is refused as a whole, even when the same edit advances a mutation, and neither history moves; a bumped model may drop a field the fence would otherwise keep. Evidence: `cli_retains_model_history_and_refuses_a_breaking_read_change_without_a_bump`; model-history misuse in `cli_refuses_misuse_of_the_model_history`; the fence rule in [compiler/tests/history.rs](../../../../crates/compiler/tests/history.rs) `models::the_fence_defers_to_the_history_for_a_bumped_model`. Executed 2026-09-15: `cargo test -p axton-compiler --locked` (37 passed).

## 11. Risks and Technical Debt

- **Accepted limitation:** the descriptor backstop (`Schema::from_value`) has no source positions. Every descriptor rule known to be reachable from source is checked earlier with a location; a rule that only the backstop refuses still reports the end of the input. Treat such a report as a missing located check.
- **Accepted limitation:** the fence checks names only; a type or identity change passes the compiler; the client classifies it as incompatible at open and rebuilds the local database ([Reconciliation](../client/storage/reconciliation.md)), using the same `Schema::compatibility` rule the history check shares. The source comment records this as deferred.
