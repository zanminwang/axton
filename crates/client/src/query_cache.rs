//! Query once result snapshots ([#158](https://github.com/zanminwang/axton/issues/158)).
//!
//! A direct Query invoked with `once` persists its complete successful result
//! and later matching calls reuse that saved value instead of issuing a
//! request. The snapshot is keyed by the compiled client contract, the Query,
//! its canonical arguments and its canonical store policy. It is never
//! applied to Models: a hit returns the saved result and writes nothing.
use crate::engine::Engine;
use crate::store::ClientStore;
use crate::{Client, schema_store};
use axton_core::{
    ActionStore, CallKind, Result, Schema, canonical_json, invalid, normalize_action_args,
    validate_action_result,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Revision of the key derivation and stored result encoding. Changing it
/// makes every earlier row unreachable, like a contract change.
pub const QUERY_CACHE_FORMAT: u64 = 1;

/// The canonical identity of one saved Query result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryCacheKey {
    /// Hex SHA-256 of the canonical components below and the format.
    pub key: String,
    /// Fingerprint of the complete compiled client schema.
    pub contract: String,
    pub name: String,
    pub version: u64,
    /// Canonical JSON of the arguments after the Query's own validation.
    pub args: String,
    /// Canonical JSON of the store policy (`true`, `false` or a map).
    pub store: String,
}

/// A committed cache row. `result` is `None` for an invalidated tombstone,
/// which keeps a generation so older responses cannot repopulate it.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryCacheEntry {
    pub generation: String,
    pub result: Option<Value>,
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The contract fingerprint of a compiled client schema: any change to it,
/// including Query outputs and Model read contracts, is a new partition.
pub fn contract_fingerprint(schema: &Schema) -> Result<String> {
    Ok(sha256_hex(&format!(
        "{QUERY_CACHE_FORMAT}:{}",
        schema_store::descriptor_text(schema)?
    )))
}

pub(crate) fn derive_key(
    schema: &Schema,
    contract: &str,
    name: &str,
    version: u64,
    args: &Value,
    store: &ActionStore,
) -> Result<QueryCacheKey> {
    let action = schema.action(name, version)?;
    if action.kind != CallKind::Query {
        return Err(invalid(format!(
            "once is only available for direct Queries; {name} v{version} is a Mutation"
        )));
    }
    let args = canonical_json(&normalize_action_args(schema, action, args)?)?;
    store.validate(action)?;
    let store = canonical_json(&store.clone().canonical().wire().unwrap_or(json!(true)))?;
    let key = sha256_hex(&canonical_json(&json!({
        "format": QUERY_CACHE_FORMAT,
        "contract": contract,
        "name": name,
        "version": version,
        "args": args,
        "store": store,
    }))?);
    Ok(QueryCacheKey {
        key,
        contract: contract.to_string(),
        name: name.to_string(),
        version,
        args,
        store,
    })
}

/// Remove rows written under any other contract. Runs inside the open
/// transaction, so repeated schema changes never accumulate snapshots.
pub(crate) fn prune<S: ClientStore>(store: &mut S, contract: &str) -> Result<()> {
    store.execute(
        "DELETE FROM axton_query_cache WHERE contract <> ?",
        &[json!(contract)],
    )?;
    Ok(())
}

impl<S: ClientStore> Engine<'_, S> {
    pub(crate) fn query_cache_entry(
        &mut self,
        key: &QueryCacheKey,
    ) -> Result<Option<QueryCacheEntry>> {
        let rows = self.rows(
            "SELECT generation, result FROM axton_query_cache WHERE key = ?",
            &[json!(key.key)],
        )?;
        let Some(row) = rows.rows.into_iter().next() else {
            return Ok(None);
        };
        let generation = row[0]
            .as_str()
            .ok_or_else(|| invalid("query cache generation is not text"))?
            .to_string();
        let result = match &row[1] {
            Value::Null => None,
            Value::String(text) => Some(serde_json::from_str(text)?),
            _ => return Err(invalid("query cache result is not text")),
        };
        Ok(Some(QueryCacheEntry { generation, result }))
    }
    /// Save `result` when the row still has the generation the request saw
    /// (`None`: no row existed). Returns whether it was saved; a newer
    /// generation means the key was invalidated after the request began.
    pub(crate) fn save_query_result(
        &mut self,
        key: &QueryCacheKey,
        generation: Option<&str>,
        result: &Value,
    ) -> Result<bool> {
        let current = self.query_cache_entry(key)?.map(|entry| entry.generation);
        if current.as_deref() != generation {
            return Ok(false);
        }
        let text = canonical_json(result)?;
        match generation {
            Some(generation) => {
                self.exec(
                    "axton_query_cache",
                    "UPDATE axton_query_cache SET result = ? WHERE key = ? AND generation = ?",
                    &[json!(text), json!(key.key), json!(generation)],
                )?;
            }
            None => {
                self.exec(
                    "axton_query_cache",
                    "INSERT INTO axton_query_cache (key, contract, name, version, args, store, generation, result) VALUES (?,?,?,?,?,?,?,?)",
                    &[
                        json!(key.key),
                        json!(key.contract),
                        json!(key.name),
                        json!(key.version),
                        json!(key.args),
                        json!(key.store),
                        json!(uuid::Uuid::new_v4().to_string()),
                        json!(text),
                    ],
                )?;
            }
        }
        Ok(true)
    }
    /// Clear the result and change the generation of every store variant of
    /// one Query argument set, and leave a fresh tombstone for each `extra`
    /// key (active requests with no row yet) so none of them can repopulate
    /// it. Returns nothing: the commit is the outcome.
    pub(crate) fn invalidate_query_results(
        &mut self,
        contract: &str,
        name: &str,
        version: u64,
        args: &str,
        extra: &[QueryCacheKey],
    ) -> Result<()> {
        let rows = self.rows(
            "SELECT key FROM axton_query_cache WHERE contract = ? AND name = ? AND version = ? AND args = ?",
            &[json!(contract), json!(name), json!(version), json!(args)],
        )?;
        for row in rows.rows {
            self.exec(
                "axton_query_cache",
                "UPDATE axton_query_cache SET result = NULL, generation = ? WHERE key = ?",
                &[json!(uuid::Uuid::new_v4().to_string()), row[0].clone()],
            )?;
        }
        for key in extra {
            self.exec(
                "axton_query_cache",
                "INSERT INTO axton_query_cache (key, contract, name, version, args, store, generation, result) VALUES (?,?,?,?,?,?,?,NULL) ON CONFLICT(key) DO NOTHING",
                &[
                    json!(key.key),
                    json!(key.contract),
                    json!(key.name),
                    json!(key.version),
                    json!(key.args),
                    json!(key.store),
                    json!(uuid::Uuid::new_v4().to_string()),
                ],
            )?;
        }
        Ok(())
    }
}

impl<S: ClientStore> Client<S> {
    /// The fingerprint partitioning this client's saved Query results.
    pub fn query_cache_contract(&self) -> &str {
        &self.query_contract
    }
    /// The canonical cache key of a direct Query call. Fails for a Mutation,
    /// unknown Query, invalid arguments or an invalid store policy, before
    /// any cache access.
    pub fn query_cache_key(
        &self,
        name: &str,
        version: u64,
        args: &Value,
        store: &ActionStore,
    ) -> Result<QueryCacheKey> {
        derive_key(
            &self.schema,
            &self.query_contract,
            name,
            version,
            args,
            store,
        )
    }
    /// The committed row for `key`, if any.
    pub fn query_cache_entry(&mut self, key: &QueryCacheKey) -> Result<Option<QueryCacheEntry>> {
        if self.session_active() {
            return Err(invalid("client transaction active"));
        }
        self.view(|engine| engine.query_cache_entry(key))
    }
    /// Validate `result` under the Query's current output contract and save
    /// it in its own local transaction, fenced by `generation` (see
    /// [`Engine::save_query_result`]). Writes no Model.
    pub fn save_query_result(
        &mut self,
        key: &QueryCacheKey,
        generation: Option<&str>,
        result: &Value,
    ) -> Result<bool> {
        let action = self.schema.action(&key.name, key.version)?;
        let result = validate_action_result(&self.schema, action, result)?;
        self.write(|engine| engine.save_query_result(key, generation, &result))
    }
    /// Invalidate every store variant of one Query argument set in the
    /// current contract, in one local transaction. Needs no network.
    pub fn invalidate_query_once(&mut self, name: &str, version: u64, args: &Value) -> Result<()> {
        let key = self.query_cache_key(name, version, args, &ActionStore::All)?;
        self.write(|engine| {
            engine.invalidate_query_results(&key.contract, &key.name, key.version, &key.args, &[])
        })
    }
}
