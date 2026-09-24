//! Durable Action submission. The intent is persisted independently of its
//! inferred optimistic Model operations.
use crate::{Client, ClientStore, Mutation, Operation, OperationKind};
use axton_core::{ActionInputDescriptor, Result, invalid, normalize_action_args};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmittedCall {
    pub call_id: String,
    pub ordinal: u64,
}

impl<S: ClientStore> Client<S> {
    pub fn submit_action(
        &mut self,
        name: &str,
        version: u64,
        args: Value,
    ) -> Result<SubmittedCall> {
        let action = self.schema.action(name, version)?;
        let args = normalize_action_args(&self.schema, action, &args)?;
        validate_bindings(&self.schema, action, &args)?;
        let operations = derive_operations(&self.schema, action, &args)?;
        let call_id = uuid::Uuid::new_v4().to_string();
        let mut mutation = Mutation::new(name, operations);
        mutation.version = version;
        mutation.call_id = Some(call_id.clone());
        mutation.args = Some(args);
        let ordinal = self.transaction(|tx| tx.enqueue(mutation))?;
        Ok(SubmittedCall { call_id, ordinal })
    }
}

pub(crate) fn validate_bindings(
    schema: &axton_core::Schema,
    action: &axton_core::ActionDescriptor,
    args: &Value,
) -> Result<()> {
    for input in &action.inputs {
        let ActionInputDescriptor::Model {
            name,
            model,
            operation,
            cardinality,
            metadata,
            ..
        } = input
        else {
            continue;
        };
        let target = &args[name];
        let targets: Vec<&Value> = match cardinality.as_str() {
            "optional" if target.is_null() => vec![],
            "list" => target
                .as_array()
                .ok_or_else(|| invalid("invalid Action list"))?
                .iter()
                .collect(),
            _ => vec![target],
        };
        for binding in metadata
            .get("bindings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let source_name = binding["slot"]
                .as_str()
                .ok_or_else(|| invalid("invalid Action binding slot"))?;
            let source_input = action
                .inputs
                .iter()
                .find(|input| input.name() == source_name)
                .ok_or_else(|| invalid("unknown Action binding source"))?;
            let ActionInputDescriptor::Model {
                model: source_model,
                ..
            } = source_input
            else {
                return Err(invalid("Action binding source must be a Model"));
            };
            let source = &args[source_name];
            if source.is_null() {
                continue;
            }
            let source_identity = source.get("identity").unwrap_or(source);
            let fields = binding["fields"]
                .as_array()
                .ok_or_else(|| invalid("invalid Action binding fields"))?;
            let local: Vec<&str> = fields
                .iter()
                .map(|v| {
                    v.as_str()
                        .ok_or_else(|| invalid("invalid Action binding field"))
                })
                .collect::<Result<_>>()?;
            let relation = schema
                .model(model)?
                .relations
                .iter()
                .find(|relation| {
                    relation.target == *source_model
                        && relation
                            .fields
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            == local
                })
                .ok_or_else(|| invalid("Action binding relation missing"))?;
            for target in &targets {
                let values = if operation == "update" {
                    &target["patch"]
                } else {
                    target
                };
                for (field, source_field) in relation.fields.iter().zip(&relation.target_fields) {
                    if let Some(actual) = values.get(field) {
                        if actual != &source_identity[source_field] {
                            return Err(invalid("Action binding mismatch"));
                        }
                    } else if operation == "create" {
                        return Err(invalid("Action binding create field missing"));
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn derive_operations(
    schema: &axton_core::Schema,
    action: &axton_core::ActionDescriptor,
    args: &Value,
) -> Result<Vec<Operation>> {
    let mut operations = Vec::new();
    for input in &action.inputs {
        let ActionInputDescriptor::Model {
            name,
            model,
            operation,
            cardinality,
            ..
        } = input
        else {
            continue;
        };
        let value = &args[name];
        let values: Vec<&Value> = match cardinality.as_str() {
            "optional" if value.is_null() => vec![],
            "list" => value
                .as_array()
                .ok_or_else(|| invalid("invalid Action list"))?
                .iter()
                .collect(),
            "single" | "optional" => vec![value],
            _ => return Err(invalid("invalid Action cardinality")),
        };
        for value in values {
            let (kind, identity, values) = match operation.as_str() {
                "create" => {
                    let mut state = value
                        .as_object()
                        .ok_or_else(|| invalid("invalid create input"))?
                        .clone();
                    let model_descriptor = action
                        .input
                        .as_ref()
                        .and_then(|s| s.models.iter().find(|m| m.name == *model))
                        .or_else(|| schema.models.iter().find(|m| m.name == *model));
                    let identity_fields = model_descriptor
                        .ok_or_else(|| invalid("Action input Model missing"))?
                        .identity
                        .clone();
                    let mut identity = serde_json::Map::new();
                    for field in identity_fields {
                        identity.insert(
                            field.clone(),
                            state
                                .remove(&field)
                                .ok_or_else(|| invalid("create identity missing"))?,
                        );
                    }
                    (
                        OperationKind::Create,
                        Value::Object(identity),
                        Some(Value::Object(state)),
                    )
                }
                "update" => (
                    OperationKind::Update,
                    value["identity"].clone(),
                    Some(value["patch"].clone()),
                ),
                "delete" => (OperationKind::Delete, value["identity"].clone(), None),
                _ => return Err(invalid("invalid Action operation")),
            };
            operations.push(Operation {
                model: model.clone(),
                op: kind,
                identity,
                values,
            });
        }
    }
    Ok(operations)
}
