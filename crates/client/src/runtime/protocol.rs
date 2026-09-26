//! The bridge contract between an SDK and its Rust-owned client runtime
//! ([#134](https://github.com/zanminwang/axton/issues/134)).
//!
//! An SDK submits complete tasks and answers the effects the runtime asks
//! for; the runtime owns every decision in between and reports outcomes as
//! events. JSON stays the cross-language encoding: every [`Input`] and
//! [`Event`] is one tagged object, and every identifier the runtime allocates
//! travels as a string so no language has to represent a 64-bit counter.
//!
//! Identities:
//!
//! | Identity | Allocated by | Purpose |
//! | --- | --- | --- |
//! | `requestId` | the SDK, increasing per bridge, never reused | route one submitted task or transaction command to its one terminal outcome |
//! | `effectId` | the runtime, unique for its lifetime | correlate one HTTP/timer/callback effect or the lifetime of one socket |
//! | `transactionId` | the runtime, fresh per callback transaction | admit commands only into the transaction that owns them |
//! | `scope` | the runtime, fresh per nested savepoint | admit commands only into the innermost open savepoint |
//! | `callId` | the existing durable call identity | final Call outcomes; never replaced by a request id |
//! | `observerId` | the runtime | route committed watch/subscription snapshots |
//!
//! Admission is not completion: the carrier acknowledges that a message was
//! copied into the runtime's mailbox, and the public Promise/Future resolves
//! only from the matching [`Event::TaskCompleted`]. A duplicate active request
//! id is a protocol error reported through [`Diagnostic::Protocol`]; it never
//! executes a second time and never touches the first route.
use crate::Report;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What an SDK sends to its runtime.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Input {
    /// One complete unit of application work. `command` is a `{kind, …}`
    /// object; the kinds are the runtime's command set
    /// ([`super::commands`]). A `transaction` task asks for an application
    /// callback: the runtime answers with an [`Operation::Callback`] effect and
    /// completes the task only after the callback's result committed or
    /// rolled back.
    #[serde(rename_all = "camelCase")]
    Task { request_id: String, command: Value },
    /// A command of the application callback that owns `transaction_id`:
    /// reads, writes, and `savepoint` / `release` / `rollbackSavepoint`.
    /// `scope` names the innermost open savepoint the command runs in, or is
    /// absent at the transaction's top level. A command naming a transaction
    /// that is not open fails with `transaction_closed`; one naming a scope
    /// that is not the innermost open one fails with `invalid transaction
    /// scope`, a structural failure that rolls the whole unit back at the
    /// callback's end. Neither joins the session. These commands are serviced
    /// on their own lane, never behind the ordinary queue their parent task is
    /// holding.
    #[serde(rename_all = "camelCase")]
    TransactionCommand {
        request_id: String,
        transaction_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        command: Value,
    },
    /// The application callback of `effect_id` finished. `ok` commits the
    /// transaction (the SDK retains the callback's own return value in
    /// language memory and resolves with it once the commit is confirmed);
    /// a failure rolls it back and the parent task fails with `error`, which
    /// is the SDK's rendering of the thrown value. A result naming a
    /// transaction that is not open is ignored.
    #[serde(rename_all = "camelCase")]
    CallbackResult {
        effect_id: String,
        transaction_id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// The host's answer to one effect. HTTP, timer, credential and
    /// prerequisite effects are single-use: their first result retires the
    /// id and a later one is ignored. A socket effect is a stream: every
    /// result carries a [`SocketEvent`] value under the same id until
    /// `closed`, and a result for an id the runtime cancelled or never issued
    /// is ignored.
    #[serde(rename_all = "camelCase")]
    EffectResult {
        effect_id: String,
        outcome: EffectOutcome,
    },
    /// Close the runtime: priority control, never a task parked behind a
    /// callback. Every pending task completes or fails, an open transaction
    /// rolls back, every effect is cancelled, observers end, then
    /// [`Event::RuntimeClosed`] is the last event.
    Close,
}

/// What one effect came to. `ok` with `value` for a success, otherwise
/// `error`. Socket results put the [`SocketEvent`] in `value`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EffectOutcome {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EffectError>,
}

impl EffectOutcome {
    pub fn success(value: Value) -> Self {
        Self {
            ok: true,
            value: Some(value),
            error: None,
        }
    }
    pub fn failure(message: impl Into<String>, status: Option<u16>) -> Self {
        Self {
            ok: false,
            value: None,
            error: Some(EffectError {
                message: message.into(),
                status,
            }),
        }
    }
}

/// Why an effect failed, as the host saw it. `status` is the HTTP status the
/// failure carried, when it had one: the runtime tells a refusal the server
/// decided from a transport failure by it, and a 401 is what asks for a
/// credential refresh.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EffectError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

/// The `value` of a socket effect's results: the stream of one socket
/// session. `opened` is optional information; `message` carries one frame;
/// `overflow` says the host's own frame buffer dropped frames; `closed` ends
/// the stream (a failure to open or a dropped socket is reported as an
/// `ok: false` result instead and ends it the same way).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum SocketEvent {
    Opened,
    Message { body: String },
    Overflow,
    Closed,
}

/// What the runtime tells its SDK, in the order it happened.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    /// The one terminal outcome of a submitted task or transaction command.
    /// The SDK removes the route before it runs application code and settles
    /// it exactly once. `error` is the engine's message; no SDK branch depends
    /// on its wording beyond the codes it already recognizes
    /// (`client_closed`, `transaction_closed`, `transaction_active`, …).
    #[serde(rename_all = "camelCase")]
    TaskCompleted {
        request_id: String,
        ok: bool,
        /// The task's value when `ok`; `null` otherwise.
        #[serde(default)]
        value: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Host work the runtime needs: execute it and answer with an
    /// [`Input::EffectResult`] (or, for a callback, an
    /// [`Input::CallbackResult`]).
    #[serde(rename_all = "camelCase")]
    Effect {
        effect_id: String,
        operation: Operation,
    },
    /// Abort the host work of this effect if it is still running. A late
    /// answer is fenced in the runtime either way; cancellation only frees
    /// the platform resource.
    #[serde(rename_all = "camelCase")]
    CancelEffect { effect_id: String },
    /// A durable call's final outcome, emitted after the settlement that
    /// decided it committed. The SDK created the Call handle when the
    /// submission task completed, so this can never outrun its registration.
    #[serde(rename_all = "camelCase")]
    CallCompleted { call_id: String, outcome: Value },
    /// Committed state of one observer: a watch's rows or a subscription's
    /// status. The SDK delivers it to the language-level listeners; a
    /// listener's exception changes nothing here.
    #[serde(rename_all = "camelCase")]
    ObserverChanged {
        observer_id: String,
        snapshot: Value,
    },
    /// Something the application should hear about that is not a task
    /// outcome: records a delivery could not apply, a lane failure, or a
    /// bridge protocol violation.
    Report { diagnostic: Diagnostic },
    /// A local transaction committed and touched these tables (framework
    /// tables included). Until watch observers move into the runtime, the
    /// SDKs re-run their watched queries on it.
    Changed { tables: Vec<String> },
    /// The runtime is gone; nothing follows. The SDK drains it, releases its
    /// platform resources and detaches the carrier.
    RuntimeClosed,
}

/// The host work one effect asks for.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Operation {
    /// Run the application's transaction callback for the `transaction` task
    /// `request_id`; its commands carry `transaction_id`, and its end is a
    /// [`Input::CallbackResult`] for this effect.
    #[serde(rename_all = "camelCase")]
    Callback {
        transaction_id: String,
        request_id: String,
    },
    /// `POST` `body` to the route; answer with the response text, or a
    /// failure carrying the HTTP status when there was one.
    Http { route: HttpRoute, body: String },
    /// Open the live socket, send `subscribe` once it is open, and stream its
    /// frames as [`SocketEvent`] results under this id until it closes or is
    /// cancelled.
    Socket { subscribe: String },
    /// Answer after `millis`; cancellation clears the timer.
    Timer { millis: u64 },
    /// Run the application's `refreshAuth` once; answer when it settles.
    /// The runtime issues at most one at a time.
    RefreshAuth,
    /// Run the application's prerequisite handler `name` with `arguments`;
    /// answer `ok` when it resolves, or a failure whose message is the reason
    /// to keep for the task `key`.
    Prerequisite {
        key: String,
        name: String,
        arguments: Value,
    },
}

/// Which backend route an HTTP effect posts to.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum HttpRoute {
    /// `/sync/mutations`: a frozen push batch.
    Push,
    /// `/sync/pull`: an ordinary catch-up or a Bootstrap page.
    Pull,
    /// `/sync/actions`: a direct Query or Mutation.
    Action,
}

/// What a [`Event::Report`] carries.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Diagnostic {
    /// Records a receipt, page or direct response could not apply; the
    /// client stays consistent and the application hears about each one.
    Records { reports: Vec<Report> },
    /// A lane or effect failure the application's `onError` would have seen:
    /// a transport error, a protocol violation the runtime closed a socket
    /// for, a failed credential refresh.
    Error { message: String },
    /// The SDK violated the bridge contract: a malformed envelope or a
    /// duplicate active request id. Nothing executed for it.
    Protocol { message: String },
}

/// Why the runtime refused to admit an input. Admission failures are answered
/// synchronously by the carrier; they are never task outcomes.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BridgeError {
    /// The runtime has closed; nothing more is admitted.
    #[error("client_closed")]
    Closed,
    /// The envelope did not decode as an [`Input`].
    #[error("malformed bridge message: {0}")]
    Malformed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelopes_round_trip_with_the_documented_spellings() {
        let inputs = [
            (
                json!({"type":"task","requestId":"42","command":{"kind":"read","key":{"model":"Todo","identity":{"id":"t"}}}}),
                Input::Task {
                    request_id: "42".into(),
                    command: json!({"kind":"read","key":{"model":"Todo","identity":{"id":"t"}}}),
                },
            ),
            (
                json!({"type":"transactionCommand","requestId":"43","transactionId":"tx7","scope":"sp1","command":{"kind":"direct","operation":{}}}),
                Input::TransactionCommand {
                    request_id: "43".into(),
                    transaction_id: "tx7".into(),
                    scope: Some("sp1".into()),
                    command: json!({"kind":"direct","operation":{}}),
                },
            ),
            (
                json!({"type":"callbackResult","effectId":"5","transactionId":"tx7","ok":false,"error":"boom"}),
                Input::CallbackResult {
                    effect_id: "5".into(),
                    transaction_id: "tx7".into(),
                    ok: false,
                    error: Some("boom".into()),
                },
            ),
            (
                json!({"type":"effectResult","effectId":"101","outcome":{"ok":true,"value":{"event":"message","body":"{}"}}}),
                Input::EffectResult {
                    effect_id: "101".into(),
                    outcome: EffectOutcome::success(json!({"event":"message","body":"{}"})),
                },
            ),
            (
                json!({"type":"effectResult","effectId":"102","outcome":{"ok":false,"error":{"message":"pull failed","status":401}}}),
                Input::EffectResult {
                    effect_id: "102".into(),
                    outcome: EffectOutcome::failure("pull failed", Some(401)),
                },
            ),
            (json!({"type":"close"}), Input::Close),
        ];
        for (wire, typed) in inputs {
            assert_eq!(
                serde_json::from_value::<Input>(wire.clone()).unwrap(),
                typed
            );
            assert_eq!(serde_json::to_value(&typed).unwrap(), wire);
        }
        let events = [
            (
                json!({"type":"taskCompleted","requestId":"42","ok":true,"value":null}),
                Event::TaskCompleted {
                    request_id: "42".into(),
                    ok: true,
                    value: Value::Null,
                    error: None,
                },
            ),
            (
                json!({"type":"taskCompleted","requestId":"43","ok":false,"value":null,"error":"transaction_closed"}),
                Event::TaskCompleted {
                    request_id: "43".into(),
                    ok: false,
                    value: Value::Null,
                    error: Some("transaction_closed".into()),
                },
            ),
            (
                json!({"type":"effect","effectId":"5","operation":{"kind":"callback","transactionId":"tx7","requestId":"42"}}),
                Event::Effect {
                    effect_id: "5".into(),
                    operation: Operation::Callback {
                        transaction_id: "tx7".into(),
                        request_id: "42".into(),
                    },
                },
            ),
            (
                json!({"type":"effect","effectId":"6","operation":{"kind":"http","route":"action","body":"{}"}}),
                Event::Effect {
                    effect_id: "6".into(),
                    operation: Operation::Http {
                        route: HttpRoute::Action,
                        body: "{}".into(),
                    },
                },
            ),
            (
                json!({"type":"effect","effectId":"7","operation":{"kind":"timer","millis":250}}),
                Event::Effect {
                    effect_id: "7".into(),
                    operation: Operation::Timer { millis: 250 },
                },
            ),
            (
                json!({"type":"cancelEffect","effectId":"6"}),
                Event::CancelEffect {
                    effect_id: "6".into(),
                },
            ),
            (
                json!({"type":"report","diagnostic":{"kind":"protocol","message":"duplicate request id 42"}}),
                Event::Report {
                    diagnostic: Diagnostic::Protocol {
                        message: "duplicate request id 42".into(),
                    },
                },
            ),
            (
                json!({"type":"changed","tables":["Todo"]}),
                Event::Changed {
                    tables: vec!["Todo".into()],
                },
            ),
            (json!({"type":"runtimeClosed"}), Event::RuntimeClosed),
        ];
        for (wire, typed) in events {
            assert_eq!(
                serde_json::from_value::<Event>(wire.clone()).unwrap(),
                typed
            );
            assert_eq!(serde_json::to_value(&typed).unwrap(), wire);
        }
        assert!(serde_json::from_value::<Input>(json!({"type":"nope"})).is_err());
    }
}
