//! C ABI of the Dart SDK: `axton_call` on its worker isolate, and the
//! runtime actor's open/submit/drain/detach.
use axton_binding::{RuntimeHost, ffi};
use serde_json::json;
use std::{
    ffi::{CStr, CString, c_char, c_void},
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
/// Open a Rust-owned client runtime
/// ([#134](https://github.com/zanminwang/axton/issues/134)). Answers the
/// runtime id, or 0 with `*error_out` set (freed with [`axton_free`]). `wake`
/// is called from the runtime's thread with `context`; a
/// `NativeCallable.listener` schedules the drain on the owning isolate.
///
/// # Safety
/// See `axton_binding::ffi::open`: `context` stays valid for `wake` until
/// [`axton_runtime_detach`] of this id returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn axton_runtime_open(
    request: *const c_char,
    wake: Option<ffi::Wake>,
    context: *mut c_void,
    error_out: *mut *mut c_char,
) -> u64 {
    unsafe { ffi::open(request, wake, context, error_out) }
}
/// Admit one envelope: 0, or 1 with `*error_out` set.
///
/// # Safety
/// See `axton_binding::ffi::submit`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn axton_runtime_submit(
    runtime: u64,
    message: *const c_char,
    error_out: *mut *mut c_char,
) -> i32 {
    unsafe { ffi::submit(runtime, message, error_out) }
}
/// The published events as a JSON array, freed with [`axton_free`].
#[unsafe(no_mangle)]
pub extern "C" fn axton_runtime_drain(runtime: u64) -> *mut c_char {
    ffi::drain(runtime)
}
/// Stop wakes and forget the runtime; the wake context may be released once
/// this returns.
#[unsafe(no_mangle)]
pub extern "C" fn axton_runtime_detach(runtime: u64) {
    ffi::detach(runtime)
}
/// # Safety
/// output must be a pointer returned by axton_call or the axton_runtime
/// functions, freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn axton_free(output: *mut c_char) {
    if !output.is_null() {
        drop(unsafe { CString::from_raw(output) });
    }
}
