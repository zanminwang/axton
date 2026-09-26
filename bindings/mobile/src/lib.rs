//! C ABI used by mobile platform modules: `axton_mobile_call` on a dedicated
//! worker queue, and the runtime actor's open/submit/drain/detach.
use axton_binding::{RuntimeHost, ffi};
use serde_json::json;
use std::{
    ffi::{CStr, CString, c_char, c_void},
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

/// Opens a Rust-owned client runtime
/// ([#134](https://github.com/zanminwang/axton/issues/134)). Answers the
/// runtime id, or 0 with `*error_out` set (freed with [`axton_mobile_free`]).
/// `wake` is called from the runtime's thread with `context`.
///
/// # Safety
/// See `axton_binding::ffi::open`: `context` stays valid for `wake` until
/// [`axton_mobile_runtime_detach`] of this id returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn axton_mobile_runtime_open(
    request: *const c_char,
    wake: Option<ffi::Wake>,
    context: *mut c_void,
    error_out: *mut *mut c_char,
) -> u64 {
    unsafe { ffi::open(request, wake, context, error_out) }
}

/// Admits one envelope: 0, or 1 with `*error_out` set.
///
/// # Safety
/// See `axton_binding::ffi::submit`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn axton_mobile_runtime_submit(
    runtime: u64,
    message: *const c_char,
    error_out: *mut *mut c_char,
) -> i32 {
    unsafe { ffi::submit(runtime, message, error_out) }
}

/// The published events as a JSON array, freed with [`axton_mobile_free`].
#[unsafe(no_mangle)]
pub extern "C" fn axton_mobile_runtime_drain(runtime: u64) -> *mut c_char {
    ffi::drain(runtime)
}

/// Stops wakes and forgets the runtime; the wake context may be released once
/// this returns.
#[unsafe(no_mangle)]
pub extern "C" fn axton_mobile_runtime_detach(runtime: u64) {
    ffi::detach(runtime)
}

/// Frees a response allocated by [`axton_mobile_call`] or the
/// `axton_mobile_runtime` functions.
///
/// # Safety
/// `output` must be null or a pointer returned by one of those functions that
/// has not already been freed.
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

    /// The wake context: a channel the test waits on, never a sleep.
    extern "C" fn wake(runtime: u64, context: *mut std::ffi::c_void) {
        let sender =
            unsafe { &*(context as *const std::sync::Mutex<std::sync::mpsc::Sender<u64>>) };
        let _ = sender.lock().unwrap().send(runtime);
    }

    fn drained(runtime: u64) -> Vec<serde_json::Value> {
        let output = super::axton_mobile_runtime_drain(runtime);
        let text = unsafe { CStr::from_ptr(output) }
            .to_str()
            .unwrap()
            .to_owned();
        unsafe { super::axton_mobile_free(output) };
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn runtime_carrier_opens_wakes_drains_and_detaches_across_the_c_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
        let (sender, wakes) = std::sync::mpsc::channel::<u64>();
        let context = Box::into_raw(Box::new(std::sync::Mutex::new(sender)));
        let request = CString::new(
            serde_json::json!({"type":"open","requestId":"1","path":dir.path().join("db"),"schema":schema})
                .to_string(),
        )
        .unwrap();
        let mut error = std::ptr::null_mut();
        let runtime = unsafe {
            super::axton_mobile_runtime_open(
                request.as_ptr(),
                Some(wake),
                context.cast(),
                &mut error,
            )
        };
        assert!(runtime > 0 && error.is_null());
        let mut events = vec![];
        let wait = |events: &mut Vec<serde_json::Value>, kind: &str| loop {
            if let Some(i) = events.iter().position(|e| e["type"] == kind) {
                return events.remove(i);
            }
            assert_eq!(
                wakes
                    .recv_timeout(std::time::Duration::from_secs(20))
                    .unwrap(),
                runtime
            );
            events.extend(drained(runtime));
        };
        let opened = wait(&mut events, "taskCompleted");
        assert_eq!(opened["ok"], true);
        assert!(opened["value"]["clientId"].is_string());

        let close = CString::new(r#"{"type":"close"}"#).unwrap();
        assert_eq!(
            unsafe { super::axton_mobile_runtime_submit(runtime, close.as_ptr(), &mut error) },
            0
        );
        wait(&mut events, "runtimeClosed");
        super::axton_mobile_runtime_detach(runtime);
        // No wake can run after detach returned: the context can go.
        drop(unsafe { Box::from_raw(context) });
        assert_eq!(
            unsafe { super::axton_mobile_runtime_submit(runtime, close.as_ptr(), &mut error) },
            1
        );
        let message = unsafe { CStr::from_ptr(error) }
            .to_str()
            .unwrap()
            .to_owned();
        unsafe { super::axton_mobile_free(error) };
        assert_eq!(message, "client_closed");

        // Refusals at admission set the error and answer 0.
        let mut error = std::ptr::null_mut();
        let refused = unsafe {
            super::axton_mobile_runtime_open(
                std::ptr::null(),
                Some(wake),
                std::ptr::null_mut(),
                &mut error,
            )
        };
        assert_eq!(refused, 0);
        assert_eq!(
            unsafe { CStr::from_ptr(error) }.to_str().unwrap(),
            "null input"
        );
        unsafe { super::axton_mobile_free(error) };
        let mut error = std::ptr::null_mut();
        let refused = unsafe {
            super::axton_mobile_runtime_open(
                request.as_ptr(),
                None,
                std::ptr::null_mut(),
                &mut error,
            )
        };
        assert_eq!(refused, 0);
        unsafe { super::axton_mobile_free(error) };
        assert_eq!(drained(u64::MAX), Vec::<serde_json::Value>::new());
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
