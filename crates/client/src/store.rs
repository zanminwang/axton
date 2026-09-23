//! Storage contract: a SQL executor with transactions. The engine owns every statement.
use axton_core::Result;
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SqlRows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

pub trait ClientStore {
    fn begin(&mut self) -> Result<()>;
    fn commit(&mut self) -> Result<()>;
    fn rollback(&mut self) -> Result<()>;
    fn savepoint(&mut self, name: &str) -> Result<()>;
    fn release(&mut self, name: &str) -> Result<()>;
    fn rollback_to(&mut self, name: &str) -> Result<()>;
    fn execute(&mut self, sql: &str, parameters: &[Value]) -> Result<usize>;
    fn execute_batch(&mut self, sql: &str) -> Result<()>;
    fn query(&mut self, sql: &str, parameters: &[Value]) -> Result<SqlRows>;
    fn query_committed(&mut self, sql: &str, parameters: &[Value]) -> Result<SqlRows>;
}
