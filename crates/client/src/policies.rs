//! Resolve schema-declared dependencies from data, without replaying application callbacks.
use crate::engine::Engine;
use crate::store::ClientStore;
use crate::{Mutation, OperationKind};
use axton_core::{RecordKey, Result, Schema, canonical_json, invalid};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn derive<S: ClientStore>(
    engine: &mut Engine<'_, S>,
    mutation: &mut Mutation,
) -> Result<()> {
    let queue = engine.queued()?;
    let schema = engine.schema;
    let mut lifecycle: BTreeSet<_> = mutation.lifecycle_dependencies.iter().copied().collect();
    for op in &mutation.operations {
        let key = schema.record_key(&op.model, &op.identity)?;
        let mut references = vec![key.clone()];
        for relation in &schema.model(&key.model)?.relations {
            if let Some(target) = reference(engine, &key, &relation.name)? {
                references.push(target);
            }
        }
        for prior in &queue {
            for previous in &prior.mutation.operations {
                let previous_key = schema.record_key(&previous.model, &previous.identity)?;
                if (previous.op == OperationKind::Create && references.contains(&previous_key))
                    || (previous.op == OperationKind::Delete
                        && op.op == OperationKind::Create
                        && previous_key == key)
                {
                    lifecycle.insert(prior.ordinal);
                }
            }
        }
        for requirement in schema.requirements.iter().filter(|r| r.model == op.model) {
            let Some(value) = op
                .values
                .as_ref()
                .and_then(|v| v.get(&requirement.field))
                .filter(|v| !v.is_null())
            else {
                continue;
            };
            let arguments: serde_json::Map<_, _> = requirement
                .arguments
                .keys()
                .map(|k| (k.clone(), value.clone()))
                .collect();
            let invocation = json!({"name":requirement.name,"arguments":arguments});
            mutation.prerequisites.push(canonical_json(&invocation)?);
        }
    }
    mutation.lifecycle_dependencies = lifecycle.into_iter().collect();
    mutation.prerequisites.sort();
    mutation.prerequisites.dedup();
    let mut sequences: BTreeSet<_> = mutation.sequence_dependencies.iter().copied().collect();
    if let Some(policy) = policy_fn(schema, mutation) {
        let current = slots(schema, mutation, policy)?;
        if let Some(after) = policy["sequence"]["after"].as_array() {
            for reference_spec in after {
                let name = reference_spec["name"]
                    .as_str()
                    .ok_or_else(|| invalid("invalid sequence descriptor"))?;
                let arguments = reference_spec["arguments"]
                    .as_object()
                    .ok_or_else(|| invalid("invalid sequence arguments"))?;
                for prior in queue.iter().filter(|q| q.mutation.name == name) {
                    let Some(prior_policy) = policy_fn(schema, &prior.mutation) else {
                        continue;
                    };
                    let targets = slots(schema, &prior.mutation, prior_policy)?;
                    let mut matches = true;
                    for (target, path) in arguments {
                        let path = path
                            .as_str()
                            .ok_or_else(|| invalid("invalid sequence path"))?;
                        let source = resolve(engine, &current, path)?;
                        if source.is_none()
                            || !targets
                                .get(target)
                                .is_some_and(|keys| keys.contains(source.as_ref().unwrap()))
                        {
                            matches = false;
                            break;
                        }
                    }
                    if matches {
                        sequences.insert(prior.ordinal);
                    }
                }
            }
        }
    }
    mutation.sequence_dependencies = sequences.into_iter().collect();
    Ok(())
}
fn policy_fn<'a>(schema: &'a Schema, mutation: &Mutation) -> Option<&'a Value> {
    schema
        .client_policies
        .iter()
        .find(|p| p["name"] == mutation.name && p["version"].as_u64() == Some(mutation.version))
}
fn slots(
    schema: &Schema,
    mutation: &Mutation,
    policy: &Value,
) -> Result<BTreeMap<String, Vec<RecordKey>>> {
    let mut result = BTreeMap::new();
    let mut at = 0;
    for slot in policy["slots"]
        .as_array()
        .ok_or_else(|| invalid("client policy slots missing"))?
    {
        let name = slot["name"]
            .as_str()
            .ok_or_else(|| invalid("slot name missing"))?;
        let mut keys = vec![];
        while let Some(op) = mutation.operations.get(at) {
            if slot["model"] != op.model || slot["operation"] != serde_json::to_value(op.op)? {
                break;
            }
            keys.push(schema.record_key(&op.model, &op.identity)?);
            at += 1;
            if slot["cardinality"] != "list" {
                break;
            }
        }
        result.insert(name.into(), keys);
    }
    Ok(result)
}
fn resolve<S: ClientStore>(
    engine: &mut Engine<'_, S>,
    slots: &BTreeMap<String, Vec<RecordKey>>,
    path: &str,
) -> Result<Option<RecordKey>> {
    let mut parts = path.split('.');
    let first = parts.next().ok_or_else(|| invalid("empty path"))?;
    let Some(keys) = slots.get(first).filter(|v| v.len() == 1) else {
        return Ok(None);
    };
    let mut key = keys[0].clone();
    for part in parts {
        let Some(next) = reference(engine, &key, part)? else {
            return Ok(None);
        };
        key = next;
    }
    Ok(Some(key))
}
fn reference<S: ClientStore>(
    engine: &mut Engine<'_, S>,
    key: &RecordKey,
    name: &str,
) -> Result<Option<RecordKey>> {
    let relation = engine
        .schema
        .model(&key.model)?
        .relations
        .iter()
        .find(|r| r.name == name)
        .ok_or_else(|| invalid("unknown relation in dependency"))?
        .clone();
    let row = match engine.read_row(key)? {
        Some(row) => Some(row),
        None => engine.truth(key)?,
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let mut identity = serde_json::Map::new();
    for (local, target) in relation.fields.iter().zip(&relation.target_fields) {
        let Some(value) = row.get(local).filter(|v| !v.is_null()) else {
            return Ok(None);
        };
        identity.insert(target.clone(), value.clone());
    }
    Ok(Some(
        engine
            .schema
            .record_key(&relation.target, &Value::Object(identity))?,
    ))
}
