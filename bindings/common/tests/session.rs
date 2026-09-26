use axton_binding::RuntimeHost;
use serde_json::{Value, json};
#[test]
fn dropped_and_rebuilt_actions_keep_terminal_call_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let mut schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    schema["actions"] = json!([{"name":"Ping","version":1,"inputs":[],"outputs":[]}]);
    let mut breaking = schema.clone();
    breaking["models"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"due","nullable":false,"type":{"kind":"scalar","name":"string"}}));
    for frozen in [false, true] {
        let path = dir.path().join(if frozen { "frozen" } else { "unsent" });
        let opened = host
            .call(json!({"op":"open","path":path,"schema":schema}))
            .unwrap()["value"]
            .clone();
        let id = opened["handle"].clone();
        let first = host
            .call(json!({"op":"submitAction","handle":id,"name":"Ping","version":1,"args":{}}))
            .unwrap()["value"]
            .clone();
        if !frozen {
            let dropped = host
                .call(json!({"op":"drop","handle":id,"ordinal":first["ordinal"]}))
                .unwrap()["value"]
                .clone();
            assert_eq!(dropped["completions"][0]["callId"], first["callId"]);
            assert_eq!(dropped["completions"][0]["outcome"]["code"], "dropped");
        }
        let left = host
            .call(json!({"op":"submitAction","handle":id,"name":"Ping","version":1,"args":{}}))
            .unwrap()["value"]
            .clone();
        if frozen {
            host.call(json!({"op":"freeze","handle":id})).unwrap();
        }
        host.call(json!({"op":"close","handle":id})).unwrap();
        let reopened = host
            .call(json!({"op":"open","path":path,"schema":breaking}))
            .unwrap()["value"]
            .clone();
        let report = host
            .call(json!({"op":"rebuild","handle":reopened["handle"],"discardPending":true}))
            .unwrap()["value"]
            .clone();
        assert!(
            report["abandonedCalls"]
                .as_array()
                .unwrap()
                .contains(&json!({"callId":left["callId"],"frozen":frozen}))
        );
        if frozen {
            assert!(
                report["abandonedCalls"]
                    .as_array()
                    .unwrap()
                    .contains(&json!({"callId":first["callId"],"frozen":true}))
            );
        }
    }
}
#[test]
fn direct_action_stays_off_queue_and_applies_completion_without_advancing_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let schema = json!({"enums":[],"models":[],"actions":[{"name":"Ping","version":1,"inputs":[{"kind":"value","name":"label","type":{"kind":"scalar","name":"string"},"nullable":false}],"outputs":[]}]});
    let id = host
        .call(json!({"op":"open","path":dir.path().join("db"),"schema":schema}))
        .unwrap()["value"]["handle"]
        .clone();
    let prepared = host.call(json!({"op":"prepareAction","handle":id,"name":"Ping","version":1,"args":{"label":"hi"}})).unwrap()["value"].clone();
    let body: Value = serde_json::from_str(prepared["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["call"]["callId"], prepared["callId"]);
    assert_eq!(
        host.call(json!({"op":"status","handle":id})).unwrap()["value"]["pending"],
        0
    );
    let applied_raw = host.call(json!({"op":"applyActionResponse","handle":id,"body":prepared["body"],"response":{"completion":{"callId":prepared["callId"],"outcome":{"status":"succeeded","result":null}},"records":[]}})).unwrap();
    assert_eq!(applied_raw["changed"], false);
    let applied = applied_raw["value"].clone();
    assert_eq!(applied["completions"][0]["callId"], prepared["callId"]);
    assert_eq!(
        host.call(json!({"op":"status","handle":id})).unwrap()["value"]["pending"],
        0
    );
    assert_eq!(
        host.call(json!({"op":"status","handle":id})).unwrap()["value"]["cursors"],
        json!({})
    );
}

#[test]
fn durable_action_completion_survives_real_sync_cycle_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let schema = json!({"enums":[],"models":[],"actions":[{"name":"Ping","version":1,"inputs":[{"kind":"value","name":"label","type":{"kind":"scalar","name":"string"},"nullable":false}],"outputs":[]}]});
    let opened = host
        .call(json!({"op":"open","path":dir.path().join("db"),"schema":schema}))
        .unwrap()["value"]
        .clone();
    let id = opened["handle"].clone();
    let submitted = host.call(json!({"op":"submitAction","handle":id,"name":"Ping","version":1,"args":{"label":"hi"}})).unwrap()["value"].clone();
    host.call(json!({"op":"startSync","handle":id,"pushOnly":true}))
        .unwrap();
    let next = host.call(json!({"op":"next","handle":id})).unwrap()["value"].clone();
    assert_eq!(next["kind"], "push");
    let applied = host.call(json!({"op":"complete","handle":id,"response":{"clientId":opened["clientId"],"batchSequence":1,"rejections":[],"completions":[{"callId":submitted["callId"],"outcome":{"status":"succeeded","result":null}}],"records":[]}})).unwrap()["value"].clone();
    assert_eq!(applied["completions"][0]["callId"], submitted["callId"]);
    assert_eq!(
        host.call(json!({"op":"status","handle":id})).unwrap()["value"]["pending"],
        0
    );
}
#[test]
fn language_commands_preserve_transaction_isolation_and_closed_handles() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    let opened = host
        .call(json!({"op":"open","path":dir.path().join("db"),"schema":schema,"owner":"u"}))
        .unwrap();
    let id = opened["value"]["handle"].clone();
    host.call(json!({"op":"begin","handle":id})).unwrap();
    host.call(json!({"op":"direct","handle":id,"operation":{"model":"Entry","op":"create","identity":{"id":"e"},"values":{"text":"hi"}}})).unwrap();
    let row = host
        .call(json!({"op":"read","handle":id,"key":{"model":"Entry","identity":{"id":"e"}}}))
        .unwrap();
    assert_eq!(row["value"]["text"], "hi");
    assert_eq!(row["changed"], false);
    host.call(json!({"op":"rollback","handle":id})).unwrap();
    let row = host
        .call(json!({"op":"read","handle":id,"key":{"model":"Entry","identity":{"id":"e"}}}))
        .unwrap();
    assert!(row["value"].is_null());
    host.call(json!({"op":"close","handle":id})).unwrap();
    assert!(host.call(json!({"op":"status","handle":id})).is_err());
}
#[test]
fn rust_selects_transport_actions_and_reuses_frozen_request_on_retry() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    let opened = host
        .call(json!({"op":"open","path":dir.path().join("db"),"schema":schema,"owner":"u"}))
        .unwrap()["value"]
        .clone();
    let id = opened["handle"].clone();
    let client_id = opened["clientId"].clone();
    // The HTTP cycle pulls from a committed boundary; the handshake establishes it.
    streaming(&mut host, &id);
    host.call(json!({"op":"startSync","handle":id})).unwrap();
    let action = host.call(json!({"op":"next","handle":id})).unwrap()["value"].clone();
    assert_eq!(action["kind"], "pull");
    host.call(json!({"op":"complete","handle":id,"response":{"cursors":{"book":{"from":0,"to":1,"head":1}},"changes":[{"model":"Entry","identity":{"id":"e"},"stamp":1,"state":{"text":"A","note":null}}]}})).unwrap();
    assert!(host.call(json!({"op":"next","handle":id})).unwrap()["value"].is_null());
    host.call(json!({"op":"enqueue","handle":id,"mutation":{"name":"Edit","operations":[{"model":"Entry","op":"update","identity":{"id":"e"},"values":{"text":"B"}}]}})).unwrap();
    host.call(json!({"op":"startSync","handle":id})).unwrap();
    let action = host.call(json!({"op":"next","handle":id})).unwrap()["value"].clone();
    assert_eq!(action["kind"], "push");
    assert_eq!(
        host.call(json!({"op":"next","handle":id})).unwrap()["value"],
        action
    );
    host.call(json!({"op":"complete","handle":id,"response":{"clientId":client_id,"batchSequence":1,"rejections":[],"records":[{"model":"Entry","identity":{"id":"e"},"stamp":2,"state":{"text":"B","note":null}}]}})).unwrap();
    assert_eq!(
        host.call(json!({"op":"status","handle":id})).unwrap()["value"]["pending"],
        0,
        "the receipt completes the push"
    );
    assert_eq!(
        host.call(json!({"op":"next","handle":id})).unwrap()["value"]["kind"],
        "pull"
    );
}

#[test]
fn live_push_cycle_keeps_receipts_but_leaves_reads_to_the_stream() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    let opened = host
        .call(json!({"op":"open","path":dir.path().join("db"),"schema":schema}))
        .unwrap()["value"]
        .clone();
    let id = opened["handle"].clone();
    let client_id = opened["clientId"].clone();
    streaming(&mut host, &id);
    host.call(json!({"op":"startSync","handle":id,"pushOnly":true}))
        .unwrap();
    assert!(host.call(json!({"op":"next","handle":id})).unwrap()["value"].is_null());
    host.call(json!({"op":"enqueue","handle":id,"mutation":{"name":"Create","operations":[{"model":"Entry","op":"create","identity":{"id":"e"},"values":{"text":"B","note":null}}]}})).unwrap();
    host.call(json!({"op":"startSync","handle":id,"pushOnly":true}))
        .unwrap();
    let action = host.call(json!({"op":"next","handle":id})).unwrap()["value"].clone();
    assert_eq!(action["kind"], "push");
    host.call(json!({"op":"startSync","handle":id,"pushOnly":true}))
        .unwrap();
    assert_eq!(
        host.call(json!({"op":"next","handle":id})).unwrap()["value"],
        action
    );
    host.call(json!({"op":"complete","handle":id,"response":{"clientId":client_id,"batchSequence":1,"rejections":[],"records":[{"model":"Entry","identity":{"id":"e"},"stamp":1,"state":{"text":"normalized","note":null}}]}})).unwrap();
    assert!(host.call(json!({"op":"next","handle":id})).unwrap()["value"].is_null());
    assert_eq!(
        host.call(json!({"op":"status","handle":id})).unwrap()["value"]["pending"],
        0,
        "the receipt completes the push without a pull"
    );
    assert_eq!(
        host.call(json!({"op":"read","handle":id,"key":{"model":"Entry","identity":{"id":"e"}}}))
            .unwrap()["value"]["text"],
        "normalized"
    );
    // The stream later carries the same authority: a no-op that advances the cursor.
    host.call(json!({"op":"pull","handle":id,"page":{"cursors":{"book":{"from":0,"to":1,"head":1}},"changes":[{"model":"Entry","identity":{"id":"e"},"stamp":1,"state":{"text":"normalized","note":null}}]}})).unwrap();
    assert_eq!(
        host.call(json!({"op":"status","handle":id})).unwrap()["value"]["cursors"]["book"],
        1
    );
    host.call(json!({"op":"startSync","handle":id})).unwrap();
    assert_eq!(
        host.call(json!({"op":"next","handle":id})).unwrap()["value"]["kind"],
        "pull"
    );
}

/// The host loop over the downlink lane: enqueue one event - which answers with
/// no actions of its own - then pump until the worker waits or has nothing left.
fn downlink(host: &mut RuntimeHost, id: &Value, mut event: Value) -> Value {
    event["op"] = json!("downlink");
    event["handle"] = id.clone();
    event["now"] = json!(0);
    event["entropy"] = json!(0);
    assert_eq!(
        host.call(event).unwrap()["value"],
        json!([]),
        "an enqueue decides nothing: the pump answers"
    );
    pump(host, id)
}
/// Pump until the worker waits or has nothing left, collecting its actions.
fn pump(host: &mut RuntimeHost, id: &Value) -> Value {
    let mut actions = vec![];
    for _ in 0..8 {
        let pumped = host
            .call(json!({"op":"downlink","handle":id,"event":"next","now":0,"entropy":0}))
            .unwrap()["value"]
            .as_array()
            .cloned()
            .unwrap();
        let stop = pumped.is_empty() || pumped.iter().any(|a| a["type"] == "wait");
        actions.extend(pumped);
        if stop {
            return Value::Array(actions);
        }
    }
    panic!("the pump never went idle: {actions:?}")
}
/// The id the host answers this catch-up request by.
fn requested(action: &Value) -> Value {
    assert_eq!(action["type"], "request", "{action}");
    action["request"].clone()
}
/// What an applied page answers: the push wake and the Scopes it committed.
fn committed(scopes: Value) -> Value {
    json!([{"type":"wake","lane":"push"},{"type":"changed","scopes":scopes}])
}

#[test]
fn downlink_and_push_drivers_have_independent_lifecycle_and_retry_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    let id = host
        .call(json!({"op":"open","path":dir.path().join("db"),"schema":schema}))
        .unwrap()["value"]["handle"]
        .clone();
    host.call(json!({"op":"channel","handle":id,"channel":"book","subscribed":true}))
        .unwrap();
    host.call(json!({"op":"connection","handle":id,"event":"start","now":0}))
        .unwrap();
    assert_eq!(
        host.call(json!({"op":"connection","handle":id,"event":"next","now":0}))
            .unwrap()["value"]["type"],
        "sync"
    );
    let opened = downlink(&mut host, &id, json!({"event":"start"}));
    assert_eq!(opened[0]["type"], "open");
    let epoch = opened[0]["epoch"].clone();
    // The live socket drops: its lane backs off while the push lane is unaffected.
    let closed = downlink(&mut host, &id, json!({"event":"closed","epoch":epoch}));
    assert_eq!(
        closed[0],
        json!({"type":"close","epoch":epoch,"reason":null})
    );
    assert_eq!(closed[1]["type"], "wait");
    host.call(json!({"op":"connection","handle":id,"event":"success","now":0}))
        .unwrap();
    assert_eq!(
        host.call(json!({"op":"connection","handle":id,"event":"next","now":0}))
            .unwrap()["value"]["type"],
        "idle"
    );
    assert_eq!(pump(&mut host, &id)[0]["type"], "wait");
    for bad in [
        json!({"op":"downlink","handle":id,"event":"unknown","now":0}),
        json!({"op":"downlink","handle":id,"event":"message","now":0}),
        json!({"op":"connection","handle":id,"event":"unknown","now":0}),
    ] {
        assert!(host.call(bad).is_err());
    }
}

/// Drive one downlink session to its streaming phase: subscribe `book`, start
/// the lane and acknowledge at head zero, which commits that Scope's first
/// delivery boundary - a registration has none until a session negotiates one
/// ([#150](https://github.com/zanminwang/axton/issues/150)). Answers with the
/// epoch of the socket the test then streams on.
fn streaming(host: &mut RuntimeHost, id: &Value) -> Value {
    host.call(json!({"op":"channel","handle":id,"channel":"book","subscribed":true}))
        .unwrap();
    let opened = downlink(host, id, json!({"event":"start"}));
    let epoch = opened[0]["epoch"].clone();
    assert_eq!(
        serde_json::from_str::<Value>(opened[0]["subscribe"].as_str().unwrap()).unwrap(),
        json!({"type":"subscribe","channels":["book"],"models":{"Entry":1}})
    );
    let ack = json!({"type":"subscribed","cursors":{"book":0}}).to_string();
    assert_eq!(
        downlink(
            host,
            id,
            json!({"event":"message","epoch":epoch,"body":ack})
        ),
        json!([{"type":"changed","scopes":["book"]},{"type":"acknowledged","scopes":["book"]}]),
        "the acknowledged head is the first boundary; no history is pulled, and \
         the handshake announces that delivery is established"
    );
    epoch
}

#[test]
fn incoming_pages_share_cursor_policy_and_do_not_overwrite_push_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    let id = host
        .call(json!({"op":"open","path":dir.path().join("db"),"schema":schema}))
        .unwrap()["value"]["handle"]
        .clone();
    let page = json!({"cursors":{"book":{"from":0,"to":1,"head":1}},"changes":[{"model":"Entry","identity":{"id":"e"},"stamp":1,"state":{"text":"A","note":null}}]});
    let epoch = streaming(&mut host, &id);
    assert_eq!(
        downlink(
            &mut host,
            &id,
            json!({"event":"message","epoch":epoch,"body":page.to_string()})
        ),
        committed(json!(["book"])),
        "the stream applies from the boundary"
    );
    assert_eq!(
        downlink(
            &mut host,
            &id,
            json!({"event":"message","epoch":epoch,"body":page.to_string()})
        ),
        json!([]),
        "covered"
    );
    let gap = json!({"cursors":{"book":{"from":2,"to":3,"head":3}},"changes":[]});
    let recovered = downlink(
        &mut host,
        &id,
        json!({"event":"message","epoch":epoch,"body":gap.to_string()}),
    );
    let repair = requested(&recovered[0]);
    host.call(json!({"op":"enqueue","handle":id,"mutation":{"name":"Edit","operations":[{"model":"Entry","op":"update","identity":{"id":"e"},"values":{"text":"B"}}]}})).unwrap();
    host.call(json!({"op":"startSync","handle":id,"pushOnly":true}))
        .unwrap();
    let push = host.call(json!({"op":"next","handle":id})).unwrap()["value"].clone();
    // The pull covers the held gap frame, which is then discarded.
    let covering = json!({"cursors":{"book":{"from":1,"to":3,"head":3}},"changes":[]});
    assert_eq!(
        downlink(
            &mut host,
            &id,
            json!({"event":"response","request":repair,"body":covering.to_string()})
        ),
        committed(json!(["book"]))
    );
    assert_eq!(
        host.call(json!({"op":"next","handle":id})).unwrap()["value"],
        push,
        "the downlink worker never touches the push cycle"
    );
    assert_eq!(push["kind"], "push");
}

#[test]
fn incoming_overlap_is_identical_with_or_without_http_request_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    for over_http in [false, true] {
        let id=host.call(json!({"op":"open","path":dir.path().join(if over_http {"http"} else {"ws"}),"schema":schema})).unwrap()["value"]["handle"].clone();
        let epoch = streaming(&mut host, &id);
        let first = json!({"cursors":{"book":{"from":0,"to":1,"head":1}},"changes":[{"model":"Entry","identity":{"id":"e"},"stamp":1,"state":{"text":"first","note":null}}]});
        let overlap = json!({"cursors":{"book":{"from":0,"to":2,"head":2}},"changes":[{"model":"Entry","identity":{"id":"e"},"stamp":2,"state":{"text":"incoming overlap","note":null}}]});
        let mut repair = Value::Null;
        if over_http {
            // A streamed gap makes the session request from cursor 0; the first
            // page then lands through the stream before the answer arrives.
            let gap = json!({"cursors":{"book":{"from":5,"to":6,"head":6}},"changes":[]});
            let recovered = downlink(
                &mut host,
                &id,
                json!({"event":"message","epoch":epoch,"body":gap.to_string()}),
            );
            repair = requested(&recovered[0]);
        }
        // While a pull is in flight, a streamed page waits in the queue and is
        // covered once the HTTP answer lands; in streaming it applies at once.
        assert_eq!(
            downlink(
                &mut host,
                &id,
                json!({"event":"message","epoch":epoch,"body":first.to_string()})
            ),
            if over_http {
                json!([])
            } else {
                committed(json!(["book"]))
            }
        );
        let applied = downlink(
            &mut host,
            &id,
            if over_http {
                json!({"event":"response","request":repair,"body":overlap.to_string()})
            } else {
                json!({"event":"message","epoch":epoch,"body":overlap.to_string()})
            },
        );
        let actions = applied.as_array().unwrap();
        assert_eq!(
            actions[..2],
            committed(json!(["book"])).as_array().unwrap()[..]
        );
        if over_http {
            // The pull applied, then the queue: `first` is covered and the gap
            // frame still does not connect, so one more pull runs.
            assert_eq!(
                serde_json::from_str::<Value>(actions[2]["body"].as_str().unwrap()).unwrap(),
                json!({"cursors":{"book":2},"models":{"Entry":1}})
            );
            assert_eq!(actions.len(), 3);
        } else {
            assert_eq!(actions.len(), 2);
        }
        assert_eq!(
            downlink(
                &mut host,
                &id,
                json!({"event":"message","epoch":epoch,"body":overlap.to_string()})
            ),
            json!([]),
            "covered"
        );
        assert_eq!(
            host.call(json!({"op":"status","handle":id})).unwrap()["value"]["cursors"]["book"],
            2
        );
        assert_eq!(
            host.call(
                json!({"op":"read","handle":id,"key":{"model":"Entry","identity":{"id":"e"}}})
            )
            .unwrap()["value"]["text"],
            "incoming overlap"
        );
        for old in [
            "live",
            "downlinkRequest",
            "downlinkPage",
            "downlinkComplete",
        ] {
            assert!(
                host.call(json!({"op":old,"handle":id,"scope":"book","page":overlap}))
                    .is_err()
            );
        }
    }
    // An HTTP answer that does not match its request ends the session with a reason.
    let id = host
        .call(json!({"op":"open","path":dir.path().join("mismatch"),"schema":schema}))
        .unwrap()["value"]["handle"]
        .clone();
    let epoch = streaming(&mut host, &id);
    let gap = json!({"cursors":{"book":{"from":5,"to":6,"head":6}},"changes":[]}).to_string();
    let recovered = downlink(
        &mut host,
        &id,
        json!({"event":"message","epoch":epoch,"body":gap}),
    );
    let repair = requested(&recovered[0]);
    let other = json!({"cursors":{"other":{"from":0,"to":2,"head":2}},"changes":[]}).to_string();
    let ended = downlink(
        &mut host,
        &id,
        json!({"event":"response","request":repair,"body":other}),
    );
    assert_eq!(ended[0]["type"], "close");
    assert_eq!(ended[0]["reason"], "response does not match pull request");
}

/// The two refusals every language binding relies on to keep its transaction
/// object honest: a transaction-scoped command after the session ended is
/// `transaction_closed`, and a sync command while a session is open is refused
/// as `client transaction active`. Both are asserted directly here; the JS and
/// Dart suites see them only as thrown errors.
#[test]
fn transaction_scoped_commands_and_sync_commands_are_refused_by_code() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    let opened = host
        .call(json!({"op":"open","path":dir.path().join("db"),"schema":schema,"owner":"u"}))
        .unwrap();
    let id = opened["value"]["handle"].clone();
    let key = json!({"model":"Entry","identity":{"id":"e"}});
    // No session yet: a transaction-scoped read is refused, a plain one is served.
    let refused = host
        .call(json!({"op":"read","handle":id,"key":key,"transaction":true}))
        .unwrap_err();
    assert_eq!(refused.to_string(), "transaction_closed");
    assert!(
        host.call(json!({"op":"read","handle":id,"key":key}))
            .unwrap()["value"]
            .is_null()
    );
    host.call(json!({"op":"begin","handle":id})).unwrap();
    host.call(json!({"op":"direct","handle":id,"transaction":true,"operation":{"model":"Entry","op":"create","identity":{"id":"e"},"values":{"text":"hi"}}})).unwrap();
    // While the session is open, sync commands are refused and change nothing.
    for op in ["freeze", "status", "tasks", "task", "outcome"] {
        let e = host.call(json!({"op":op,"handle":id})).unwrap_err();
        assert_eq!(e.to_string(), "client transaction active", "{op}");
    }
    host.call(json!({"op":"commit","handle":id})).unwrap();
    // After the commit the same transaction-scoped read is closed again, the
    // committed row is visible to a plain read, and sync commands work.
    let refused = host
        .call(json!({"op":"read","handle":id,"key":key,"transaction":true}))
        .unwrap_err();
    assert_eq!(refused.to_string(), "transaction_closed");
    assert_eq!(
        host.call(json!({"op":"read","handle":id,"key":key}))
            .unwrap()["value"]["text"],
        "hi"
    );
    assert_eq!(
        host.call(json!({"op":"status","handle":id})).unwrap()["value"]["pending"],
        0
    );
    host.call(json!({"op":"close","handle":id})).unwrap();
    assert_eq!(
        host.call(json!({"op":"status","handle":id}))
            .unwrap_err()
            .to_string(),
        "client_closed"
    );
}

/// An incompatible schema at open keeps the old file while it holds unsent
/// work, sends it through the same handle, then `rebuild` switches to a fresh
/// file; every step is visible in `status().schema`.
#[test]
fn incompatible_schema_reports_pending_work_and_rebuild_switches_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut host = RuntimeHost::default();
    let schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    let mut breaking = schema.clone();
    breaking["models"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"due","nullable":false,"type":{"kind":"scalar","name":"string"}}));
    let opened = host
        .call(json!({"op":"open","path":path,"schema":schema}))
        .unwrap()["value"]
        .clone();
    assert_eq!(opened["schema"]["rebuilt"], false);
    let id = opened["handle"].clone();
    host.call(json!({"op":"direct","handle":id,"operation":{"model":"Entry","op":"create","identity":{"id":"e"},"values":{"text":"A","note":null}}})).unwrap();
    host.call(json!({"op":"enqueue","handle":id,"mutation":{"name":"Edit","operations":[{"model":"Entry","op":"update","identity":{"id":"e"},"values":{"text":"B"}}]}})).unwrap();
    host.call(json!({"op":"freeze","handle":id})).unwrap();
    host.call(json!({"op":"close","handle":id})).unwrap();

    let opened = host
        .call(json!({"op":"open","path":path,"schema":breaking}))
        .unwrap()["value"]
        .clone();
    let id = opened["handle"].clone();
    let client_id = opened["clientId"].clone();
    assert_eq!(opened["schema"]["rebuilt"], false);
    assert_eq!(opened["schema"]["pending"]["pending"], 1);
    assert!(
        opened["schema"]["pending"]["reason"]
            .as_str()
            .unwrap()
            .contains("due")
    );
    let status = host.call(json!({"op":"status","handle":id})).unwrap()["value"].clone();
    assert_eq!(status["pending"], 1);
    assert_eq!(
        status["schema"]["pending"]["oldFile"].as_str().unwrap(),
        path.to_string_lossy()
    );
    assert!(
        host.call(json!({"op":"rebuild","handle":id})).is_err(),
        "unsent work blocks the rebuild"
    );
    host.call(json!({"op":"ack","handle":id,"sequence":1,"receipt":{"clientId":client_id,"batchSequence":1,"rejections":[],"records":[{"model":"Entry","identity":{"id":"e"},"stamp":2,"state":{"text":"B","note":null}}]}})).unwrap();
    let report = host.call(json!({"op":"rebuild","handle":id})).unwrap();
    assert_eq!(report["value"]["leftPending"], 0);
    assert!(
        report["value"]["newFile"]
            .as_str()
            .unwrap()
            .ends_with("db.1")
    );
    assert_eq!(report["changed"], true, "watchers learn the tables changed");
    let status = host.call(json!({"op":"status","handle":id})).unwrap()["value"].clone();
    assert_eq!(status["schema"]["rebuilt"], true);
    assert!(status["schema"]["pending"].is_null());
    assert!(
        host.call(json!({"op":"read","handle":id,"key":{"model":"Entry","identity":{"id":"e"}}}))
            .unwrap()["value"]
            .is_null(),
        "the fresh file is empty"
    );
    assert!(path.exists(), "the old file is kept");
}

/// Open `path` under a schema its file no longer fits while the file still
/// holds unsent work: the handle keeps the old file until `rebuild` switches
/// it to a fresh one in place.
fn rebuildable(host: &mut RuntimeHost, path: &std::path::Path) -> Value {
    let schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    let mut breaking = schema.clone();
    breaking["models"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"due","nullable":false,"type":{"kind":"scalar","name":"string"}}));
    let id = host
        .call(json!({"op":"open","path":path,"schema":schema}))
        .unwrap()["value"]["handle"]
        .clone();
    host.call(json!({"op":"direct","handle":id,"operation":{"model":"Entry","op":"create","identity":{"id":"e"},"values":{"text":"A","note":null}}})).unwrap();
    host.call(json!({"op":"enqueue","handle":id,"mutation":{"name":"Edit","operations":[{"model":"Entry","op":"update","identity":{"id":"e"},"values":{"text":"B"}}]}})).unwrap();
    host.call(json!({"op":"close","handle":id})).unwrap();
    let opened = host
        .call(json!({"op":"open","path":path,"schema":breaking}))
        .unwrap()["value"]
        .clone();
    assert_eq!(opened["schema"]["pending"]["pending"], 1);
    opened["handle"].clone()
}
/// The committed delivery positions of the handle's replica.
fn cursors(host: &mut RuntimeHost, id: &Value) -> Value {
    status(host, id)["cursors"].clone()
}
fn status(host: &mut RuntimeHost, id: &Value) -> Value {
    host.call(json!({"op":"status","handle":id})).unwrap()["value"].clone()
}
/// The epoch of the session an `open` action begins.
fn epoch(action: &Value) -> u64 {
    assert_eq!(action["type"], "open", "{action}");
    action["epoch"].as_u64().unwrap()
}
/// The actions of these types, in order.
fn only<'a>(actions: &'a Value, kind: &str) -> Vec<&'a Value> {
    actions
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["type"] == kind)
        .collect()
}
/// The request id of the one historical page these actions ask for.
fn loading(actions: &Value) -> u64 {
    actions
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["type"] == "request" && a["bootstrap"] == true)
        .unwrap_or_else(|| panic!("a bootstrap request: {actions}"))["request"]
        .as_u64()
        .unwrap()
}
/// A record the old replica's I/O would write if it reached the fresh file,
/// complete under the rebuilt schema.
fn late() -> Value {
    json!({"model":"Entry","identity":{"id":"late"},"stamp":9,"state":{"text":"late","note":null,"due":"now"}})
}
/// A historical page of `book` over `(0, 7]`.
fn historical(records: Value) -> String {
    json!({"mode":"bootstrap","channel":"book","from":0,"to":7,"until":7,"head":9,"records":records})
        .to_string()
}
/// Everything the old replica's socket `old`, its catch-up `pull` and its
/// historical page `load` can still deliver: every socket event, and an
/// answer and a failure of each HTTP class. Each would move a cursor, write
/// a record, complete or fail a load, or end the session if it reached the
/// fresh replica.
fn stale(old: u64, pull: Option<u64>, load: Option<u64>) -> Vec<Value> {
    let page =
        json!({"cursors":{"book":{"from":0,"to":9,"head":9}},"changes":[late()]}).to_string();
    let ack = json!({"type":"subscribed","cursors":{"book":3}}).to_string();
    let mut events = vec![
        json!({"event":"message","epoch":old,"body":page}),
        json!({"event":"message","epoch":old,"body":ack}),
        json!({"event":"overflow","epoch":old}),
    ];
    if let Some(pull) = pull {
        events.push(json!({"event":"response","request":pull,"body":page}));
        events.push(json!({"event":"failed","request":pull}));
    }
    if let Some(load) = load {
        events.push(json!({"event":"response","request":load,"body":historical(json!([late()]))}));
        events.push(json!({"event":"failed","request":load,"status":400}));
        events.push(json!({"event":"failed","request":load}));
    }
    events.push(json!({"event":"closed","epoch":old}));
    events
}
/// Deliver each event from the old replica's I/O: none answers with an
/// action, and the fresh replica's status and records do not change.
fn inert(host: &mut RuntimeHost, id: &Value, events: Vec<Value>) {
    let before = status(host, id);
    for stale in events {
        assert_eq!(downlink(host, id, stale.clone()), json!([]), "{stale}");
    }
    assert_eq!(status(host, id), before, "the fresh replica is untouched");
    assert!(
        host.call(
            json!({"op":"read","handle":id,"key":{"model":"Entry","identity":{"id":"late"}}})
        )
        .unwrap()["value"]
            .is_null(),
        "no stale record landed"
    );
}
/// Hand the worker an event without pumping: it is queued when the rebuild
/// begins.
fn queue(host: &mut RuntimeHost, id: &Value, mut event: Value) {
    event["op"] = json!("downlink");
    event["handle"] = id.clone();
    event["now"] = json!(0);
    event["entropy"] = json!(0);
    assert_eq!(host.call(event).unwrap()["value"], json!([]));
}
fn rebuild(host: &mut RuntimeHost, id: &Value) {
    host.call(json!({"op":"rebuild","handle":id,"discardPending":true}))
        .unwrap();
}

/// A rebuild under a running Downlink lane keeps it running: the next pump,
/// with no second `start`, tells the host to abandon the old replica's I/O and
/// opens a session for the carried Scope under a new epoch. Nothing the old
/// socket or its catch-up queued or still delivers reaches the fresh file,
/// before the new handshake or after it - while a new catch-up is in flight -
/// and the lane then advances from the new acknowledged head
/// ([#162](https://github.com/zanminwang/axton/issues/162)).
#[test]
fn a_rebuild_keeps_a_running_downlink_lane_running_under_fresh_identifiers() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let id = rebuildable(&mut host, &dir.path().join("db"));
    let old = streaming(&mut host, &id).as_u64().unwrap();
    let gap = json!({"cursors":{"book":{"from":2,"to":3,"head":3}},"changes":[]});
    let recovered = downlink(
        &mut host,
        &id,
        json!({"event":"message","epoch":old,"body":gap.to_string()}),
    );
    let pull = requested(&recovered[0]).as_u64().unwrap();
    assert_eq!(recovered[0]["bootstrap"], false, "an ordinary catch-up");
    // Arrived before the rebuild, never pumped.
    let covering = json!({"cursors":{"book":{"from":0,"to":3,"head":3}},"changes":[late()]});
    queue(
        &mut host,
        &id,
        json!({"event":"message","epoch":old,"body":covering.to_string()}),
    );
    queue(
        &mut host,
        &id,
        json!({"event":"response","request":pull,"body":covering.to_string()}),
    );

    rebuild(&mut host, &id);
    let rebuilt = status(&mut host, &id);
    assert_eq!(rebuilt["channels"], json!(["book"]), "the Scope is carried");
    assert_eq!(rebuilt["cursors"], json!({}), "with no origin");
    let resumed = pump(&mut host, &id);
    assert_eq!(
        resumed[0],
        json!({"type":"reset"}),
        "the old I/O is abandoned first, without another start: {resumed}"
    );
    let new = epoch(&resumed[1]);
    assert!(new > old, "epoch {new} after {old}");
    assert_eq!(
        serde_json::from_str::<Value>(resumed[1]["subscribe"].as_str().unwrap()).unwrap()["channels"],
        json!(["book"])
    );
    assert_eq!(resumed.as_array().unwrap().len(), 2, "{resumed}");
    assert_eq!(
        pump(&mut host, &id),
        json!([]),
        "the reset is announced once"
    );
    assert_eq!(status(&mut host, &id), rebuilt, "nothing queued applied");
    // Whatever the old socket and catch-up still deliver belongs to nothing.
    inert(&mut host, &id, stale(old, Some(pull), None));

    let ack = json!({"type":"subscribed","cursors":{"book":5}}).to_string();
    assert_eq!(
        downlink(
            &mut host,
            &id,
            json!({"event":"message","epoch":new,"body":ack})
        ),
        json!([{"type":"changed","scopes":["book"]},{"type":"acknowledged","scopes":["book"]}]),
        "the new handshake commits the carried Scope's first boundary"
    );
    assert_eq!(cursors(&mut host, &id), json!({"book":5}));
    let gap = json!({"cursors":{"book":{"from":7,"to":8,"head":8}},"changes":[]});
    let repair = downlink(
        &mut host,
        &id,
        json!({"event":"message","epoch":new,"body":gap.to_string()}),
    );
    let fresh = requested(&repair[0]).as_u64().unwrap();
    assert!(fresh > pull, "request {fresh} after {pull}");
    // With a new session established and a new catch-up in flight, the old
    // ones' events still match nothing.
    inert(&mut host, &id, stale(old, Some(pull), None));
    let answer = json!({"cursors":{"book":{"from":5,"to":8,"head":8}},"changes":[{"model":"Entry","identity":{"id":"fresh"},"stamp":8,"state":{"text":"fresh","note":null,"due":"now"}}]});
    assert_eq!(
        downlink(
            &mut host,
            &id,
            json!({"event":"response","request":fresh,"body":answer.to_string()})
        ),
        committed(json!(["book"])),
        "the lane advances from the new head"
    );
    assert_eq!(cursors(&mut host, &id), json!({"book":8}));
    assert_eq!(
        host.call(
            json!({"op":"read","handle":id,"key":{"model":"Entry","identity":{"id":"fresh"}}})
        )
        .unwrap()["value"]["text"],
        "fresh"
    );
}

/// The same transition with a historical page in flight: its answer or
/// failure - queued before the rebuild, delivered before the new handshake or
/// after it while the fresh identity's own page is in flight - completes and
/// fails nothing on the fresh replica, and the fresh page is asked for under
/// a request id the old one never held
/// ([#162](https://github.com/zanminwang/axton/issues/162)).
#[test]
fn a_rebuild_fences_a_bootstrap_request_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let id = rebuildable(&mut host, &dir.path().join("db"));
    let subscription = host
        .call(json!({"op":"scopeSubscribe","handle":id,"scope":"book"}))
        .unwrap()["value"]["subscriptionId"]
        .clone();
    host.call(
        json!({"op":"scopeBootstrap","handle":id,"scope":"book","subscriptionId":subscription}),
    )
    .unwrap();
    let old = epoch(&downlink(&mut host, &id, json!({"event":"start"}))[0]);
    let ack = json!({"type":"subscribed","cursors":{"book":7}}).to_string();
    let load = loading(&downlink(
        &mut host,
        &id,
        json!({"event":"message","epoch":old,"body":ack}),
    ));
    queue(
        &mut host,
        &id,
        json!({"event":"response","request":load,"body":historical(json!([late()]))}),
    );

    rebuild(&mut host, &id);
    let resumed = pump(&mut host, &id);
    assert_eq!(resumed[0], json!({"type":"reset"}), "{resumed}");
    let new = epoch(&resumed[1]);
    assert!(new > old, "epoch {new} after {old}");
    assert_eq!(resumed.as_array().unwrap().len(), 2, "{resumed}");
    let carried = host
        .call(json!({"op":"scopeState","handle":id,"scope":"book"}))
        .unwrap()["value"]["subscriptionId"]
        .clone();
    assert_ne!(carried, subscription, "a fresh identity");
    let state = |host: &mut RuntimeHost| {
        host.call(
            json!({"op":"scopeBootstrapState","handle":id,"scope":"book","subscriptionId":carried}),
        )
        .unwrap()["value"]
            .clone()
    };
    inert(&mut host, &id, stale(old, None, Some(load)));
    let untouched = state(&mut host);
    assert_eq!(
        untouched["run"], 0,
        "no run of the fresh identity was touched: {untouched}"
    );
    assert_eq!(untouched["error"], Value::Null);

    // The new session commits the origin and a new load asks for its first
    // page under an id no earlier request held.
    downlink(
        &mut host,
        &id,
        json!({"event":"message","epoch":new,"body":ack}),
    );
    host.call(json!({"op":"scopeBootstrap","handle":id,"scope":"book","subscriptionId":carried}))
        .unwrap();
    let fresh = loading(&downlink(&mut host, &id, json!({"event":"wake"})));
    assert!(fresh > load, "request {fresh} after {load}");
    let requested = state(&mut host);
    assert_eq!(requested["run"], 1, "{requested}");
    inert(&mut host, &id, stale(old, None, Some(load)));
    assert_eq!(
        state(&mut host),
        requested,
        "the old answer and failures neither complete nor fail the fresh run"
    );
    let applied = downlink(
        &mut host,
        &id,
        json!({"event":"response","request":fresh,"body":historical(json!([]))}),
    );
    assert_eq!(
        applied[0],
        json!({"type":"bootstrap","scope":"book","subscriptionId":carried,"state":"catching_up","run":1,"cursor":7,"barrier":9,"error":null}),
        "the fresh page applies to the fresh run"
    );
}

/// A rebuild keeps a paused lane paused: the first pump only resets the old
/// I/O - a historical page among it - and no pump opens or requests anything
/// until `resume`, which opens exactly one session
/// ([#162](https://github.com/zanminwang/axton/issues/162)).
#[test]
fn a_rebuild_keeps_a_paused_downlink_lane_paused_until_resume() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let id = rebuildable(&mut host, &dir.path().join("db"));
    let subscription = host
        .call(json!({"op":"scopeSubscribe","handle":id,"scope":"book"}))
        .unwrap()["value"]["subscriptionId"]
        .clone();
    host.call(
        json!({"op":"scopeBootstrap","handle":id,"scope":"book","subscriptionId":subscription}),
    )
    .unwrap();
    let old = epoch(&downlink(&mut host, &id, json!({"event":"start"}))[0]);
    let ack = json!({"type":"subscribed","cursors":{"book":7}}).to_string();
    let load = loading(&downlink(
        &mut host,
        &id,
        json!({"event":"message","epoch":old,"body":ack}),
    ));
    let paused = downlink(&mut host, &id, json!({"event":"pause"}));
    assert!(only(&paused, "open").is_empty(), "{paused}");

    rebuild(&mut host, &id);
    assert_eq!(
        pump(&mut host, &id),
        json!([{"type":"reset"}]),
        "the old I/O is abandoned while paused, and nothing new starts"
    );
    assert_eq!(
        pump(&mut host, &id),
        json!([]),
        "the reset is announced once"
    );
    inert(&mut host, &id, stale(old, None, Some(load)));
    assert_eq!(
        downlink(&mut host, &id, json!({"event":"wake"})),
        json!([]),
        "still paused"
    );
    let resumed = downlink(&mut host, &id, json!({"event":"resume"}));
    let opened = only(&resumed, "open");
    assert_eq!(opened.len(), 1, "resume opens once: {resumed}");
    assert!(epoch(opened[0]) > old);
    assert!(only(&resumed, "request").is_empty(), "{resumed}");
    assert!(
        only(&downlink(&mut host, &id, json!({"event":"wake"})), "open").is_empty(),
        "and only once"
    );
}

/// A rebuild cannot restart a stopped lane: it announces the reset and stays
/// inert through wakes and `resume` until an explicit `start`
/// ([#162](https://github.com/zanminwang/axton/issues/162)).
#[test]
fn a_rebuild_leaves_a_stopped_downlink_lane_stopped_until_start() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let id = rebuildable(&mut host, &dir.path().join("db"));
    let old = streaming(&mut host, &id).as_u64().unwrap();
    downlink(&mut host, &id, json!({"event":"stop"}));

    rebuild(&mut host, &id);
    assert_eq!(pump(&mut host, &id), json!([{"type":"reset"}]));
    assert_eq!(pump(&mut host, &id), json!([]));
    inert(&mut host, &id, stale(old, None, None));
    for event in ["wake", "resume"] {
        assert_eq!(
            downlink(&mut host, &id, json!({"event":event})),
            json!([]),
            "{event} restarts nothing"
        );
    }
    let started = downlink(&mut host, &id, json!({"event":"start"}));
    assert!(epoch(&started[0]) > old, "{started}");
}

/// A running lane with no Channel still announces the reset, so the host
/// drops whatever the old replica left, and then idles until a registration
/// wakes it ([#162](https://github.com/zanminwang/axton/issues/162)).
#[test]
fn a_rebuild_under_a_running_lane_with_no_channel_resets_and_idles() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let id = rebuildable(&mut host, &dir.path().join("db"));
    assert_eq!(
        downlink(&mut host, &id, json!({"event":"start"})),
        json!([]),
        "nothing subscribed: the lane idles"
    );

    rebuild(&mut host, &id);
    assert_eq!(pump(&mut host, &id), json!([{"type":"reset"}]));
    assert_eq!(pump(&mut host, &id), json!([]), "still idle");
    host.call(json!({"op":"channel","handle":id,"channel":"book","subscribed":true}))
        .unwrap();
    let opened = downlink(&mut host, &id, json!({"event":"wake"}));
    assert_eq!(opened.as_array().unwrap().len(), 1, "{opened}");
    epoch(&opened[0]);
}

/// A refused rebuild changes neither the replica nor the lane: no reset is
/// announced, and the old socket, catch-up and historical page stay the
/// lane's own - their answers apply as they would have
/// ([#162](https://github.com/zanminwang/axton/issues/162)).
#[test]
fn a_refused_rebuild_leaves_the_lane_and_its_identifiers_valid() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let id = rebuildable(&mut host, &dir.path().join("db"));
    let subscription = host
        .call(json!({"op":"scopeSubscribe","handle":id,"scope":"book"}))
        .unwrap()["value"]["subscriptionId"]
        .clone();
    host.call(
        json!({"op":"scopeBootstrap","handle":id,"scope":"book","subscriptionId":subscription}),
    )
    .unwrap();
    let old = epoch(&downlink(&mut host, &id, json!({"event":"start"}))[0]);
    let ack = json!({"type":"subscribed","cursors":{"book":7}}).to_string();
    let load = loading(&downlink(
        &mut host,
        &id,
        json!({"event":"message","epoch":old,"body":ack}),
    ));
    let gap = json!({"cursors":{"book":{"from":8,"to":9,"head":9}},"changes":[]});
    let recovered = downlink(
        &mut host,
        &id,
        json!({"event":"message","epoch":old,"body":gap.to_string()}),
    );
    let pull = requested(&recovered[0]).as_u64().unwrap();

    assert!(
        host.call(json!({"op":"rebuild","handle":id})).is_err(),
        "unsent work refuses the rebuild"
    );
    assert!(
        !status(&mut host, &id)["schema"]["pending"].is_null(),
        "the old replica stays"
    );
    assert_eq!(
        pump(&mut host, &id),
        json!([]),
        "no reset: the lane and its session are untouched"
    );
    let covering = json!({"cursors":{"book":{"from":7,"to":9,"head":9}},"changes":[]});
    assert_eq!(
        downlink(
            &mut host,
            &id,
            json!({"event":"response","request":pull,"body":covering.to_string()})
        ),
        committed(json!(["book"])),
        "the catch-up still answers"
    );
    let applied = downlink(
        &mut host,
        &id,
        json!({"event":"response","request":load,"body":historical(json!([]))}),
    );
    assert_eq!(
        only(&applied, "bootstrap")[0]["subscriptionId"],
        subscription,
        "the historical page still answers: {applied}"
    );
    let next = json!({"cursors":{"book":{"from":9,"to":10,"head":10}},"changes":[]});
    assert_eq!(
        downlink(
            &mut host,
            &id,
            json!({"event":"message","epoch":old,"body":next.to_string()})
        ),
        committed(json!(["book"])),
        "the socket still streams"
    );
    assert_eq!(cursors(&mut host, &id), json!({"book":10}));
}

/// The Scope commands the SDK handles are built on: register durable intent,
/// read the committed state, and remove exactly the registration an identity
/// names ([#150](https://github.com/zanminwang/axton/issues/150)). An
/// uninitialized boundary travels as JSON `null`; zero is a delivery position
/// and never stands for "no boundary".
#[test]
fn scope_commands_register_read_and_remove_one_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    let id = host
        .call(json!({"op":"open","path":dir.path().join("db"),"schema":schema}))
        .unwrap()["value"]["handle"]
        .clone();
    assert!(
        host.call(json!({"op":"scopeState","handle":id,"scope":"book"}))
            .unwrap()["value"]
            .is_null(),
        "no row means unsubscribed"
    );
    let registered = host
        .call(json!({"op":"scopeSubscribe","handle":id,"scope":"book"}))
        .unwrap();
    assert_eq!(
        registered["value"],
        json!({"scope":"book","subscriptionId":1,"startingCursor":null,"cursor":null}),
        "a fresh registration carries no boundary at all, not zero"
    );
    assert_eq!(registered["changed"], true, "the registration committed");
    let again = host
        .call(json!({"op":"scopeSubscribe","handle":id,"scope":"book"}))
        .unwrap();
    assert_eq!(
        again["value"], registered["value"],
        "repeating it reads the stored identity and cursors untouched"
    );
    assert_eq!(again["changed"], false, "nothing was written");
    assert_eq!(
        host.call(json!({"op":"scopeState","handle":id,"scope":"book"}))
            .unwrap()["value"],
        registered["value"]
    );
    // A name the wire refuses names no Scope: the registration is refused
    // before a row exists, so no pump can be poisoned by one.
    for blank in ["", " ", "\t\n"] {
        assert!(
            host.call(json!({"op":"scopeSubscribe","handle":id,"scope":blank}))
                .is_err(),
            "a blank Scope name is refused: {blank:?}"
        );
        assert!(
            host.call(json!({"op":"channel","handle":id,"channel":blank,"subscribed":true}))
                .is_err(),
            "the transaction command holds the same rule: {blank:?}"
        );
    }
    assert_eq!(
        host.call(json!({"op":"status","handle":id})).unwrap()["value"]["channels"],
        json!(["book"]),
        "nothing of a refused registration was written"
    );
    // The transaction command shares the ledger: the same row, the same identity.
    host.call(json!({"op":"channel","handle":id,"channel":"book","subscribed":true}))
        .unwrap();
    assert_eq!(
        host.call(json!({"op":"scopeState","handle":id,"scope":"book"}))
            .unwrap()["value"]["subscriptionId"],
        1
    );
    for malformed in [
        json!(null),
        json!("1"),
        json!(0),
        json!(-1),
        json!(1.5),
        json!(9007199254740992u64),
    ] {
        assert!(
            host.call(
                json!({"op":"scopeUnsubscribe","handle":id,"scope":"book","subscriptionId":malformed})
            )
            .is_err(),
            "a malformed subscription id is refused: {malformed}"
        );
    }
    assert_eq!(
        host.call(json!({"op":"scopeUnsubscribe","handle":id,"scope":"book","subscriptionId":2}))
            .unwrap()["value"],
        json!({"removed":false}),
        "another identity's unsubscribe removes nothing"
    );
    assert_eq!(
        host.call(json!({"op":"status","handle":id})).unwrap()["value"]["channels"],
        json!(["book"]),
        "the registration is untouched"
    );
    let removed = host
        .call(json!({"op":"scopeUnsubscribe","handle":id,"scope":"book","subscriptionId":1}))
        .unwrap();
    assert_eq!(removed["value"], json!({"removed":true}));
    assert_eq!(removed["changed"], true);
    assert!(
        host.call(json!({"op":"scopeState","handle":id,"scope":"book"}))
            .unwrap()["value"]
            .is_null()
    );
    // Identities are never recycled: the next registration is a new one.
    assert_eq!(
        host.call(json!({"op":"scopeSubscribe","handle":id,"scope":"book"}))
            .unwrap()["value"]["subscriptionId"],
        2
    );
    // Scope work is not transaction work: it owns its own local transaction.
    host.call(json!({"op":"begin","handle":id})).unwrap();
    for op in ["scopeSubscribe", "scopeState"] {
        assert!(
            host.call(json!({"op":op,"handle":id,"scope":"other"}))
                .is_err(),
            "{op} is refused while a client transaction is open"
        );
    }
    assert!(
        host.call(json!({"op":"scopeUnsubscribe","handle":id,"scope":"book","subscriptionId":2}))
            .is_err()
    );
    host.call(json!({"op":"rollback","handle":id})).unwrap();
}

/// The Bootstrap commands behind the SDK's `bootstrap()`: registration is a
/// local write that needs no connection, the stored run is readable by the same
/// identity, and the lane asks for the first page once an origin exists
/// ([#151](https://github.com/zanminwang/axton/issues/151)).
#[test]
fn bootstrap_commands_register_read_and_schedule_one_page() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    let id = host
        .call(json!({"op":"open","path":dir.path().join("db"),"schema":schema}))
        .unwrap()["value"]["handle"]
        .clone();
    let registered = host
        .call(json!({"op":"scopeSubscribe","handle":id,"scope":"book"}))
        .unwrap()["value"]
        .clone();
    let subscription = registered["subscriptionId"].clone();
    // Registered offline, before #150 commits an origin: durable intent, and
    // nothing for the lane to ask for yet.
    let requested = host
        .call(
            json!({"op":"scopeBootstrap","handle":id,"scope":"book","subscriptionId":subscription}),
        )
        .unwrap();
    assert_eq!(
        requested["value"],
        json!({"scope":"book","subscriptionId":subscription,"state":"requested","run":1,"cursor":0,"barrier":null,"error":null})
    );
    assert_eq!(requested["changed"], true, "the registration committed");
    assert_eq!(
        host.call(json!({"op":"scopeBootstrapState","handle":id,"scope":"book","subscriptionId":subscription}))
            .unwrap()["value"],
        requested["value"],
        "the stored run reads back unchanged"
    );
    let started = downlink(&mut host, &id, json!({"event":"start"}));
    assert!(
        !started
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["type"] == "request"),
        "no origin, no interval: {started}"
    );
    // The acknowledgement commits the origin, which is the bound the interval
    // was missing, so the page is asked for in the host loop that ran it.
    let epoch = started[0]["epoch"].clone();
    let acknowledged = downlink(
        &mut host,
        &id,
        json!({"event":"message","epoch":epoch,"body":json!({"type":"subscribed","cursors":{"book":7}}).to_string()}),
    );
    let page = acknowledged
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["type"] == "request" && a["bootstrap"] == true)
        .unwrap_or_else(|| panic!("a bootstrap request: {acknowledged}"))
        .clone();
    assert_eq!(
        serde_json::from_str::<Value>(page["body"].as_str().unwrap()).unwrap(),
        json!({"mode":"bootstrap","channel":"book","models":{"Entry":1},"after":0,"until":7}),
        "the interval is bounded by the committed origin"
    );
    // Nothing asks twice: the wake the SDK sends after a commit finds that
    // request already in flight.
    let woken = downlink(&mut host, &id, json!({"event":"wake"}));
    assert!(
        !woken
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["type"] == "request" && a["bootstrap"] == true),
        "one request at a time: {woken}"
    );
    // The answer commits its authority and its progress together, and the
    // committed run travels to the host as one `bootstrap` action.
    let applied = downlink(
        &mut host,
        &id,
        json!({"event":"response","request":page["request"],"body":json!({"mode":"bootstrap","channel":"book","from":0,"to":7,"until":7,"head":9,"records":[]}).to_string()}),
    );
    assert_eq!(
        applied[0],
        json!({"type":"bootstrap","scope":"book","subscriptionId":subscription,"state":"catching_up","run":1,"cursor":7,"barrier":9,"error":null}),
        "the terminal page fixed the barrier; delivery at 7 has not reached 9"
    );
    assert_eq!(
        host.call(json!({"op":"scopeBootstrapState","handle":id,"scope":"book","subscriptionId":subscription}))
            .unwrap()["value"]["state"],
        "catching_up"
    );
    // A malformed identity names no registration, and neither does another one.
    for malformed in [json!(null), json!("1"), json!(0), json!(-1), json!(1.5)] {
        assert!(
            host.call(
                json!({"op":"scopeBootstrap","handle":id,"scope":"book","subscriptionId":malformed})
            )
            .is_err(),
            "a malformed subscription id is refused: {malformed}"
        );
    }
    for op in ["scopeBootstrap", "scopeBootstrapState"] {
        let error = host
            .call(json!({"op":op,"handle":id,"scope":"book","subscriptionId":99}))
            .expect_err("another identity");
        // The refusal crosses the binding with its stable prefix intact: it is
        // what both SDKs match to raise their own `subscription.closed`.
        assert!(
            error.to_string().starts_with("subscription.closed:"),
            "{error}"
        );
        assert!(error.to_string().contains("is closed"), "{error}");
        assert!(
            host.call(json!({"op":op,"handle":id,"scope":"absent","subscriptionId":1}))
                .is_err(),
            "{op} for a Scope that is not subscribed"
        );
    }
    // Load work is not transaction work: it owns its own local transaction.
    host.call(json!({"op":"begin","handle":id})).unwrap();
    for op in ["scopeBootstrap", "scopeBootstrapState"] {
        assert!(
            host.call(json!({"op":op,"handle":id,"scope":"book","subscriptionId":subscription}))
                .is_err(),
            "{op} is refused while a client transaction is open"
        );
    }
    host.call(json!({"op":"rollback","handle":id})).unwrap();
}

/// Closing the client is not unsubscribing: the rows and their boundaries
/// survive it, and only `scopeUnsubscribe` removes one.
#[test]
fn closing_a_client_keeps_the_subscriptions_unsubscribe_removes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut host = RuntimeHost::default();
    let schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    let id = host
        .call(json!({"op":"open","path":&path,"schema":schema}))
        .unwrap()["value"]["handle"]
        .clone();
    let registered = host
        .call(json!({"op":"scopeSubscribe","handle":id,"scope":"book"}))
        .unwrap()["value"]
        .clone();
    let epoch = {
        let opened = downlink(&mut host, &id, json!({"event":"start"}));
        opened[0]["epoch"].clone()
    };
    downlink(
        &mut host,
        &id,
        json!({"event":"message","epoch":epoch,"body":json!({"type":"subscribed","cursors":{"book":7}}).to_string()}),
    );
    let initialized = host
        .call(json!({"op":"scopeState","handle":id,"scope":"book"}))
        .unwrap()["value"]
        .clone();
    assert_eq!(
        initialized,
        json!({"scope":"book","subscriptionId":registered["subscriptionId"],"startingCursor":7,"cursor":7}),
        "the acknowledged head is the committed boundary"
    );
    host.call(json!({"op":"close","handle":id})).unwrap();
    let reopened = host
        .call(json!({"op":"open","path":&path,"schema":schema}))
        .unwrap()["value"]["handle"]
        .clone();
    assert_eq!(
        host.call(json!({"op":"scopeState","handle":reopened,"scope":"book"}))
            .unwrap()["value"],
        initialized,
        "closing the client deleted nothing"
    );
    assert_eq!(
        host.call(json!({"op":"scopeUnsubscribe","handle":reopened,"scope":"book","subscriptionId":registered["subscriptionId"]}))
            .unwrap()["value"],
        json!({"removed":true})
    );
    host.call(json!({"op":"close","handle":reopened})).unwrap();
    let last = host
        .call(json!({"op":"open","path":&path,"schema":schema}))
        .unwrap()["value"]["handle"]
        .clone();
    assert!(
        host.call(json!({"op":"scopeState","handle":last,"scope":"book"}))
            .unwrap()["value"]
            .is_null(),
        "an unsubscribe is durable"
    );
}

#[test]
fn action_store_option_travels_beside_args_on_both_routes() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let schema = json!({"enums":[],"models":[],"actions":[{"name":"Ping","version":1,"inputs":[{"kind":"value","name":"store","type":{"kind":"scalar","name":"string"},"nullable":false}],"outputs":[]}]});
    let id = host
        .call(json!({"op":"open","path":dir.path().join("db"),"schema":schema}))
        .unwrap()["value"]["handle"]
        .clone();
    let prepared = host.call(json!({"op":"prepareAction","handle":id,"name":"Ping","version":1,"args":{"store":"biz"},"store":false})).unwrap()["value"].clone();
    let body: Value = serde_json::from_str(prepared["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["call"]["store"], false);
    assert_eq!(body["call"]["args"], json!({"store":"biz"}));
    for bad in [json!({"missing":false}), json!("no")] {
        assert!(host.call(json!({"op":"submitAction","handle":id,"name":"Ping","version":1,"args":{"store":"biz"},"store":bad})).is_err());
        assert!(host.call(json!({"op":"prepareAction","handle":id,"name":"Ping","version":1,"args":{"store":"biz"},"store":bad})).is_err());
    }
    host.call(json!({"op":"submitAction","handle":id,"name":"Ping","version":1,"args":{"store":"biz"},"store":false}))
        .unwrap();
    let frozen = host.call(json!({"op":"freeze","handle":id})).unwrap()["value"].clone();
    let frozen: Value = serde_json::from_str(frozen.as_str().unwrap()).unwrap();
    assert_eq!(frozen["mutations"][0]["store"], false);
    assert_eq!(frozen["mutations"][0]["args"], json!({"store":"biz"}));
}

/// Query once through the native command boundary: Rust decides Cached,
/// Join or Fetch; the host executes the exact prepared body and finishes or
/// fails the flight. The transaction guard holds for hits and invalidation.
#[test]
fn query_once_commands_decide_cache_join_and_fetch_under_the_transaction_guard() {
    let dir = tempfile::tempdir().unwrap();
    let mut host = RuntimeHost::default();
    let schema = json!({"enums":[],"models":[],"actions":[
        {"name":"Echo","version":1,"kind":"query","inputs":[{"kind":"value","name":"label","type":{"kind":"scalar","name":"string"},"nullable":false}],
         "outputs":[{"name":"label","kind":"value","type":{"kind":"scalar","name":"string"},"cardinality":"single","source":"handlerValue"}]},
        {"name":"Ping","version":1,"inputs":[],"outputs":[]}]});
    let id = host
        .call(json!({"op":"open","path":dir.path().join("db"),"schema":schema}))
        .unwrap()["value"]["handle"]
        .clone();
    let once = |host: &mut RuntimeHost, extra: Value| {
        let mut request =
            json!({"op":"queryOnce","handle":id,"name":"Echo","version":1,"args":{"label":"hi"}});
        for (k, v) in extra.as_object().unwrap() {
            request[k] = v.clone();
        }
        host.call(request)
    };
    let fetch = once(&mut host, json!({})).unwrap()["value"].clone();
    assert_eq!(fetch["decision"], "fetch");
    let flight = fetch["flightId"].clone();
    let body: Value = serde_json::from_str(fetch["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["call"]["callId"], fetch["callId"]);
    assert_eq!(body["call"]["name"], "Echo");
    assert!(
        body["call"].get("once").is_none(),
        "no cache control reaches the wire"
    );
    let join = once(&mut host, json!({})).unwrap()["value"].clone();
    assert_eq!(join, json!({"decision":"join","flightId":flight}));
    let finished = host.call(json!({"op":"finishQueryOnce","handle":id,"flightId":flight,"response":{"completion":{"callId":fetch["callId"],"outcome":{"status":"succeeded","result":{"label":"hi"}}},"records":[]}})).unwrap();
    assert_eq!(
        finished["value"]["completions"][0]["callId"],
        fetch["callId"]
    );
    let cached = once(&mut host, json!({})).unwrap();
    assert_eq!(
        cached["value"],
        json!({"decision":"cached","result":{"label":"hi"}})
    );
    assert_eq!(cached["changed"], false, "a hit commits nothing");
    // Refresh fetches; releasing it keeps the snapshot.
    let refresh = once(&mut host, json!({"refresh":true})).unwrap()["value"].clone();
    assert_eq!(refresh["decision"], "fetch");
    let released = host
        .call(json!({"op":"failQueryOnce","handle":id,"flightId":refresh["flightId"]}))
        .unwrap();
    assert_eq!(released["value"], json!({"released":true}));
    assert_eq!(
        once(&mut host, json!({})).unwrap()["value"]["decision"],
        "cached"
    );
    assert!(once(&mut host, json!({"refresh":"yes"})).is_err());
    assert!(once(&mut host, json!({"store":7})).is_err());
    assert!(
        host.call(json!({"op":"queryOnce","handle":id,"name":"Ping","version":1,"args":{}}))
            .is_err(),
        "a Mutation has no once route"
    );
    // The application transaction guard covers hits and invalidation.
    host.call(json!({"op":"begin","handle":id})).unwrap();
    assert!(once(&mut host, json!({})).is_err());
    assert!(
        host.call(json!({"op":"invalidateQueryOnce","handle":id,"name":"Echo","version":1,"args":{"label":"hi"}}))
            .is_err()
    );
    host.call(json!({"op":"rollback","handle":id})).unwrap();
    let invalidated = host
        .call(json!({"op":"invalidateQueryOnce","handle":id,"name":"Echo","version":1,"args":{"label":"hi"}}))
        .unwrap();
    assert_eq!(invalidated["value"], Value::Null);
    assert_eq!(
        once(&mut host, json!({})).unwrap()["value"]["decision"],
        "fetch"
    );
    // A closed runtime's flights match nothing.
    host.call(json!({"op":"close","handle":id})).unwrap();
    assert!(
        host.call(json!({"op":"failQueryOnce","handle":id,"flightId":flight}))
            .is_err()
    );
}
