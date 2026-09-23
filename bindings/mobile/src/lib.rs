//! C ABI used by mobile platform modules on a dedicated worker queue.
use axton_binding::RuntimeHost;
use serde_json::json;
use std::{
    ffi::{CStr, CString, c_char},
    sync::{Mutex, OnceLock},
};

static HOST: OnceLock<Mutex<RuntimeHost>> = OnceLock::new();

/// Calls the process-wide AXTON runtime host.
///
/// # Safety
/// `input` must point to a valid NUL-terminated UTF-8 string for the duration
/// of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn axton_mobile_call(input: *const c_char) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        if input.is_null() {
            return Err("null input".to_string());
        }
        let text = unsafe { CStr::from_ptr(input) }
            .to_str()
            .map_err(|error| error.to_string())?;
        let request = serde_json::from_str(text).map_err(|error| error.to_string())?;
        // A panic while holding the lock poisons it for the rest of the process,
        // matching the Dart carrier; the app must be relaunched after such a fault.
        HOST.get_or_init(|| Mutex::new(RuntimeHost::default()))
            .lock()
            .map_err(|_| "runtime poisoned".to_string())?
            .call(request)
            .map_err(|error| error.to_string())
    });
    let response = match result {
        Ok(Ok(value)) => json!({"ok": true, "result": value}),
        Ok(Err(error)) => json!({"ok": false, "error": error}),
        Err(_) => json!({"ok": false, "error": "runtime panic"}),
    };
    CString::new(response.to_string())
        .expect("serialized JSON contains no raw NUL")
        .into_raw()
}

/// Frees a response allocated by [`axton_mobile_call`].
///
/// # Safety
/// `output` must be null or a pointer returned by `axton_mobile_call` that has
/// not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn axton_mobile_free(output: *mut c_char) {
    if !output.is_null() {
        drop(unsafe { CString::from_raw(output) });
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, CString};

    fn call(input: &str) -> serde_json::Value {
        let input = CString::new(input).unwrap();
        let output = unsafe { super::axton_mobile_call(input.as_ptr()) };
        assert!(!output.is_null());
        let text = unsafe { CStr::from_ptr(output) }
            .to_str()
            .unwrap()
            .to_owned();
        unsafe { super::axton_mobile_free(output) };
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn rejects_null_and_malformed_inputs_as_json_errors() {
        let output = unsafe { super::axton_mobile_call(std::ptr::null()) };
        let value: serde_json::Value = unsafe { CStr::from_ptr(output) }
            .to_str()
            .map(|text| serde_json::from_str(text).unwrap())
            .unwrap();
        unsafe { super::axton_mobile_free(output) };
        assert_eq!(
            value,
            serde_json::json!({"ok": false, "error": "null input"})
        );

        let malformed = call("{");
        assert_eq!(malformed["ok"], false);
        assert!(malformed["error"].as_str().unwrap().contains("EOF"));
    }

    #[test]
    fn returns_the_runtime_host_envelope_across_the_c_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
        let request = serde_json::json!({
            "op": "open",
            "path": dir.path().join("mobile.db"),
            "schema": schema,
            "owner": "mobile-test"
        });
        let opened = call(&request.to_string());
        assert_eq!(opened["ok"], true);
        assert!(opened["result"]["value"]["handle"].is_number());

        let failed = call(
            &serde_json::json!({
                "op": "open",
                "path": dir.path().join("missing").join("sub").join("mobile.db"),
                "schema": schema
            })
            .to_string(),
        );
        assert_eq!(failed["ok"], false, "open in a missing directory must fail");
        assert!(!failed["error"].as_str().unwrap().is_empty());

        let handle = opened["result"]["value"]["handle"].clone();
        let closed = call(&serde_json::json!({"op":"close", "handle":handle}).to_string());
        assert_eq!(closed["ok"], true);

        let rejected = call(&serde_json::json!({"op":"status", "handle":handle}).to_string());
        assert_eq!(rejected["ok"], false);
        assert!(
            rejected["error"]
                .as_str()
                .unwrap()
                .contains("client_closed")
        );
    }
}
