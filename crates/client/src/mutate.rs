//! Optimistic writes, truth holding, rebuild and cascades. Authority lands
//! through `authority.rs`.
use crate::ddl::before_table;
use crate::engine::Engine;
use crate::queue::{LocalWrite, OpKind, QueuedOp};
use crate::rows::merge_identity;
use crate::store::ClientStore;
use crate::{Operation, OperationKind};
use axton_core::{RecordKey, Result, invalid};
use serde_json::Value;
use std::collections::BTreeSet;

// One entry of a record's local history above its base.
enum Step {
    // An operation of a call still in the queue: wire or effect.
    Pending(QueuedOp),
    // A settled local write retained at its place.
    Settled(LocalWrite),
}
impl Step {
    // Committed local order: a call's operations by position, an accepted
    // write at its wire operation's position, after the operations there.
    fn order(&self) -> (u64, u64, u64) {
        match self {
            Step::Pending(q) => (q.ordinal, q.position, 0),
            Step::Settled(w) => (w.ordinal, w.position, w.sequence),
        }
    }
}

// Apply a settled local write. It was validated when it was made, so it is
// never a replay conflict: a create sets the row it created (a recreation
// keeps its newer content), an update of a row that no longer exists
// changes nothing (it cannot preserve a record whose creation went), and a
// delete removes the row.
pub(crate) fn apply_settled(row: &mut Option<Value>, op: &Operation) {
    match op.op {
        OperationKind::Create => {
            if let Some(values) = &op.values {
                *row = Some(merge_identity(&op.identity, values));
            }
        }
        OperationKind::Update => {
            if let (Some(current), Some(patch)) =
                (row.as_mut(), op.values.as_ref().and_then(Value::as_object))
            {
                for (k, v) in patch {
                    current[k] = v.clone();
                }
            }
        }
        OperationKind::Delete => *row = None,
    }
}
// The base with only the settled writes applied: what stays visible when a
// pending operation no longer replays.
fn settled_view(base: &Option<Value>, steps: &[Step]) -> Option<Value> {
    let mut row = base.clone();
    for step in steps {
        if let Step::Settled(write) = step {
            apply_settled(&mut row, &write.op);
        }
    }
    row
}

pub fn apply_to_row(row: &mut Option<Value>, op: &Operation) -> Result<()> {
    match op.op {
        OperationKind::Create => {
            if row.is_some() {
                return Err(invalid("create already exists"));
            }
            let values = op
                .values
                .as_ref()
                .ok_or_else(|| invalid("create values missing"))?;
            *row = Some(merge_identity(&op.identity, values));
        }
        OperationKind::Update => {
            let current = row.as_mut().ok_or_else(|| invalid("update row missing"))?;
            let patch = op
                .values
                .as_ref()
                .and_then(Value::as_object)
                .ok_or_else(|| invalid("patch missing"))?;
            for (k, v) in patch {
                current[k] = v.clone();
            }
        }
        OperationKind::Delete => {
            *row = None;
        }
    }
    Ok(())
}

impl<S: ClientStore> Engine<'_, S> {
    pub fn read_row(&mut self, key: &RecordKey) -> Result<Option<Value>> {
        let model = self.schema.model(&key.model)?.clone();
        self.row_get(&key.model, &model, &key.identity)
    }
    pub(crate) fn before_get(&mut self, key: &RecordKey) -> Result<Option<Value>> {
        let model = self.schema.model(&key.model)?.clone();
        self.row_get(&before_table(&key.model), &model, &key.identity)
    }
    pub(crate) fn before_set(&mut self, key: &RecordKey, row: Option<&Value>) -> Result<()> {
        let model = self.schema.model(&key.model)?.clone();
        let table = before_table(&key.model);
        match row {
            Some(row) => self.row_upsert(&table, &model, row),
            None => self.row_delete(&table, &model, &key.identity),
        }
    }
    pub(crate) fn main_set(&mut self, key: &RecordKey, row: Option<&Value>) -> Result<()> {
        let model = self.schema.model(&key.model)?.clone();
        match row {
            Some(row) => self.row_upsert(&key.model, &model, row),
            None => self.row_delete(&key.model, &model, &key.identity),
        }
    }
    // The last known server state of a record with the settled local writes
    // on it: the before image and its retained writes while it is dirty with
    // pending mutations, otherwise the visible row itself.
    pub fn truth(&mut self, key: &RecordKey) -> Result<Option<Value>> {
        if self.dirty(key)? {
            let base = self.before_get(key)?;
            let steps = self.history(key)?;
            Ok(settled_view(&base, &steps))
        } else {
            self.read_row(key)
        }
    }
    // Everything above one record's base, in committed local order.
    fn history(&mut self, key: &RecordKey) -> Result<Vec<Step>> {
        let mut steps: Vec<Step> = self.ops_for(key)?.into_iter().map(Step::Pending).collect();
        steps.extend(self.local_writes_for(key)?.into_iter().map(Step::Settled));
        steps.sort_by_key(Step::order);
        Ok(steps)
    }
    pub fn apply_main(&mut self, op: &Operation) -> Result<()> {
        let key = self.schema.record_key(&op.model, &op.identity)?;
        let model = self.schema.model(&op.model)?.clone();
        match op.op {
            OperationKind::Create => {
                let values = op
                    .values
                    .as_ref()
                    .ok_or_else(|| invalid("create values missing"))?;
                let row = merge_identity(&op.identity, values);
                if self.read_row(&key)?.is_some() {
                    return Err(invalid("create already exists"));
                }
                self.row_insert(&op.model, &model, &row)
            }
            OperationKind::Update => {
                let mut row = self.read_row(&key)?;
                apply_to_row(&mut row, op)?;
                self.row_upsert(
                    &op.model,
                    &model,
                    row.as_ref().ok_or_else(|| invalid("update row missing"))?,
                )
            }
            OperationKind::Delete => self.row_delete(&op.model, &model, &op.identity),
        }
    }
    pub fn hold_truth(&mut self, key: &RecordKey) -> Result<()> {
        if self.dirty(key)? {
            return Ok(());
        }
        let model = self.schema.model(&key.model)?.clone();
        self.copy_aside(&model, &key.identity)
    }
    // Rebuild one record from its base and its history in committed local
    // order. Settled writes no earlier pending operation precedes are folded
    // into the base and forgotten; with nothing pending left the base itself
    // becomes the row. When a pending operation no longer replays, the base
    // with the settled writes stays visible and the failing mutation's
    // ordinal is returned and marked diverged; the queue is untouched
    // ([#122](https://github.com/zanminwang/axton/issues/122)).
    pub fn rebuild(&mut self, key: &RecordKey) -> Result<Option<u64>> {
        let mut base = self.before_get(key)?;
        let mut steps = self.history(key)?;
        let settled = steps
            .iter()
            .take_while(|step| matches!(step, Step::Settled(_)))
            .count();
        for step in steps.drain(..settled) {
            if let Step::Settled(write) = step {
                apply_settled(&mut base, &write.op);
                self.retain_local_operation(key, &write.op)?;
                self.delete_local_write(write.sequence)?;
            }
        }
        if steps.is_empty() {
            self.main_set(key, base.as_ref())?;
            self.before_set(key, None)?;
            return Ok(None);
        }
        if settled > 0 {
            self.before_set(key, base.as_ref())?;
        }
        let mut row = base.clone();
        let mut failed = None;
        for step in &steps {
            match step {
                Step::Pending(queued) => {
                    if apply_to_row(&mut row, &queued.op).is_err() {
                        failed = Some(queued.ordinal);
                        break;
                    }
                }
                Step::Settled(write) => apply_settled(&mut row, &write.op),
            }
        }
        let fallback = settled_view(&base, &steps);
        let result = if failed.is_some() {
            fallback.clone()
        } else {
            row
        };
        if self.main_set(key, result.as_ref()).is_err() {
            self.main_set(key, fallback.as_ref())?;
        }
        if let Some(ordinal) = failed {
            self.set_diverged(ordinal)?;
        }
        Ok(failed)
    }
    // Every record reachable from `parent` through declared cascading deletes.
    pub fn descendants(&mut self, parent: &RecordKey) -> Result<Vec<RecordKey>> {
        self.descendants_where(parent, |_, _| Ok(true))
    }
    pub(crate) fn descendants_where(
        &mut self,
        parent: &RecordKey,
        mut admit: impl FnMut(&mut Self, &RecordKey) -> Result<bool>,
    ) -> Result<Vec<RecordKey>> {
        let schema = self.schema;
        let mut seen = BTreeSet::from([parent.encoded()?]);
        let mut todo = vec![parent.clone()];
        let mut result = vec![];
        while let Some(parent) = todo.pop() {
            for model in &schema.models {
                for relation in &model.relations {
                    if relation.target != parent.model || relation.on_delete != "delete" {
                        continue;
                    }
                    let filter: Vec<(String, Value)> = relation
                        .fields
                        .iter()
                        .zip(&relation.target_fields)
                        .map(|(local, target)| (local.clone(), parent.identity[target].clone()))
                        .collect();
                    let mut identities = self.identities_where(&model.name, model, &filter)?;
                    identities.extend(self.identities_where(
                        &before_table(&model.name),
                        model,
                        &filter,
                    )?);
                    for identity in identities {
                        let child = schema.record_key(&model.name, &identity)?;
                        if seen.insert(child.encoded()?) && admit(self, &child)? {
                            todo.push(child.clone());
                            result.push(child);
                        }
                    }
                }
            }
        }
        Ok(result)
    }
    // Whether the last delete of `parent` by call `ordinal` is followed by a
    // local recreation: a create later in the same call, or an accepted
    // receipt state retained after it. A recreation
    // by a later pending call is not one: the server has yet to answer both.
    fn recreated_locally(&mut self, parent: &RecordKey, ordinal: u64) -> Result<bool> {
        let steps = self.history(parent)?;
        let Some(at) = steps.iter().rposition(|step| {
            matches!(step, Step::Pending(q) if q.ordinal == ordinal && q.op.op == OperationKind::Delete)
        }) else {
            return Ok(false);
        };
        Ok(steps[at + 1..].iter().any(|step| match step {
            Step::Pending(q) => q.ordinal == ordinal && q.op.op == OperationKind::Create,
            Step::Settled(w) => w.op.op == OperationKind::Create,
        }))
    }
    // Extend queued deletes to descendants that appeared after they were
    // queued. A parent visible again because a local write recreated it
    // after the delete keeps its current children: they belong to that
    // recreation, not to the earlier delete.
    pub fn refresh_pending(&mut self) -> Result<()> {
        for queued in self.queued()? {
            let deletes: Vec<Operation> = queued
                .mutation
                .operations
                .iter()
                .filter(|op| op.op == OperationKind::Delete)
                .cloned()
                .collect();
            for op in deletes {
                let parent = self.schema.record_key(&op.model, &op.identity)?;
                if self.read_row(&parent)?.is_some()
                    && self.recreated_locally(&parent, queued.ordinal)?
                {
                    continue;
                }
                for child in self.descendants(&parent)? {
                    let already = queued.mutation.effects.iter().any(|e| {
                        e.op == OperationKind::Delete
                            && e.model == child.model
                            && e.identity == child.identity
                    });
                    if already {
                        continue;
                    }
                    self.hold_truth(&child)?;
                    self.append_op(
                        queued.ordinal,
                        OpKind::Effect,
                        &Operation {
                            model: child.model.clone(),
                            identity: child.identity.clone(),
                            op: OperationKind::Delete,
                            values: None,
                        },
                    )?;
                    // A child whose own replay fails was already reported when
                    // it was held; here only the delete effect is extended.
                    self.rebuild(&child)?;
                }
            }
        }
        Ok(())
    }

    // Apply one queued operation in local order and hand every write to
    // `record` as it happens: each record's base is held first, a delete's
    // cascade to its descendants comes before the delete (as `cascade`),
    // then the operation itself (as `own`), so a cascade is stored at its
    // trigger.
    pub(crate) fn apply_in_order(
        &mut self,
        op: &Operation,
        own: OpKind,
        cascade: OpKind,
        mut record: impl FnMut(&mut Self, OpKind, Operation) -> Result<()>,
    ) -> Result<()> {
        let key = self.schema.record_key(&op.model, &op.identity)?;
        self.hold_truth(&key)?;
        if op.op == OperationKind::Delete {
            for child in self.descendants(&key)? {
                self.hold_truth(&child)?;
                let delete = Operation {
                    model: child.model,
                    identity: child.identity,
                    op: OperationKind::Delete,
                    values: None,
                };
                self.apply_main(&delete)?;
                record(self, cascade, delete)?;
            }
        }
        self.apply_main(op)?;
        record(self, own, op.clone())
    }
}
