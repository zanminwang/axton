//! C ABI called only on the Dart SDK's worker isolate.
use axton_binding::RuntimeHost;
use serde_json::json;
use std::{
    ffi::{CStr, CString, c_char},
    sync::{Mutex, OnceLock},
};
static HOST: OnceLock<Mutex<RuntimeHost>> = OnceLock::new();
/// # Safety
/// input must point to a valid NUL-terminated UTF-8 string for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn axton_call(input: *const c_char) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        if input.is_null() {
            return Err("null input".to_string());
        }
        let text = unsafe { CStr::from_ptr(input) }
            .to_str()
            .map_err(|e| e.to_string())?;
        let request = serde_json::from_str(text).map_err(|e| e.to_string())?;
        HOST.get_or_init(|| Mutex::new(RuntimeHost::default()))
            .lock()
            .map_err(|_| "runtime poisoned".to_string())?
            .call(request)
            .map_err(|e| e.to_string())
    });
    let value = match result {
        Ok(Ok(value)) => json!({"ok":true,"result":value}),
        Ok(Err(error)) => json!({"ok":false,"error":error}),
        Err(_) => json!({"ok":false,"error":"runtime panic"}),
    };
    CString::new(value.to_string())
        .expect("JSON escapes NUL")
        .into_raw()
}
/// # Safety
/// output must be a pointer returned by axton_call, freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn axton_free(output: *mut c_char) {
    if !output.is_null() {
        drop(unsafe { CString::from_raw(output) });
    }
}
