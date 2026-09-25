//! Authoritative readback of one mutation: the records it changed get their
//! stamps, its loaders read them back inside the same savepoint, and its
//! publications go out at those stamps. The batch keeps the last successful
//! result per record ([Server / Push](../../../docs/engineering/architecture/server/engine/push.md)).
use crate::host::{HostExt, HostRequest, Loaded, PublicationIntent, Published, RecordRef, Stamped};
use crate::{Config, Error, Host, Result, code, internal};
use axton_core::{AuthorityRecord, RecordKey};
use serde_json::Value;
use std::collections::BTreeMap;

/// What one mutation's readback produced: its authority, or the code that
/// refused it. A refusal rolls the mutation back; nothing else in the batch
/// is affected.
pub(crate) enum Outcome {
    Records(Vec<AuthorityRecord>),
    Refused(String),
}

/// Records in canonical key order, deduplicated by `(model, identity)`.
pub(crate) type Changes = BTreeMap<String, RecordKey>;

fn unregistered() -> Error {
    Error::new(code::LOADER_UNREGISTERED, "unregistered loader")
}

/// Resolve a record the handler named into a canonical key, refusing models
/// this backend does not load.
pub(crate) fn resolve(config: &Config, record: &RecordRef) -> Result<RecordKey> {
    if !config.loaders.contains(&record.model) {
        return Err(unregistered());
    }
    config
        .schema
        .record_key(&record.model, &record.identity)
        .map_err(|e| Error::new(code::HANDLER_INVALID, e.to_string()))
}

pub(crate) fn insert(changes: &mut Changes, key: RecordKey) -> Result<()> {
    changes.insert(key.encoded().map_err(internal)?, key);
    Ok(())
}

/// Allocate stamps for `changes`, read them back at the client's declared
/// versions and carry out `publications`, all in the mutation's savepoint.
pub(crate) async fn read_back(
    config: &Config,
    declared: &BTreeMap<String, u64>,
    owner: &str,
    changes: &Changes,
    publications: &[PublicationIntent],
    host: &impl Host,
) -> Result<Outcome> {
    // Every changed model must be one the client declared a read contract
    // for; otherwise the response could not be applied. A missing declaration
    // is a read refusal of this mutation, never a fallback to another version.
    let mut versions: BTreeMap<&str, u64> = BTreeMap::new();
    for key in changes.values() {
        if !config.loaders.contains(&key.model) {
            return Err(unregistered());
        }
        let Some(version) = declared.get(&key.model) else {
            return Ok(Outcome::Refused(code::MODEL_VERSION_UNSUPPORTED.into()));
        };
        if config.contract(&key.model, *version).is_none() {
            return Ok(Outcome::Refused(code::MODEL_VERSION_UNSUPPORTED.into()));
        }
        versions.insert(&key.model, *version);
    }
    let stamps = allocate_stamps(changes, host).await?;
    // Loaders read the changed records grouped by model, at the declared version.
    let mut groups: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
    for (encoded, key) in changes {
        groups.entry(&key.model).or_default().push(encoded);
    }
    let mut records = vec![];
    for (model, encoded_keys) in groups {
        let version = versions[model];
        let contract = config
            .contract(model, version)
            .ok_or_else(|| internal(format!("model {model} v{version} is not retained")))?;
        let identities: Vec<Value> = encoded_keys
            .iter()
            .map(|k| changes[*k].identity.clone())
            .collect();
        let loaded: Loaded = host
            .call_typed(HostRequest::Load {
                model: model.to_string(),
                version,
                identities,
                owner: owner.into(),
            })
            .await?;
        let rows = match loaded {
            Loaded::Refused { rejection } => return Ok(Outcome::Refused(rejection)),
            Loaded::Rows(rows) => rows,
            // A thrown loader error rejects only this mutation; it never
            // aborts the rest of the batch.
            Loaded::Failed { .. } => return Ok(Outcome::Refused(code::LOADER_FAILED.into())),
        };
        // An answer the served contract cannot accept is this mutation's
        // content problem: it rejects only this mutation.
        if rows.len() != encoded_keys.len() {
            return Ok(Outcome::Refused(code::LOADER_INVALID.into()));
        }
        for (encoded, state) in encoded_keys.iter().zip(rows) {
            let key = &changes[*encoded];
            let state = match state {
                None => Value::Null,
                Some(state) => match contract.normalize_state(model, &state) {
                    Ok(state) => state,
                    Err(_) => return Ok(Outcome::Refused(code::LOADER_INVALID.into())),
                },
            };
            records.push(AuthorityRecord {
                model: key.model.clone(),
                identity: key.identity.clone(),
                stamp: stamps[*encoded],
                state,
                error: None,
            });
        }
    }
    publish_intents(config, changes, &stamps, publications, host).await?;
    Ok(Outcome::Records(records))
}

/// One stamp per changed record, taken in canonical key order so that two
/// transactions touching the same records lock them the same way.
pub(crate) async fn allocate_stamps(
    changes: &Changes,
    host: &impl Host,
) -> Result<BTreeMap<String, u64>> {
    let mut stamps: BTreeMap<String, u64> = BTreeMap::new();
    for (encoded, key) in changes {
        let Stamped(stamp) = host
            .call_typed(HostRequest::AdvanceStamp {
                model: key.model.clone(),
                identity_key: key.encoded_identity().map_err(internal)?,
            })
            .await?;
        stamps.insert(encoded.clone(), stamp);
    }
    Ok(stamps)
}

/// Carry out publication intents at the stamps allocated for the change set.
/// A published record outside the change set keeps its current stamp,
/// initialized only when it has none: distribution never advances a version.
pub(crate) async fn publish_intents(
    config: &Config,
    changes: &Changes,
    stamps: &BTreeMap<String, u64>,
    publications: &[PublicationIntent],
    host: &impl Host,
) -> Result<()> {
    for intent in publications {
        // The one channel-name rule, as every frame and registration applies
        // it: a name that is nothing but whitespace names no channel either.
        if axton_core::check_channel(&intent.channel).is_err() {
            return Err(Error::new(
                code::PUBLISH_INVALID,
                "channel must not be empty",
            ));
        }
        let members: Changes = match &intent.records {
            None => changes.clone(),
            Some(records) => {
                let mut members = Changes::new();
                for record in records {
                    insert(&mut members, resolve(config, record)?)?;
                }
                members
            }
        };
        for (encoded, key) in &members {
            let stamp = match stamps.get(encoded) {
                Some(stamp) => *stamp,
                None => {
                    let Stamped(stamp) = host
                        .call_typed(HostRequest::EnsureStamp {
                            model: key.model.clone(),
                            identity_key: key.encoded_identity().map_err(internal)?,
                        })
                        .await?;
                    stamp
                }
            };
            publish_one(host, &intent.channel, key, stamp).await?;
        }
    }
    Ok(())
}

/// Invalidate one record on one channel at its current stamp.
pub(crate) async fn publish_one(
    host: &impl Host,
    channel: &str,
    key: &RecordKey,
    stamp: u64,
) -> Result<()> {
    let request = HostRequest::Publish {
        channel: channel.into(),
        model: key.model.clone(),
        identity: key.identity.clone(),
        identity_key: key.encoded_identity().map_err(internal)?,
        stamp,
    };
    let Published { stamp: carried, .. } = host.call_typed(request.clone()).await?;
    if carried != stamp {
        return Err(request.invalid_response(format!(
            "invalidation carries stamp {carried}, record is at {stamp}"
        )));
    }
    Ok(())
}
