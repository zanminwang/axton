//! Shared Action contracts. The compiler, client and server use these same
//! descriptors to normalize invocation data before any application code runs.
use crate::{
    AuthorityRecord, EnumDescriptor, FieldDescriptor, MAX_SAFE_INTEGER, ModelDescriptor, Result,
    Schema, ValueType, canonical_json, invalid,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionIntent {
    pub call_id: String,
    pub name: String,
    pub version: u64,
    pub args: Value,
}
impl ActionIntent {
    pub fn normalize(mut self, schema: &Schema) -> Result<Self> {
        self.call_id = normalize_call_id(&self.call_id)?;
        let action = schema.action(&self.name, self.version)?;
        self.args = normalize_action_args(schema, action, &self.args)?;
        Ok(self)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DirectActionRequest {
    pub call: ActionIntent,
    pub models: BTreeMap<String, u64>,
}
impl DirectActionRequest {
    pub fn decode(bytes: &[u8], schema: &Schema) -> Result<Self> {
        if bytes.len() > crate::limits::PUSH_BYTES {
            return Err(invalid("direct Action request exceeds byte limit"));
        }
        let raw: Value = serde_json::from_slice(bytes)?;
        let models = crate::protocol::read_action_models(&raw["models"])?;
        let call: ActionIntent = serde_json::from_value(
            raw.get("call")
                .cloned()
                .ok_or_else(|| invalid("direct Action call missing"))?,
        )?;
        let call = call.normalize(schema)?;
        validate_action_models(schema, schema.action(&call.name, call.version)?, &models)?;
        Ok(Self { call, models })
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(canonical_json(&serde_json::to_value(self)?)?.into_bytes())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DirectActionResponse {
    pub completion: CallCompletion,
    pub records: Vec<AuthorityRecord>,
}
impl DirectActionResponse {
    pub fn decode(bytes: &[u8], request: &DirectActionRequest, schema: &Schema) -> Result<Self> {
        if bytes.len() > crate::limits::PUSH_BYTES {
            return Err(invalid("direct Action response exceeds byte limit"));
        }
        let raw: Value = serde_json::from_slice(bytes)?;
        let rejection = if raw["completion"]["outcome"]["status"] == "failed" {
            vec![serde_json::json!({"ordinal":1,"code":raw["completion"]["outcome"]["code"]})]
        } else {
            vec![]
        };
        let wrapper = serde_json::json!({"clientId":"direct","batchSequence":1,"rejections":rejection,"completions":[raw["completion"]],"records":raw["records"]});
        let frozen = crate::PushRequest::decode_actions(serde_json::json!({"clientId":"direct","batchSequence":1,"models":request.models,"mutations":[{"ordinal":1,"callId":request.call.call_id,"name":request.call.name,"version":request.call.version,"args":request.call.args}]}).to_string().as_bytes(), schema)?;
        let receipt =
            crate::PushReceipt::decode_actions(wrapper.to_string().as_bytes(), &frozen, schema)?;
        Ok(Self {
            completion: receipt.completions.into_iter().next().unwrap(),
            records: receipt.records,
        })
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(canonical_json(&serde_json::to_value(self)?)?.into_bytes())
    }
}

pub fn normalize_call_id(value: &str) -> Result<String> {
    let id = uuid::Uuid::parse_str(value).map_err(|_| invalid("invalid callId UUID"))?;
    if value.len() != 36
        || id.get_variant() != uuid::Variant::RFC4122
        || !(1..=8).contains(&id.get_version_num())
    {
        return Err(invalid("invalid callId UUID format/version"));
    }
    Ok(id.to_string())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ActionOutcome {
    Succeeded {
        result: Value,
    },
    Failed {
        code: String,
        execution: ExecutionState,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionState {
    Rejected,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallCompletion {
    pub call_id: String,
    pub outcome: ActionOutcome,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionDescriptor {
    pub name: String,
    pub version: u64,
    pub inputs: Vec<ActionInputDescriptor>,
    pub outputs: Vec<ActionOutputDescriptor>,
    #[serde(default)]
    pub input: Option<ActionInputSnapshot>,
    #[serde(default, rename = "outputEnums")]
    pub output_enums: Vec<EnumDescriptor>,
    /// Retained policy metadata (bindings, prerequisites and sequence) stays
    /// intact when the client persists and reopens a schema.
    #[serde(flatten)]
    pub policy: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ActionInputSnapshot {
    #[serde(default)]
    pub models: Vec<ModelDescriptor>,
    #[serde(default)]
    pub enums: Vec<EnumDescriptor>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ActionInputDescriptor {
    Value {
        name: String,
        #[serde(rename = "type")]
        value_type: ValueType,
        nullable: bool,
        #[serde(default)]
        list: bool,
        #[serde(flatten)]
        metadata: BTreeMap<String, Value>,
    },
    Model {
        name: String,
        model: String,
        operation: String,
        cardinality: String,
        #[serde(default, rename = "allowedPatchFields")]
        allowed_patch_fields: Option<Vec<String>>,
        #[serde(flatten)]
        metadata: BTreeMap<String, Value>,
    },
}
impl ActionInputDescriptor {
    pub fn name(&self) -> &str {
        match self {
            Self::Value { name, .. } | Self::Model { name, .. } => name,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionOutputDescriptor {
    pub name: String,
    pub kind: String,
    pub cardinality: String,
    pub source: ActionOutputSource,
    #[serde(rename = "type", default)]
    pub value_type: Option<ValueType>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_read_version: Option<u64>,
    #[serde(default)]
    pub handler_type: Option<ActionIdentityType>,
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ActionOutputSource {
    Named(ActionNamedSource),
    InputIdentity {
        #[serde(rename = "inputIdentity")]
        input_identity: String,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActionNamedSource {
    HandlerValue,
    HandlerIdentity,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionIdentityType {
    pub kind: String,
    pub model: String,
    pub fields: Vec<ActionIdentityField>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionIdentityField {
    pub name: String,
    #[serde(rename = "type")]
    pub value_type: ValueType,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelReadDescriptor {
    pub name: String,
    pub version: u64,
    pub identity: Vec<String>,
    pub fields: Vec<FieldDescriptor>,
    #[serde(default)]
    pub enums: Vec<EnumDescriptor>,
}

impl Schema {
    pub fn action(&self, name: &str, version: u64) -> Result<&ActionDescriptor> {
        self.actions
            .iter()
            .find(|a| a.name == name && a.version == version)
            .ok_or_else(|| invalid(format!("unknown Action {name} v{version}")))
    }
    pub(crate) fn validate_actions(&self) -> Result<()> {
        let mut reads = BTreeSet::new();
        for model in &self.result_models {
            if !reads.insert((model.name.as_str(), model.version)) {
                return Err(invalid("duplicate result Model read contract"));
            }
            Schema::from_value(
                serde_json::json!({"models":[{"name":model.name,"version":model.version,"identity":model.identity,"fields":model.fields}],"enums":model.enums}),
            )?;
        }
        let mut seen = BTreeSet::new();
        for action in &self.actions {
            if action.name.is_empty()
                || action.version == 0
                || action.version > MAX_SAFE_INTEGER
                || !seen.insert((action.name.as_str(), action.version))
            {
                return Err(invalid("invalid or duplicate Action descriptor"));
            }
            let mut names = BTreeSet::new();
            let input_schema = input_schema(self, action)?;
            for input in &action.inputs {
                if input.name().is_empty() || !names.insert(input.name()) {
                    return Err(invalid("invalid Action input name"));
                }
                match input {
                    ActionInputDescriptor::Value {
                        value_type,
                        nullable,
                        list,
                        ..
                    } => {
                        if *list && *nullable {
                            return Err(invalid("Action lists cannot be nullable"));
                        }
                        input_schema.validate_action_type(value_type)?;
                    }
                    ActionInputDescriptor::Model {
                        model,
                        operation,
                        cardinality,
                        allowed_patch_fields,
                        ..
                    } => {
                        self.model(model)?;
                        let descriptor = input_schema.model(model)?;
                        if !["create", "update", "delete"].contains(&operation.as_str())
                            || !valid_cardinality(cardinality)
                        {
                            return Err(invalid("invalid Action Model input"));
                        }
                        if let Some(fields) = allowed_patch_fields {
                            let mut seen = BTreeSet::new();
                            if operation != "update"
                                || fields.iter().any(|name| {
                                    !seen.insert(name)
                                        || descriptor.identity.contains(name)
                                        || !descriptor
                                            .fields
                                            .iter()
                                            .any(|field| &field.name == name)
                                })
                            {
                                return Err(invalid("invalid Action allowed patch field"));
                            }
                        }
                    }
                }
            }
            names.clear();
            for output in &action.outputs {
                if output.name.is_empty()
                    || !names.insert(output.name.as_str())
                    || !valid_cardinality(&output.cardinality)
                {
                    return Err(invalid("invalid Action output"));
                }
                match output.kind.as_str() {
                    "value" => {
                        let ty = output
                            .value_type
                            .as_ref()
                            .ok_or_else(|| invalid("missing Action value type"))?;
                        let mut local = self.clone();
                        if !action.output_enums.is_empty() {
                            local.enums = action.output_enums.clone();
                        }
                        local.validate_action_type(ty)?;
                        if !matches!(
                            output.source,
                            ActionOutputSource::Named(ActionNamedSource::HandlerValue)
                        ) {
                            return Err(invalid("Action value output needs handlerValue source"));
                        }
                    }
                    "model" | "deleteIdentity" => {
                        let model = output
                            .model
                            .as_deref()
                            .ok_or_else(|| invalid("missing Action output Model"))?;
                        self.model(model)?;
                        if output.kind == "model" {
                            self.result_model(
                                model,
                                output
                                    .model_read_version
                                    .ok_or_else(|| invalid("missing Model read version"))?,
                            )?;
                        }
                        match &output.source {
                            ActionOutputSource::Named(ActionNamedSource::HandlerIdentity) => {
                                let handler = output.handler_type.as_ref().ok_or_else(|| {
                                    invalid("missing Action identity handler type")
                                })?;
                                let descriptor = self.model(model)?;
                                if handler.kind != "identity"
                                    || handler.model != model
                                    || handler.fields.len() != descriptor.identity.len()
                                    || handler.fields.iter().zip(&descriptor.identity).any(
                                        |(field, name)| {
                                            field.name != *name
                                                || descriptor
                                                    .fields
                                                    .iter()
                                                    .find(|f| &f.name == name)
                                                    .is_none_or(|f| {
                                                        serde_json::to_value(&f.value_type).ok()
                                                            != serde_json::to_value(
                                                                &field.value_type,
                                                            )
                                                            .ok()
                                                    })
                                        },
                                    )
                                {
                                    return Err(invalid("invalid Action identity handler type"));
                                }
                            }
                            ActionOutputSource::InputIdentity { input_identity: name } => {
                                if !action.inputs.iter().any(|input| matches!(input, ActionInputDescriptor::Model { name: input_name, model: input_model, .. } if input_name == name && input_model == model)) { return Err(invalid("Action output source does not name matching Model input")); }
                            }
                            _ => return Err(invalid("invalid Action Model output source")),
                        }
                    }
                    _ => return Err(invalid("invalid Action output kind")),
                }
            }
        }
        Ok(())
    }
    pub fn result_model(&self, name: &str, version: u64) -> Result<&ModelReadDescriptor> {
        self.result_models
            .iter()
            .find(|m| m.name == name && m.version == version)
            .ok_or_else(|| invalid(format!("unknown result Model {name} v{version}")))
    }
    pub(crate) fn validate_action_type(&self, ty: &ValueType) -> Result<()> {
        match ty {
            ValueType::Enum { name } if !self.enums.iter().any(|e| &e.name == name) => {
                Err(invalid("unknown Action enum"))
            }
            ValueType::List { .. } => Err(invalid(
                "Action value type uses cardinality instead of nested list",
            )),
            _ => Ok(()),
        }
    }
}
pub fn validate_action_models(
    schema: &Schema,
    action: &ActionDescriptor,
    models: &BTreeMap<String, u64>,
) -> Result<()> {
    for (name, version) in models {
        if schema.model(name)?.version != *version {
            return Err(invalid("unsupported local Model read version"));
        }
    }
    let required: BTreeSet<&str> = action
        .inputs
        .iter()
        .filter_map(|input| match input {
            ActionInputDescriptor::Model { model, .. } => Some(model.as_str()),
            _ => None,
        })
        .chain(
            action
                .outputs
                .iter()
                .filter_map(|output| output.model.as_deref()),
        )
        .collect();
    for name in required {
        if models.get(name) != Some(&schema.model(name)?.version) {
            return Err(invalid(format!(
                "Action needs local Model read contract {name}"
            )));
        }
    }
    Ok(())
}
fn valid_cardinality(s: &str) -> bool {
    ["single", "optional", "list"].contains(&s)
}
fn input_schema(schema: &Schema, action: &ActionDescriptor) -> Result<Schema> {
    if let Some(input) = &action.input {
        let mut result = schema.clone();
        result.models = input.models.clone();
        result.enums = input.enums.clone();
        Ok(result)
    } else {
        Ok(schema.clone())
    }
}
pub fn normalize_action_args(
    schema: &Schema,
    action: &ActionDescriptor,
    args: &Value,
) -> Result<Value> {
    let object = args
        .as_object()
        .ok_or_else(|| invalid("Action args must be an object"))?;
    if object
        .keys()
        .any(|key| !action.inputs.iter().any(|i| i.name() == key))
    {
        return Err(invalid("Action args contain undeclared input"));
    }
    let input_schema = input_schema(schema, action)?;
    let mut normalized = Map::new();
    for input in &action.inputs {
        let missing = Value::Null;
        let value = match object.get(input.name()) {
            Some(value) => value,
            None if matches!(input, ActionInputDescriptor::Model { cardinality, .. } if cardinality == "optional") => {
                &missing
            }
            None => return Err(invalid("Action required input missing")),
        };
        let result = match input {
            ActionInputDescriptor::Value {
                name,
                value_type,
                nullable,
                list,
                ..
            } => {
                let ty = if *list {
                    ValueType::List {
                        element: Box::new(value_type.clone()),
                    }
                } else {
                    value_type.clone()
                };
                input_schema.normalize_value(
                    &FieldDescriptor {
                        name: name.clone(),
                        value_type: ty,
                        nullable: *nullable,
                        default: None,
                    },
                    value,
                )?
            }
            ActionInputDescriptor::Model {
                model,
                operation,
                cardinality,
                allowed_patch_fields,
                ..
            } => normalize_cardinality(value, cardinality, |item| {
                normalize_model_input(
                    &input_schema,
                    model,
                    operation,
                    allowed_patch_fields.as_deref(),
                    item,
                )
            })?,
        };
        normalized.insert(input.name().to_string(), result);
    }
    Ok(Value::Object(normalized))
}
fn normalize_cardinality(
    value: &Value,
    cardinality: &str,
    one: impl Fn(&Value) -> Result<Value>,
) -> Result<Value> {
    match cardinality {
        "optional" if value.is_null() => Ok(Value::Null),
        "list" => Ok(Value::Array(
            value
                .as_array()
                .ok_or_else(|| invalid("expected Action list"))?
                .iter()
                .map(one)
                .collect::<Result<Vec<_>>>()?,
        )),
        "single" | "optional" => one(value),
        _ => Err(invalid("invalid Action cardinality")),
    }
}
fn normalize_model_input(
    schema: &Schema,
    model: &str,
    operation: &str,
    allowed: Option<&[String]>,
    value: &Value,
) -> Result<Value> {
    match operation {
        "create" => {
            let object = value
                .as_object()
                .ok_or_else(|| invalid("create input must be object"))?;
            let descriptor = schema.model(model)?;
            let identity: Map<String, Value> = descriptor
                .identity
                .iter()
                .filter_map(|k| object.get(k).map(|v| (k.clone(), v.clone())))
                .collect();
            let key = schema.record_key(model, &Value::Object(identity))?;
            let state = schema.normalize_state(model, value)?;
            let mut result = key.identity.as_object().unwrap().clone();
            result.extend(state.as_object().unwrap().clone());
            Ok(Value::Object(result))
        }
        "delete" => {
            let object = exact_object(value, &["identity"])?;
            Ok(
                serde_json::json!({"identity":schema.record_key(model, &object["identity"])?.identity}),
            )
        }
        "update" => {
            let object = exact_object(value, &["identity", "patch"])?;
            let identity = schema.record_key(model, &object["identity"])?.identity;
            let patch = schema.validate_patch(model, &object["patch"])?;
            if let Some(allowed) = allowed
                && patch
                    .as_object()
                    .unwrap()
                    .keys()
                    .any(|k| !allowed.contains(k))
            {
                return Err(invalid("disallowed Action patch field"));
            }
            Ok(serde_json::json!({"identity":identity,"patch":patch}))
        }
        _ => Err(invalid("invalid Action operation")),
    }
}
fn exact_object<'a>(value: &'a Value, keys: &[&str]) -> Result<&'a Map<String, Value>> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("expected Action object"))?;
    if object.len() != keys.len() || keys.iter().any(|k| !object.contains_key(*k)) {
        return Err(invalid("invalid Action object fields"));
    }
    Ok(object)
}
pub fn validate_action_result(
    schema: &Schema,
    action: &ActionDescriptor,
    result: &Value,
) -> Result<Value> {
    if action.outputs.is_empty() {
        return if result.is_null() {
            Ok(Value::Null)
        } else {
            Err(invalid("void Action result must be null"))
        };
    }
    let object = result
        .as_object()
        .ok_or_else(|| invalid("Action result must be object"))?;
    if object.len() != action.outputs.len()
        || object
            .keys()
            .any(|key| !action.outputs.iter().any(|o| &o.name == key))
    {
        return Err(invalid("Action result must contain exactly named outputs"));
    }
    let mut normalized = Map::new();
    let retained_input = input_schema(schema, action)?;
    for output in &action.outputs {
        let value =
            normalize_cardinality(
                &object[&output.name],
                &output.cardinality,
                |one| match output.kind.as_str() {
                    "value" => {
                        let mut local = schema.clone();
                        if !action.output_enums.is_empty() {
                            local.enums = action.output_enums.clone();
                        }
                        local.normalize_value(
                            &FieldDescriptor {
                                name: output.name.clone(),
                                value_type: output
                                    .value_type
                                    .clone()
                                    .ok_or_else(|| invalid("missing output type"))?,
                                nullable: false,
                                default: None,
                            },
                            one,
                        )
                    }
                    "model" => normalize_result_model(
                        schema,
                        output
                            .model
                            .as_deref()
                            .ok_or_else(|| invalid("missing output model"))?,
                        output
                            .model_read_version
                            .ok_or_else(|| invalid("missing output read version"))?,
                        one,
                    ),
                    "deleteIdentity" => Ok(retained_input
                        .record_key(
                            output
                                .model
                                .as_deref()
                                .ok_or_else(|| invalid("missing output model"))?,
                            one,
                        )?
                        .identity),
                    _ => Err(invalid("invalid output kind")),
                },
            )?;
        normalized.insert(output.name.clone(), value);
    }
    Ok(Value::Object(normalized))
}
/// Validate a saved result against the read contracts frozen for its call,
/// then add only fields introduced by a compatible local read-contract
/// evolution. This is separate from strict validation of a fresh result.
/// Server replay of an older saved response may use the same seam.
pub fn validate_action_result_after_read_upgrade(
    schema: &Schema,
    action: &ActionDescriptor,
    frozen_reads: &[ModelReadDescriptor],
    result: &Value,
) -> Result<Value> {
    let mut frozen_schema = schema.clone();
    frozen_schema.result_models = frozen_reads.to_vec();
    let mut projected = result.clone();
    if let Some(outputs) = projected.as_object_mut() {
        for output in &action.outputs {
            if output.kind != "model" {
                continue;
            }
            let model = output
                .model
                .as_deref()
                .ok_or_else(|| invalid("missing output model"))?;
            let version = output
                .model_read_version
                .ok_or_else(|| invalid("missing output read version"))?;
            let frozen = frozen_schema.result_model(model, version)?;
            let current = schema.result_model(model, version)?;
            let value = outputs
                .get_mut(&output.name)
                .ok_or_else(|| invalid("missing Action output"))?;
            match output.cardinality.as_str() {
                "list" => {
                    for item in value
                        .as_array_mut()
                        .ok_or_else(|| invalid("invalid Action result list"))?
                    {
                        project_to_frozen_read_fields(item, frozen, current)?;
                    }
                }
                "optional" if value.is_null() => {}
                "single" | "optional" => project_to_frozen_read_fields(value, frozen, current)?,
                _ => return Err(invalid("invalid Action result cardinality")),
            }
        }
    }
    validate_action_result(&frozen_schema, action, &projected)?;
    let mut expanded = result.clone();
    if let Some(outputs) = expanded.as_object_mut() {
        for output in &action.outputs {
            if output.kind != "model" {
                continue;
            }
            let model = output
                .model
                .as_deref()
                .ok_or_else(|| invalid("missing output model"))?;
            let version = output
                .model_read_version
                .ok_or_else(|| invalid("missing output read version"))?;
            let frozen = frozen_schema.result_model(model, version)?;
            let current = schema.result_model(model, version)?;
            let value = outputs
                .get_mut(&output.name)
                .ok_or_else(|| invalid("missing Action output"))?;
            match output.cardinality.as_str() {
                "list" => {
                    for item in value
                        .as_array_mut()
                        .ok_or_else(|| invalid("invalid Action result list"))?
                    {
                        add_new_read_fields(item, frozen, current)?;
                    }
                }
                "optional" if value.is_null() => {}
                "single" | "optional" => add_new_read_fields(value, frozen, current)?,
                _ => return Err(invalid("invalid Action result cardinality")),
            }
        }
    }
    validate_action_result(schema, action, &expanded)
}

fn project_to_frozen_read_fields(
    value: &mut Value,
    frozen: &ModelReadDescriptor,
    current: &ModelReadDescriptor,
) -> Result<()> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| invalid("Model result must be object"))?;
    if object
        .keys()
        .any(|name| !current.fields.iter().any(|field| &field.name == name))
    {
        return Err(invalid("Model result contains undeclared current field"));
    }
    object.retain(|name, _| frozen.fields.iter().any(|field| &field.name == name));
    Ok(())
}

fn add_new_read_fields(
    value: &mut Value,
    frozen: &ModelReadDescriptor,
    current: &ModelReadDescriptor,
) -> Result<()> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| invalid("Model result must be object"))?;
    for field in &current.fields {
        if frozen.fields.iter().any(|prior| prior.name == field.name)
            || object.contains_key(&field.name)
        {
            continue;
        }
        let added = if field.nullable {
            Value::Null
        } else {
            field
                .default
                .clone()
                .ok_or_else(|| invalid("new Model result field has no default"))?
        };
        object.insert(field.name.clone(), added);
    }
    Ok(())
}
fn normalize_result_model(
    schema: &Schema,
    model: &str,
    version: u64,
    value: &Value,
) -> Result<Value> {
    let descriptor = schema.result_model(model, version)?;
    let mut local = schema.clone();
    local.models = vec![ModelDescriptor {
        name: descriptor.name.clone(),
        version,
        identity: descriptor.identity.clone(),
        fields: descriptor.fields.clone(),
        relations: vec![],
        unique: vec![],
    }];
    local.enums = descriptor.enums.clone();
    let object = value
        .as_object()
        .ok_or_else(|| invalid("Model result must be object"))?;
    let identity: Map<String, Value> = descriptor
        .identity
        .iter()
        .filter_map(|k| object.get(k).map(|v| (k.clone(), v.clone())))
        .collect();
    let key = local.record_key(model, &Value::Object(identity))?;
    if object.len() != descriptor.fields.len()
        || object
            .keys()
            .any(|key| !descriptor.fields.iter().any(|f| &f.name == key))
    {
        return Err(invalid("Model result must contain exactly read fields"));
    }
    let state_fields: Map<String, Value> = object
        .iter()
        .filter(|(key, _)| !descriptor.identity.contains(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let state = local.validate_state(model, &Value::Object(state_fields))?;
    let mut result = key.identity.as_object().unwrap().clone();
    result.extend(state.as_object().unwrap().clone());
    Ok(Value::Object(result))
}
pub fn materialize_action_model(
    schema: &Schema,
    model: &str,
    version: u64,
    identity: &Value,
    state: &Value,
) -> Result<Value> {
    let descriptor = schema.result_model(model, version)?;
    let mut local = schema.clone();
    local.models = vec![ModelDescriptor {
        name: descriptor.name.clone(),
        version,
        identity: descriptor.identity.clone(),
        fields: descriptor.fields.clone(),
        relations: vec![],
        unique: vec![],
    }];
    local.enums = descriptor.enums.clone();
    let key = local.record_key(model, identity)?;
    let fields = state
        .as_object()
        .ok_or_else(|| invalid("Model result state must be object"))?;
    if fields.keys().any(|name| {
        descriptor.identity.contains(name)
            || !descriptor.fields.iter().any(|field| &field.name == name)
    }) {
        return Err(invalid("Model result state has unknown or identity field"));
    }
    let normalized = local.validate_state(model, state)?;
    let mut result = key.identity.as_object().unwrap().clone();
    result.extend(normalized.as_object().unwrap().clone());
    Ok(Value::Object(result))
}
