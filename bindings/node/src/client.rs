use napi::{Error,Result};
use napi_derive::napi;
use std::sync::{Mutex,OnceLock};
use axton_binding::RuntimeHost;
static HOST:OnceLock<Mutex<RuntimeHost>>=OnceLock::new();
#[napi]
pub async fn client_call(request:String)->Result<String>{
 let value=serde_json::from_str(&request).map_err(|e|Error::from_reason(e.to_string()))?;
 let result=HOST.get_or_init(||Mutex::new(RuntimeHost::default())).lock().map_err(|_|Error::from_reason("runtime poisoned"))?.call(value).map_err(|e|Error::from_reason(e.to_string()))?;
 Ok(result.to_string())
}
