//! Model tables. Reconciliation makes them match the compiled schema and
//! resets a Model whose storage shape changed.
use crate::store::ClientStore;
use axton_core::{
    Compatibility, FieldDescriptor, ModelDescriptor, Result, ScalarType, Schema, ValueType, invalid,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub fn before_table(model: &str) -> String {
    format!("axton_before_{model}")
}

pub fn storage_type(value_type: &ValueType) -> &'static str {
    match value_type {
        ValueType::Scalar {
            name: ScalarType::Boolean | ScalarType::Int,
        } => "INTEGER",
        ValueType::Scalar {
            name: ScalarType::Float,
        } => "REAL",
        _ => "TEXT",
    }
}

fn literal(field: &FieldDescriptor) -> Result<String> {
    let value = field.default.as_ref().ok_or_else(|| {
        invalid(format!(
            "column {} is not nullable and has no default",
            field.name
        ))
    })?;
    Ok(match value {
        Value::Bool(b) => i64::from(*b).to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Null => {
            return Err(invalid(format!(
                "column {} default cannot be null",
                field.name
            )));
        }
        other => format!("'{}'", serde_json::to_string(other)?.replace('\'', "''")),
    })
}

fn column(field: &FieldDescriptor) -> String {
    let null = if field.nullable { "" } else { " NOT NULL" };
    format!(
        "{} {}{null}",
        quote(&field.name),
        storage_type(&field.value_type)
    )
}

fn table_ddl(table: &str, model: &ModelDescriptor) -> String {
    let columns = model
        .fields
        .iter()
        .map(column)
        .collect::<Vec<_>>()
        .join(", ");
    let key = model
        .identity
        .iter()
        .map(|f| quote(f))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "CREATE TABLE IF NOT EXISTS {} ({columns}, PRIMARY KEY ({key}))",
        quote(table)
    )
}

pub fn model_ddl(model: &ModelDescriptor) -> Vec<String> {
    let mut statements = vec![
        table_ddl(&model.name, model),
        table_ddl(&before_table(&model.name), model),
    ];
    for fields in &model.unique {
        let name = format!("{}_{}_unique", model.name, fields.join("_"));
        let columns = fields
            .iter()
            .map(|f| quote(f))
            .collect::<Vec<_>>()
            .join(", ");
        statements.push(format!(
            "CREATE UNIQUE INDEX IF NOT EXISTS {} ON {} ({columns})",
            quote(&name),
            quote(&model.name)
        ));
    }
    statements
}

struct Existing {
    columns: BTreeMap<String, String>, // name -> declared type
    identity: Vec<String>,             // pk columns in key order
}

fn existing<S: ClientStore>(store: &mut S, table: &str) -> Result<Option<Existing>> {
    let rows = store.query(&format!("PRAGMA table_info({})", quote(table)), &[])?;
    if rows.rows.is_empty() {
        return Ok(None);
    }
    let mut columns = BTreeMap::new();
    let mut keyed = vec![];
    for row in rows.rows {
        let name = row[1]
            .as_str()
            .ok_or_else(|| invalid("table_info name"))?
            .to_string();
        let ty = row[2].as_str().unwrap_or("").to_ascii_uppercase();
        let pk = row[5].as_i64().unwrap_or(0);
        if pk > 0 {
            keyed.push((pk, name.clone()));
        }
        columns.insert(name, ty);
    }
    keyed.sort();
    Ok(Some(Existing {
        columns,
        identity: keyed.into_iter().map(|(_, n)| n).collect(),
    }))
}

/// Whether the table, as stored, cannot hold `model` in place: its identity
/// or a column's type differs, or a required field without a default is
/// missing. Extra columns do not count.
fn misshaped(current: &Existing, model: &ModelDescriptor) -> bool {
    current.identity != model.identity
        || model
            .fields
            .iter()
            .any(|field| match current.columns.get(&field.name) {
                Some(ty) => ty != storage_type(&field.value_type),
                None => !field.nullable && field.default.is_none(),
            })
}

fn drop_tables<S: ClientStore>(store: &mut S, model: &str) -> Result<()> {
    for table in [model.to_owned(), before_table(model)] {
        store.execute(&format!("DROP TABLE IF EXISTS {}", quote(&table)), &[])?;
    }
    Ok(())
}

/// Make the tables match `schema`. `stored` is the schema they were built
/// for, absent for a fresh file. A Model whose storage shape changed
/// ([`Schema::model_compatibility`] finds it incompatible, or the table cannot
/// hold it) is reset: its table and before-image are dropped and created
/// again. A Model only `stored` has is dropped. An added nullable or
/// defaulted field is added in place. Returns the reset Models, removed ones
/// included; their record state is the caller's.
pub fn reconcile<S: ClientStore>(
    store: &mut S,
    stored: Option<&Schema>,
    schema: &Schema,
) -> Result<Vec<String>> {
    let mut reset = vec![];
    if let Some(stored) = stored {
        for old in &stored.models {
            if schema.model(&old.name).is_err() {
                drop_tables(store, &old.name)?;
                reset.push(old.name.clone());
            }
        }
    }
    for model in &schema.models {
        if let Some(current) = existing(store, &model.name)? {
            let incompatible = stored.is_some_and(|stored| {
                matches!(
                    Schema::model_compatibility(stored, schema, &model.name),
                    Compatibility::Incompatible(_)
                )
            });
            if !incompatible && !misshaped(&current, model) {
                add_fields(store, model, &current)?;
                for statement in model_ddl(model).into_iter().skip(2) {
                    store.execute(&statement, &[])?;
                }
                continue;
            }
            drop_tables(store, &model.name)?;
            reset.push(model.name.clone());
        }
        for statement in model_ddl(model) {
            store.execute(&statement, &[])?;
        }
    }
    Ok(reset)
}

fn add_fields<S: ClientStore>(
    store: &mut S,
    model: &ModelDescriptor,
    current: &Existing,
) -> Result<()> {
    for field in &model.fields {
        if current.columns.contains_key(&field.name) {
            continue;
        }
        let mut definition = column(field);
        if !field.nullable {
            definition.push_str(&format!(" DEFAULT {}", literal(field)?));
        }
        for table in [model.name.clone(), before_table(&model.name)] {
            store.execute(
                &format!("ALTER TABLE {} ADD COLUMN {definition}", quote(&table)),
                &[],
            )?;
        }
    }
    Ok(())
}
