mod client;
/// The transaction-bridge probe used only by the bindings' integration test.
/// It is compiled into the addon only with `--features probe`, so the normal
/// addon exposes no probe API.
#[cfg(feature = "probe")]
mod probe;
mod server;

/// Internal synchronous sample of this addon's monotonic diagnostics clock.
#[napi_derive::napi]
pub fn diagnostics_clock_us() -> f64 {
    axton_binding::diagnostics::clock_us() as f64
}
#[napi_derive::napi]
pub fn diagnostics_clock_domain() -> String {
    axton_binding::diagnostics::clock_domain()
}
