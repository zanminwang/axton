# Define a schema and generate interfaces

The schema connects local operations to typed backend inputs. It describes the records your app keeps locally; your backend's tables and business logic can have a different shape.

## Define records and an operation

Create `models/entry.model`:

```text
model Entry {
  id String
  text String
  note String?
  @@id(id)
}

mutation Edit {
  entry Entry.update<text,note>
}
```

`@@id(id)` defines identity. `String?` is nullable. `Edit` declares one update slot named `entry`; clients can change `text` and `note` but cannot change identity through that patch. It generates both the local operation and the backend's `EditInput` type.

## Generate from a source checkout

For the To-do example, run from the repository root:

```sh
cargo run -p ahead-compiler -- compile \
  examples/todo/models examples/todo/generated/node \
  --backend-runtime ../../../../packages/server/index.mts \
  --client-runtime ../../../../packages/client-js/index.mts
```

The runtime import paths are relative to the generated output directory. Adjust them when generating into another directory; [examples/todo/generate.sh](https://github.com/zanminwang/ahead/blob/main/examples/todo/generate.sh) also emits the React Native client from the same schema. Follow [getting started](../getting-started.md) to build the required native artifacts. Packages are not currently published; the default package specifiers are not a registry installation guide.

The compiler writes TypeScript and Dart clients, typed backend interfaces, descriptors and retained mutation history. See the [compiler reference](reference.md) for every output and option. Do not edit generated files by hand.

## Connect the generated layers

| Generated interface | Your use |
| --- | --- |
| `client.models.entry` | Local get, query and watch |
| `client.mutate.edit` | Apply the local update and queue `Edit` |
| `Handlers<Tx>.edit` | Implement authoritative business logic for `Edit` |
| `Loaders<Tx>.entry` | Return current records from your backend |
| Backend `Entry(identity)` | Identify a changed record in `publish` or `changes.add` |

On the client, an update slot takes `{ identity, values }` in TypeScript. In the handler, its decoded input is `{ identity, patch }`. Dart exposes a typed `EditEntryUpdate` whose fields use `Present`. These are generated views of the same mutation contract, not independently matched API names.

See [generated client usage](../frontend/client-api.md) and [backend usage](../backend/api.md) for complete examples.

## Group several changes into one mutation

Declare several named slots in the same `mutation`. For example:

```text
model Project {
  id String
  title String
  @@id(id)
}
model Task {
  id String
  projectId String
  title String
  @@id(id)
}
mutation CreateProject {
  project Project.create
  tasks Task.create[]
}
```

`CreateProject` becomes one typed client call and one backend handler. Its declared operations apply together locally, and its backend business writes share one mutation savepoint. The list slot supplies zero or more complete task records. If you need a declared relationship as well, add a reference; a field named `projectId` alone does not create one automatically.

Several `client.mutate` calls commit locally one at a time and have separate backend rejection outcomes. Choose one multi-slot mutation when the business operation must be accepted or rejected as one unit.

## Relations, prerequisites and ordering

Declare a forward reference with `@reference(via: [field])` and an inverse with the related model type. Relations generate local navigation methods and let the runtime enforce the declared contract. The [relations fixture](https://github.com/zanminwang/ahead/blob/main/fixtures/compiler/relations.model) shows `Book.comments` and `Comment.book`.

Prerequisites declare host work that must finish before sending a mutation. For example, a schema can declare `prerequisite Uploaded(key String)` and use `@requires(Uploaded(key: self))` on a field. Your application provides the async callback through [runPrerequisites](../frontend/runtime.md#prerequisites). Prerequisites are not automatically implemented uploads.

Slot bindings and `@@sequence` express operation dependencies. Use the [compiler's tested declarations](https://github.com/zanminwang/ahead/blob/main/crates/compiler/tests/compiler.rs) as syntax examples for these advanced features; they affect scheduling, not just generated types.

## Evolve the contract

Keep `history/mutations.json` and `history/models.json` beside your `.model` files and commit them. Regenerating retains prior contracts: each queued mutation keeps a defined input contract, and each published model version keeps the record shape its readers expect.

A compatible change can keep the same version; breaking slot/input/policy changes require `@@version(n)` with a newer version on the mutation, and a breaking change to the records a model returns (a required field, a rename, a removal, a type change, a new enum value) requires a newer `@@version(n)` on the model. Implement every supported handler version exposed by the generated backend interface. Do not delete history to silence a compatibility error. To steer clients away from a field, enum value or slot before removing it, mark it `@deprecated(reason: "…")`; the generated code carries the notice and everything keeps working.

There are three separate responsibilities: compiler compatibility checks, the client's local database, and migration of your backend database. The client handles its database on its own at open, applying a compatible change in place and rebuilding an incompatible one beside the old file; the compile and your backend migration are yours. Read [compiler compatibility](reference.md#history-and-compatibility) and [opening and schema changes](../frontend/runtime.md#opening-and-schema-changes) before shipping a schema change.
