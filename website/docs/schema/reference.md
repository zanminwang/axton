# Schema compiler reference

The compiler reads sorted `.model` files and generates client and backend interfaces for the same contract. State transitions execute in Rust; generated code performs typed construction, conversion and forwarding. Start with [define a schema](define.md) for a walkthrough.

## Command

```sh
cargo run -p axton-compiler -- compile INPUT_DIR OUTPUT_DIR \
  --backend-runtime BACKEND_IMPORT \
  --client-runtime CLIENT_IMPORT
```

| Argument / option | Meaning |
| --- | --- |
| `INPUT_DIR` | Directory containing `.model` files; sorted and compiled together |
| `OUTPUT_DIR` | Destination for generated artifacts |
| `--backend-runtime SPEC` | TypeScript backend import; default `@axton/server` |
| `--client-runtime SPEC` | TypeScript client import; default `@axton/client` |
| `--mutation-history FILE` | Override the retained mutation history path; default `INPUT_DIR/history/mutations.json` |
| `--initialize-mutation-history` | Allow a missing explicitly selected mutation history file; only version 1 declarations |
| `--model-history FILE` | Override the retained model history path; default `INPUT_DIR/history/models.json` |
| `--initialize-model-history` | Allow a missing explicitly selected model history file; only version 1 declarations |
| `--action-history FILE` | Override retained Action history; default `INPUT_DIR/history/actions.json` for Action schemas |
| `--initialize-action-history` | Allow a missing explicitly selected Action history; new Actions must start at version 1 |
| `--schema-fence FILE` | Published schema to check; default existing output `schema.json` |

For source-checkout use, supply runtime paths relative to the output directory; see the [working command](define.md#generate-from-a-source-checkout). Default package names are not evidence of published packages. Unknown syntax or incompatible contracts fail with a diagnostic that names the file and line of the offending declaration. Validation runs before generated artifacts are replaced.

## Outputs

| File | Contents |
| --- | --- |
| `schema.json` | Client descriptor with Model read versions, Action contracts and requirements |
| `backend.json` | Backend descriptor with retained Action input/output contracts and Model read contracts; legacy mutation inputs remain where used |
| `generated.ts` | TypeScript records, identities, patches, Model facades and Action bindings |
| `client.ts` | Schema-bound `GeneratedClient`, channels and runtime re-exports |
| `backend.ts` | Typed `Handlers`, `Loaders`, inputs, `ActionRejected`, record references and a bound `createBackend` |
| `generated.dart` | Dart Models, patches, Action bindings and generated client |

History is not generated output: `INPUT_DIR/history/mutations.json` retains legacy mutation inputs where used, `INPUT_DIR/history/models.json` retains Model reads, and `INPUT_DIR/history/actions.json` retains both Action inputs and outputs when Actions exist. A refused compile leaves all histories and outputs untouched. Generated Action methods and the bound backend factory are executable.

Dart output imports `package:axton/axton.dart`. Commit the history used to generate released clients; regenerating from an empty history loses compatibility information. A history left at the superseded `OUTPUT_DIR/mutation-history.json` is read once, rewritten at the new default and reported on stderr; the old file stays where it is and you can delete it after committing the new one.

## Fields and identities

| Declaration | TypeScript | Dart | Meaning |
| --- | --- | --- | --- |
| `String` | `string` | `String` | Text |
| `UUID` | `string` | `String` | UUID represented as text |
| `Int` | `number` | `int` | Integer within the supported JSON safe range |
| `Float` | `number` | `double` | Finite number |
| `Bool` / `Boolean` | `boolean` | `bool` | Boolean |
| `DateTime` | `Date` | `DateTime` | Converted to/from UTC wire text |
| Enum name | Generated enum type | Generated enum type | One declared value |
| `T?` | Nullable type | Nullable type | Field can be null |
| `T[]` | Array | List | List of scalar/enum values |

`@deprecated` or `@deprecated(reason: "…")` after a field, an enum value or a mutation slot marks it deprecated, as in GraphQL: generated TypeScript carries `@deprecated` JSDoc and generated Dart carries `@Deprecated`, so editors and the Dart analyzer flag uses. Nothing else changes: the member stays in the schema and in every contract, and removing it still follows the versioning rules below.

`@@id(field,...)` defines identity, including composite keys. Identity fields must be nonnullable. `@@version(n)` names the Model read contract returned by a Loader; it defaults to 1 and is independent of Action versions (see [History and compatibility](#history-and-compatibility)). Model names starting with `axton_` or `sqlite_` (in any letter case) are reserved. Names used by generated clients, including `Actions`, `DirectCalls`, `ActionPort`, `GeneratedClient` and `GeneratedTransaction`, are also reserved. `@@unique(field,...)` declares a unique group that the client's local database enforces; your application database needs its own constraint ([What your backend owns](../backend/api.md#what-your-backend-owns)). Generated patches exclude identity fields. Complete records contain nullable fields; an optional patch field is a separate concept.

TypeScript omission leaves a patch field unchanged; null clears a nullable field. Dart uses `Present<T>` to distinguish supplied values from omission. Generated TypeScript is intended for `exactOptionalPropertyTypes`.

## Relations

```text
model Book {
  id String
  comments Comment[]
  @@id(id)
}
model Comment {
  id String
  bookId String
  book Book @reference(via: [bookId], onTargetDelete: delete)
  @@id(id)
}
```

A reference names the local fields matching the target identity. `onTargetDelete` accepts `none` (default) or `delete`. The cascade runs on the client only: deleting a `Book` locally deletes its `Comment` rows locally, and those deletes are never sent. A handler that deletes a book must delete its comments itself, report them with `changes.add` and publish them to their channels ([What your backend owns](../backend/api.md#what-your-backend-owns)). Inverse declarations generate navigation without storing another copy of the relationship. Singular inverses require a unique foreign key. Named references/inverses can disambiguate multiple relations; see the [parser tests](https://github.com/zanminwang/axton/blob/main/crates/compiler/tests/compiler.rs) for validated examples.

## Actions

`action Name(inputs) { outputs }` defines one generated input/output pair for queued and direct calls. Braces may be omitted when there are no explicit outputs. Ordinary inputs use scalar or enum types; `String?` is a required argument whose value can be null. Model operands use `Model.create`, `Model.update<fields>` or `Model.delete`, optionally followed by `?` or `[]`. An optional Model operand may be omitted or null; a list has zero or more elements. Omitted update fields stay omitted, while explicit null clears a nullable field. TypeScript flattens operands into `ModelCreate`, `ModelUpdate<K>` and `ModelDelete`; Dart uses `Present<T>` to represent supplied patch fields.

Create/update operands imply full Model result fields bound to their input identities. Delete operands imply identity confirmations. The versioned backend handler receives `{ ctx, args }`, separating trusted framework context from caller-supplied values. Explicit scalar/enum outputs are supplied by the handler; explicit Model outputs are selected with identity objects containing exactly the Model's `@@id` fields. For a composite identity, every key field is required. A nullable output field is present with null when absent; a list preserves order and duplicates; an Action with no outputs returns void. Nullable lists, nested lists, nullable list elements and output names colliding with implicit fields are invalid. `call` is reserved in the Action namespace.

The generated TypeScript and Dart client provides `client.actions.name(args)` returning an `ActionCall<Output>` after local acceptance, and `client.actions.call.name(args)` returning the final `Output`. The handle exposes only `status` and `wait()`. Local submission errors reject before a handle exists; terminal business failures appear in `wait()` as `ActionError`; direct-call failures reject with `ActionError`. Direct calls do not queue or apply automatic optimism and use a finite timeout. Neither Action route is allowed inside an application-owned local transaction. Standalone `client.models` CRUD is local-only; `tx.models` has local reads and CRUD without Action calls or watch.

Model results are resolved through the shared Loader path. A returned Model is the snapshot for that invocation, even if a later call in the same batch changes the row or local optimism changes the current `client.models` view. Batch-final records settle the local authoritative base and cannot reconstruct an earlier per-call result. The backend persists each outcome atomically with business writes and retains it without TTL or automatic pruning. Live client handles keep business results in memory, while pending and completion state remains durable. Per-output ephemeral policy belongs to [#116](https://github.com/zanminwang/axton/issues/116); [#143](https://github.com/zanminwang/axton/issues/143) tool behavior is separate.

## History and compatibility

Actions retain every version's input and output contract, including the Model read version selected for each Model output. A new Action starts at version 1. Changing an output's name, type, cardinality or identity source requires a newer Action version; an incompatible input change also requires a bump. Earlier versions stay in `history/actions.json` and the generated backend handler interfaces. A schema with no Actions does not need Action history.

Backend descriptors retain declared mutation versions with their input schemas and known field sets. The generated `Handlers` interface groups them under one key per mutation, such as `edit: { v1, v2 }`; input types keep names such as `EditInput` for the latest version and `EditV1Input` for older ones.

Same-version changes must be backward compatible. Adding nullable create fields or additional permitted patch/enum values can be compatible. Required create fields, field removal, changed types, reordered slots, changed bindings and changed dependency policy require a new mutation version. Versions cannot decrease and retained mutations cannot disappear.

Models are versioned separately, by the shape of the records a client reads. Within one model version you may add a nullable field; an older client ignores it and a newer client reads it as `null` from older records. Adding a required field, renaming, removing or retyping a field, or adding a value to an enum the model returns, requires `@@version(n+1)` on the model. The compiler keeps the older version's fields and enum values in `history/models.json` exactly as published, so a value added later never reaches the older contract, and `backend.json` lists every retained version. Changing a model's identity is refused at any version. The generated `Loaders` interface groups the retained versions under one key per model, such as `entry: { v1, v2 }`, with `Entry` for the current version and `EntryV1` for the older record shape ([loaders](../backend/api.md#loaders)).

An explicitly selected missing history requires `--initialize-mutation-history`, which refuses an existing history and declarations above version 1. The schema fence prevents removing published model/field names. Broader identity/type/nullability compatibility fences remain incomplete; a passing compile is not a guarantee that any local database migration is supported.

See [local migration](../frontend/runtime.md#opening-and-schema-changes) and separately migrate your backend tables.

## Verify generated APIs

Build the root native libraries, then run:

```sh
bash integration/generated-api/verify.sh
```

The checks cover Rust parser/history/CLI behavior, TypeScript positive and expected-error fixtures, Node native integration, Dart analysis and Dart native integration.
