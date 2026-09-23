# Generate

## 1. Introduction and Goals

Generate emits one runtime descriptor per side and one typed surface per language from the same validated definitions, so client, server and application code cannot disagree about shapes.

## 3. Context and Scope

- Input: `Validated` from [Validate](validate.md).
- Output: the descriptor value `{schema, mutations, actions, loaders, uniqueConstraints, inverses, requirements, prerequisites}` (`generate::descriptors`), of which `schema` (`generate::schema`) is the client descriptor; the emitters render the generated code from that value, and the CLI writes the files below after substituting retained versions from history.

Files written by the CLI, each through a temporary file and rename:

| File | Content | Consumer |
| --- | --- | --- |
| `schema.json` | client descriptor: enums, models (each with the `version` of the read contract the client expects), requirements, prerequisites, `clientPolicies` (every retained mutation version) | [Client / Frontend interface](../client/frontend-interface.md) at open |
| `backend.json` | server config: schema, retained mutation versions with input snapshots and `knownFields`, retained Action versions, `models` (every retained model read contract: `{name, version, identity, fields, enums}`), loaders (model names), inverses, unique constraints | [Server / Backend interface](../server/backend-interface.md) |
| `generated.ts` | types, codecs, mutation builders, model classes, ports, embedded schema | [Typed API / Client](../sdks/typed-api/client.md) |
| `backend.ts` | `Handlers<Tx>`, `Loaders<Tx>`, input types and `RecordRef` helpers; legacy-only schemas also get a `createBackend` wrapper | [Typed API / Server](../sdks/typed-api/server.md) |
| `client.ts` | `GeneratedClient` over `@ahead/client` | [Typed API / Client](../sdks/typed-api/client.md) |
| `generated.dart` | all of the above for Dart in one file | [Typed API / Client](../sdks/typed-api/client.md) |
| `history/mutations.json` (in the input directory) | retained inputs per version | [Validate](validate.md) on the next run |
| `history/models.json` (in the input directory) | retained read contracts per model version | [Validate](validate.md) on the next run |
| `history/actions.json` (in the input directory, only for schemas declaring Actions) | retained Action inputs and outputs per version | [Validate](validate.md) on the next run |

The CLI writes history beside the schema, at `history/mutations.json`, `history/models.json`, and, when Actions exist, `history/actions.json`; `--mutation-history FILE`, `--model-history FILE`, and `--action-history FILE` override those locations. `--initialize-action-history` explicitly creates a missing Action history and requires every initial Action version to be 1. A schema without Actions neither needs nor writes Action history. Commit history to Git so subsequent compilation can retain old contracts. Every output is staged as a temporary file and moved into place only after all of them were written. When the default mutation history is absent and one exists at the superseded `OUTPUT_DIR/mutation-history.json`, the CLI reads it, writes the new location, leaves the old file in place and reports the move on stderr; it never reinitializes a history silently.

During the compiler transition, `actions` is a separate top-level backend descriptor array. Each Action has one record per retained version with `name`, `version`, ordered `inputs`, ordered `outputs`, `sequence`, captured Model input contracts, referenced enums, requirements, and prerequisites. A value input has `kind: value`, a structured scalar/enum `type`, `nullable`, `list`, `cardinality`, and `required: true`; a Model input has `kind: model`, `model`, `operation`, `cardinality`, optional `allowedPatchFields`, and relation `bindings`. Outputs carry `name`, `kind` (`value`, `model`, or `deleteIdentity`), `cardinality`, and `source`; ordinary outputs use structured `type` and Model outputs use `model`. The source is `"handlerValue"`, `"handlerIdentity"`, or `{ "inputIdentity": "operandName" }`. Model outputs also carry `modelReadVersion`; explicit Model outputs carry an identity-shaped `handlerType` with every key field and type. The server resolver must use that retained read version. The client result type is the Model record or null/list according to output cardinality; a handler supplies the identity object for a handler-selected Model result. An empty `outputs` array is a void Action. Both delivery modes use the same Action record. The old `mutations` and `schema.clientPolicies` arrays remain the executable legacy path; Action values never become client policies. TypeScript and Dart Action contracts are emitted now, but #142 supplies executable queue/direct bindings and the shared Loader result path. For a schema with Actions, the backend artifact exposes registration types without a callable factory until that binding exists.

The import specifiers for the runtime packages are configurable (`--backend-runtime`, `--client-runtime`).

## 5. Building Block View

- **Descriptors.** `descriptors` and `schema` are pure functions of `Validated`: the same schema renders the same value. The `.model` text is the source of truth; the JSON descriptors are what the runtimes validate at load time; generated language code embeds the descriptor verbatim. The emitters read the descriptor value rather than `Validated`; that keeps one shape between the descriptor files and the embedded schema.
- **TypeScript.** Per model: `Name`, `NameIdentity`, `NamePatch`, decode and encode functions; `NameModel` with `get`, `query` (equality `where`, scalar `orderBy`, `limit`) and relation accessors; `NameLiveModel.watch`; `NameTxModel` with direct `create`, `update`, `delete`. `NameModel` is generic in its port type; subclasses select live or write ports without `declare` class fields, which React Native Babel presets reject. Per mutation: a typed args interface and a builder that emits wire operations. Handler registration is one key per mutation, `lowerFirst(name)`, holding a `v<n>` member for every retained version; a mutation retaining only v1 also accepts a bare function ([Typed API / Server](../sdks/typed-api/server.md#9-architecture-decisions)). Input type names stay `NameInput` for the latest version and `NameV<n>Input` for older ones. Loaders follow the same shape: one key per model with a `v<n>` member per retained model version, each returning that version's record type, `Name` for the schema's own version and `NameV<n>` (an interface generated from the retained contract, enum values inline) for an older one; a v1-only model also accepts a bare function.
- **Dart.** The same surface with `Present<T>` wrappers for patch and filter presence, named parameters for mutations, and a `libraryPath` requirement outside iOS.
- **Dates.** Encoded with `toISOString()` / `toUtc().toIso8601String()`, decoded with `new Date` / `DateTime.parse` ([Types](../schema/types.md)).

Code: `descriptors` and `schema` in [compiler/generate.rs](../../../../crates/compiler/src/generate.rs); emitters in [compiler/emit.rs](../../../../crates/compiler/src/emit.rs), re-exported by `generate`; file output in [compiler/main.rs](../../../../crates/compiler/src/main.rs).

## 9. Architecture Decisions

**History storage ([#91](https://github.com/zanminwang/ahead/issues/91)).** Keep compiler-maintained history beside the application's schema, separate from disposable generated code, and commit it to Git:

```text
ahead/
├── schema.model
└── history/
    ├── mutations.json
    └── models.json
```

`ahead/` above is the compiler's `INPUT_DIR`. Compilation reads retained definitions, [validates changes](validate.md), and updates history to generate version-specific types and runtime descriptors. Handlers and loaders remain application code. Both files are written at this layout. A `models.json` snapshot is the published read contract of one [model version](../schema/models.md#9-architecture-decisions): `{name, version, identity, fields, enums}`, with the enums copied as they were when that version was published. `backend.json` carries every snapshot under `models`; the generated backend embeds the same list and types one loader per snapshot.

Generated APIs carry the [member deprecation notices](../schema/mutations.md#9-architecture-decisions) without changing descriptors, history or runtime behavior: the compiler output lists them under `deprecations` (`{kind: field | enumValue | slot, …, reason}`), beside the descriptors and never inside them. TypeScript: a `/** @deprecated reason */` line before the field in `Name`, `NameIdentity` and `NamePatch`, before the slot in `NameArgs` and the backend input types, and before an enum's union type naming the deprecated value (a string union has no per-member position). Dart: `@Deprecated('reason')` on the record, patch, filter and update-class fields, on the enum value and on the mutation's named parameter.

## 10. Quality Requirements

- Descriptor generation is deterministic and separate from validation: the same `Validated` renders the same bytes, `descriptors(validate(parse(s)))` equals `compile(s)`, and the client descriptor loads in core. Evidence: [compiler/tests/parse.rs](../../../../crates/compiler/tests/parse.rs) `generate_is_a_pure_function_of_the_validated_schema`, `compile_is_parse_then_validate_then_generate_and_declarations_are_plain_data`; verified 2026-09-15 by compiling `fixtures/compiler/*.model` (each alone), the whole fixture directory and `examples/rust-round-trip/models` with the binaries before and after the split and diffing every output file (no differences).
- Generated TypeScript and Dart compile against valid usage and forward calls unchanged to the runtime. Evidence: [compiler/tests/compiler.rs](../../../../crates/compiler/tests/compiler.rs) emitter tests; [integration/generated-api/test.ts](../../../../integration/generated-api/test.ts); [generated_test.dart](../../../../integration/generated-api/generated_test.dart).
- Misuse is a TypeScript compile error: identity in a patch, disallowed patch field, wrong filter type, enum typo. Evidence: the `@ts-expect-error` block in `test.ts`. Dart negatives are not asserted.
- Every model carries its `version` in the embedded client schema, and the backend embeds every retained model contract. Evidence: `model_versions_reach_every_generated_surface`; `cli_output_is_deterministic` compiles the model history twice and compares bytes. Executed 2026-09-15: `cargo test -p ahead-compiler --locked` (37 passed), `bash integration/generated-api/verify.sh` (passed; the checked-in `fixtures/compiler` now retains `Entry` v1 without `tags` beside v2).
- Deprecation notices reach every generated surface and no descriptor. Evidence: `deprecations_reach_every_generated_surface_and_leave_the_descriptors_alone`; the Dart analyzer reports the fixture's deprecated enum value, field and slot with their reasons through [negative/check.sh](../../../../integration/generated-api/negative/check.sh) (the `deprecated_member_use_from_same_package` lint is enabled in that directory). The TypeScript JSDoc is asserted as text; its editor rendering was checked by reading the generated file, not by a tool.
- Retained model versions are grouped under the model's loader key, an older contract is its own record type, and the bare-function shorthand exists only for a v1-only model. Evidence: `backend_emitter_groups_loader_versions_under_the_model_name`; the loader `@ts-expect-error` negatives in [test.ts](../../../../integration/generated-api/test.ts). Executed 2026-09-15: `cargo test -p ahead-compiler --locked` (38 passed), `bash integration/generated-api/verify.sh` (passed).
- Retained mutation versions are grouped under the mutation's handler key, with the bare-function shorthand only for a v1-only mutation. Evidence: `backend_emitter_groups_handler_versions_under_the_mutation_name`, `backend_emitter_accepts_a_bare_function_only_for_a_v1_only_mutation`; the `@ts-expect-error` negatives for a bare function and a `v3` key in [test.ts](../../../../integration/generated-api/test.ts). Executed 2026-09-15: `cargo test -p ahead-compiler --locked` (25 passed), `bash integration/generated-api/verify.sh` (passed).

## 11. Risks and Technical Debt

- **Accepted limitation:** generated `open` signatures still accept a `migration` option the runtime ignores; removing or defining it belongs to [#20](https://github.com/zanminwang/ahead/issues/20).
- **Potential risk:** the Dart schema is embedded in a raw triple-quoted string; a schema string literal containing `'''` would break the file. Only relevant once string-valued attributes such as defaults exist.
