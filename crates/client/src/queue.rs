//! Pending mutations, their operations, dependencies, prerequisites, the push in flight and rejections.
use crate::engine::{Engine, as_u64};
use crate::store::ClientStore;
use crate::{Mutation, Operation, OperationKind};
use axton_core::{ModelReadDescriptor, RecordKey, Rejection, Result, canonical_json, invalid};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpKind {
    Wire,
    Companion,
    Effect,
}
#[derive(Clone, Debug)]
pub struct QueuedOp {
    pub ordinal: u64,
    pub position: u64,
    pub kind: OpKind,
    pub op: Operation,
}
#[derive(Clone, Debug)]
pub struct Queued {
    pub ordinal: u64,
    pub push: Option<u64>,
    /// A replay of one of its operations failed over newer authority; the
    /// base is visible and the mutation is still sent.
    pub diverged: bool,
    pub mutation: Mutation,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").to_string()
}
fn op_text(op: OperationKind) -> &'static str {
    match op {
        OperationKind::Create => "create",
        OperationKind::Update => "update",
        OperationKind::Delete => "delete",
    }
}
fn kind_text(kind: OpKind) -> &'static str {
    match kind {
        OpKind::Wire => "wire",
        OpKind::Companion => "companion",
        OpKind::Effect => "effect",
    }
}
fn decode_op(row: &[Value]) -> Result<QueuedOp> {
    // columns: ordinal, position, kind, model, identity, op, values
    let kind = match row[2].as_str() {
        Some("wire") => OpKind::Wire,
        Some("companion") => OpKind::Companion,
        Some("effect") => OpKind::Effect,
        _ => return Err(invalid("unknown operation kind")),
    };
    let op = match row[5].as_str() {
        Some("create") => OperationKind::Create,
        Some("update") => OperationKind::Update,
        Some("delete") => OperationKind::Delete,
        _ => return Err(invalid("unknown operation")),
    };
    Ok(QueuedOp {
        ordinal: as_u64(&row[0])?,
        position: as_u64(&row[1])?,
        kind,
        op: Operation {
            model: text(&row[3]),
            op,
            identity: serde_json::from_str(row[4].as_str().unwrap_or("null"))?,
            values: row[6].as_str().map(serde_json::from_str).transpose()?,
        },
    })
}

impl<S: ClientStore> Engine<'_, S> {
    fn bump(&mut self, column: &str) -> Result<u64> {
        let current = self
            .scalar(&format!("SELECT {column} FROM axton_client"), &[])?
            .ok_or_else(|| invalid("client row missing"))?;
        let value = as_u64(&current)?;
        let next = value
            .checked_add(1)
            .filter(|v| *v <= axton_core::MAX_SAFE_INTEGER)
            .ok_or_else(|| invalid("counter exhausted"))?;
        self.exec(
            "axton_client",
            &format!("UPDATE axton_client SET {column}=?"),
            &[json!(next)],
        )?;
        Ok(value)
    }
    pub fn allocate_ordinal(&mut self) -> Result<u64> {
        self.bump("next_ordinal")
    }
    pub fn allocate_push(&mut self) -> Result<u64> {
        self.bump("next_push")
    }
    fn insert_op(
        &mut self,
        ordinal: u64,
        position: u64,
        kind: OpKind,
        op: &Operation,
    ) -> Result<()> {
        let key = self.schema.record_key(&op.model, &op.identity)?;
        self.exec(
            "axton_mutation_operation",
            "INSERT INTO axton_mutation_operation (ordinal, position, kind, model, identity, op, \"values\") VALUES (?,?,?,?,?,?,?)",
            &[
                json!(ordinal),
                json!(position),
                json!(kind_text(kind)),
                json!(op.model),
                json!(key.encoded_identity()?),
                json!(op_text(op.op)),
                match &op.values {
                    Some(v) => json!(serde_json::to_string(v)?),
                    None => Value::Null,
                },
            ],
        )?;
        Ok(())
    }
    pub fn insert_mutation(&mut self, ordinal: u64, mutation: &Mutation) -> Result<()> {
        let args = mutation.args.as_ref().map(canonical_json).transpose()?;
        self.exec(
            "axton_mutation",
            "INSERT INTO axton_mutation (ordinal, name, version, push, call_id, args) VALUES (?,?,?,NULL,?,?)",
            &[
                json!(ordinal),
                json!(mutation.name),
                json!(mutation.version),
                mutation.call_id.as_ref().map_or(Value::Null, |v| json!(v)),
                args.map_or(Value::Null, Value::String),
            ],
        )?;
        let mut position = 0;
        for (kind, ops) in [
            (OpKind::Wire, &mutation.operations),
            (OpKind::Companion, &mutation.companion),
            (OpKind::Effect, &mutation.effects),
        ] {
            for op in ops {
                self.insert_op(ordinal, position, kind, op)?;
                position += 1;
            }
        }
        for (kind, deps) in [
            ("lifecycle", &mutation.lifecycle_dependencies),
            ("sequence", &mutation.sequence_dependencies),
        ] {
            for dep in deps {
                self.exec("axton_mutation_dependency", "INSERT OR IGNORE INTO axton_mutation_dependency (ordinal, depends_on, kind) VALUES (?,?,?)", &[json!(ordinal), json!(dep), json!(kind)])?;
            }
        }
        for key in &mutation.prerequisites {
            self.exec("axton_mutation_prerequisite", "INSERT OR IGNORE INTO axton_mutation_prerequisite (ordinal, key, error) VALUES (?,?,NULL)", &[json!(ordinal), json!(key)])?;
        }
        Ok(())
    }
    pub fn add_effect(&mut self, ordinal: u64, op: &Operation) -> Result<()> {
        let next = self.scalar(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM axton_mutation_operation WHERE ordinal=?",
            &[json!(ordinal)],
        )?;
        let position = as_u64(&next.unwrap_or(json!(0)))?;
        self.insert_op(ordinal, position, OpKind::Effect, op)
    }
    fn ops_by_ordinal(
        &mut self,
        filter: &str,
        params: &[Value],
    ) -> Result<BTreeMap<u64, Vec<QueuedOp>>> {
        let rows = self.rows(&format!("SELECT ordinal, position, kind, model, identity, op, \"values\" FROM axton_mutation_operation {filter} ORDER BY ordinal, position"), params)?;
        let mut result: BTreeMap<u64, Vec<QueuedOp>> = BTreeMap::new();
        for row in &rows.rows {
            let op = decode_op(row)?;
            result.entry(op.ordinal).or_default().push(op);
        }
        Ok(result)
    }
    fn queued_where(&mut self, filter: &str, params: &[Value]) -> Result<Vec<Queued>> {
        let mutations = self.rows(
            &format!(
                "SELECT ordinal, name, version, push, diverged, call_id, args FROM axton_mutation {filter} ORDER BY ordinal"
            ),
            params,
        )?;
        if mutations.rows.is_empty() {
            return Ok(vec![]);
        }
        let ops = self.ops_by_ordinal(filter, params)?;
        let deps = self.rows(
            &format!(
                "SELECT ordinal, depends_on, kind FROM axton_mutation_dependency {filter} ORDER BY ordinal, depends_on"
            ),
            params,
        )?;
        let prerequisites = self.rows(
            &format!(
                "SELECT ordinal, key FROM axton_mutation_prerequisite {filter} ORDER BY ordinal, key"
            ),
            params,
        )?;
        let mut result = vec![];
        for row in &mutations.rows {
            let ordinal = as_u64(&row[0])?;
            let mut mutation = Mutation::new(text(&row[1]), vec![]);
            mutation.version = as_u64(&row[2])?;
            mutation.call_id = row[5].as_str().map(str::to_owned);
            mutation.args = row[6].as_str().map(serde_json::from_str).transpose()?;
            for op in ops.get(&ordinal).into_iter().flatten() {
                match op.kind {
                    OpKind::Wire => mutation.operations.push(op.op.clone()),
                    OpKind::Companion => mutation.companion.push(op.op.clone()),
                    OpKind::Effect => mutation.effects.push(op.op.clone()),
                }
            }
            for dep in deps
                .rows
                .iter()
                .filter(|d| as_u64(&d[0]).ok() == Some(ordinal))
            {
                let target = as_u64(&dep[1])?;
                if dep[2] == "lifecycle" {
                    mutation.lifecycle_dependencies.push(target);
                } else {
                    mutation.sequence_dependencies.push(target);
                }
            }
            for p in prerequisites
                .rows
                .iter()
                .filter(|p| as_u64(&p[0]).ok() == Some(ordinal))
            {
                mutation.prerequisites.push(text(&p[1]));
            }
            result.push(Queued {
                ordinal,
                push: row[3].as_u64(),
                diverged: row[4].as_u64().unwrap_or(0) != 0,
                mutation,
            });
        }
        Ok(result)
    }
    pub fn queued(&mut self) -> Result<Vec<Queued>> {
        self.queued_where("", &[])
    }
    pub fn queued_one(&mut self, ordinal: u64) -> Result<Option<Queued>> {
        Ok(self
            .queued_where("WHERE ordinal=?", &[json!(ordinal)])?
            .into_iter()
            .next())
    }
    pub fn ops_for(&mut self, key: &RecordKey) -> Result<Vec<QueuedOp>> {
        Ok(self
            .ops_by_ordinal(
                "WHERE model=? AND identity=?",
                &[json!(key.model), json!(key.encoded_identity()?)],
            )?
            .into_values()
            .flatten()
            .collect())
    }
    /// Mark a queued mutation as diverged; completion or rejection removes
    /// the row and with it the mark.
    pub fn set_diverged(&mut self, ordinal: u64) -> Result<()> {
        self.exec(
            "axton_mutation",
            "UPDATE axton_mutation SET diverged=1 WHERE ordinal=?",
            &[json!(ordinal)],
        )?;
        Ok(())
    }
    pub fn dirty(&mut self, key: &RecordKey) -> Result<bool> {
        Ok(self
            .scalar(
                "SELECT 1 FROM axton_mutation_operation WHERE model=? AND identity=? LIMIT 1",
                &[json!(key.model), json!(key.encoded_identity()?)],
            )?
            .is_some())
    }
    pub fn delete_mutations(&mut self, ordinals: &[u64]) -> Result<()> {
        for ordinal in ordinals {
            self.exec(
                "axton_mutation",
                "DELETE FROM axton_mutation WHERE ordinal=?",
                &[json!(ordinal)],
            )?;
        }
        for table in [
            "axton_mutation_operation",
            "axton_mutation_dependency",
            "axton_mutation_prerequisite",
        ] {
            self.changed.insert(table.into());
        }
        Ok(())
    }
    pub fn assign_push(&mut self, ordinals: &[u64], push: u64) -> Result<()> {
        for ordinal in ordinals {
            self.exec(
                "axton_mutation",
                "UPDATE axton_mutation SET push=? WHERE ordinal=?",
                &[json!(push), json!(ordinal)],
            )?;
        }
        Ok(())
    }
    /// The push that was frozen and not yet completed, if any. Completion
    /// deletes a push's rows, so any assigned push is in flight; the queue
    /// never holds more than one.
    pub fn in_flight(&mut self) -> Result<Option<u64>> {
        let rows = self.rows(
            "SELECT DISTINCT push FROM axton_mutation WHERE push IS NOT NULL ORDER BY push",
            &[],
        )?;
        let pushes: Vec<u64> = rows
            .rows
            .iter()
            .map(|r| as_u64(&r[0]))
            .collect::<Result<_>>()?;
        if pushes.len() > 1 {
            return Err(invalid("more than one push in flight"));
        }
        Ok(pushes.first().copied())
    }
    /// The sequence of the last push a receipt completed. A receipt at or
    /// below it is a duplicate and changes nothing.
    pub fn last_completed_push(&mut self) -> Result<u64> {
        let value = self
            .scalar("SELECT last_completed_push FROM axton_client", &[])?
            .ok_or_else(|| invalid("client row missing"))?;
        as_u64(&value)
    }
    /// Remember that `push` completed and forget its frozen declaration.
    pub fn set_last_completed_push(&mut self, push: u64) -> Result<()> {
        self.exec(
            "axton_client",
            "UPDATE axton_client SET last_completed_push=?, push_models=NULL, push_results=NULL",
            &[json!(push)],
        )?;
        Ok(())
    }
    /// The read contracts the push in flight declared, frozen when it was
    /// allocated so a retry sends what the original request sent.
    pub fn push_models(&mut self) -> Result<Option<Value>> {
        Ok(self
            .scalar("SELECT push_models FROM axton_client", &[])?
            .and_then(|v| v.as_str().map(serde_json::from_str::<Value>))
            .transpose()?)
    }
    pub fn set_push_models(&mut self, models: &Value) -> Result<()> {
        self.exec(
            "axton_client",
            "UPDATE axton_client SET push_models=?",
            &[json!(serde_json::to_string(models)?)],
        )?;
        Ok(())
    }
    pub fn push_result_reads(&mut self) -> Result<Option<Vec<ModelReadDescriptor>>> {
        self.scalar("SELECT push_results FROM axton_client", &[])?
            .and_then(|v| v.as_str().map(str::to_owned))
            .map(|text| serde_json::from_str(&text).map_err(Into::into))
            .transpose()
    }
    pub fn set_push_result_reads(&mut self, reads: &[ModelReadDescriptor]) -> Result<()> {
        self.exec(
            "axton_client",
            "UPDATE axton_client SET push_results=?",
            &[json!(canonical_json(&serde_json::to_value(reads)?)?)],
        )?;
        Ok(())
    }
    pub fn prerequisite_keys(&mut self) -> Result<Vec<(String, Option<String>)>> {
        let rows = self.rows(
            "SELECT key, MAX(error) FROM axton_mutation_prerequisite GROUP BY key ORDER BY key",
            &[],
        )?;
        Ok(rows
            .rows
            .into_iter()
            .map(|r| (text(&r[0]), r[1].as_str().map(str::to_owned)))
            .collect())
    }
    pub fn resolve_prerequisite(&mut self, key: &str) -> Result<usize> {
        self.exec(
            "axton_mutation_prerequisite",
            "DELETE FROM axton_mutation_prerequisite WHERE key=?",
            &[json!(key)],
        )
    }
    pub fn fail_prerequisite(&mut self, key: &str, error: &str) -> Result<usize> {
        self.exec(
            "axton_mutation_prerequisite",
            "UPDATE axton_mutation_prerequisite SET error=? WHERE key=?",
            &[json!(error), json!(key)],
        )
    }
    pub fn reset_prerequisite(&mut self, key: &str) -> Result<usize> {
        self.exec(
            "axton_mutation_prerequisite",
            "UPDATE axton_mutation_prerequisite SET error=NULL WHERE key=?",
            &[json!(key)],
        )
    }
    pub fn insert_rejection(
        &mut self,
        ordinal: u64,
        name: &str,
        code: &str,
        detail: &Value,
    ) -> Result<()> {
        self.exec(
            "axton_rejection",
            "INSERT OR REPLACE INTO axton_rejection (ordinal, name, code, detail) VALUES (?,?,?,?)",
            &[
                json!(ordinal),
                json!(name),
                json!(code),
                json!(serde_json::to_string(detail)?),
            ],
        )?;
        Ok(())
    }
    pub fn rejections(&mut self) -> Result<Vec<Rejection>> {
        let rows = self.rows(
            "SELECT ordinal, code FROM axton_rejection ORDER BY ordinal",
            &[],
        )?;
        rows.rows
            .iter()
            .map(|r| {
                Ok(Rejection {
                    ordinal: as_u64(&r[0])?,
                    code: text(&r[1]),
                })
            })
            .collect()
    }
    pub fn rejection_details(&mut self) -> Result<Vec<Value>> {
        let rows = self.rows("SELECT detail FROM axton_rejection ORDER BY ordinal", &[])?;
        rows.rows
            .iter()
            .map(|r| Ok(serde_json::from_str(r[0].as_str().unwrap_or("null"))?))
            .collect()
    }
    pub fn delete_rejection(&mut self, ordinal: u64) -> Result<()> {
        self.exec(
            "axton_rejection",
            "DELETE FROM axton_rejection WHERE ordinal=?",
            &[json!(ordinal)],
        )?;
        Ok(())
    }
}
