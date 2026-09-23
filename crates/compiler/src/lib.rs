//! The schema compiler: [`parse`] turns `.model` text into declarations,
//! [`validate`] checks them into a typed [`Validated`] schema, and
//! [`generate`] renders the runtime descriptors and the generated code.
use serde_json::Value;
mod emit;
pub mod generate;
mod history;
pub mod parse;
pub mod validate;
pub use emit::{backend_typescript, client_typescript, dart, typescript};
pub use history::{
    check_fence, reconcile_action_history, reconcile_history, reconcile_model_history,
};
pub use parse::{Declarations, Pos, parse};
pub use validate::{Validated, validate};

/// Parse, validate and generate the descriptors of one schema source.
pub fn compile(source: &str) -> Result<Value, String> {
    Ok(generate::descriptors(&validate(&parse(source)?)?))
}
