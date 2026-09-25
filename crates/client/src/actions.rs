//! Durable Action submission. The intent is persisted independently of its
//! inferred optimistic Model operations.
use crate::{ApplyReport, Client, ClientStore, Mutation, Operation, OperationKind};
use axton_core::{
    ActionInputDescriptor, ActionIntent, DirectActionRequest, DirectActionResponse, Result,
    invalid, normalize_action_args,
};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmittedCall {
    pub call_id: String,
    pub ordinal: u64,
}

impl<S: ClientStore> Client<S> {
    /// Prepare a direct invocation without touching local persistence.
    pub fn prepare_action(
        &self,
        name: &str,
        version: u64,
        args: Value,
    ) -> Result<DirectActionRequest> {
        let action = self.schema.action(name, version)?;
        let args = normalize_action_args(&self.schema, action, &args)?;
        validate_bindings(&self.schema, action, &args)?;
        Ok(DirectActionRequest {
            call: ActionIntent {
                call_id: uuid::Uuid::new_v4().to_string(),
                name: name.into(),
                version,
                args,
                store: Default::default(),
            },
            models: self.declared_models(),
        })
    }

    /// Apply authoritative direct results in one short local transaction.
    /// The transient completion is exposed only after that transaction commits.
    pub fn apply_action_response(
        &mut self,
        request: &DirectActionRequest,
        bytes: &[u8],
    ) -> Result<ApplyReport> {
        let response = DirectActionResponse::decode(bytes, request, &self.schema)?;
        if response.records.is_empty() {
            let mut report = ApplyReport::default();
            report.completions.push(response.completion);
            return Ok(report);
        }
        self.write(|engine| {
            let mut report = engine.apply_records(&response.records)?;
            report.completions.push(response.completion);
            Ok(report)
        })
    }
    pub fn apply_action_response_bytes(
        &mut self,
        request: &[u8],
        response: &[u8],
    ) -> Result<ApplyReport> {
        let request = DirectActionRequest::decode(request, &self.schema)?;
        self.apply_action_response(&request, response)
    }
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
    axton_core::validate_action_bindings(schema, action, args)
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
                "update" | "delete" => {
                    let model_descriptor = action
                        .input
                        .as_ref()
                        .and_then(|snapshot| {
                            snapshot.models.iter().find(|entry| entry.name == *model)
                        })
                        .or_else(|| schema.models.iter().find(|entry| entry.name == *model))
                        .ok_or_else(|| invalid("Action input Model missing"))?;
                    let mut fields = value
                        .as_object()
                        .ok_or_else(|| invalid("invalid flat Model input"))?
                        .clone();
                    let mut identity = serde_json::Map::new();
                    for field in &model_descriptor.identity {
                        identity.insert(
                            field.clone(),
                            fields
                                .remove(field)
                                .ok_or_else(|| invalid("Action identity missing"))?,
                        );
                    }
                    if operation == "update" {
                        (
                            OperationKind::Update,
                            Value::Object(identity),
                            Some(Value::Object(fields)),
                        )
                    } else {
                        (OperationKind::Delete, Value::Object(identity), None)
                    }
                }
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
