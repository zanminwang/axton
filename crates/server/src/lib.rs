//! Server protocol orchestration. Host calls run in the application's outer transaction.
pub mod error;
pub mod host;
pub mod live;
mod readback;
use axton_core::{
    AuthorityRecord, CursorRange, PullPage, PullRequest, PushReceipt, PushRequest, RecordKey,
    Rejection, Schema, limits, read_counter,
};
pub use error::{Error, code};
use host::{Acknowledged, Claimed, Handled, Head, HostExt, HostRequest, Invalidation, Loaded};
use readback::{Changes, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
};
pub type Result<T> = std::result::Result<T, Error>;
/// The host reports its own failures as text; the engine files them under the `host` code.
pub type HostResult<T> = std::result::Result<T, String>;
pub trait Host: Send + Sync {
    fn call(&self, request: Value) -> Pin<Box<dyn Future<Output = HostResult<Value>> + Send + '_>>;
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Config {
    pub schema: Schema,
    pub mutations: Vec<Mutation>,
    pub loaders: Vec<String>,
    /// Every retained model read contract, one per `(name, version)`. Absent
    /// in a hand-written config, in which case each model is retained at the
    /// schema's own version.
    #[serde(default)]
    pub models: Vec<ModelContract>,
}
/// One retained model read contract, as the compiler keeps it in
/// `history/models.json`: the record structure a loader of `version` returns
/// and the enums those fields use, as they were when the version was published.
#[derive(Clone, Deserialize, Serialize)]
pub struct ModelContract {
    pub name: String,
    pub version: u64,
    pub identity: Vec<String>,
    pub fields: Vec<axton_core::FieldDescriptor>,
    #[serde(default)]
    pub enums: Vec<axton_core::EnumDescriptor>,
    /// The contract as a one-model schema, for normalizing loader rows.
    #[serde(skip)]
    contract: Option<Schema>,
}
impl ModelContract {
    fn schema(&self) -> Schema {
        Schema {
            enums: self.enums.clone(),
            models: vec![axton_core::ModelDescriptor {
                name: self.name.clone(),
                version: self.version,
                identity: self.identity.clone(),
                fields: self.fields.clone(),
                relations: vec![],
                unique: vec![],
            }],
            requirements: vec![],
            prerequisites: vec![],
            client_policies: vec![],
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Mutation {
    pub name: String,
    pub version: u64,
    pub slots: Vec<Slot>,
    #[serde(default)]
    pub input: Option<Schema>,
    #[serde(default, rename = "knownFields")]
    pub known_fields: BTreeMap<String, Vec<String>>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Slot {
    pub name: String,
    pub model: String,
    pub operation: String,
    pub cardinality: String,
    #[serde(default)]
    pub allowed_patch_fields: Vec<String>,
    #[serde(default)]
    pub bindings: Vec<Binding>,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Binding {
    pub fields: Vec<String>,
    pub slot: String,
}
impl Config {
    pub fn decode(value: Value) -> Result<Self> {
        let c: Self = serde_json::from_value(value).map_err(config_invalid)?;
        c.schema.validate().map_err(config_invalid)?;
        let mut versions = BTreeSet::new();
        for m in &c.mutations {
            if m.name.is_empty()
                || read_counter(&json!(m.version), true).is_err()
                || !versions.insert((&m.name, m.version))
            {
                return Err(Error::new(
                    code::CONFIG_INVALID,
                    "invalid mutation descriptor",
                ));
            }
            let mut slots = BTreeSet::new();
            let schema = m.input.as_ref().unwrap_or(&c.schema);
            schema.validate().map_err(config_invalid)?;
            for s in &m.slots {
                let model = schema.model(&s.model).map_err(config_invalid)?;
                let mut capabilities = BTreeSet::new();
                if s.operation != "update" && !s.allowed_patch_fields.is_empty() {
                    return Err(Error::new(
                        code::CONFIG_INVALID,
                        "patch capabilities require update operation",
                    ));
                }
                for field in &s.allowed_patch_fields {
                    if !capabilities.insert(field)
                        || model.identity.contains(field)
                        || !model.fields.iter().any(|f| f.name == *field)
                    {
                        return Err(Error::new(code::CONFIG_INVALID, "invalid patch capability"));
                    }
                }

                if !slots.insert(&s.name)
                    || !["single", "optional", "list"].contains(&s.cardinality.as_str())
                    || !["create", "update", "delete"].contains(&s.operation.as_str())
                {
                    return Err(Error::new(code::CONFIG_INVALID, "invalid slot descriptor"));
                }
            }
        }
        for m in &c.mutations {
            let schema = m.input.as_ref().unwrap_or(&c.schema);
            for slot in &m.slots {
                for binding in &slot.bindings {
                    let parent =
                        m.slots
                            .iter()
                            .find(|s| s.name == binding.slot)
                            .ok_or_else(|| {
                                Error::new(code::CONFIG_INVALID, "binding target missing")
                            })?;
                    let parent_model = schema.model(&parent.model).map_err(config_invalid)?;
                    let child = schema.model(&slot.model).map_err(config_invalid)?;
                    if parent.cardinality != "single"
                        || binding.fields.len() != parent_model.identity.len()
                        || binding
                            .fields
                            .iter()
                            .any(|n| !child.fields.iter().any(|f| f.name == *n))
                    {
                        return Err(Error::new(
                            code::CONFIG_INVALID,
                            "invalid binding descriptor",
                        ));
                    }
                }
            }
        }
        for loader in &c.loaders {
            c.schema.model(loader).map_err(config_invalid)?;
        }
        let mut c = c;
        if c.models.is_empty() {
            c.models = c
                .schema
                .models
                .iter()
                .map(|m| ModelContract {
                    name: m.name.clone(),
                    version: m.version,
                    identity: m.identity.clone(),
                    fields: m.fields.clone(),
                    enums: c
                        .schema
                        .enums
                        .iter()
                        .filter(|e| {
                            m.fields.iter().any(|f| {
                                matches!(&f.value_type, axton_core::ValueType::Enum { name } if *name == e.name)
                            })
                        })
                        .cloned()
                        .collect(),
                    contract: None,
                })
                .collect();
        }
        let mut retained = BTreeSet::new();
        for contract in &mut c.models {
            let current = c.schema.model(&contract.name).map_err(config_invalid)?;
            if read_counter(&json!(contract.version), true).is_err()
                || !retained.insert((contract.name.clone(), contract.version))
                || contract.identity != current.identity
            {
                return Err(Error::new(
                    code::CONFIG_INVALID,
                    format!(
                        "invalid model contract {} v{}",
                        contract.name, contract.version
                    ),
                ));
            }
            let schema = contract.schema();
            schema.validate().map_err(config_invalid)?;
            contract.contract = Some(schema);
        }
        for model in &c.schema.models {
            if !retained.contains(&(model.name.clone(), model.version)) {
                return Err(Error::new(
                    code::CONFIG_INVALID,
                    format!(
                        "model {} v{} is not a retained contract",
                        model.name, model.version
                    ),
                ));
            }
        }
        Ok(c)
    }
    /// Check a client's declared read contracts: every declared model must
    /// exist and every declared version must be retained. Nothing is inferred
    /// for a model the client did not declare; a page holding one is refused
    /// by [`process_pull`] with the same code.
    pub fn check_declared(&self, models: &BTreeMap<String, u64>) -> Result<()> {
        for (name, version) in models {
            if self.schema.model(name).is_err() {
                return Err(Error::new(
                    code::MODEL_VERSION_UNSUPPORTED,
                    format!("model {name} is not served by this backend"),
                )
                .with_details(json!({"model":name,"version":version})));
            }
            if self.contract(name, *version).is_none() {
                return Err(Error::new(
                    code::MODEL_VERSION_UNSUPPORTED,
                    format!("model {name} v{version} is not a retained read contract"),
                )
                .with_details(json!({"model":name,"version":version})));
            }
        }
        Ok(())
    }
    /// The read contract a loader of `version` serves for `model`, or `None`
    /// when that version is not retained.
    pub fn contract(&self, model: &str, version: u64) -> Option<&Schema> {
        self.models
            .iter()
            .find(|m| m.name == model && m.version == version)
            .and_then(|m| m.contract.as_ref())
    }
    fn descriptor(&self, body: &Value) -> Result<&Mutation> {
        let name = body["name"]
            .as_str()
            .ok_or_else(|| Error::code("mutation.invalid"))?;
        let version = version(body).ok_or_else(|| Error::code("mutation.invalid"))?;
        self.mutations
            .iter()
            .find(|m| m.name == name && m.version == version)
            .ok_or_else(|| Error::code("mutation.invalid"))
    }
}
fn config_invalid(e: impl std::fmt::Display) -> Error {
    Error::new(code::CONFIG_INVALID, e.to_string())
}
fn internal(e: impl std::fmt::Display) -> Error {
    Error::new(code::INTERNAL, e.to_string())
}
fn storage_invalid(e: impl std::fmt::Display) -> Error {
    Error::new(code::STORAGE_INVALID, e.to_string())
}
fn request_invalid(e: impl std::fmt::Display) -> Error {
    Error::new(code::REQUEST_INVALID, e.to_string())
}
fn version(body: &Value) -> Option<u64> {
    read_counter(body.get("version").unwrap_or(&json!(1)), true).ok()
}
fn invalid<T>(r: axton_core::Result<T>) -> Result<T> {
    r.map_err(|_| Error::code("mutation.invalid"))
}
pub fn decode_arguments(config: &Value, body: &Value) -> Result<Value> {
    decode(&Config::decode(config.clone())?, body).map(|(args, _)| args)
}
/// The decoded slot arguments and the records the uploaded operations target,
/// in operation order: the seed of the mutation's change set.
fn decode(c: &Config, body: &Value) -> Result<(Value, Vec<RecordKey>)> {
    let d = c.descriptor(body)?;
    let schema = d.input.as_ref().unwrap_or(&c.schema);
    let ops = body["operations"]
        .as_array()
        .ok_or_else(|| Error::code("mutation.invalid"))?;
    let mut at = 0;
    let mut args = Map::new();
    let mut targets = vec![];
    for slot in &d.slots {
        let mut values = vec![];
        while at < ops.len() && ops[at]["model"] == slot.model && ops[at]["op"] == slot.operation {
            let op = &ops[at];
            let model = invalid(schema.model(&slot.model))?;
            let source = op["identity"]
                .as_object()
                .ok_or_else(|| Error::code("mutation.invalid"))?;
            let identity = Value::Object(
                source
                    .iter()
                    .filter(|(k, _)| model.identity.contains(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            );
            let key = invalid(schema.record_key(&slot.model, &identity))?;
            let mut argument = json!({"identity":key.identity});
            targets.push(key.clone());
            if slot.operation != "delete" {
                let source = op["values"]
                    .as_object()
                    .ok_or_else(|| Error::code("mutation.invalid"))?;
                let mut data = Map::new();
                if slot.operation == "create" {
                    for field in model
                        .fields
                        .iter()
                        .filter(|f| !model.identity.contains(&f.name))
                    {
                        let v = source
                            .get(&field.name)
                            .or({
                                if field.nullable {
                                    Some(&Value::Null)
                                } else {
                                    None
                                }
                            })
                            .ok_or_else(|| Error::code("mutation.invalid"))?;
                        data.insert(
                            field.name.clone(),
                            invalid(schema.normalize_value(field, v))?,
                        );
                    }
                    argument["data"] = Value::Object(data);
                } else {
                    for (name, v) in source {
                        let known = d
                            .known_fields
                            .get(&slot.model)
                            .map(|names| names.contains(name))
                            .unwrap_or_else(|| model.fields.iter().any(|f| f.name == *name));
                        if known && !slot.allowed_patch_fields.contains(name) {
                            return Err(Error::code(format!(
                                "{}.not_allowed",
                                machine_name(&d.name)
                            )));
                        }
                        if let Some(field) = model.fields.iter().find(|f| f.name == *name) {
                            if !slot.allowed_patch_fields.contains(name) {
                                return Err(Error::code(format!(
                                    "{}.not_allowed",
                                    machine_name(&d.name)
                                )));
                            }
                            data.insert(name.clone(), invalid(schema.normalize_value(field, v))?);
                        }
                    }
                    argument["patch"] = Value::Object(data);
                }
            }
            values.push(argument);
            at += 1;
            if slot.cardinality != "list" {
                break;
            }
        }
        let value = if slot.cardinality == "list" {
            Value::Array(values)
        } else if values.len() == 1 {
            values.remove(0)
        } else if slot.cardinality == "optional" {
            Value::Null
        } else {
            return Err(Error::code("mutation.invalid"));
        };
        args.insert(slot.name.clone(), value);
    }
    if at != ops.len() {
        return Err(Error::code("mutation.invalid"));
    }
    for slot in d.slots.iter().filter(|s| s.operation == "create") {
        for binding in &slot.bindings {
            let parent = d
                .slots
                .iter()
                .find(|s| s.name == binding.slot)
                .ok_or_else(|| Error::code("mutation.invalid"))?;
            let parent_model = schema.model(&parent.model).map_err(config_invalid)?;
            let rows = if slot.cardinality == "list" {
                args[&slot.name]
                    .as_array()
                    .ok_or_else(|| Error::code("mutation.invalid"))?
                    .clone()
            } else if args[&slot.name].is_null() {
                vec![]
            } else {
                vec![args[&slot.name].clone()]
            };
            for row in rows {
                for (field, id) in binding.fields.iter().zip(&parent_model.identity) {
                    let actual = row["identity"].get(field).unwrap_or(&row["data"][field]);
                    if actual != &args[&parent.name]["identity"][id] {
                        return Err(Error::code(format!("{}.invalid", machine_name(&d.name))));
                    }
                }
            }
        }
    }
    Ok((Value::Object(args), targets))
}
fn machine_name(name: &str) -> String {
    let mut s = String::new();
    for ch in name.chars() {
        if ch.is_ascii_uppercase() {
            if !s.is_empty() {
                s.push('_')
            }
            s.push(ch.to_ascii_lowercase())
        } else {
            s.push(ch)
        }
    }
    s
}
pub(crate) fn valid_code(s: &str) -> bool {
    let mut parts = s.split(['.', '_', '-']);
    let first = parts.next().unwrap_or("");
    first.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && first
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && parts.all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}
fn principal(owner: &str) -> Result<()> {
    if owner.trim().is_empty() {
        Err(Error::new(code::PRINCIPAL_INVALID, "invalid principal"))
    } else {
        Ok(())
    }
}
async fn head(host: &impl Host, channel: &str) -> Result<u64> {
    let Head(cursor) = host
        .call_typed(HostRequest::Head {
            channel: channel.into(),
        })
        .await?;
    Ok(cursor)
}
/// Process one push: every mutation runs in its own savepoint, its changed
/// records are stamped and read back by the loaders in that savepoint, and
/// the receipt carries the final authority of every record a successful
/// mutation changed. An unsupported mutation version, a handler failure, a
/// loader failure and an undeclared or unretained model read contract each
/// reject only the mutation they belong to; the rest of the batch stands.
/// The receipt is stored before the outer transaction commits, so a retry
/// answers from storage without running a handler.
pub async fn process_push(
    config: &Config,
    owner: &str,
    bytes: &[u8],
    host: &impl Host,
) -> Result<String> {
    principal(owner)?;
    let request = PushRequest::decode(bytes).map_err(request_invalid)?;
    let locked: Claimed = host
        .call_typed(HostRequest::Claim {
            owner: owner.into(),
            client_id: request.client_id.clone(),
        })
        .await?;
    if locked.client_id != request.client_id {
        return Err(storage_invalid("storage client mismatch"));
    }
    if locked.owner != owner {
        return Err(Error::code(code::OWNER_MISMATCH));
    }
    let last = locked.sequence;
    if request.batch_sequence == last {
        return locked
            .receipt
            .ok_or_else(|| storage_invalid("receipt missing"));
    }
    if request.batch_sequence < last {
        return Err(Error::code(code::OVERLAP));
    }
    if request.batch_sequence != last + 1 {
        return Err(Error::code(code::GAP));
    }
    let mut rejections = vec![];
    // The last successful authority per record, in canonical key order.
    let mut results: BTreeMap<String, axton_core::AuthorityRecord> = BTreeMap::new();
    for m in &request.mutations {
        // A mutation naming a version this backend does not serve rejects
        // only itself; its handler never runs.
        if let (Some(name), Some(v)) = (m.raw["name"].as_str(), version(&m.raw))
            && config.mutations.iter().any(|d| d.name == name)
            && !config
                .mutations
                .iter()
                .any(|d| d.name == name && d.version == v)
        {
            rejections.push(Rejection {
                ordinal: m.ordinal,
                code: code::MUTATION_VERSION_UNSUPPORTED.into(),
            });
            continue;
        }
        // `decode` resolves the same descriptor first, so both refuse together.
        let (name, mutation_version) = match config.descriptor(&m.raw) {
            Ok(d) => (d.name.clone(), d.version),
            Err(refused) => {
                rejections.push(Rejection {
                    ordinal: m.ordinal,
                    code: refused.code,
                });
                continue;
            }
        };
        let (args, targets) = match decode(config, &m.raw) {
            Ok(decoded) => decoded,
            Err(refused) => {
                rejections.push(Rejection {
                    ordinal: m.ordinal,
                    code: refused.code,
                });
                continue;
            }
        };
        let Acknowledged = host
            .call_typed(HostRequest::Savepoint { ordinal: m.ordinal })
            .await?;
        // Host returns only explicit refusal as data; every thrown error aborts the outer transaction.
        let settlement: Handled = host
            .call_typed(HostRequest::Handle {
                name,
                version: mutation_version,
                arguments: args,
                owner: owner.into(),
                ordinal: m.ordinal,
            })
            .await?;
        let outcome = match settlement {
            Handled::Rejected { rejection } => Outcome::Refused(rejection),
            // A thrown handler error rejects only this mutation; it never
            // reaches the caller as a business rejection.
            Handled::Failed { .. } => Outcome::Refused(code::HANDLER_FAILED.into()),
            Handled::Settled {
                changes,
                publications,
            } => {
                let mut set = Changes::new();
                for key in targets {
                    readback::insert(&mut set, key)?;
                }
                for record in &changes {
                    readback::insert(&mut set, readback::resolve(config, record)?)?;
                }
                readback::read_back(config, &request.models, owner, &set, &publications, host)
                    .await?
            }
        };
        match outcome {
            Outcome::Refused(code) => {
                let Acknowledged = host
                    .call_typed(HostRequest::Rollback { ordinal: m.ordinal })
                    .await?;
                rejections.push(Rejection {
                    ordinal: m.ordinal,
                    code,
                });
            }
            Outcome::Records(records) => {
                for record in records {
                    let key = config
                        .schema
                        .record_key(&record.model, &record.identity)
                        .map_err(internal)?;
                    results.insert(key.encoded().map_err(internal)?, record);
                }
            }
        }
        let Acknowledged = host
            .call_typed(HostRequest::Release { ordinal: m.ordinal })
            .await?;
    }
    let receipt = PushReceipt {
        client_id: request.client_id.clone(),
        batch_sequence: request.batch_sequence,
        rejections,
        records: results.into_values().collect(),
    };
    let text = String::from_utf8(receipt.encode().map_err(internal)?).map_err(internal)?;
    let Acknowledged = host
        .call_typed(HostRequest::SaveReceipt {
            owner: owner.into(),
            client_id: request.client_id.clone(),
            sequence: request.batch_sequence,
            receipt: text.clone(),
        })
        .await?;
    Ok(text)
}
/// Serve one pull for every channel it names: scan each channel after its
/// cursor, load every changed record once at the version the client
/// declared, and answer a page whose changes are receipt-shaped records. A
/// record one loader call cannot read fails alone: the call is retried one
/// identity at a time and the failing identities become `error` changes.
pub async fn process_pull(
    config: &Config,
    owner: &str,
    bytes: &[u8],
    host: &impl Host,
) -> Result<String> {
    principal(owner)?;
    let request = PullRequest::decode(bytes).map_err(request_invalid)?;
    config.check_declared(&request.models)?;
    let mut cursors: BTreeMap<String, CursorRange> = BTreeMap::new();
    // Every changed record once, keyed canonically, at its current stamp.
    let mut records: BTreeMap<String, (RecordKey, u64)> = BTreeMap::new();
    for (channel, from) in &request.cursors {
        let maximum = head(host, channel).await?;
        if *from > maximum {
            return Err(request_invalid(format!(
                "cursor ahead of head on {channel}"
            )));
        }
        let rows: Vec<Invalidation> = host
            .call_typed(HostRequest::Scan {
                channel: channel.clone(),
                after: *from,
                limit: limits::PULL_CHANGES as u64,
            })
            .await?;
        if rows.len() > limits::PULL_CHANGES {
            return Err(storage_invalid("invalid scan size"));
        }
        let mut previous = *from;
        for row in &rows {
            if row.channel != *channel || row.cursor <= previous || row.cursor > maximum {
                return Err(storage_invalid("invalid invalidation order"));
            }
            previous = row.cursor;
            if !config.loaders.contains(&row.model) {
                return Err(Error::new(code::LOADER_UNREGISTERED, "unregistered loader"));
            }
            let key = config
                .schema
                .record_key(&row.model, &row.identity)
                .map_err(storage_invalid)?;
            if row.identity_key != key.encoded_identity().map_err(storage_invalid)? {
                return Err(storage_invalid("noncanonical identity"));
            }
            let encoded = key.encoded().map_err(internal)?;
            let entry = records.entry(encoded).or_insert((key, row.stamp));
            entry.1 = entry.1.max(row.stamp);
        }
        let to = if rows.len() == limits::PULL_CHANGES {
            previous
        } else {
            maximum
        };
        cursors.insert(
            channel.clone(),
            CursorRange {
                from: *from,
                to,
                head: maximum,
            },
        );
    }
    // Loaders read every changed record grouped by model, at the declared
    // version; a model the client did not declare is not in its read contract.
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (encoded, (key, _)) in &records {
        groups
            .entry(key.model.clone())
            .or_default()
            .push(encoded.clone());
    }
    let mut states: BTreeMap<String, std::result::Result<Value, String>> = BTreeMap::new();
    for (model, encoded_keys) in groups {
        let version = *request.models.get(&model).ok_or_else(|| {
            Error::new(
                code::MODEL_VERSION_UNSUPPORTED,
                format!("model {model} is not declared by the client"),
            )
            .with_details(json!({"model":model}))
        })?;
        let contract = config
            .contract(&model, version)
            .ok_or_else(|| internal(format!("model {model} v{version} is not retained")))?;
        let identities: Vec<Value> = encoded_keys
            .iter()
            .map(|k| records[k].0.identity.clone())
            .collect();
        let loaded: Loaded = host
            .call_typed(HostRequest::Load {
                model: model.clone(),
                version,
                identities: identities.clone(),
                owner: owner.into(),
            })
            .await?;
        // A record whose read fails in any way fails alone: a refusal or a
        // thrown error answered as data, a row the served contract does not
        // accept, or a batched answer that cannot be matched to its records.
        let normalize = |state: Option<Value>| -> std::result::Result<Value, String> {
            match state {
                None => Ok(Value::Null),
                Some(state) => contract
                    .normalize_state(&model, &state)
                    .map_err(|_| code::LOADER_INVALID.to_string()),
            }
        };
        let outcome: Vec<std::result::Result<Value, String>> = match loaded {
            Loaded::Rows(rows) if rows.len() == encoded_keys.len() => {
                rows.into_iter().map(normalize).collect()
            }
            Loaded::Rows(_) if encoded_keys.len() == 1 => {
                vec![Err(code::LOADER_INVALID.to_string())]
            }
            refused @ (Loaded::Refused { .. } | Loaded::Failed { .. })
                if encoded_keys.len() == 1 =>
            {
                vec![Err(refusal_code(refused))]
            }
            _ => {
                // One call for many records could not say which record failed:
                // ask for each on its own so the others still get their rows.
                let mut each = Vec::with_capacity(encoded_keys.len());
                for identity in &identities {
                    let one: Loaded = host
                        .call_typed(HostRequest::Load {
                            model: model.clone(),
                            version,
                            identities: vec![identity.clone()],
                            owner: owner.into(),
                        })
                        .await?;
                    each.push(match one {
                        Loaded::Rows(mut rows) if rows.len() == 1 => normalize(rows.remove(0)),
                        Loaded::Rows(_) => Err(code::LOADER_INVALID.to_string()),
                        refused => Err(refusal_code(refused)),
                    });
                }
                each
            }
        };
        for (encoded, state) in encoded_keys.iter().zip(outcome) {
            states.insert(encoded.clone(), state);
        }
    }
    let changes = records
        .iter()
        .map(|(encoded, (key, stamp))| {
            let (state, error) = match states.remove(encoded) {
                Some(Ok(state)) => (state, None),
                Some(Err(code)) => (Value::Null, Some(code)),
                None => (Value::Null, None),
            };
            AuthorityRecord {
                model: key.model.clone(),
                identity: key.identity.clone(),
                stamp: *stamp,
                state,
                error,
            }
        })
        .collect();
    let page = PullPage { cursors, changes };
    String::from_utf8(page.encode().map_err(internal)?).map_err(internal)
}
/// The code a refused or failed load contributes to a record's `error`.
fn refusal_code(loaded: Loaded) -> String {
    match loaded {
        Loaded::Refused { rejection } => rejection,
        Loaded::Failed { .. } | Loaded::Rows(_) => code::LOADER_FAILED.into(),
    }
}
/// Settle a business change made outside a handler, in the application's
/// transaction: the same `{changes, publications}` shape a handler answers
/// with. Every changed record gets its next stamp and the publications go
/// out at those stamps; nothing is read back, since no client is waiting for
/// a receipt. Answers `[{model, identity, stamp}]` for the changed records.
pub async fn settle_external(
    config: &Config,
    settlement: &Value,
    host: &impl Host,
) -> Result<Value> {
    let settled: Handled = serde_json::from_value(settlement.clone())
        .map_err(|e| Error::new(code::PUBLISH_INVALID, e.to_string()))?;
    let Handled::Settled {
        changes,
        publications,
    } = settled
    else {
        return Err(Error::new(
            code::PUBLISH_INVALID,
            "an external settlement carries changes and publications",
        ));
    };
    let mut set = Changes::new();
    for record in &changes {
        readback::insert(&mut set, readback::resolve(config, record)?)?;
    }
    let stamps = readback::allocate_stamps(&set, host).await?;
    readback::publish_intents(config, &set, &stamps, &publications, host).await?;
    Ok(Value::Array(
        set.iter()
            .map(|(encoded, key)| {
                json!({"model": key.model, "identity": key.identity, "stamp": stamps[encoded]})
            })
            .collect(),
    ))
}
