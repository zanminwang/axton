//! Sync carrier admission and publication adapter. Envelopes belong to axton_protocols::wire.
use crate::{
    Config, Host, HostResult, Result, backend_interface::HostExt, internal, request_invalid,
};
use axton_core::{ActionInputDescriptor, CallKind};
use axton_protocols::server_bridge::{HostRequest, Loaded};
use axton_protocols::wire;
use serde_json::{Value, json};
use std::{future::Future, pin::Pin};

pub use axton_protocols::server_bridge::HandlerContext;
pub(crate) fn handler_context(owner: &str, context: &wire::RequestContext) -> HandlerContext {
    HandlerContext {
        owner: owner.into(),
        stream: context.stream.clone(),
        store_id: context.store_id.clone(),
        materialization: context.materialization.clone(),
    }
}

pub fn validate_mutation_batch(config: &Config, bytes: &[u8]) -> Result<String> {
    let request: wire::MutationRequest = wire::decode(bytes).map_err(request_invalid)?;
    for mutation in &request.mutations {
        validate_member(config, mutation)?;
    }
    String::from_utf8(wire::encode(&request).map_err(request_invalid)?).map_err(request_invalid)
}
pub(crate) enum MutationInput {
    Supported(Value),
    Unsupported,
}
pub(crate) fn validate_member(config: &Config, m: &wire::Mutation) -> Result<MutationInput> {
    let Some(action) = config
        .schema
        .actions
        .iter()
        .find(|action| action.name == m.name && action.version == m.version)
    else {
        return Ok(MutationInput::Unsupported);
    };
    if action.kind != CallKind::Mutation {
        return Err(request_invalid("expected Mutation"));
    }
    // The artifact fingerprint is immutable Batch intent, not today's schema authority.
    // Trusted retained name/version normalization permits compatible descriptor evolution.
    for op in &m.operations {
        let slot = op.input_path.split('[').next().unwrap_or_default();
        let input = action
            .inputs
            .iter()
            .find(|i| i.name() == slot)
            .ok_or_else(|| request_invalid("unknown input path"))?;
        match input {
            ActionInputDescriptor::Value { .. } => {
                if op.operation != wire::Operation::Argument {
                    return Err(request_invalid("value input must be argument"));
                }
            }
            ActionInputDescriptor::Model {
                model, operation, ..
            } => {
                if op.operation == wire::Operation::Argument {
                    if !op.value.is_null() && op.value != json!([]) {
                        return Err(request_invalid("Model argument must be null or empty list"));
                    }
                } else if op.model.as_ref() != Some(model)
                    || serde_json::to_value(&op.operation).map_err(internal)? != json!(operation)
                {
                    return Err(request_invalid("Model slot operation mismatch"));
                }
            }
        }
    }
    let args = wire::reconstruct_input(&m.operations).map_err(request_invalid)?;
    let normalized = axton_core::normalize_action_args(&config.schema, action, &args)
        .map_err(request_invalid)?;
    Ok(MutationInput::Supported(normalized))
}
/// Adapts existing normalization/Loader machinery to the sync persistence seam.
/// Every nested Loader publication uses the same transaction's reservations.
pub(crate) struct PublicationHost<'a, H: Host>(pub &'a H, std::sync::Mutex<Option<String>>);
impl<'a, H: Host> PublicationHost<'a, H> {
    pub(crate) fn new(host: &'a H) -> Self {
        Self(host, std::sync::Mutex::new(None))
    }
    pub(crate) fn refusal(&self) -> Option<String> {
        self.1.lock().ok().and_then(|mut saved| saved.take())
    }
}
impl<H: Host> Host for PublicationHost<'_, H> {
    fn call(
        &self,
        mut request: Value,
    ) -> Pin<Box<dyn Future<Output = HostResult<Value>> + Send + '_>> {
        Box::pin(async move {
            let loader = request["op"] == "load";
            if matches!(
                request["op"].as_str(),
                Some("readTracking" | "readPositions" | "guardRecords" | "applyStreamMembers")
            ) {
                request = json!({"op":"sync","request":request});
            }
            let answer = self.0.call(request).await?;
            if loader
                && let Ok(Loaded::Refused { rejection }) = serde_json::from_value(answer.clone())
            {
                *self
                    .1
                    .lock()
                    .map_err(|_| "refusal tracker poisoned".to_string())? = Some(rejection);
            }
            Ok(answer)
        })
    }
}
pub(crate) async fn call<T: serde::de::DeserializeOwned + Send>(
    host: &impl Host,
    request: Value,
) -> Result<T> {
    host.call_typed(HostRequest::Sync {
        request: serde_json::from_value(request).map_err(internal)?,
    })
    .await
}
pub async fn process_batch_member(
    config: &Config,
    owner: &str,
    bytes: &[u8],
    ordinal: u64,
    host: &impl Host,
) -> Result<String> {
    let request: wire::MutationRequest =
        wire::decode(validate_mutation_batch(config, bytes)?.as_bytes())
            .map_err(request_invalid)?;
    let result = crate::mutation_batch::execute(config, owner, &request, ordinal, host).await?;
    axton_core::canonical_json(&serde_json::to_value(result).map_err(internal)?).map_err(internal)
}
/// Experimental bounded commit group. The caller owns one outer transaction.
/// Every member retains its own savepoint, outcome and publication cursor scope.
pub async fn process_batch_group(
    config: &Config,
    owner: &str,
    bytes: &[u8],
    start: u64,
    count: u64,
    host: &impl Host,
) -> Result<Vec<String>> {
    let request: wire::MutationRequest = wire::decode(bytes).map_err(request_invalid)?;
    let end = start
        .checked_add(count)
        .ok_or_else(|| request_invalid("group overflow"))?;
    if count == 0 || count > 32 || end > request.mutations.len() as u64 {
        return Err(request_invalid("invalid Mutation group range"));
    }
    for mutation in &request.mutations {
        validate_member(config, mutation)?;
    }
    let mut store = crate::mutation_batch::Store::claim(owner, &request, host).await?;
    let mut results = Vec::new();
    let mut pending = Vec::new();
    let mut first = None;
    for ordinal in start..end {
        let fresh = !store.saved(&request, ordinal);
        let scope = format!(
            "{}:{}:{}",
            request.context.store_id, request.batch_id, request.mutations[ordinal as usize].id
        );
        let scoped = MemberHost { host, scope };
        let result = crate::mutation_batch::execute_claimed(
            config, owner, &request, ordinal, &scoped, &mut store, false,
        )
        .await?;
        results.push(
            axton_core::canonical_json(&serde_json::to_value(&result).map_err(internal)?)
                .map_err(internal)?,
        );
        if fresh {
            first.get_or_insert(ordinal);
            pending.push(result);
        }
    }
    if let Some(ordinal) = first {
        let _: axton_protocols::server_bridge::Acknowledged = call(host, json!({
            "op":"saveResults", "storeId":request.context.store_id, "batchId":request.batch_id,
            "ordinal":ordinal, "count":request.mutations.len(), "digest":request.digest, "results":pending,
        })).await?;
    }
    Ok(results)
}
struct MemberHost<'a, H: Host> {
    host: &'a H,
    scope: String,
}
impl<H: Host> Host for MemberHost<'_, H> {
    fn call(
        &self,
        mut request: Value,
    ) -> Pin<Box<dyn Future<Output = HostResult<Value>> + Send + '_>> {
        Box::pin(async move {
            if request["op"] == "sync" && request["request"]["op"] == "applyStreamMembers" {
                request["request"]["publicationScope"] = json!(self.scope);
            }
            self.host.call(request).await
        })
    }
}
pub fn encode_batch_acknowledgement(bytes: &[u8], results: &[String]) -> Result<String> {
    let request: wire::MutationRequest = wire::decode(bytes).map_err(request_invalid)?;
    let ack = wire::BatchAcknowledgement {
        context: request.context.clone(),
        batch_id: request.batch_id,
        digest: request.digest.clone(),
        results: results
            .iter()
            .map(|s| serde_json::from_str(s).map_err(crate::storage_invalid))
            .collect::<Result<_>>()?,
    };
    // The server keyed every accepted member's input under its schema.
    wire::validate_acknowledgement(&request, &ack, |_, _| true).map_err(crate::storage_invalid)?;
    String::from_utf8(wire::encode(&ack).map_err(internal)?).map_err(internal)
}
