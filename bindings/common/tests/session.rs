use axton_binding::RuntimeHost;
use serde_json::{Value, json};
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
    host.call(json!({"op":"channel","handle":id,"channel":"book","subscribed":true}))
        .unwrap();
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
    host.call(json!({"op":"channel","handle":id,"channel":"book","subscribed":true}))
        .unwrap();
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

#[test]
fn live_and_push_drivers_have_independent_lifecycle_and_retry_state() {
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
    let opened = host
        .call(json!({"op":"live","handle":id,"event":"start","now":0}))
        .unwrap()["value"]
        .clone();
    assert_eq!(opened[0]["type"], "open");
    let epoch = opened[0]["epoch"].clone();
    // The live socket drops: its lane backs off while the push lane is unaffected.
    let closed = host
        .call(json!({"op":"live","handle":id,"event":"closed","epoch":epoch,"now":0,"entropy":0}))
        .unwrap()["value"]
        .clone();
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
    assert_eq!(
        host.call(json!({"op":"live","handle":id,"event":"next","now":0}))
            .unwrap()["value"][0]["type"],
        "wait"
    );
    for bad in [
        json!({"op":"live","handle":id,"event":"unknown","now":0}),
        json!({"op":"live","handle":id,"event":"message","now":0}),
        json!({"op":"connection","handle":id,"event":"unknown","now":0}),
    ] {
        assert!(host.call(bad).is_err());
    }
}

/// Drive one live session to its streaming phase: subscribe `book`, start the
/// lane, acknowledge, and answer the first catch-up with `first`.
fn streaming(host: &mut RuntimeHost, id: &Value, first: &Value) -> Value {
    host.call(json!({"op":"channel","handle":id,"channel":"book","subscribed":true}))
        .unwrap();
    let opened = host
        .call(json!({"op":"live","handle":id,"event":"start","now":0}))
        .unwrap()["value"]
        .clone();
    let epoch = opened[0]["epoch"].clone();
    assert_eq!(
        serde_json::from_str::<Value>(opened[0]["subscribe"].as_str().unwrap()).unwrap(),
        json!({"type":"subscribe","channels":["book"],"models":{"Entry":1}})
    );
    // The acknowledged head is beyond the durable cursor: one pull from it.
    let ack = json!({"type":"subscribed","cursors":{"book":first["cursors"]["book"]["head"]}})
        .to_string();
    let requested = host
        .call(json!({"op":"live","handle":id,"event":"message","epoch":epoch,"body":ack,"now":0}))
        .unwrap()["value"]
        .clone();
    if first["cursors"]["book"]["head"] == 0 {
        assert_eq!(requested, json!([]), "at the head: no catch-up");
        return epoch;
    }
    assert_eq!(requested[0]["type"], "request");
    assert_eq!(
        serde_json::from_str::<Value>(requested[0]["body"].as_str().unwrap()).unwrap()["cursors"],
        json!({"book":0})
    );
    host.call(json!({"op":"live","handle":id,"event":"catchUp","epoch":epoch,"body":first.to_string(),"now":0}))
        .unwrap();
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
    let epoch = streaming(&mut host, &id, &page);
    let deliver = |host: &mut RuntimeHost, event: &str, page: &Value| {
        host.call(json!({"op":"live","handle":id,"event":event,"epoch":epoch,"body":page.to_string(),"now":0}))
            .unwrap()["value"]
            .clone()
    };
    assert_eq!(deliver(&mut host, "message", &page), json!([]), "covered");
    let gap = json!({"cursors":{"book":{"from":2,"to":3,"head":3}},"changes":[]});
    let recovered = deliver(&mut host, "message", &gap);
    assert_eq!(recovered[0]["type"], "request", "a gap recovers over HTTP");
    host.call(json!({"op":"enqueue","handle":id,"mutation":{"name":"Edit","operations":[{"model":"Entry","op":"update","identity":{"id":"e"},"values":{"text":"B"}}]}})).unwrap();
    host.call(json!({"op":"startSync","handle":id,"pushOnly":true}))
        .unwrap();
    let push = host.call(json!({"op":"next","handle":id})).unwrap()["value"].clone();
    // The pull covers the held gap frame, which is then discarded.
    let covering = json!({"cursors":{"book":{"from":1,"to":3,"head":3}},"changes":[]});
    assert_eq!(
        deliver(&mut host, "catchUp", &covering),
        json!([{"type":"wake","lane":"push"}])
    );
    assert_eq!(
        host.call(json!({"op":"next","handle":id})).unwrap()["value"],
        push,
        "the live session never touches the push cycle"
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
        let empty = json!({"cursors":{"book":{"from":0,"to":0,"head":0}},"changes":[]});
        let epoch = streaming(&mut host, &id, &empty);
        let deliver = |host: &mut RuntimeHost, event: &str, page: &Value| {
            host.call(json!({"op":"live","handle":id,"event":event,"epoch":epoch,"body":page.to_string(),"now":0}))
                .unwrap()["value"]
                .clone()
        };
        let first = json!({"cursors":{"book":{"from":0,"to":1,"head":1}},"changes":[{"model":"Entry","identity":{"id":"e"},"stamp":1,"state":{"text":"first","note":null}}]});
        let overlap = json!({"cursors":{"book":{"from":0,"to":2,"head":2}},"changes":[{"model":"Entry","identity":{"id":"e"},"stamp":2,"state":{"text":"incoming overlap","note":null}}]});
        if over_http {
            // A streamed gap makes the session request from cursor 0; the first
            // page then lands through the stream before the answer arrives.
            let gap = json!({"cursors":{"book":{"from":5,"to":6,"head":6}},"changes":[]});
            assert_eq!(deliver(&mut host, "message", &gap)[0]["type"], "request");
        }
        // While a pull is in flight, a streamed page waits in the queue and is
        // covered once the HTTP answer lands; in streaming it applies at once.
        assert_eq!(
            deliver(&mut host, "message", &first),
            if over_http {
                json!([])
            } else {
                json!([{"type":"wake","lane":"push"}])
            }
        );
        assert_eq!(
            deliver(
                &mut host,
                if over_http { "catchUp" } else { "message" },
                &overlap
            ),
            if over_http {
                // The pull applied, then the queue: `first` is covered and the
                // gap frame still does not connect, so one more pull runs.
                json!([{"type":"wake","lane":"push"},{"type":"request","epoch":epoch,"body":"{\"cursors\":{\"book\":2},\"models\":{\"Entry\":1}}"}])
            } else {
                json!([{"type":"wake","lane":"push"}])
            },
            "applied over {}",
            if over_http { "HTTP" } else { "the stream" }
        );
        assert_eq!(
            host.call(json!({"op":"live","handle":id,"event":"message","epoch":epoch,"body":overlap.to_string(),"now":0}))
                .unwrap()["value"],
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
            "downlinkRequest",
            "downlinkPage",
            "downlinkComplete",
            "downlinkLive",
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
    let first = json!({"cursors":{"book":{"from":0,"to":0,"head":0}},"changes":[]});
    let epoch = streaming(&mut host, &id, &first);
    let gap = json!({"cursors":{"book":{"from":5,"to":6,"head":6}},"changes":[]}).to_string();
    host.call(json!({"op":"live","handle":id,"event":"message","epoch":epoch,"body":gap,"now":0}))
        .unwrap();
    let other = json!({"cursors":{"other":{"from":0,"to":2,"head":2}},"changes":[]}).to_string();
    let ended = host
        .call(json!({"op":"live","handle":id,"event":"catchUp","epoch":epoch,"body":other,"now":0}))
        .unwrap()["value"]
        .clone();
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
