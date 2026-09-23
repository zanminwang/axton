//! Generate: the validated schema to the runtime descriptors, and (through
//! the emitters in [`crate::emit`]) to the typed SDK code. Pure functions of
//! [`Validated`]: the same input renders the same bytes.
pub use crate::emit::{backend_typescript, client_typescript, dart, typescript};
use crate::validate::{Deprecation, FieldType, Mutation, Validated};
use serde_json::{Value, json};

/// The compiler output consumed by the emitters, the history and the CLI:
/// `{schema, mutations, loaders, uniqueConstraints, inverses, requirements,
/// prerequisites}`. `schema` is [`schema`]; `mutations` are the mutations as
/// declared, which the CLI replaces with every retained version.
pub fn descriptors(v: &Validated) -> Value {
    json!({
        "schema": schema(v),
        "mutations": mutations(v),
        "loaders": v.models.iter().map(|m| &m.name).collect::<Vec<_>>(),
        "uniqueConstraints": v.unique_constraints.iter().map(|c| json!({"model":c.model,"fields":c.fields})).collect::<Vec<_>>(),
        "inverses": v.inverses.iter().map(|i| json!({
            "model":i.model,"name":i.name,"target":i.target,"list":i.list,"nullable":i.nullable,
            "relationName":i.relation_name,"reference":i.reference,"fields":i.fields,
        })).collect::<Vec<_>>(),
        "requirements": requirements(v),
        "prerequisites": prerequisites(v),
        "deprecations": v.deprecations.iter().map(|d| match d {
            Deprecation::EnumValue { enum_name, value, reason } => json!({"kind":"enumValue","enum":enum_name,"value":value,"reason":reason}),
            Deprecation::Field { model, field, reason } => json!({"kind":"field","model":model,"field":field,"reason":reason}),
            Deprecation::Slot { mutation, slot, reason } => json!({"kind":"slot","mutation":mutation,"slot":slot,"reason":reason}),
        }).collect::<Vec<_>>(),
    })
}

/// The client descriptor (`schema.json` before the CLI substitutes retained
/// mutation versions for `clientPolicies`), the shape `axton_core::Schema` loads.
pub fn schema(v: &Validated) -> Value {
    let models: Vec<Value> = v
        .models
        .iter()
        .map(|m| {
            json!({
                "name": m.name,
                "version": m.version,
                "identity": m.identity,
                "fields": m.fields.iter().map(|f| json!({"name":f.name,"nullable":f.nullable,"type":field_type(&f.ty)})).collect::<Vec<_>>(),
                "relations": m.relations.iter().map(|r| json!({
                    "name":r.name,"target":r.target,"fields":r.fields,"targetFields":r.target_fields,
                    "onDelete":r.on_delete.descriptor_name(),
                })).collect::<Vec<_>>(),
                "unique": m.unique,
            })
        })
        .collect();
    let enums: Vec<Value> = v
        .enums
        .iter()
        .map(|e| json!({"name":e.name,"values":e.values}))
        .collect();
    json!({
        "models": models,
        "enums": enums,
        "requirements": requirements(v),
        "prerequisites": prerequisites(v),
        "clientPolicies": mutations(v),
    })
}

fn field_type(ty: &FieldType) -> Value {
    match ty {
        FieldType::Scalar(s) => json!({"kind":"scalar","name":s.descriptor_name()}),
        FieldType::Enum(name) => json!({"kind":"enum","name":name}),
        FieldType::List(element) => json!({"kind":"list","element":field_type(element)}),
    }
}

fn mutations(v: &Validated) -> Vec<Value> {
    v.mutations.iter().map(mutation).collect()
}

fn mutation(m: &Mutation) -> Value {
    let slots: Vec<Value> = m
        .slots
        .iter()
        .map(|s| {
            let mut value = json!({
                "name": s.name,
                "model": s.model,
                "operation": s.operation.descriptor_name(),
                "cardinality": s.cardinality.descriptor_name(),
            });
            if let Some(fields) = &s.allowed_patch_fields {
                value["allowedPatchFields"] = json!(fields);
            }
            if !s.bindings.is_empty() {
                value["bindings"] = s
                    .bindings
                    .iter()
                    .map(|b| json!({"slot":b.slot,"fields":b.fields}))
                    .collect();
            }
            value
        })
        .collect();
    let sequence = match &m.sequence {
        None => Value::Null,
        Some(sequence) => json!({
            "after": sequence.after.iter().map(|call| json!({
                "name": call.mutation,
                "arguments": call.bindings.iter().map(|b| (b.slot.clone(), Value::from(b.path.join(".")))).collect::<serde_json::Map<_, _>>(),
            })).collect::<Vec<_>>(),
        }),
    };
    json!({"name":m.name,"version":m.version,"slots":slots,"sequence":sequence})
}

fn requirements(v: &Validated) -> Vec<Value> {
    v.requirements
        .iter()
        .map(|r| {
            json!({
                "model": r.model,
                "field": r.field,
                "name": r.prerequisite,
                "arguments": r.arguments.iter().map(|a| (a.clone(), Value::from("self"))).collect::<serde_json::Map<_, _>>(),
            })
        })
        .collect()
}

fn prerequisites(v: &Validated) -> Vec<Value> {
    v.prerequisites
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "fields": p.fields.iter().map(|f| json!({"name":f.name,"type":f.type_name})).collect::<Vec<_>>(),
            })
        })
        .collect()
}
