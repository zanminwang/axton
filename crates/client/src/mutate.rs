//! Optimistic writes, truth holding, rebuild and cascades. Authority lands
//! through `authority.rs`.
use crate::ddl::before_table;
use crate::engine::Engine;
use crate::rows::merge_identity;
use crate::store::ClientStore;
use crate::{Mutation, Operation, OperationKind, actions, policies};
use axton_core::{ActionIntent, RecordKey, Result, Schema, invalid};
use serde_json::Value;
use std::collections::BTreeSet;

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

fn normalize(schema: &Schema, op: &mut Operation) -> Result<()> {
    op.identity = schema.record_key(&op.model, &op.identity)?.identity;
    match op.op {
        OperationKind::Create => {
            let values = op
                .values
                .as_ref()
                .ok_or_else(|| invalid("create values missing"))?;
            op.values = Some(schema.normalize_state(&op.model, values)?);
        }
        OperationKind::Update => {
            let values = op
                .values
                .as_ref()
                .ok_or_else(|| invalid("update values missing"))?;
            op.values = Some(schema.validate_patch(&op.model, values)?);
        }
        OperationKind::Delete => {
            if op.values.is_some() {
                return Err(invalid("delete cannot contain values"));
            }
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
    /// The last known server state of a record: the before image while it is
    /// dirty with pending mutations, otherwise the visible row itself.
    pub fn truth(&mut self, key: &RecordKey) -> Result<Option<Value>> {
        if self.dirty(key)? {
            self.before_get(key)
        } else {
            self.read_row(key)
        }
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
    /// Replay every still-queued operation for one record over its held truth.
    /// When a replay fails the truth stays visible and the failing mutation's
    /// ordinal is returned and marked diverged; the queue is untouched
    /// ([#122](https://github.com/zanminwang/axton/issues/122)).
    pub fn rebuild(&mut self, key: &RecordKey) -> Result<Option<u64>> {
        let truth = self.before_get(key)?;
        let ops = self.ops_for(key)?;
        let mut row = truth.clone();
        let mut failed = None;
        for queued in &ops {
            if apply_to_row(&mut row, &queued.op).is_err() {
                failed = Some(queued.ordinal);
                break;
            }
        }
        let result = if failed.is_some() { truth.clone() } else { row };
        if self.main_set(key, result.as_ref()).is_err() {
            self.main_set(key, truth.as_ref())?;
        }
        if ops.is_empty() {
            self.before_set(key, None)?;
        }
        if let Some(ordinal) = failed {
            self.set_diverged(ordinal)?;
        }
        Ok(failed)
    }
    /// Every record reachable from `parent` through declared cascading deletes.
    pub fn descendants(&mut self, parent: &RecordKey) -> Result<Vec<RecordKey>> {
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
                        if seen.insert(child.encoded()?) {
                            todo.push(child.clone());
                            result.push(child);
                        }
                    }
                }
            }
        }
        Ok(result)
    }
    /// Extend queued deletes to descendants that appeared after they were queued.
    pub fn refresh_pending(&mut self) -> Result<()> {
        for queued in self.queued()? {
            let deletes: Vec<Operation> = queued
                .mutation
                .operations
                .iter()
                .chain(&queued.mutation.companion)
                .filter(|op| op.op == OperationKind::Delete)
                .cloned()
                .collect();
            for op in deletes {
                let parent = self.schema.record_key(&op.model, &op.identity)?;
                for child in self.descendants(&parent)? {
                    let already = queued
                        .mutation
                        .effects
                        .iter()
                        .any(|e| e.model == child.model && e.identity == child.identity);
                    if already {
                        continue;
                    }
                    self.hold_truth(&child)?;
                    self.add_effect(
                        queued.ordinal,
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
    pub fn enqueue(&mut self, mut mutation: Mutation) -> Result<u64> {
        if mutation.name.trim().is_empty()
            || mutation.version == 0
            || (mutation.operations.is_empty() && mutation.call_id.is_none())
        {
            return Err(invalid("invalid named mutation"));
        }
        if mutation.call_id.is_some() != mutation.args.is_some() {
            return Err(invalid("Action identity and args must appear together"));
        }
        if mutation.call_id.is_none() && !mutation.store.is_all() {
            return Err(invalid("store policy requires an Action call"));
        }
        if let (Some(call_id), Some(args)) = (&mutation.call_id, &mutation.args) {
            let intent = ActionIntent {
                call_id: call_id.clone(),
                name: mutation.name.clone(),
                version: mutation.version,
                args: args.clone(),
                store: mutation.store.clone(),
            }
            .normalize(self.schema)?;
            let descriptor = self.schema.action(&intent.name, intent.version)?;
            actions::validate_bindings(self.schema, descriptor, &intent.args)?;
            let expected = actions::derive_operations(self.schema, descriptor, &intent.args)?;
            if intent.call_id != *call_id
                || intent.args != *args
                || serde_json::to_value(expected)? != serde_json::to_value(&mutation.operations)?
                || !mutation.companion.is_empty()
                || !mutation.effects.is_empty()
                || !mutation.prerequisites.is_empty()
                || !mutation.lifecycle_dependencies.is_empty()
                || !mutation.sequence_dependencies.is_empty()
            {
                return Err(invalid(
                    "Action queue row does not match its canonical intent",
                ));
            }
        }
        for dependency in mutation
            .lifecycle_dependencies
            .iter()
            .chain(&mutation.sequence_dependencies)
        {
            if self.queued_one(*dependency)?.is_none() {
                return Err(invalid("unknown mutation dependency"));
            }
        }
        mutation.effects.clear();
        let mut effects = vec![];
        let wire = mutation.operations.len();
        let mut all: Vec<Operation> = mutation
            .operations
            .drain(..)
            .chain(mutation.companion.drain(..))
            .collect();
        // Each hold_truth runs before its operation reaches the queue, so `dirty`
        // still reflects only earlier mutations.
        for op in all.iter_mut() {
            normalize(self.schema, op)?;
            let key = self.schema.record_key(&op.model, &op.identity)?;
            self.hold_truth(&key)?;
            if op.op == OperationKind::Delete {
                for child in self.descendants(&key)? {
                    self.hold_truth(&child)?;
                    let effect = Operation {
                        model: child.model,
                        identity: child.identity,
                        op: OperationKind::Delete,
                        values: None,
                    };
                    self.apply_main(&effect)?;
                    effects.push(effect);
                }
            }
            self.apply_main(op)?;
        }
        mutation.companion = all.split_off(wire);
        mutation.operations = all;
        mutation.effects = effects;
        policies::derive(self, &mut mutation)?;
        let ordinal = self.allocate_ordinal()?;
        self.insert_mutation(ordinal, &mutation)?;
        Ok(ordinal)
    }
    /// A local write that is never sent: it moves the truth along with the row.
    pub fn direct(&mut self, mut operation: Operation) -> Result<()> {
        normalize(self.schema, &mut operation)?;
        let key = self
            .schema
            .record_key(&operation.model, &operation.identity)?;
        if operation.op == OperationKind::Delete {
            for child in self.descendants(&key)? {
                self.direct_one(Operation {
                    model: child.model,
                    identity: child.identity,
                    op: OperationKind::Delete,
                    values: None,
                })?;
            }
        }
        self.direct_one(operation)
    }
    fn direct_one(&mut self, operation: Operation) -> Result<()> {
        let key = self
            .schema
            .record_key(&operation.model, &operation.identity)?;
        let is_dirty = self.dirty(&key)?;
        self.apply_main(&operation)?;
        if is_dirty {
            let mut truth = self.before_get(&key)?;
            if truth.is_none() && operation.op != OperationKind::Create {
                // The row's existence is itself pending: there is no rollback base
                // to advance. The direct write lives only in the main row and goes
                // with the create if the create is rejected.
                return Ok(());
            }
            if operation.op == OperationKind::Delete {
                self.before_set(&key, None)?;
            } else if apply_to_row(&mut truth, &operation).is_ok() {
                self.before_set(&key, truth.as_ref())?;
            } else {
                let current = self.read_row(&key)?;
                self.before_set(&key, current.as_ref())?;
            }
        }
        Ok(())
    }
    /// Stop following a channel. Records it delivered stay: a channel is a
    /// delivery path, not an owner, so local content, stamps, before images
    /// and pending operations are all retained.
    pub fn unsubscribe(&mut self, channel: &str) -> Result<()> {
        self.delete_subscription(channel)
    }
}
