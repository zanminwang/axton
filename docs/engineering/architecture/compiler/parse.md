# Parse

## 1. Introduction and Goals

Parse turns one or more `.model` files into structured declarations with token positions, so a schema author can be pointed at the offending place.

## 3. Context and Scope

- Input: the CLI concatenates every `*.model` file in the input directory in path order; the library takes one string.
- Output: `Declarations`, a typed tree of enums, models, legacy mutations, Actions (ordinary inputs, Model operands and named outputs), and prerequisites, each with source positions. Directive arguments, bindings and sequence arguments stay as JSON expressions. [Validate](validate.md) consumes it; `compile` is `generate::descriptors(validate(parse(source)))`.
- Errors: `line:col: message (found 'token')` for syntax errors; the CLI rewrites the line into `path:line` for the file that contains it. Every declaration, field, slot, directive and prerequisite field also keeps its token position for [Validate](validate.md).

## 5. Building Block View

- **Lexer.** Identifiers and digit runs, double-quoted strings with backslash escapes, the punctuation `{ } ( ) [ ] ? , . @ < > :`, and `//` line comments. Anything else is an error. Every token carries its line and column.
- **Grammar.** `enum`, `model`, `mutation`, `action` and `prerequisite` declarations. An Action uses `action Name(input Type, operand Model.create?) { output Type }`; braces are optional for void output. `@version` and `@sequence` appear above an Action; `@@id` and `@@unique` stay inside Models. Ordinary Action values support scalar or enum types, `?` for nullable values and `[]` for lists; Model operands support create/update/delete and single, optional or list cardinality. The existing directive argument grammar still applies. Unsupported Action member annotations are rejected.
- **CLI.** `ahead compile INPUT_DIR OUTPUT_DIR [--mutation-history FILE] [--initialize-mutation-history] [--model-history FILE] [--initialize-model-history] [--action-history FILE] [--initialize-action-history] [--schema-fence FILE] [--backend-runtime SPEC] [--client-runtime SPEC]`.

Code: `lex`, `Parser`, `parse` and the `Declarations` types in [compiler/parse.rs](../../../../crates/compiler/src/parse.rs); file handling in [compiler/main.rs](../../../../crates/compiler/src/main.rs).

## 10. Quality Requirements

- A syntax error names its line. Evidence: [compiler/tests/compiler.rs](../../../../crates/compiler/tests/compiler.rs) `rejects_unknown_with_location`.
- With several input files, the CLI names the file and the line within that file, for syntax and semantic errors alike. Evidence: [compiler/tests/cli.rs](../../../../crates/compiler/tests/cli.rs) `cli_relocates_errors_into_the_file_that_declares_them`.
- Parsing records every declaration with its position and applies no semantic rule; an unknown type parses and is refused only by Validate. Evidence: [compiler/tests/parse.rs](../../../../crates/compiler/tests/parse.rs) `parse_keeps_every_declaration_with_its_position`, `parse_reports_syntax_errors_with_the_found_token_and_nothing_semantic`.
- A model `@@version` follows the mutation rules: positive, within the safe range, at most one per declaration, 1 when omitted. Evidence: `model_version_follows_the_mutation_rules`.
- `@deprecated` parses on fields, enum values and slots with an optional string reason; another argument, a non-string reason or a repeat is refused. Evidence: `deprecated_is_a_field_level_directive_with_an_optional_reason`.
- The split changes no output: the checked-in fixtures and the example compile to byte-identical files before and after it. Evidence: `compile_is_parse_then_validate_then_generate_and_declarations_are_plain_data`; verified 2026-09-14 by compiling `fixtures/compiler/*.model` and `integration/e2e/fixtures/round-trip/models` with both binaries and diffing the output files of each (seven then; six since mutation history moved out of the output directory).

## 11. Risks and Technical Debt

- **Accepted limitation:** there are no numeric or boolean literals outside `@@version` (on mutations and models); this is the parsing half of the missing field default ([#27](https://github.com/zanminwang/ahead/issues/27)).
