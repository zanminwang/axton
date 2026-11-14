//! Pending mutations, their operations, dependencies, prerequisites, the push in flight and rejections.
use crate::authority::Held;
use crate::engine::{Engine, as_u64};
use crate::store::ClientStore;
use crate::{Mutation, Operation, OperationKind};
use axton_core::{RecordKey, Result, invalid};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpKind {
    Wire,
    Effect,
}
#[derive(Clone, Debug)]
pub struct QueuedOp {
    pub ordinal: u64,
    pub position: u64,
    pub kind: OpKind,
    pub op: Operation,
}
// A settled local write retained at its place in a record's local order:
// the receipt-finalized state of a target the server accepted, kept at its
// wire operation's `(ordinal, position)` until no earlier pending work on
// the record needs it. `sequence` orders writes that share a place.
#[derive(Clone, Debug)]
pub struct LocalWrite {
    pub sequence: u64,
    pub ordinal: u64,
    pub position: u64,
    pub op: Operation,
}
#[derive(Clone, Debug)]
pub struct Queued {
    pub ordinal: u64,
    pub push: Option<u64>,
    // A replay of one of its operations failed over newer authority; the
    // base is visible and the mutation is still sent.
    pub diverged: bool,
    pub mutation: Mutation,
}

// A queued Mutation's local operations, by ordinal, in step order.
type OpsByOrdinal = BTreeMap<u64, Vec<QueuedOp>>;

fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").to_string()
}

fn decode_operation(
    model: &Value,
    identity: &Value,
    op: &Value,
    values: &Value,
) -> Result<Operation> {
    let op = match op.as_str() {
        Some("create") => OperationKind::Create,
        Some("update") => OperationKind::Update,
        Some("delete") => OperationKind::Delete,
        _ => return Err(invalid("unknown operation")),
    };
    Ok(Operation {
        model: text(model),
        op,
        identity: serde_json::from_str(identity.as_str().unwrap_or("null"))?,
        values: values.as_str().map(serde_json::from_str).transpose()?,
    })
}
fn decode_op(row: &[Value]) -> Result<QueuedOp> {
    // columns: ordinal, position, kind, model, identity, op, values
    let kind = match row[2].as_str() {
        Some("wire") => OpKind::Wire,
        Some("effect") => OpKind::Effect,
        _ => return Err(invalid("unknown operation kind")),
    };
    Ok(QueuedOp {
        ordinal: as_u64(&row[0])?,
        position: as_u64(&row[1])?,
        kind,
        op: decode_operation(&row[3], &row[4], &row[5], &row[6])?,
    })
}
fn decode_local_write(row: &[Value]) -> Result<LocalWrite> {
    // columns: sequence, ordinal, position, disposition, model, identity, op, values
    if row[3].as_str() != Some("accepted") {
        return Err(invalid("unknown local write disposition"));
    }
    Ok(LocalWrite {
        sequence: as_u64(&row[0])?,
        ordinal: as_u64(&row[1])?,
        position: as_u64(&row[2])?,
        op: decode_operation(&row[4], &row[5], &row[6], &row[7])?,
    })
}

impl<S: ClientStore> Engine<'_, S> {
    pub fn allocate_ordinal(&mut self) -> Result<u64> {
        self.allocate("next_mutation_id")
    }

    fn insert_op(
        &mut self,
        ordinal: u64,
        position: u64,
        kind: OpKind,
        op: &Operation,
    ) -> Result<()> {
        self.insert_owned(ordinal, position, kind, None, op)
    }
    pub fn add_effect(&mut self, ordinal: u64, op: &Operation) -> Result<()> {
        self.append_op(ordinal, OpKind::Effect, op)
    }
    // Store `op` at the next position of queued call `ordinal`: after every
    // operation the call already holds. A cascade delete appended later is
    // an effect of a wire delete.
    pub(crate) fn append_op(&mut self, ordinal: u64, kind: OpKind, op: &Operation) -> Result<()> {
        let next = self.scalar(
            "SELECT COALESCE(MAX(step), -1) + 1 FROM axton_mutation_queue_operation WHERE mutation_id=?",
            &[json!(ordinal)],
        )?;
        let position = as_u64(&next.unwrap_or(json!(0)))?;
        self.insert_op(ordinal, position, kind, op)
    }
    // The operations of one queued call with their positions and kinds.
    pub(crate) fn call_ops(&mut self, ordinal: u64) -> Result<Vec<QueuedOp>> {
        Ok(self
            .ops_by_ordinal("AND q.id=?", &[json!(ordinal)])?
            .0
            .into_values()
            .flatten()
            .collect())
    }
    // The settled local writes retained for one record, in allocation order.
    pub(crate) fn local_writes_for(&mut self, key: &RecordKey) -> Result<Vec<LocalWrite>> {
        let rows = self.rows(
            "SELECT sequence, ordinal, position, disposition, model, identity, op, \"values\" FROM axton_local_write WHERE model=? AND identity=? ORDER BY sequence",
            &[json!(key.model), json!(key.encoded_identity()?)],
        )?;
        rows.rows.iter().map(|r| decode_local_write(r)).collect()
    }
    pub(crate) fn delete_local_write(&mut self, sequence: u64) -> Result<()> {
        self.exec(
            "axton_local_write",
            "DELETE FROM axton_local_write WHERE sequence=?",
            &[json!(sequence)],
        )?;
        Ok(())
    }
    // Forget every settled local write of one record: new authority replaced
    // the base they belong to.
    pub(crate) fn delete_local_writes(&mut self, key: &RecordKey) -> Result<()> {
        self.exec(
            "axton_local_write",
            "DELETE FROM axton_local_write WHERE model=? AND identity=?",
            &[json!(key.model), json!(key.encoded_identity()?)],
        )?;
        Ok(())
    }
    // A file written while Mutation companion writes and direct Model writes
    // existed may hold `companion` queue operations and `independent` local
    // writes. Open deletes both in its own transaction and rebuilds the
    // records among them that a live Mutation still holds; their Mutations
    // stay queued. Every other record is left as it is: a refused Mutation's
    // refusal already restored its records, and a direct write on a record
    // nothing held stays as cache. A record of a Model the current schema
    // lacks or does not share with its Mutation is not held. A file without
    // them costs one empty read.
    pub(crate) fn purge_retired_writes(&mut self) -> Result<()> {
        let touched = self.rows(
            "SELECT model,identity FROM axton_mutation_queue_operation WHERE kind='companion' UNION SELECT model,identity FROM axton_local_write WHERE disposition<>'accepted'",
            &[],
        )?;
        if touched.rows.is_empty() {
            return Ok(());
        }
        let mut held = Held::new();
        for row in &touched.rows {
            let (Some(model), Some(identity)) = (row[0].as_str(), row[1].as_str()) else {
                continue;
            };
            let Ok(key) = self
                .schema
                .record_key(model, &serde_json::from_str(identity)?)
            else {
                continue;
            };
            // Read before the deletes: a record held only by a companion is
            // rebuilt without it.
            if self.dirty(&key)? {
                held.insert(key.encoded()?, key);
            }
        }
        self.exec(
            "axton_mutation_queue_operation",
            "DELETE FROM axton_mutation_queue_operation WHERE kind='companion'",
            &[],
        )?;
        self.exec(
            "axton_local_write",
            "DELETE FROM axton_local_write WHERE disposition<>'accepted'",
            &[],
        )?;
        self.rebuild_held(&held)?;
        Ok(())
    }
    // Every reader of a queued Mutation's local operations loads them here.
    // An operation on a Model the Mutation does not share with the current
    // schema is left out, and its Mutation is returned as diverged.
    fn ops_by_ordinal(
        &mut self,
        filter: &str,
        params: &[Value],
    ) -> Result<(OpsByOrdinal, BTreeSet<u64>)> {
        let rows = self.rows(&format!("SELECT o.mutation_id,o.step,o.kind,o.model,o.identity,o.operation,o.value,q.descriptor FROM axton_mutation_queue_operation o JOIN axton_mutation_queue q ON q.id=o.mutation_id WHERE o.model IS NOT NULL AND q.reconciled=0 AND q.rejection_code IS NULL {filter} ORDER BY o.mutation_id,o.step"), params)?;
        let mut result = OpsByOrdinal::new();
        let mut skipped = BTreeSet::new();
        for row in &rows.rows {
            let descriptor = row[7]
                .as_str()
                .ok_or_else(|| invalid("descriptor missing"))?;
            if !self.shared(descriptor, &text(&row[3]))? {
                skipped.insert(as_u64(&row[0])?);
                continue;
            }
            let op = decode_op(row)?;
            result.entry(op.ordinal).or_default().push(op);
        }
        Ok((result, skipped))
    }
    pub(crate) fn queued_where(&mut self, filter: &str, params: &[Value]) -> Result<Vec<Queued>> {
        let mutations = self.rows(
            &format!(
                "SELECT q.id,q.name,q.descriptor_version,q.batch_id,q.diverged,(SELECT id FROM axton_store)||':'||q.id FROM axton_mutation_queue q WHERE q.reconciled=0 AND q.rejection_code IS NULL {filter} ORDER BY q.id"
            ),
            params,
        )?;
        if mutations.rows.is_empty() {
            return Ok(vec![]);
        }
        let (ops, skipped) = self.ops_by_ordinal(filter, params)?;
        let deps = self.rows(
            &format!(
                "SELECT d.ordinal,d.depends_on,d.kind FROM axton_mutation_dependency d JOIN axton_mutation_queue q ON q.id=d.ordinal WHERE q.reconciled=0 AND q.rejection_code IS NULL {filter} ORDER BY d.ordinal,d.depends_on"
            ),
            params,
        )?;
        let prerequisites = self.rows(
            &format!(
                "SELECT p.ordinal,p.key FROM axton_mutation_prerequisite p JOIN axton_mutation_queue q ON q.id=p.ordinal WHERE q.reconciled=0 AND q.rejection_code IS NULL {filter} ORDER BY p.ordinal,p.key"
            ),
            params,
        )?;
        let mut result = vec![];
        for row in &mutations.rows {
            let ordinal = as_u64(&row[0])?;
            let mut mutation = Mutation::new(text(&row[1]), vec![]);
            mutation.version = as_u64(&row[2])?;
            mutation.call_id = row[5].as_str().map(str::to_owned);
            mutation.args = Some(self.input(ordinal)?);
            for op in ops.get(&ordinal).into_iter().flatten() {
                match op.kind {
                    OpKind::Wire => mutation.operations.push(op.op.clone()),
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
                diverged: row[4].as_u64().unwrap_or(0) != 0 || skipped.contains(&ordinal),
                mutation,
            });
        }
        Ok(result)
    }
    pub fn queued(&mut self) -> Result<Vec<Queued>> {
        self.queued_where("", &[])
    }
    pub fn ops_for(&mut self, key: &RecordKey) -> Result<Vec<QueuedOp>> {
        Ok(self
            .ops_by_ordinal(
                "AND o.model=? AND o.identity=?",
                &[json!(key.model), json!(key.encoded_identity()?)],
            )?
            .0
            .into_values()
            .flatten()
            .collect())
    }
    // Mark a queued mutation as diverged; completion or rejection removes
    // the row and with it the mark.
    pub fn set_diverged(&mut self, ordinal: u64) -> Result<()> {
        self.exec(
            "axton_mutation_queue",
            "UPDATE axton_mutation_queue SET diverged=1 WHERE id=?",
            &[json!(ordinal)],
        )?;
        Ok(())
    }
    // A record is dirty while a queued Mutation it shares with the current
    // schema has an operation on it.
    pub fn dirty(&mut self, key: &RecordKey) -> Result<bool> {
        let descriptors = self.rows(
            "SELECT DISTINCT q.descriptor FROM axton_mutation_queue_operation o JOIN axton_mutation_queue q ON q.id=o.mutation_id WHERE o.model=? AND o.identity=? AND q.reconciled=0 AND q.rejection_code IS NULL",
            &[json!(key.model), json!(key.encoded_identity()?)],
        )?;
        for row in &descriptors.rows {
            let descriptor = row[0]
                .as_str()
                .ok_or_else(|| invalid("descriptor missing"))?;
            if self.shared(descriptor, &key.model)? {
                return Ok(true);
            }
        }
        Ok(false)
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

    pub fn rejection_details(&mut self) -> Result<Vec<Value>> {
        Ok(self.refused_acts()?.into_iter().map(|act| {
                let records:Vec<_>=act.act.operations.iter().map(|op|json!({"model":op.model,"identity":op.identity})).collect();
                json!({"ordinal":act.id,"code":act.code,"mutation":{"name":act.name,"version":act.version,"args":act.act.args,"operations":act.act.operations},"records":records})
            }).collect())
    }
}
