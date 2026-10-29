//! Cursor pages from a current Serializable view; no server payload persistence.
use crate::{
    Config, Error, Host, Result,
    backend_interface::HostExt,
    internal,
    protocol_v05::{Publication05, call},
    request_invalid, storage_invalid,
};
use axton_core::canonical_json;
use axton_protocols::server_bridge::{
    Acknowledged, BootstrapEffects, HandledAction, HostRequest, Loaded, LoaderMode, MemberKey,
    StreamIntent,
};
use axton_protocols::sync::{self as v05, Validate};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[tracing::instrument(target = "axton::server", name = "delivery.encode", skip_all)]
fn encode<T: serde::Serialize + Validate>(v: &T) -> Result<String> {
    String::from_utf8(v05::encode(v).map_err(internal)?).map_err(internal)
}
#[derive(Deserialize)]
struct Binding {
    principal: String,
    stream: String,
}
async fn claim_binding(owner: &str, context: &v05::RequestContext, host: &impl Host) -> Result<()> {
    crate::principal(owner)?;
    let s:Binding=call(host,json!({"op":"claimStore","storeId":context.store_id,"principal":owner,"stream":context.stream})).await?;
    if s.principal != owner || s.stream != context.stream {
        return Err(Error::code("store.binding"));
    }
    Ok(())
}
async fn bind(owner: &str, context: &v05::RequestContext, host: &impl Host) -> Result<()> {
    claim_binding(owner, context, host).await?;
    let allowed: bool = call(host, json!({"op":"admit","owner":owner,"context":context})).await?;
    if !allowed {
        return Err(Error::code("stream.forbidden"));
    }
    Ok(())
}
/// Ordinary reads validate immutable ownership without locking Mutation progress.
/// A new Store is claimed only after the application read has returned.
async fn inspect_read_binding(
    owner: &str,
    context: &v05::RequestContext,
    host: &impl Host,
) -> Result<bool> {
    crate::principal(owner)?;
    let binding: Option<Binding> = call(
        host,
        json!({"op":"inspectStore","storeId":context.store_id}),
    )
    .await?;
    if let Some(s) = &binding
        && (s.principal != owner || s.stream != context.stream)
    {
        return Err(Error::code("store.binding"));
    }
    let allowed: bool = call(host, json!({"op":"admit","owner":owner,"context":context})).await?;
    if !allowed {
        return Err(Error::code("stream.forbidden"));
    }
    Ok(binding.is_some())
}
/// Delivery validates immutable ownership, not Mutation progress. Only a missing
/// Store requires the atomic claim path; existing reads take no progress lock.
async fn bind_delivery(owner: &str, context: &v05::RequestContext, host: &impl Host) -> Result<()> {
    if !inspect_read_binding(owner, context, host).await? {
        claim_binding(owner, context, host).await?;
    }
    Ok(())
}
#[tracing::instrument(target = "axton::server", name = "bootstrap.prepare", skip_all)]
async fn prepare(
    config: &Config,
    owner: &str,
    context: &v05::RequestContext,
    host: &impl Host,
) -> Result<()> {
    let done: bool = call(
        host,
        json!({"op":"bootstrapState","storeId":context.store_id}),
    )
    .await?;
    if !done {
        // Bootstrap initialization writes Store state. Keep its claim/ordering
        // guard, while a completed Store's reconnect remains a snapshot read.
        claim_binding(owner, context, host).await?;
        let effects:BootstrapEffects=call(host,json!({"op":"handleBootstrap05","owner":owner,"storeId":context.store_id,"stream":context.stream})).await?;
        let declarations = effects
            .declarations
            .into_iter()
            .map(Into::into)
            .collect::<Vec<_>>();
        crate::settlement::settle_changes(config, &Default::default(), &declarations, host).await?;
        let _: Acknowledged = call(
            host,
            json!({"op":"finishBootstrap","storeId":context.store_id}),
        )
        .await?;
    }
    Ok(())
}
/// Handshake does not carry a client cursor. Preparation and initial head commit together.
pub async fn handshake05(
    config: &Config,
    owner: &str,
    bytes: &[u8],
    raw: &impl Host,
) -> Result<String> {
    let r: v05::HandshakeRequest = v05::decode(bytes).map_err(request_invalid)?;
    let context = v05::RequestContext {
        protocol: 5,
        store_id: r.store_id.clone(),
        stream: r.stream.clone(),
        materialization: v05::materialization_id(
            &config.schema,
            config
                .protocol5
                .as_ref()
                .map(|p| p.projection_generation.as_str())
                .unwrap_or("1"),
        )
        .map_err(internal)?,
    };
    bind_delivery(owner, &context, raw).await?;
    let host = Publication05::new(raw);
    prepare(config, owner, &context, &host).await?;
    let head: u64 = call(&host, json!({"op":"deliveryHead","stream":r.stream})).await?;
    encode(&v05::HandshakeResponse {
        protocol: 5,
        store_id: r.store_id,
        stream: r.stream,
        head,
    })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Position {
    model: String,
    identity_key: String,
    cursor: u64,
    kind: String,
}
fn context_schema<'a>(config: &'a Config, context: &v05::RequestContext) -> &'a axton_core::Schema {
    config
        .protocol5
        .as_ref()
        .and_then(|p| p.materializations.get(&context.materialization))
        .map(|m| &m.schema)
        .unwrap_or(&config.schema)
}
fn bootstrap_model(
    config: &Config,
    context: &v05::RequestContext,
    name: &str,
    version: u64,
) -> bool {
    let schema = context_schema(config, context);
    schema
        .models
        .iter()
        .any(|m| m.name == name && m.version == version && m.bootstrap)
}
fn selection(
    config: &Config,
    context: &v05::RequestContext,
    bootstrap: bool,
) -> Result<Option<Vec<String>>> {
    let models = crate::mutation_batch::models(config, context)?;
    Ok(Some({
        models
            .iter()
            .filter(|(m, v)| !bootstrap || bootstrap_model(config, context, m, **v))
            .map(|(m, _)| m.clone())
            .collect()
    }))
}
#[tracing::instrument(target = "axton::server", name = "delivery.prepare", skip_all)]
async fn prepared_candidates(
    config: &Config,
    owner: &str,
    context: &v05::RequestContext,
    after: u64,
    models: Option<Vec<String>>,
    keys: Option<Vec<MemberKey>>,
    host: &impl Host,
) -> Result<Vec<Position>> {
    let versions = crate::mutation_batch::models(config, context)?;
    let page_size = keys
        .is_none()
        .then(|| cursor_page_size(context_schema(config, context)))
        .flatten();
    let mut prepared = std::collections::BTreeSet::new();
    loop {
        let positions:Option<Vec<Position>>=call(host,json!({"op":"deliveryCandidates","stream":context.stream,"after":after,"models":models,"keys":keys,"capacity":100000,"pageSize":page_size})).await?;
        let positions = positions.ok_or_else(|| Error::code("delivery.capacity"))?;
        let fresh = positions
            .iter()
            .filter(|p| {
                p.kind == "upsert" && !prepared.contains(&(p.model.clone(), p.identity_key.clone()))
            })
            .collect::<Vec<_>>();
        if fresh.is_empty() {
            return Ok(positions);
        }
        let mut groups = BTreeMap::<String, Vec<Value>>::new();
        for p in fresh {
            prepared.insert((p.model.clone(), p.identity_key.clone()));
            groups
                .entry(p.model.clone())
                .or_default()
                .push(serde_json::from_str(&p.identity_key).map_err(storage_invalid)?);
        }
        if prepared.len() > 100000 {
            return Err(Error::code("delivery.capacity"));
        }
        for (model, identities) in groups {
            let version = *versions
                .get(&model)
                .ok_or_else(|| Error::code("context_mismatch"))?;
            for ids in identities.chunks(500) {
                let loaded: Loaded = host
                    .call_typed(HostRequest::Load {
                        mode: Some(LoaderMode::Prepare),
                        model: model.clone(),
                        version,
                        identities: ids.to_vec(),
                        owner: owner.into(),
                    })
                    .await?;
                match loaded {
                    Loaded::Rows(rows) if rows.is_empty() => {}
                    Loaded::Refused { rejection } => return Err(Error::code(rejection)),
                    _ => return Err(Error::code("loader.failed")),
                }
            }
        }
    }
}
/// Constraint-free reads can seek directly in the cursor index. Constrained
/// Models retain complete metadata discovery to close cross-cursor components.
fn cursor_page_size(schema: &axton_core::Schema) -> Option<usize> {
    (!schema.models.iter().any(|model| {
        !model.unique.is_empty()
            || model
                .relations
                .iter()
                .any(|relation| relation.on_delete == "delete")
    }))
    .then_some(500)
}
#[tracing::instrument(target = "axton::server", name = "delivery.load", skip_all)]
async fn materialize(
    config: &Config,
    owner: &str,
    context: &v05::RequestContext,
    positions: Vec<Position>,
    host: &impl Host,
) -> Result<Vec<v05::AuthorityChange>> {
    let versions = crate::mutation_batch::models(config, context)?;
    let mut changes = Vec::new();
    let mut staged_bytes = 0usize;
    let mut by_model = BTreeMap::<String, Vec<Position>>::new();
    for p in positions {
        if p.kind == "remove" {
            changes.push(v05::AuthorityChange::Remove {
                key: v05::RecordKey {
                    model: p.model,
                    identity: serde_json::from_str(&p.identity_key).map_err(storage_invalid)?,
                },
                cursor: p.cursor,
            });
        } else {
            by_model.entry(p.model.clone()).or_default().push(p);
        }
    }
    for (model, group) in by_model {
        let version = *versions
            .get(&model)
            .ok_or_else(|| Error::code("context_mismatch"))?;
        for group in group.chunks(500) {
            let keys = group
                .iter()
                .map(|p| serde_json::from_str(&p.identity_key).map_err(storage_invalid))
                .collect::<Result<Vec<Value>>>()?;
            let loaded: Loaded = host
                .call_typed(HostRequest::Load {
                    mode: Some(LoaderMode::Canonical),
                    model: model.clone(),
                    version,
                    identities: keys.clone(),
                    owner: owner.into(),
                })
                .await?;
            let rows = match loaded {
                Loaded::Rows(rows) => rows,
                Loaded::Refused { rejection } => return Err(Error::code(rejection)),
                Loaded::Failed { .. } => return Err(Error::code("loader.failed")),
            };
            let schema = config
                .contract(&model, version)
                .ok_or_else(|| Error::code("model_version_unsupported"))?;
            if rows.len() != group.len() {
                return Err(Error::code("loader.invalid"));
            }
            for ((p, identity), row) in group.iter().zip(keys).zip(rows) {
                let state = match row {
                    Some(row) => schema
                        .normalize_state(&model, &row)
                        .map_err(|_| Error::code("loader.invalid"))?,
                    None => Value::Null,
                };
                let change = v05::AuthorityChange::Record {
                    key: v05::RecordKey {
                        model: model.clone(),
                        identity,
                    },
                    cursor: p.cursor,
                    state,
                };
                staged_bytes = staged_bytes
                    .checked_add(serde_json::to_vec(&change).map_err(internal)?.len())
                    .ok_or_else(|| Error::code("delivery.capacity"))?;
                if staged_bytes > 256 * 1024 * 1024 {
                    return Err(Error::code("delivery.capacity"));
                }
                changes.push(change);
            }
        }
    }
    Ok(changes)
}
fn coalesce(units: Vec<v05::DeliveryUnit>, target: usize) -> Vec<v05::DeliveryUnit> {
    let mut packed: Vec<v05::DeliveryUnit> = Vec::new();
    for mut unit in units {
        if let Some(last) = packed
            .last_mut()
            .filter(|last| last.changes.len() + unit.changes.len() <= target)
        {
            last.changes.append(&mut unit.changes);
            if unit.through.is_some() {
                last.through = unit.through;
            }
        } else {
            unit.index = packed.len() as u64;
            packed.push(unit);
        }
    }
    packed
}
fn components(
    schema: &axton_core::Schema,
    changes: &[v05::AuthorityChange],
    after: u64,
    through: u64,
    head: u64,
) -> Result<Vec<v05::DeliveryUnit>> {
    let unique = schema
        .models
        .iter()
        .filter(|m| !m.unique.is_empty())
        .map(|m| m.name.clone())
        .collect();
    // Conservative cascade closure: navigation-only relations impose no existence constraint.
    let mut representatives = BTreeMap::new();
    let mut dependencies = Vec::new();
    for c in changes {
        representatives
            .entry(c.key().model.clone())
            .or_insert_with(|| c.key().clone());
    }
    for model in &schema.models {
        for relation in &model.relations {
            if relation.on_delete == "delete"
                && let (Some(a), Some(b)) = (
                    representatives.get(&model.name),
                    representatives.get(&relation.target),
                )
            {
                for c in changes
                    .iter()
                    .filter(|c| c.key().model == model.name || c.key().model == relation.target)
                {
                    dependencies.push((a.clone(), c.key().clone()));
                }
                dependencies.push((a.clone(), b.clone()));
            }
        }
    }
    v05::plan_units(changes, &unique, &dependencies, after, through, head).map_err(internal)
}
#[tracing::instrument(target = "axton::server", name = "delivery.units", skip_all)]
fn units(
    schema: &axton_core::Schema,
    changes: &[v05::AuthorityChange],
    after: u64,
    through: u64,
    head: u64,
) -> Result<Vec<v05::DeliveryUnit>> {
    components(schema, changes, after, through, head).map(|units| coalesce(units, 500))
}
/// Select complete dependency components before canonical loading. The page
/// covers only the prefix before its first omitted component. Components may
/// include ahead-of-boundary companions, but independent future work is deferred.
#[tracing::instrument(target = "axton::server", name = "delivery.page", skip_all)]
fn page_positions(
    schema: &axton_core::Schema,
    positions: Vec<Position>,
    after: u64,
    through: u64,
    head: u64,
) -> Result<(Vec<Position>, u64)> {
    let metadata = positions
        .iter()
        .map(|p| {
            let key = v05::RecordKey {
                model: p.model.clone(),
                identity: serde_json::from_str(&p.identity_key).map_err(storage_invalid)?,
            };
            Ok(if p.kind == "remove" {
                v05::AuthorityChange::Remove {
                    key,
                    cursor: p.cursor,
                }
            } else {
                v05::AuthorityChange::Record {
                    key,
                    cursor: p.cursor,
                    state: Value::Null,
                }
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let groups = components(schema, &metadata, after, through, head)?;
    let mut selected = std::collections::BTreeSet::new();
    let mut count = 0;
    let mut covered = through;
    for group in groups {
        let minimum = group.changes.iter().map(v05::AuthorityChange::cursor).min();
        if minimum.is_none_or(|minimum| minimum > through) {
            break;
        }
        if count > 0 && count + group.changes.len() > 500 {
            covered = through.min(minimum.unwrap() - 1);
            break;
        }
        count += group.changes.len();
        for change in group.changes {
            selected.insert((
                change.key().model.clone(),
                canonical_json(&change.key().identity).map_err(internal)?,
            ));
        }
    }
    let positions = positions
        .into_iter()
        .filter(|p| selected.contains(&(p.model.clone(), p.identity_key.clone())))
        .collect();
    Ok((positions, covered))
}
/// Validate an indexed transport fragment against its response manifest.
pub(crate) fn verify_indexed_part(
    header: &v05::DeliveryHeader,
    part: &v05::DeliveryPart,
) -> Result<()> {
    part.validate().map_err(storage_invalid)?;
    if part.plan_id != header.plan_id
        || part.plan_digest != header.digest
        || header
            .units
            .get(part.unit as usize)
            .and_then(|u| u.parts.get(part.part as usize))
            != Some(&v05::part_digest(part).map_err(storage_invalid)?)
        || part.changes.iter().any(|c| {
            c.cursor() > header.observed_head
                || header.after.is_some_and(|after| c.cursor() <= after)
        })
    {
        return Err(storage_invalid("indexed part mismatch"));
    }
    Ok(())
}
/// All fragments of the selected page travel in this response. Large atomic
/// components may exceed the normal page target but are never split across reads.
#[tracing::instrument(target = "axton::server", name = "delivery.response", skip_all)]
fn response(frozen: v05::FrozenDelivery) -> Result<v05::DeliveryResponse> {
    let response = v05::DeliveryResponse {
        header: frozen.header,
        parts: frozen.parts,
    };
    if serde_json::to_vec(&response).map_err(internal)?.len() > 256 * 1024 * 1024 {
        return Err(Error::code("delivery.capacity"));
    }
    Ok(response)
}
#[tracing::instrument(target = "axton::server", name = "delivery.plan", skip_all)]
pub async fn process_delivery05(
    config: &Config,
    owner: &str,
    bytes: &[u8],
    raw: &impl Host,
) -> Result<String> {
    let r: v05::DeltaRequest = v05::decode(bytes).map_err(request_invalid)?;
    crate::mutation_batch::models(config, &r.context)?;
    bind_delivery(owner, &r.context, raw).await?;
    let host = Publication05::new(raw);
    prepare(config, owner, &r.context, &host).await?;
    let positions = prepared_candidates(
        config,
        owner,
        &r.context,
        r.after,
        selection(config, &r.context, r.bootstrap)?,
        None,
        &host,
    )
    .await?;
    let head: u64 = call(
        &host,
        json!({"op":"deliveryHead","stream":r.context.stream}),
    )
    .await?;
    if r.through > head {
        return Err(request_invalid("through ahead of head"));
    }
    // A bounded candidate query returns one lookahead row plus its complete
    // cursor tie. It proves no coverage beyond its maximum returned position.
    let through = if cursor_page_size(context_schema(config, &r.context))
        .is_some_and(|size| positions.len() > size)
    {
        r.through.min(
            positions
                .iter()
                .map(|position| position.cursor)
                .max()
                .unwrap(),
        )
    } else {
        r.through
    };
    let (positions, through) = page_positions(
        context_schema(config, &r.context),
        positions,
        r.after,
        through,
        head,
    )?;
    let changes = materialize(config, owner, &r.context, positions, &host).await?;
    let units = units(
        context_schema(config, &r.context),
        &changes,
        r.after,
        through,
        head,
    )?;
    let now: u64 = call(&host, json!({"op":"deliveryNow"})).await?;
    let frozen = tracing::info_span!(target: "axton::server", "delivery.freeze")
        .in_scope(|| {
            v05::freeze_delivery(
                r.context.clone(),
                uuid::Uuid::new_v4().to_string(),
                if r.bootstrap {
                    v05::DeliveryPurpose::Bootstrap
                } else {
                    v05::DeliveryPurpose::Sync
                },
                r.after,
                through,
                head,
                now + 300000,
                units,
                500,
            )
        })
        .map_err(internal)?;
    encode(&response(frozen)?)
}
#[tracing::instrument(target = "axton::server", name = "delivery.materialize", skip_all)]
pub async fn process_materialization05(
    config: &Config,
    owner: &str,
    bytes: &[u8],
    raw: &impl Host,
) -> Result<String> {
    let r: v05::MaterializationRequest = v05::decode(bytes).map_err(request_invalid)?;
    let versions = crate::mutation_batch::models(config, &r.context)?;
    bind_delivery(owner, &r.context, raw).await?;
    if let v05::MaterializationOwner::Schema {
        previous_materialization,
    } = &r.owner
    {
        let previous = v05::RequestContext {
            materialization: previous_materialization.clone(),
            ..r.context.clone()
        };
        crate::mutation_batch::models(config, &previous)?;
    }
    if let v05::MaterializationOwner::Settlement {
        batch_id,
        mutation_id,
    } = r.owner
    {
        let results: Vec<v05::MutationResult> = call(
            raw,
            json!({"op":"readResults","storeId":r.context.store_id,"batchId":batch_id}),
        )
        .await?;
        let result = results
            .iter()
            .find(|v| v.mutation_id == mutation_id)
            .ok_or_else(|| request_invalid("unknown settlement owner"))?;
        let targets = match &result.outcome {
            v05::MutationOutcome::Accepted { targets, .. } => targets,
            _ => return Err(request_invalid("rejected settlement owner")),
        };
        if r.keys.iter().any(|k| {
            !targets
                .iter()
                .any(|t| matches!(t,v05::SettlementTarget::Stream{key,..} if key==k))
        }) {
            return Err(request_invalid("unowned settlement target"));
        }
    }
    for (model, version) in &r.models {
        if versions.get(model) != Some(version)
            || !bootstrap_model(config, &r.context, model, *version)
        {
            return Err(request_invalid("unselected schema Model"));
        }
    }
    let host = Publication05::new(raw);
    let positions = prepared_candidates(
        config,
        owner,
        &r.context,
        0,
        Some(r.models.keys().cloned().collect()),
        Some(
            r.keys
                .iter()
                .map(|k| {
                    MemberKey::from_key(&axton_core::RecordKey {
                        model: k.model.clone(),
                        identity: k.identity.clone(),
                    })
                })
                .collect(),
        ),
        &host,
    )
    .await?;
    let head: u64 = call(
        &host,
        json!({"op":"deliveryHead","stream":r.context.stream}),
    )
    .await?;
    let changes = materialize(config, owner, &r.context, positions, &host).await?;
    let mut units = units(context_schema(config, &r.context), &changes, 0, head, head)?;
    for u in &mut units {
        u.through = None;
    }
    let now: u64 = call(&host, json!({"op":"deliveryNow"})).await?;
    let frozen = v05::freeze_materialization(
        r.context.clone(),
        uuid::Uuid::new_v4().to_string(),
        r.owner.clone(),
        head,
        now + 300000,
        units,
        500,
    )
    .map_err(internal)?;
    encode(&v05::MaterializationResponse {
        request_id: r.request_id,
        delivery: response(frozen)?,
    })
}
/// Query/Fetch reuse retained normalization and snapshot assembly; snapshots
/// carry null cursors; only explicit authenticated Query tracking enrolls.
pub async fn process_read05(
    config: &Config,
    owner: &str,
    bytes: &[u8],
    raw: &impl Host,
) -> Result<String> {
    let r: v05::ReadRequest = v05::decode(bytes).map_err(request_invalid)?;
    let versions = crate::mutation_batch::models(config, &r.context)?;
    let mut bound = inspect_read_binding(owner, &r.context, raw).await?;
    let host = Publication05::new(raw);
    let _: Acknowledged = host
        .call_typed(HostRequest::Savepoint { ordinal: 1 })
        .await?;
    let read = async {
        match &r.invocation {
            v05::ReadInvocation::Fetch { key, version } => {
                let cache_version = *versions
                    .get(&key.model)
                    .ok_or_else(|| Error::code("model_version_unsupported"))?;
                let k = config
                    .contract(&key.model, *version)
                    .ok_or_else(|| Error::code("model_version_unsupported"))?
                    .record_key(&key.model, &key.identity)
                    .map_err(request_invalid)?;
                let state = crate::action_results::load_one_canonical_state(
                    config, owner, &k, *version, &host,
                )
                .await?;
                // Caller and cache contracts may project the same identity differently.
                let cache_state = if !r.store || cache_version == *version {
                    state.clone()
                } else {
                    crate::action_results::load_one_canonical_state(
                        config,
                        owner,
                        &k,
                        cache_version,
                        &host,
                    )
                    .await?
                };
                if !bound {
                    bind(owner, &r.context, raw).await?;
                }
                let result = if state.is_null() {
                    Value::Null
                } else {
                    let mut f = k.identity.as_object().unwrap().clone();
                    f.extend(state.as_object().unwrap().clone());
                    Value::Object(f)
                };
                Ok((
                    result,
                    vec![v05::ReadRecord {
                        key: v05::RecordKey {
                            model: k.model,
                            identity: k.identity,
                        },
                        cursor: (),
                        state: cache_state,
                    }],
                ))
            }
            v05::ReadInvocation::Query {
                name,
                version,
                args,
            } => {
                let action = config
                    .schema
                    .action(name, *version)
                    .map_err(|_| Error::code("action_version_unsupported"))?;
                if action.kind != axton_core::CallKind::Query {
                    return Err(request_invalid("expected Query"));
                }
                let args = axton_core::normalize_action_args(&config.schema, action, args)
                    .map_err(request_invalid)?;
                let handled: HandledAction = host
                    .call_typed(HostRequest::HandleAction {
                        name: name.clone(),
                        version: *version,
                        arguments: args.clone(),
                        owner: owner.into(),
                        call_id: r.request_id.clone(),
                        ordinal: 1,
                        context: Some(crate::protocol_v05::handler_context(owner, &r.context)),
                    })
                    .await?;
                let outputs = match handled {
                    HandledAction::Settled {
                        outputs,
                        changes,
                        declarations,
                    } if changes.is_empty()
                        && !declarations
                            .iter()
                            .any(|d| matches!(d, StreamIntent::Invalidate { .. })) =>
                    {
                        if !declarations.is_empty() && !bound {
                            bind(owner, &r.context, raw).await?;
                            bound = true;
                        }
                        crate::settlement::settle_changes(
                            config,
                            &Default::default(),
                            &declarations,
                            &host,
                        )
                        .await?;
                        outputs
                    }
                    HandledAction::Settled { .. } => {
                        return Err(Error::code("query.effects_forbidden"));
                    }
                    HandledAction::Rejected { rejection } => {
                        return Err(Error::code(rejection));
                    }
                    HandledAction::Failed { .. } => {
                        return Err(Error::code("handler.failed"));
                    }
                };
                let (result, snapshots) = crate::action_results::assemble_snapshots(
                    config,
                    owner,
                    action,
                    &args,
                    &outputs,
                    crate::action_results::SnapshotPolicy {
                        models: &versions,
                        canonical: true,
                        cache: r.store,
                    },
                    &host,
                )
                .await?;
                let result = axton_core::validate_action_result(&config.schema, action, &result)
                    .map_err(|_| Error::code("handler.invalid"))?;
                // Ordinary reads claim a new Store only after every application await.
                if !bound {
                    bind(owner, &r.context, raw).await?;
                }
                Ok((
                    result,
                    snapshots
                        .into_iter()
                        .map(|s| v05::ReadRecord {
                            key: v05::RecordKey {
                                model: s.key.model,
                                identity: s.key.identity,
                            },
                            cursor: (),
                            state: s.state,
                        })
                        .collect(),
                ))
            }
        }
    };
    let answer = read.await;
    if answer.is_err() {
        let _: Acknowledged = host
            .call_typed(HostRequest::Rollback { ordinal: 1 })
            .await?;
    }
    let _: Acknowledged = host.call_typed(HostRequest::Release { ordinal: 1 }).await?;
    let (outcome, records) = match answer {
        Ok((result, records)) => (v05::ReadOutcome::Succeeded { result }, records),
        Err(e)
            if !matches!(
                e.code.as_str(),
                "host"
                    | "host.invalid"
                    | "storage.invalid"
                    | "internal"
                    | "loader.failed"
                    | "handler.failed"
                    | "loader.invalid"
                    | "handler.invalid"
            ) =>
        {
            (
                v05::ReadOutcome::Failed {
                    code: e.code,
                    message: Some(e.message),
                },
                vec![],
            )
        }
        Err(e) => return Err(e),
    };
    encode(&v05::ReadResponse {
        context: r.context,
        request_id: r.request_id,
        outcome,
        records,
    })
}

/// A fresh live offer observes its finite through under the same transaction.
/// Subsequent fragments keep that exact bound; no delayed notification cursor
/// is attached to content loaded at a later head.
pub async fn process_live05(
    config: &Config,
    owner: &str,
    bytes: &[u8],
    host: &impl Host,
) -> Result<String> {
    let mut r: v05::DeltaRequest = v05::decode(bytes).map_err(request_invalid)?;
    bind_delivery(owner, &r.context, host).await?;
    r.through = call(host, json!({"op":"deliveryHead","stream":r.context.stream})).await?;
    process_delivery05(config, owner, &v05::encode(&r).map_err(internal)?, host).await
}
#[cfg(test)]
mod tests {
    use super::*;
    fn entry_schema() -> axton_core::Schema {
        serde_json::from_value(json!({"enums":[],"actions":[],"models":[{
            "name":"Entry","version":1,"identity":["id"],"fields":[
                {"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}},
                {"name":"text","nullable":false,"type":{"kind":"scalar","name":"string"}}
            ]
        }]}))
        .unwrap()
    }
    fn position(id: u64, cursor: u64) -> Position {
        Position {
            model: "Entry".into(),
            identity_key: format!("{{\"id\":\"{id}\"}}"),
            cursor,
            kind: "upsert".into(),
        }
    }
    #[test]
    fn cursor_pages_cover_a_prefix_and_skip_independent_future_versions() {
        let schema = entry_schema();
        let (page, through) = page_positions(
            &schema,
            (1..=1000).map(|id| position(id, id)).collect(),
            0,
            1000,
            1002,
        )
        .unwrap();
        assert_eq!(page.len(), 500);
        assert_eq!(through, 500);
        let mut current = (501..=1000).map(|id| position(id, id)).collect::<Vec<_>>();
        current[0].cursor = 1001;
        current[1].cursor = 1002;
        current[1].kind = "remove".into();
        let (page, through) = page_positions(&schema, current, 500, 1000, 1002).unwrap();
        assert_eq!(page.len(), 498);
        assert_eq!(through, 1000);
        assert!(page.iter().all(|p| p.cursor <= 1000));
        let (latest, through) = page_positions(
            &schema,
            vec![
                position(501, 1001),
                Position {
                    kind: "remove".into(),
                    ..position(502, 1002)
                },
            ],
            1000,
            1002,
            1002,
        )
        .unwrap();
        assert_eq!(latest.len(), 2);
        assert_eq!(latest[1].kind, "remove");
        assert_eq!(through, 1002);
    }
    #[test]
    fn cursor_pages_keep_large_same_cursor_and_unique_components_whole() {
        let mut schema = entry_schema();
        let (page, through) = page_positions(
            &schema,
            (1..=1001).map(|id| position(id, 1)).collect(),
            0,
            1,
            1,
        )
        .unwrap();
        assert_eq!(page.len(), 1001);
        assert_eq!(through, 1);
        schema.models[0].unique = vec![vec!["text".into()]];
        let (page, through) =
            page_positions(&schema, vec![position(1, 1), position(2, 2)], 0, 1, 2).unwrap();
        assert_eq!(
            page.len(),
            2,
            "ahead unique companion remains in the atomic component"
        );
        assert_eq!(through, 1);
    }
    #[test]
    fn indexed_fragment_rejects_changed_payload_without_scanning_other_units() {
        let context = v05::RequestContext {
            protocol: 5,
            store_id: "s".into(),
            stream: "User:a".into(),
            materialization: "m".into(),
        };
        let frozen = v05::freeze_delivery(
            context,
            "p".into(),
            v05::DeliveryPurpose::Sync,
            0,
            1,
            1,
            1000,
            vec![v05::DeliveryUnit {
                index: 0,
                through: Some(1),
                changes: vec![],
            }],
            500,
        )
        .unwrap();
        assert!(verify_indexed_part(&frozen.header, &frozen.parts[0]).is_ok());
        let mut part = frozen.parts[0].clone();
        part.changes.push(v05::AuthorityChange::Remove {
            key: v05::RecordKey {
                model: "Entry".into(),
                identity: json!({"id":"x"}),
            },
            cursor: 1,
        });
        assert!(verify_indexed_part(&frozen.header, &part).is_err());
    }
    #[test]
    fn coalesces_independent_components_without_splitting_atomic_units() {
        let unit = |index, rows| v05::DeliveryUnit {
            index,
            through: Some(index + 1),
            changes: (0..rows)
                .map(|i| v05::AuthorityChange::Record {
                    key: v05::RecordKey {
                        model: "Entry".into(),
                        identity: json!({"id":format!("{index}-{i}")}),
                    },
                    cursor: index + 1,
                    state: json!({}),
                })
                .collect(),
        };
        let packed = coalesce(
            vec![unit(0, 200), unit(1, 200), unit(2, 800), unit(3, 1)],
            500,
        );
        assert_eq!(packed.len(), 3);
        assert_eq!(packed[0].changes.len(), 400);
        assert_eq!(packed[0].through, Some(2));
        assert_eq!(packed[1].changes.len(), 800);
        assert_eq!(packed[2].index, 2);
    }
}
