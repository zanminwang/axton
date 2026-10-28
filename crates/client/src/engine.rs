//! One handle per open transaction. Every module adds methods to it.
use crate::changes::ModelChanges;
use crate::store::{ClientStore, SqlRows};
use axton_core::{RecordKey, Result, Schema, invalid};
use serde_json::Value;
use std::collections::BTreeSet;

pub struct Engine<'a, S: ClientStore> {
    pub store: &'a mut S,
    pub schema: &'a Schema,
    pub changed: &'a mut BTreeSet<String>,
    pub committed: bool,
    pub(crate) model_changes: Option<&'a mut ModelChanges>,
}

impl<'a, S: ClientStore> Engine<'a, S> {
    pub fn new(
        store: &'a mut S,
        schema: &'a Schema,
        changed: &'a mut BTreeSet<String>,
        committed: bool,
    ) -> Self {
        Self {
            store,
            schema,
            changed,
            committed,
            model_changes: None,
        }
    }
    pub(crate) fn tracking(mut self, changes: &'a mut ModelChanges) -> Self {
        self.model_changes = Some(changes);
        self
    }
    pub(crate) fn reborrow(&mut self) -> Engine<'_, S> {
        Engine {
            store: &mut *self.store,
            schema: self.schema,
            changed: &mut *self.changed,
            committed: self.committed,
            model_changes: self.model_changes.as_deref_mut(),
        }
    }
    pub fn rows(&mut self, sql: &str, parameters: &[Value]) -> Result<SqlRows> {
        if self.committed {
            self.store.query_committed(sql, parameters)
        } else {
            self.store.query(sql, parameters)
        }
    }
    pub fn exec(&mut self, table: &str, sql: &str, parameters: &[Value]) -> Result<usize> {
        self.changed.insert(table.to_string());
        let affected = self.store.execute(sql, parameters)?;
        // Statements without a precise record identity (including Store reset)
        // conservatively invalidate the Model. Engine tables never do.
        if self.schema.model(table).is_ok()
            && let Some(changes) = &mut self.model_changes
        {
            changes.all(table);
        }
        Ok(affected)
    }
    pub(crate) fn exec_record(
        &mut self,
        table: &str,
        key: RecordKey,
        sql: &str,
        parameters: &[Value],
    ) -> Result<usize> {
        let encoded = key.encoded()?;
        self.changed.insert(table.to_string());
        let affected = self.store.execute(sql, parameters)?;
        if affected != 0
            && let Some(changes) = &mut self.model_changes
        {
            changes.record(encoded, key);
        }
        Ok(affected)
    }
    pub fn scalar(&mut self, sql: &str, parameters: &[Value]) -> Result<Option<Value>> {
        Ok(self
            .rows(sql, parameters)?
            .rows
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next()))
    }
}

pub(crate) fn as_u64(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|v| u64::try_from(v).ok()))
        .ok_or_else(|| invalid("expected an unsigned integer"))
}
