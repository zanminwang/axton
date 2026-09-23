//! SQLite implements the client's storage contract with one writer and one reader connection.
use axton_client::{ClientStore, SqlRows};
use axton_core::{Result, invalid};
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, params_from_iter};
use serde_json::Value;
use std::path::Path;

pub struct SqliteStore {
    writer: Connection,
    reader: Connection,
}

fn db(e: rusqlite::Error) -> axton_core::Error {
    invalid(format!("sqlite: {e}"))
}

fn parameter(value: &Value) -> Result<SqlValue> {
    Ok(match value {
        Value::Null => SqlValue::Null,
        Value::Bool(v) => SqlValue::Integer(i64::from(*v)),
        Value::Number(v) => {
            if let Some(v) = v.as_i64() {
                SqlValue::Integer(v)
            } else {
                SqlValue::Real(v.as_f64().ok_or_else(|| invalid("invalid SQL number"))?)
            }
        }
        Value::String(v) => SqlValue::Text(v.clone()),
        Value::Array(_) | Value::Object(_) => SqlValue::Text(serde_json::to_string(value)?),
    })
}

fn rows(connection: &Connection, sql: &str, parameters: &[Value]) -> Result<SqlRows> {
    let mut statement = connection.prepare(sql).map_err(db)?;
    if !statement.readonly() || statement.column_count() == 0 {
        return Err(invalid("SQL write statements are forbidden"));
    }
    let columns = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let values = parameters
        .iter()
        .map(parameter)
        .collect::<Result<Vec<_>>>()?;
    let mut cursor = statement.query(params_from_iter(values)).map_err(db)?;
    let mut output = vec![];
    while let Some(row) = cursor.next().map_err(db)? {
        let mut record = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            record.push(match row.get_ref(index).map_err(db)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(v) => Value::from(v),
                ValueRef::Real(v) => Value::from(v),
                ValueRef::Text(v) => Value::from(
                    std::str::from_utf8(v).map_err(|_| invalid("SQL text must be UTF8"))?,
                ),
                ValueRef::Blob(_) => {
                    return Err(invalid("SQL blobs cannot cross the JSON boundary"));
                }
            });
        }
        output.push(record);
    }
    Ok(SqlRows {
        columns,
        rows: output,
    })
}

fn name_ok(name: &str) -> Result<()> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(invalid("invalid savepoint name"));
    }
    Ok(())
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let writer = Connection::open(&path).map_err(db)?;
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=1000;",
            )
            .map_err(db)?;
        let reader = Connection::open(&path).map_err(db)?;
        reader
            .execute_batch(
                "PRAGMA query_only=ON; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=1000;",
            )
            .map_err(db)?;
        // A double-quoted name that is not a column must be an error, never a
        // string literal: with the legacy fallback a statement prepared
        // against a stale schema silently returns the column's name as text.
        for connection in [&writer, &reader] {
            connection
                .set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DQS_DML, false)
                .map_err(db)?;
            connection
                .set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DQS_DDL, false)
                .map_err(db)?;
        }
        Ok(Self { writer, reader })
    }
}

impl ClientStore for SqliteStore {
    fn begin(&mut self) -> Result<()> {
        self.writer.execute_batch("BEGIN IMMEDIATE").map_err(db)
    }
    fn commit(&mut self) -> Result<()> {
        self.writer.execute_batch("COMMIT").map_err(db)
    }
    fn rollback(&mut self) -> Result<()> {
        self.writer.execute_batch("ROLLBACK").map_err(db)
    }
    fn savepoint(&mut self, name: &str) -> Result<()> {
        name_ok(name)?;
        self.writer
            .execute_batch(&format!("SAVEPOINT {name}"))
            .map_err(db)
    }
    fn release(&mut self, name: &str) -> Result<()> {
        name_ok(name)?;
        self.writer
            .execute_batch(&format!("RELEASE {name}"))
            .map_err(db)
    }
    fn rollback_to(&mut self, name: &str) -> Result<()> {
        name_ok(name)?;
        self.writer
            .execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name}"))
            .map_err(db)
    }
    fn execute(&mut self, sql: &str, parameters: &[Value]) -> Result<usize> {
        let values = parameters
            .iter()
            .map(parameter)
            .collect::<Result<Vec<_>>>()?;
        self.writer
            .execute(sql, params_from_iter(values))
            .map_err(db)
    }
    fn execute_batch(&mut self, sql: &str) -> Result<()> {
        self.writer.execute_batch(sql).map_err(db)
    }
    fn query(&mut self, sql: &str, parameters: &[Value]) -> Result<SqlRows> {
        rows(&self.writer, sql, parameters)
    }
    fn query_committed(&mut self, sql: &str, parameters: &[Value]) -> Result<SqlRows> {
        rows(&self.reader, sql, parameters)
    }
}
