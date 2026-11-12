# Reconciliation

## 1. Introduction and Goals

Schema artifacts preserve the original Mutation input contract and exact frozen request. Canonical read materialization is separate: a compatible descriptor reorder keeps queued input, while a changed read contract upgrades the Store ([upgrade](../../protocols/sync.md#upgrade)); Model DDL alone proves no coverage. Open compares each Model's stored shape with the schema the tables were built for ([`Schema::model_compatibility`](../../../../../crates/core/src/schema.rs)). A new Model gets its tables, and a new nullable or defaulted field is added to the table and its before-image. Any other change resets the Model: a removed or renamed field, a changed type or nullability, a changed identity, unique constraints, relations or enum values, a required field with no default, or a removed Model. A reset drops and recreates the table and its before-image (a removed Model's are only dropped) and deletes the Model's authority and local write rows. The queue is untouched. A reset with an unchanged materialization still upgrades, so Bootstrap fills the Model again. Open never fails for a schema change; a Model version that goes backwards still refuses to open. Format-4 files are refused intact rather than rebuilt or wiped automatically.

## 5. Building Block View

[Implementation](../../../../../crates/client/src/store05.rs) owns this component. [Protocol 5](../../protocols/sync.md) owns shared context, delivery and settlement rules.


### The table contract

Each public Model table uses its Model name and field columns. Engine tables use the `axton_` prefix and are not application query surfaces. Ordinary application reads see the replayed projection.
