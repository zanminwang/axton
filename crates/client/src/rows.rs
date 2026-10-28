//! JSON ↔ SQL row codec and operations on a model's main and before tables.
use crate::ddl::{before_table, quote};
use crate::engine::{Engine, as_u64};
use crate::store::ClientStore;
use axton_core::{ModelDescriptor, RecordKey, Result, ScalarType, ValueType, invalid};
use serde_json::{Map, Value};

pub fn columns_sql(model: &ModelDescriptor) -> String {
    model
        .fields
        .iter()
        .map(|f| quote(&f.name))
        .collect::<Vec<_>>()
        .join(",")
}
pub fn identity_where(model: &ModelDescriptor) -> String {
    model
        .identity
        .iter()
        .map(|f| format!("{}=?", quote(f)))
        .collect::<Vec<_>>()
        .join(" AND ")
}
pub fn identity_params(model: &ModelDescriptor, identity: &Value) -> Vec<Value> {
    model.identity.iter().map(|f| identity[f].clone()).collect()
}
fn row_params(model: &ModelDescriptor, row: &Value) -> Vec<Value> {
    model
        .fields
        .iter()
        .map(|f| row.get(&f.name).cloned().unwrap_or(Value::Null))
        .collect()
}
fn decode_value(value_type: &ValueType, value: &Value) -> Result<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    Ok(match value_type {
        ValueType::Scalar {
            name: ScalarType::Boolean,
        } => Value::Bool(value.as_i64().unwrap_or(0) != 0),
        ValueType::List { .. } => serde_json::from_str(
            value
                .as_str()
                .ok_or_else(|| invalid("list column must be text"))?,
        )?,
        _ => value.clone(),
    })
}
pub fn decode_row(model: &ModelDescriptor, columns: &[String], row: &[Value]) -> Result<Value> {
    let mut object = Map::new();
    for (column, value) in columns.iter().zip(row) {
        let field = model
            .fields
            .iter()
            .find(|f| &f.name == column)
            .ok_or_else(|| invalid(format!("unknown column {column}")))?;
        object.insert(column.clone(), decode_value(&field.value_type, value)?);
    }
    Ok(Value::Object(object))
}
pub fn merge_identity(identity: &Value, state: &Value) -> Value {
    let mut object = identity.as_object().cloned().unwrap_or_default();
    for (k, v) in state.as_object().into_iter().flatten() {
        object.insert(k.clone(), v.clone());
    }
    Value::Object(object)
}
fn filter_sql(filter: &[(String, Value)]) -> (String, Vec<Value>) {
    if filter.is_empty() {
        return ("1".into(), vec![]);
    }
    let mut clauses = vec![];
    let mut params = vec![];
    for (field, value) in filter {
        if value.is_null() {
            clauses.push(format!("{} IS NULL", quote(field)));
        } else {
            clauses.push(format!("{}=?", quote(field)));
            params.push(value.clone());
        }
    }
    (clauses.join(" AND "), params)
}

impl<S: ClientStore> Engine<'_, S> {
    // Only concrete main rows enter the candidate journal. Before images and
    // other engine tables retain their existing table-only notifications.
    fn exec_row(
        &mut self,
        table: &str,
        model: &ModelDescriptor,
        identity: &Value,
        sql: &str,
        parameters: &[Value],
    ) -> Result<usize> {
        if table == model.name {
            self.exec_record(
                table,
                RecordKey {
                    model: model.name.clone(),
                    identity: identity.clone(),
                },
                sql,
                parameters,
            )
        } else {
            self.exec(table, sql, parameters)
        }
    }
    pub fn row_get(
        &mut self,
        table: &str,
        model: &ModelDescriptor,
        identity: &Value,
    ) -> Result<Option<Value>> {
        let sql = format!(
            "SELECT {} FROM {} WHERE {}",
            columns_sql(model),
            quote(table),
            identity_where(model)
        );
        let rows = self.rows(&sql, &identity_params(model, identity))?;
        rows.rows
            .first()
            .map(|r| decode_row(model, &rows.columns, r))
            .transpose()
    }
    pub fn row_insert(&mut self, table: &str, model: &ModelDescriptor, row: &Value) -> Result<()> {
        let placeholders = vec!["?"; model.fields.len()].join(",");
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({placeholders})",
            quote(table),
            columns_sql(model)
        );
        let identity = Value::Object(
            model
                .identity
                .iter()
                .map(|f| (f.clone(), row[f].clone()))
                .collect(),
        );
        self.exec_row(table, model, &identity, &sql, &row_params(model, row))?;
        Ok(())
    }
    pub fn row_upsert(&mut self, table: &str, model: &ModelDescriptor, row: &Value) -> Result<()> {
        let placeholders = vec!["?"; model.fields.len()].join(",");
        let key = model
            .identity
            .iter()
            .map(|f| quote(f))
            .collect::<Vec<_>>()
            .join(",");
        let updates = model
            .fields
            .iter()
            .filter(|f| !model.identity.contains(&f.name))
            .map(|f| format!("{0}=excluded.{0}", quote(&f.name)))
            .collect::<Vec<_>>();
        let action = if updates.is_empty() {
            "DO NOTHING".to_string()
        } else {
            format!("DO UPDATE SET {}", updates.join(","))
        };
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({placeholders}) ON CONFLICT({key}) {action}",
            quote(table),
            columns_sql(model)
        );
        let identity = Value::Object(
            model
                .identity
                .iter()
                .map(|f| (f.clone(), row[f].clone()))
                .collect(),
        );
        self.exec_row(table, model, &identity, &sql, &row_params(model, row))?;
        Ok(())
    }
    pub fn row_delete(
        &mut self,
        table: &str,
        model: &ModelDescriptor,
        identity: &Value,
    ) -> Result<()> {
        let sql = format!(
            "DELETE FROM {} WHERE {}",
            quote(table),
            identity_where(model)
        );
        self.exec_row(
            table,
            model,
            identity,
            &sql,
            &identity_params(model, identity),
        )?;
        Ok(())
    }
    pub(crate) fn rows_where_keys(
        &mut self,
        model: &ModelDescriptor,
        filter: &[(String, Value)],
        keys: &[RecordKey],
    ) -> Result<Vec<Value>> {
        let (clause, filter_params) = filter_sql(filter);
        // Stay below conservative SQLite variable/expression limits. Filters
        // and identities remain SQL predicates, preserving normal query rules.
        let batch = ((900usize.saturating_sub(filter_params.len())) / model.identity.len().max(1))
            .clamp(1, 128);
        let mut result = vec![];
        for keys in keys.chunks(batch) {
            let identities = vec![format!("({})", identity_where(model)); keys.len()].join(" OR ");
            let sql = format!(
                "SELECT {} FROM {} WHERE ({clause}) AND ({identities})",
                columns_sql(model),
                quote(&model.name)
            );
            let mut parameters = filter_params.clone();
            for key in keys {
                parameters.extend(identity_params(model, &key.identity));
            }
            let rows = self.rows(&sql, &parameters)?;
            result.extend(
                rows.rows
                    .iter()
                    .map(|row| decode_row(model, &rows.columns, row))
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        Ok(result)
    }
    pub fn rows_where(
        &mut self,
        table: &str,
        model: &ModelDescriptor,
        filter: &[(String, Value)],
    ) -> Result<Vec<Value>> {
        let (clause, params) = filter_sql(filter);
        let sql = format!(
            "SELECT {} FROM {} WHERE {clause}",
            columns_sql(model),
            quote(table)
        );
        let rows = self.rows(&sql, &params)?;
        rows.rows
            .iter()
            .map(|r| decode_row(model, &rows.columns, r))
            .collect()
    }
    pub fn identities_where(
        &mut self,
        table: &str,
        model: &ModelDescriptor,
        filter: &[(String, Value)],
    ) -> Result<Vec<Value>> {
        Ok(self
            .rows_where(table, model, filter)?
            .into_iter()
            .map(|row| {
                Value::Object(
                    model
                        .identity
                        .iter()
                        .map(|f| (f.clone(), row[f].clone()))
                        .collect(),
                )
            })
            .collect())
    }
    pub fn copy_aside(&mut self, model: &ModelDescriptor, identity: &Value) -> Result<()> {
        let before = before_table(&model.name);
        let sql = format!(
            "INSERT OR IGNORE INTO {} ({cols}) SELECT {cols} FROM {} WHERE {}",
            quote(&before),
            quote(&model.name),
            identity_where(model),
            cols = columns_sql(model)
        );
        self.exec(&before, &sql, &identity_params(model, identity))?;
        Ok(())
    }
    pub fn count(&mut self, table: &str) -> Result<u64> {
        let value = self.scalar(&format!("SELECT COUNT(*) FROM {}", quote(table)), &[])?;
        as_u64(&value.unwrap_or(Value::from(0)))
    }
}
