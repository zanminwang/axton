//! Per-member execution with a business savepoint. A commit group may share its
//! Store lock and persist a contiguous outcome suffix in the outer transaction.
use crate::{
    Config, Error, Host, Result,
    backend_interface::HostExt,
    batch_entry::{self, PublicationHost},
    internal, request_invalid, storage_invalid,
};
use axton_core::{ActionInputDescriptor, RecordKey};
use axton_protocols::server_bridge::members::{MemberPosition, PositionKind};
use axton_protocols::server_bridge::{
    Acknowledged, HandledAction, HostRequest, Loaded, LoaderMode, MemberKey,
};
use axton_protocols::wire;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Store {
    principal: String,
    stream: String,
    last_processed_batch_id: u64,
    progress: u64,
    current_digest: Option<String>,
    current_count: Option<u64>,
    last_digest: Option<String>,
    last_count: Option<u64>,
}
impl Store {
    pub(crate) async fn claim(
        owner: &str,
        request: &wire::MutationRequest,
        host: &impl Host,
    ) -> Result<Self> {
        if owner.trim().is_empty() {
            return Err(Error::code(crate::code::PRINCIPAL_INVALID));
        }
        batch_entry::call(host,json!({"op":"claimStore","storeId":request.context.store_id,"principal":owner,"stream":request.context.stream})).await
    }
    pub(crate) fn saved(&self, request: &wire::MutationRequest, ordinal: u64) -> bool {
        request.batch_id == self.last_processed_batch_id || ordinal < self.progress
    }
    async fn save(
        &mut self,
        request: &wire::MutationRequest,
        ordinal: u64,
        result: &wire::MutationResult,
        persist: bool,
        host: &impl Host,
    ) -> Result<()> {
        let count = request.mutations.len() as u64;
        if persist {
            let _:Acknowledged=batch_entry::call(host,json!({"op":"saveResult","storeId":request.context.store_id,"batchId":request.batch_id,"ordinal":ordinal,"count":count,"result":result})).await?;
        }
        self.progress = ordinal + 1;
        if self.progress == count {
            self.last_processed_batch_id = request.batch_id;
            self.last_digest = self.current_digest.take();
            self.last_count = self.current_count.take();
            self.progress = 0;
        }
        Ok(())
    }
}
pub(crate) async fn execute(
    config: &Config,
    owner: &str,
    request: &wire::MutationRequest,
    ordinal: u64,
    raw_host: &impl Host,
) -> Result<wire::MutationResult> {
    if ordinal >= request.mutations.len() as u64 {
        return Err(request_invalid("member ordinal out of range"));
    }
    let mut store = Store::claim(owner, request, raw_host).await?;
    execute_claimed(config, owner, request, ordinal, raw_host, &mut store, true).await
}
#[tracing::instrument(target = "axton::server", name = "mutation.execute", skip_all)]
pub(crate) async fn execute_claimed(
    config: &Config,
    owner: &str,
    request: &wire::MutationRequest,
    ordinal: u64,
    raw_host: &impl Host,
    store: &mut Store,
    persist: bool,
) -> Result<wire::MutationResult> {
    let member = request
        .mutations
        .get(usize::try_from(ordinal).map_err(request_invalid)?)
        .ok_or_else(|| request_invalid("member ordinal out of range"))?;
    if store.principal != owner || store.stream != request.context.stream {
        return Err(Error::code("store.binding"));
    }
    models(config, &request.context)?;
    let allowed: bool = batch_entry::call(
        raw_host,
        json!({"op":"admit","owner":owner,"context":request.context}),
    )
    .await?;
    if !allowed {
        return Err(Error::code("stream.forbidden"));
    }
    let count = request.mutations.len() as u64;
    let completed = request.batch_id == store.last_processed_batch_id;
    let current = request.batch_id
        == store
            .last_processed_batch_id
            .checked_add(1)
            .ok_or_else(|| Error::code("batch.sequence"))?;
    if !completed && !current {
        return Err(Error::code("batch.sequence"));
    }
    let (digest, size) = if completed {
        (&store.last_digest, store.last_count)
    } else {
        (&store.current_digest, store.current_count)
    };
    if (completed || digest.is_some())
        && (digest.as_deref() != Some(request.digest.as_str()) || size != Some(count))
    {
        return Err(Error::code("batch.conflict"));
    }
    if completed || ordinal < store.progress {
        let saved:Option<wire::MutationResult>=batch_entry::call(raw_host,json!({"op":"readResult","storeId":request.context.store_id,"batchId":request.batch_id,"ordinal":ordinal})).await?;
        let saved = saved.ok_or_else(|| storage_invalid("committed member result missing"))?;
        if saved.mutation_id != member.id {
            return Err(storage_invalid("saved member mismatch"));
        }
        return Ok(saved);
    }
    if ordinal != store.progress {
        return Err(Error::code("batch.progress"));
    }
    if store.current_digest.is_none() {
        let _:Acknowledged=batch_entry::call(raw_host,json!({"op":"beginBatch","storeId":request.context.store_id,"batchId":request.batch_id,"digest":request.digest,"count":count})).await?;
        store.current_digest = Some(request.digest.clone());
        store.current_count = Some(count);
    }
    // A retained version this backend no longer serves, or whose input its
    // current schema cannot key, is refused as that one Mutation.
    let supported = match batch_entry::validate_member(config, member)? {
        batch_entry::MutationInput::Supported(args) => match input_keys(config, member, &args) {
            Ok(keys) => Some((args, keys)),
            Err(error) => {
                tracing::info!(
                    name: "mutation.unkeyable",
                    target: "axton::server",
                    mutation = %member.name,
                    version = member.version,
                    %error,
                    "refusing a Mutation whose input the current schema cannot key"
                );
                None
            }
        },
        batch_entry::MutationInput::Unsupported => None,
    };
    let Some((args, input_keys)) = supported else {
        let result = wire::MutationResult {
            mutation_id: member.id,
            outcome: wire::MutationOutcome::Rejected {
                code: crate::code::MUTATION_VERSION_UNSUPPORTED.into(),
                message: None,
            },
        };
        store
            .save(request, ordinal, &result, persist, raw_host)
            .await?;
        return Ok(result);
    };
    let host = PublicationHost::new(raw_host);
    let _: Acknowledged = host
        .call_typed(HostRequest::Savepoint {
            ordinal: ordinal + 1,
        })
        .await?;
    let handled: HandledAction = host
        .call_typed(HostRequest::HandleAction {
            name: member.name.clone(),
            version: member.version,
            arguments: args.clone(),
            owner: owner.into(),
            call_id: format!(
                "{}:{}:{}",
                request.context.store_id, request.batch_id, member.id
            ),
            ordinal: ordinal + 1,
            context: Some(batch_entry::handler_context(owner, &request.context)),
        })
        .await?;
    let outcome = match handled {
        HandledAction::Rejected { rejection } => {
            let _: Acknowledged = host
                .call_typed(HostRequest::Rollback {
                    ordinal: ordinal + 1,
                })
                .await?;
            wire::MutationOutcome::Rejected {
                code: rejection,
                message: None,
            }
        }
        HandledAction::Failed { error } => {
            return Err(Error::new(crate::code::HANDLER_FAILED, error));
        }
        HandledAction::Settled {
            outputs,
            changes,
            declarations,
        } => {
            let accepted:Result<wire::MutationOutcome>=async {
            let mut changed = crate::settlement::Changes::new();
            for r in &changes {
                crate::settlement::insert(&mut changed, crate::settlement::resolve(config, r)?)?;
            }
            crate::settlement::settle_changes(config, &changed, &declarations, &host).await?;
            let action = config
                .schema
                .action(&member.name, member.version)
                .map_err(request_invalid)?;
            let models = models(config, &request.context)?;
            let mut inputs = crate::settlement::Changes::new();
            for key in &input_keys {
                crate::settlement::insert(&mut inputs, key.clone())?;
            }
            let mut keys = crate::action_results::snapshot_keys(config, action, &args, &outputs)?;
            keys.extend(inputs.values().cloned());
            prepare(config, owner, &models, keys, &host).await?;
            let positions:Vec<Option<MemberPosition>>=batch_entry::call(raw_host,json!({"op":"targetPositions","stream":request.context.stream,"records":inputs.values().map(MemberKey::from_key).collect::<Vec<_>>()})).await?;
            if positions.len() != inputs.len() {
                return Err(storage_invalid("target position count mismatch"));
            }
            let mut targets = vec![];
            for (key, position) in inputs.values().zip(positions) {
                let state = crate::action_results::load_state(
                    config,
                    owner,
                    key,
                    *models
                        .get(&key.model)
                        .ok_or_else(|| request_invalid("Model outside descriptor"))?,
                    true,
                    &host,
                )
                .await?;
                let wire_key = wire::RecordKey {
                    model: key.model.clone(),
                    identity: key.identity.clone(),
                };
                let record = wire::ReadRecord {
                    key: wire_key.clone(),
                    cursor: (),
                    state,
                };
                targets.push(match position {
                    Some(p) if p.kind == PositionKind::Upsert => {
                        if p.key != *key || p.stream != request.context.stream {
                            return Err(storage_invalid("target position mismatch"));
                        }
                        wire::SettlementTarget::Stream {
                            key: wire_key,
                            cursor: p.cursor,
                            fallback: record,
                        }
                    }
                    _ => wire::SettlementTarget::Private { record },
                });
            }
            let (result, _) = crate::action_results::assemble_snapshots(
                config,
                owner,
                action,
                &args,
                &outputs,
                crate::action_results::SnapshotPolicy {
                    models: &models,
                    canonical: true,
                    cache: true,
                },
                &host,
            )
            .await?;
            let result = axton_core::validate_action_result(&config.schema, action, &result)
                .map_err(internal)?;
            let sync_cursor = crate::head(&host, &request.context.stream).await?;
            Ok(wire::MutationOutcome::Accepted {
                sync_cursor,
                materialization: request.context.materialization.clone(),
                result,
                targets,
            })
            }.await;
            match accepted {
                Ok(outcome) => outcome,
                Err(error) => {
                    let Some(code) = host.refusal() else {
                        return Err(error);
                    };
                    let _: Acknowledged = host
                        .call_typed(HostRequest::Rollback {
                            ordinal: ordinal + 1,
                        })
                        .await?;
                    wire::MutationOutcome::Rejected {
                        code,
                        message: None,
                    }
                }
            }
        }
    };
    let _: Acknowledged = host
        .call_typed(HostRequest::Release {
            ordinal: ordinal + 1,
        })
        .await?;
    let result = wire::MutationResult {
        mutation_id: member.id,
        outcome,
    };
    store
        .save(request, ordinal, &result, persist, raw_host)
        .await?;
    Ok(result)
}
/// The records a member's Model inputs name, keyed under the current schema.
fn input_keys(
    config: &Config,
    member: &wire::Mutation,
    args: &serde_json::Value,
) -> Result<Vec<RecordKey>> {
    let action = config
        .schema
        .action(&member.name, member.version)
        .map_err(request_invalid)?;
    let mut keys = vec![];
    for input in &action.inputs {
        if let ActionInputDescriptor::Model { name, model, .. } = input {
            for identity in
                crate::actions::input_identities(&config.schema, model, &args[name], input)?
            {
                keys.push(
                    config
                        .schema
                        .record_key(model, &identity)
                        .map_err(request_invalid)?,
                );
            }
        }
    }
    Ok(keys)
}
pub(crate) fn models(
    config: &Config,
    context: &wire::RequestContext,
) -> Result<BTreeMap<String, u64>> {
    let active = wire::materialization_id(&config.schema, &config.projection_generation)
        .map_err(internal)?;
    crate::materialization::model_versions(
        config,
        &context.materialization,
        &active,
        &config.materializations,
        wire::materialization_id,
    )
}

async fn prepare(
    _config: &Config,
    owner: &str,
    models: &BTreeMap<String, u64>,
    keys: Vec<RecordKey>,
    host: &impl Host,
) -> Result<()> {
    let keys: BTreeMap<String, RecordKey> = keys
        .into_iter()
        .map(|k| Ok((k.encoded().map_err(internal)?, k)))
        .collect::<Result<_>>()?;
    if keys.len() > 10000 {
        return Err(Error::code("publication.capacity"));
    }
    for key in keys.values() {
        let loaded: Loaded = host
            .call_typed(HostRequest::Load {
                mode: Some(LoaderMode::Prepare),
                model: key.model.clone(),
                version: *models
                    .get(&key.model)
                    .ok_or_else(|| request_invalid("Model outside descriptor"))?,
                identities: vec![key.identity.clone()],
                owner: owner.into(),
            })
            .await?;
        if !matches!(loaded,Loaded::Rows(ref rows) if rows.is_empty()) {
            return Err(Error::code(crate::code::LOADER_FAILED));
        }
    }
    Ok(())
}
