//! Shared schema-driven semantics. Business model types live in generated SDKs.
mod actions;
pub mod authority;
mod loads;
pub mod materialization;
mod protocol;
mod schema;
pub use actions::*;
pub use loads::*;
pub use protocol::*;
pub use schema::*;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
pub fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}
pub fn canonical_json(value: &serde_json::Value) -> Result<String> {
    fn write(value: &serde_json::Value, output: &mut Vec<u8>) -> Result<()> {
        match value {
            serde_json::Value::Object(map) => {
                let mut entries: Vec<_> = map.iter().collect();
                // JavaScript Array.sort compares UTF-16 code units, not UTF-8 bytes.
                entries.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
                output.push(b'{');
                for (index, (key, value)) in entries.into_iter().enumerate() {
                    if index > 0 {
                        output.push(b',');
                    }
                    serde_json::to_writer(&mut *output, key)?;
                    output.push(b':');
                    write(value, output)?;
                }
                output.push(b'}');
            }
            serde_json::Value::Array(values) => {
                output.push(b'[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(b',');
                    }
                    write(value, output)?;
                }
                output.push(b']');
            }
            _ => serde_jcs::to_writer(output, value)?,
        }
        Ok(())
    }
    // Write the entire value into one buffer instead of allocating and joining
    // a canonical String for every child of every object and array.
    let mut output = Vec::new();
    write(value, &mut output)?;
    String::from_utf8(output).map_err(|error| invalid(error.to_string()))
}
