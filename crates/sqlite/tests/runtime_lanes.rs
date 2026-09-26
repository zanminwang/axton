//! The runtime owns the operation lifecycles
//! ([#134](https://github.com/zanminwang/axton/issues/134), checkpoint 2):
//! the connection lanes, the Downlink worker, direct calls, Query once,
//! prerequisites and rebuild fencing, over a real SQLite store. The test is
//! the host: it answers every effect the runtime asks for, with a fixed
//! clock that a fired timer advances. No sleeps, no threads.
mod common;
use axton_client::runtime::{ClientRuntime, Input};
use axton_client::*;
use axton_sqlite::SqliteStore;
use common::ack;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

const ENTROPY: u64 = 200;

fn schema_value() -> Value {
    let mut schema: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    schema["actions"] = json!([
        {"name":"Rename","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"update","cardinality":"single"}],
         "outputs":[{"name":"text","kind":"value","type":{"kind":"scalar","name":"string"},"cardinality":"single","source":"handlerValue"}]},
        {"name":"Echo","version":1,"kind":"query","inputs":[{"kind":"value","name":"label","type":{"kind":"scalar","name":"string"},"nullable":false}],
         "outputs":[{"name":"label","kind":"value","type":{"kind":"scalar","name":"string"},"cardinality":"single","source":"handlerValue"}]},
        {"name":"Ping","version":1,"inputs":[],"outputs":[]}
    ]);
    schema
}
fn factory() -> StoreFactory<SqliteStore> {
    Box::new(|p| SqliteStore::open(p))
}

/// The host of one runtime: it keeps every event, the effects still
/// outstanding, and the clock.
struct Host {
    runtime: ClientRuntime<SqliteStore>,
    now: u64,
    open: BTreeMap<String, Value>,
    _dir: Option<tempfile::TempDir>,
}
fn host() -> Host {
    let dir = tempfile::tempdir().unwrap();
    let runtime = ClientRuntime::open_at(
        dir.path().join("db"),
        Schema::from_value(schema_value()).unwrap(),
        factory(),
        false,
    )
    .unwrap();
    Host::of(runtime, Some(dir))
}
impl Host {
    fn of(runtime: ClientRuntime<SqliteStore>, dir: Option<tempfile::TempDir>) -> Self {
        Self {
            runtime,
            now: 1_000,
            open: BTreeMap::new(),
            _dir: dir,
        }
    }
    fn submit(&mut self, input: Value) {
        let input: Input = serde_json::from_value(input).unwrap();
        self.runtime.receive(input, self.now, ENTROPY).unwrap();
    }
    fn task(&mut self, id: &str, command: Value) {
        self.submit(json!({"type":"task","requestId":id,"command":command}));
    }
    fn connect(&mut self, refresh: bool) {
        self.task(
            "connect",
            json!({"kind":"connect","directTimeoutMs":1000,"refreshAuth":refresh}),
        );
        let events = self.run();
        assert!(events.contains(&done("connect", Value::Null)), "{events:?}");
    }
    /// Track what the host was asked to do.
    fn record(&mut self, events: &[Value]) {
        for event in events {
            match event["type"].as_str().unwrap() {
                "effect" => {
                    self.open.insert(
                        event["effectId"].as_str().unwrap().into(),
                        event["operation"].clone(),
                    );
                }
                "cancelEffect" => {
                    self.open.remove(event["effectId"].as_str().unwrap());
                }
                _ => {}
            }
        }
    }
    /// What the runtime said since the last call: admission first, then every
    /// step until it has nothing runnable.
    fn run(&mut self) -> Vec<Value> {
        let mut events = self.take();
        while self.runtime.step(self.now, ENTROPY) {
            events.extend(self.take());
        }
        events
    }
    /// One step at a time until `id` completes: what the task's completion is
    /// observed with, and nothing after it.
    fn until(&mut self, id: &str) -> Vec<Value> {
        let mut events = self.take();
        loop {
            if events
                .iter()
                .any(|e| e["type"] == "taskCompleted" && e["requestId"] == id)
            {
                return events;
            }
            assert!(self.runtime.step(self.now, ENTROPY), "{id} never completed");
            events.extend(self.take());
        }
    }
    fn take(&mut self) -> Vec<Value> {
        let events: Vec<Value> = self
            .runtime
            .take_events()
            .into_iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect();
        self.record(&events);
        events
    }
    /// The outstanding effects of one kind (and route, for HTTP).
    fn outstanding(&self, kind: &str, route: Option<&str>) -> Vec<(String, Value)> {
        self.open
            .iter()
            .filter(|(_, op)| op["kind"] == kind && route.is_none_or(|r| op["route"] == r))
            .map(|(id, op)| (id.clone(), op.clone()))
            .collect()
    }
    fn one(&self, kind: &str, route: Option<&str>) -> (String, Value) {
        let found = self.outstanding(kind, route);
        assert_eq!(found.len(), 1, "one {kind} {route:?}: {:?}", self.open);
        found.into_iter().next().unwrap()
    }
    fn http(&self, route: &str) -> (String, String) {
        let (id, op) = self.one("http", Some(route));
        (id, op["body"].as_str().unwrap().to_string())
    }
    fn socket(&self) -> String {
        self.one("socket", None).0
    }
    fn answer(&mut self, id: &str, outcome: Value) {
        let streaming = self.open.get(id).is_some_and(|op| op["kind"] == "socket")
            && outcome["ok"] == true
            && outcome["value"]["event"] != "closed";
        if !streaming {
            self.open.remove(id);
        }
        self.submit(json!({"type":"effectResult","effectId":id,"outcome":outcome}));
    }
    fn ok(&mut self, id: &str, body: &str) {
        self.answer(id, json!({"ok":true,"value":{"status":200,"body":body}}));
    }
    fn fail(&mut self, id: &str, message: &str, status: Option<u16>) {
        let mut error = json!({"message":message});
        if let Some(status) = status {
            error["status"] = json!(status);
        }
        self.answer(id, json!({"ok":false,"error":error}));
    }
    fn frame(&mut self, socket: &str, body: &str) {
        self.answer(
            socket,
            json!({"ok":true,"value":{"event":"message","body":body}}),
        );
    }
    /// The timer's time passed: the clock moves and the timer answers.
    fn fire(&mut self, timer: &str) {
        let millis = self.open[timer]["millis"].as_u64().unwrap();
        self.now += millis;
        self.answer(timer, json!({"ok":true,"value":null}));
    }
    fn client(&mut self) -> &mut Client<SqliteStore> {
        self.runtime.client()
    }
    fn text(&mut self, id: &str) -> Option<Value> {
        let key = RecordKey {
            model: "Entry".into(),
            identity: json!({ "id": id }),
        };
        self.client()
            .read(&key)
            .unwrap()
            .map(|row| row["text"].clone())
    }
    /// Subscribe `book`, and on the socket that follows acknowledge `head`.
    fn streaming(&mut self, head: u64) -> (String, u64) {
        self.task("subscribe", json!({"kind":"scopeSubscribe","scope":"book"}));
        let events = self.run();
        let epoch = signals(&events, "opened")[0]["epoch"].as_u64().unwrap();
        let socket = self.socket();
        self.frame(&socket, &ack(&[("book", head)]));
        self.run();
        (socket, epoch)
    }
    fn completion<'a>(&self, events: &'a [Value], id: &str) -> &'a Value {
        events
            .iter()
            .find(|e| e["type"] == "taskCompleted" && e["requestId"] == id)
            .unwrap_or_else(|| panic!("{id} did not complete: {events:?}"))
    }
}

fn done(id: &str, value: Value) -> Value {
    json!({"type":"taskCompleted","requestId":id,"ok":true,"value":value})
}
fn failed(id: &str, error: &str) -> Value {
    json!({"type":"taskCompleted","requestId":id,"ok":false,"value":null,"error":error})
}
fn signals(events: &[Value], lane: &str) -> Vec<Value> {
    events
        .iter()
        .filter(|e| e["type"] == "laneSignal" && e["signal"]["lane"] == lane)
        .map(|e| e["signal"].clone())
        .collect()
}
fn position(events: &[Value], matches: impl Fn(&Value) -> bool) -> usize {
    events
        .iter()
        .position(matches)
        .unwrap_or_else(|| panic!("not found in {events:?}"))
}
fn changes(events: &[Value], table: &str) -> bool {
    events
        .iter()
        .any(|e| e["type"] == "changed" && e["tables"].as_array().unwrap().contains(&json!(table)))
}
fn errors(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e["type"] == "report" && e["diagnostic"]["kind"] == "error")
        .map(|e| e["diagnostic"]["message"].as_str().unwrap().to_string())
        .collect()
}
fn cancelled(events: &[Value], id: &str) -> bool {
    events.contains(&json!({"type":"cancelEffect","effectId":id}))
}
fn page(from: u64, to: u64, id: &str, text: &str) -> String {
    json!({"cursors":{"book":{"from":from,"to":to,"head":to}},"changes":[{"model":"Entry","identity":{"id":id},"stamp":to,"state":{"text":text,"note":null}}]}).to_string()
}
fn call_id(body: &str) -> String {
    serde_json::from_str::<Value>(body).unwrap()["call"]["callId"]
        .as_str()
        .unwrap()
        .to_string()
}
fn create(id: &str, text: &str) -> Value {
    json!({"kind":"direct","operation":{"model":"Entry","op":"create","identity":{"id":id},"values":{"text":text}}})
}
fn rename(text: &str) -> Value {
    json!({"kind":"invoke","name":"Rename","version":1,"args":{"entry":{"id":"e","text":text}}})
}
fn renamed(body: &str, text: &str) -> String {
    json!({"completion":{"callId":call_id(body),"outcome":{"status":"succeeded","result":{"text":text}}},
           "records":[{"model":"Entry","identity":{"id":"e"},"stamp":1,"state":{"text":text,"note":null}}]})
    .to_string()
}
fn echo(extra: Value) -> Value {
    let mut command = json!({"kind":"invoke","name":"Echo","version":1,"args":{"label":"hi"}});
    for (k, v) in extra.as_object().unwrap() {
        command[k] = v.clone();
    }
    command
}
fn echoed(body: &str, label: &str) -> String {
    json!({"completion":{"callId":call_id(body),"outcome":{"status":"succeeded","result":{"label":label}}},"records":[]})
        .to_string()
}
fn receipt(client_id: &str, body: &str) -> String {
    let push: Value = serde_json::from_str(body).unwrap();
    let completions: Vec<Value> = push["mutations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| json!({"callId":m["callId"],"outcome":{"status":"succeeded","result":null}}))
        .collect();
    json!({"clientId":client_id,"batchSequence":push["batchSequence"],"rejections":[],"completions":completions,"records":[]})
        .to_string()
}

#[test]
fn connect_starts_both_lanes_and_the_worker_streams_catches_up_and_applies() {
    let mut h = host();
    h.connect(true);
    assert!(
        h.outstanding("socket", None).is_empty(),
        "no Scope is registered: no socket"
    );
    h.task("again", json!({"kind":"connect"}));
    assert!(
        h.run()
            .contains(&failed("again", "connection already active"))
    );
    // The legacy host-driven lane commands cannot drive the owned lanes.
    h.task("start", json!({"kind":"connection","event":"start"}));
    h.task("pump", json!({"kind":"downlink","event":"next"}));
    let events = h.run();
    assert!(events.contains(&failed("start", "connection already active")));
    assert!(events.contains(&failed("pump", "connection already active")));

    h.task("subscribe", json!({"kind":"scopeSubscribe","scope":"book"}));
    let events = h.run();
    let socket = h.socket();
    let subscribe: Value =
        serde_json::from_str(h.open[&socket]["subscribe"].as_str().unwrap()).unwrap();
    assert_eq!(
        subscribe,
        json!({"type":"subscribe","channels":["book"],"models":{"Entry":1}})
    );
    let epoch = signals(&events, "opened")[0]["epoch"].as_u64().unwrap();
    assert!(
        position(&events, |e| e["type"] == "taskCompleted"
            && e["requestId"] == "subscribe")
            < position(&events, |e| e["type"] == "effect"
                && e["effectId"] == socket.as_str()),
        "the registration commits before the socket is asked for"
    );

    // The handshake commits the first boundary: `changed`, then the signals.
    h.answer(&socket, json!({"ok":true,"value":{"event":"opened"}}));
    h.frame(&socket, &ack(&[("book", 0)]));
    let events = h.run();
    let committed = position(&events, |e| e["type"] == "changed");
    assert!(committed < position(&events, |e| e["signal"]["lane"] == "changed"));
    assert_eq!(
        signals(&events, "changed"),
        [json!({"lane":"changed","scopes":["book"]})]
    );
    assert_eq!(
        signals(&events, "acknowledged"),
        [json!({"lane":"acknowledged","scopes":["book"]})]
    );
    assert_eq!(h.client().cursor("book").unwrap(), Some(0));

    // A streamed page applies in one transaction, announced before its signal.
    h.frame(&socket, &page(0, 1, "e", "first"));
    let events = h.run();
    assert!(
        position(&events, |e| e["type"] == "changed"
            && e["tables"].as_array().unwrap().contains(&json!("Entry")))
            < position(&events, |e| e["signal"]["lane"] == "changed")
    );
    assert_eq!(h.text("e"), Some(json!("first")));

    // A gap asks for a catch-up over HTTP; a page streamed meanwhile waits.
    h.frame(&socket, &page(5, 6, "e", "gap"));
    let events = h.run();
    let (pull, body) = h.http("pull");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        json!({"cursors":{"book":1},"models":{"Entry":1}})
    );
    assert_eq!(
        signals(&events, "requests"),
        [json!({"lane":"requests","outstanding":1})]
    );
    h.frame(&socket, &page(1, 2, "e", "queued"));
    assert!(
        !changes(&h.run(), "Entry"),
        "nothing applies while a catch-up is out"
    );
    // Its answer overlaps the queued page: applied once, the queued page is
    // covered, and the gap still does not connect, so one more catch-up runs.
    h.ok(&pull, &page(1, 3, "e", "incoming overlap"));
    let events = h.run();
    assert!(changes(&events, "Entry"));
    assert_eq!(
        signals(&events, "requests"),
        [
            json!({"lane":"requests","outstanding":0}),
            json!({"lane":"requests","outstanding":1})
        ]
    );
    let (_, body) = h.http("pull");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["cursors"],
        json!({"book":3})
    );
    assert_eq!(h.client().cursor("book").unwrap(), Some(3));
    assert_eq!(h.text("e"), Some(json!("incoming overlap")));
    assert_eq!(signals(&events, "opened"), Vec::<Value>::new());
    let _ = epoch;
}

#[test]
fn a_direct_call_waits_without_the_writer_and_succeeds_only_after_its_apply_commits() {
    let mut h = host();
    h.connect(false);
    let (socket, _) = h.streaming(0);
    h.task("seed", create("e", "seed"));
    h.run();
    h.task("call", rename("server"));
    h.run();
    let (http, body) = h.http("action");
    let (timer, deadline) = h.one("timer", None);
    assert_eq!(deadline["millis"], 1000);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["call"]["name"],
        "Rename"
    );
    // While the request is out, a local write and an unrelated page commit.
    h.task("local", create("x", "local"));
    h.frame(&socket, &page(0, 1, "p", "paged"));
    let events = h.run();
    assert!(events.contains(&done("local", Value::Null)));
    assert_eq!(h.text("x"), Some(json!("local")));
    assert_eq!(h.text("p"), Some(json!("paged")));
    assert!(!events.iter().any(|e| e["requestId"] == "call"));
    // The answer: its apply commits, then the call's completion, then the task.
    h.ok(&http, &renamed(&body, "server"));
    let events = h.until("call");
    let applied = position(&events, |e| {
        e["type"] == "changed" && e["tables"].as_array().unwrap().contains(&json!("Entry"))
    });
    let completed = position(&events, |e| e["type"] == "callCompleted");
    let success = position(&events, |e| e["requestId"] == "call");
    assert!(applied < completed && completed < success, "{events:?}");
    assert_eq!(
        events[success],
        done(
            "call",
            json!({"outcome":{"status":"succeeded","result":{"text":"server"}}})
        )
    );
    assert_eq!(events[completed]["callId"], call_id(&body));
    assert_eq!(
        h.text("e"),
        Some(json!("server")),
        "observed after the commit"
    );
    assert!(cancelled(&events, &timer), "the deadline is gone");

    // `store:false` with nothing to write opens no transaction.
    h.task("quiet", echo(json!({"store":false})));
    h.run();
    let (http, body) = h.http("action");
    let generation = h.client().generation();
    h.ok(&http, &echoed(&body, "hi"));
    let events = h.run();
    assert_eq!(h.client().generation(), generation);
    assert!(!events.iter().any(|e| e["type"] == "changed"), "{events:?}");
    assert_eq!(
        events.last().unwrap(),
        &done(
            "quiet",
            json!({"outcome":{"status":"succeeded","result":{"label":"hi"}}})
        )
    );

    // The result is the invocation's snapshot: optimism queued after the
    // response changes the Model, not the returned outcome.
    h.task("snapshot", rename("authority"));
    h.run();
    let (http, body) = h.http("action");
    h.ok(&http, &renamed(&body, "authority"));
    h.task(
        "optimism",
        json!({"kind":"enqueue","mutation":{"name":"Edit","operations":[{"model":"Entry","op":"update","identity":{"id":"e"},"values":{"text":"local"}}]}}),
    );
    let events = h.run();
    assert_eq!(
        h.completion(&events, "snapshot"),
        &done(
            "snapshot",
            json!({"outcome":{"status":"succeeded","result":{"text":"authority"}}})
        )
    );
    assert_eq!(h.completion(&events, "optimism")["ok"], true);
    assert_eq!(h.text("e"), Some(json!("local")));
}
#[test]
fn the_push_lane_freezes_sends_settles_and_backs_off_with_one_shared_refresh() {
    let mut h = host();
    h.connect(true);
    let client_id = h.client().client_id().to_string();
    h.task(
        "submit",
        json!({"kind":"submitAction","name":"Ping","version":1,"args":{}}),
    );
    let events = h.run();
    let accepted = h.completion(&events, "submit").clone();
    let call = accepted["value"]["callId"].as_str().unwrap().to_string();
    assert_eq!(accepted["value"]["ordinal"], 1);
    assert!(!events.iter().any(|e| e["type"] == "callCompleted"));
    let (push, body) = h.http("push");
    assert!(
        position(&events, |e| e["requestId"] == "submit")
            < position(&events, |e| e["effectId"] == push.as_str()),
        "acceptance completes before the lane sends it"
    );
    // The receipt settles in one transaction; the call's outcome follows it.
    h.ok(&push, &receipt(&client_id, &body));
    let events = h.run();
    let settled = position(&events, |e| e["type"] == "changed");
    let outcome = position(&events, |e| e["type"] == "callCompleted");
    assert!(settled < outcome);
    assert_eq!(
        events[outcome],
        json!({"type":"callCompleted","callId":call,"outcome":{"status":"succeeded","result":null}})
    );
    assert_eq!(h.client().pending_count().unwrap(), 0);
    assert!(h.outstanding("http", Some("push")).is_empty());

    // A transport failure: reported, backed off, retried with the same bytes.
    h.task(
        "again",
        json!({"kind":"submitAction","name":"Ping","version":1,"args":{}}),
    );
    h.run();
    let (push, body) = h.http("push");
    h.fail(&push, "offline", None);
    let events = h.run();
    assert_eq!(errors(&events), ["offline"]);
    let (timer, wait) = h.one("timer", None);
    assert!((200..=300).contains(&wait["millis"].as_u64().unwrap()));
    assert!(h.outstanding("http", Some("push")).is_empty());
    h.fire(&timer);
    h.run();
    let (push, retried) = h.http("push");
    assert_eq!(retried, body, "the frozen batch is sent again unchanged");

    // A 401 on the push and on the socket together: one refresh for both.
    // The commits of the subscription wake the push lane, which still has
    // its one batch out.
    let (socket, _) = h.streaming(0);
    assert_eq!(h.http("push").0, push, "one frozen batch in flight");
    h.fail(&push, "unauthorized", Some(401));
    h.fail(&socket, "unauthorized", Some(401));
    let events = h.run();
    assert_eq!(errors(&events), ["unauthorized", "unauthorized"]);
    let (refresh, _) = h.one("refreshAuth", None);
    assert!(
        h.outstanding("timer", None).is_empty(),
        "both wait for the refresh before backing off"
    );
    assert_eq!(signals(&events, "ended").len(), 1);
    h.answer(&refresh, json!({"ok":true,"value":null}));
    h.run();
    assert!(h.outstanding("refreshAuth", None).is_empty());
    assert_eq!(
        h.outstanding("timer", None).len(),
        2,
        "each lane backs off on its own schedule: {:?}",
        h.open
    );
}

#[test]
fn receipts_direct_applies_and_pages_each_commit_in_their_own_step_beside_a_bootstrap() {
    let mut h = host();
    h.connect(false);
    let client_id = h.client().client_id().to_string();
    h.task("subscribe", json!({"kind":"scopeSubscribe","scope":"book"}));
    let events = h.run();
    let subscription = h.completion(&events, "subscribe")["value"]["subscriptionId"].clone();
    h.task(
        "load",
        json!({"kind":"scopeBootstrap","scope":"book","subscriptionId":subscription}),
    );
    h.run();
    let socket = h.socket();
    h.frame(&socket, &ack(&[("book", 7)]));
    h.run();
    let (load, body) = h.http("pull");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        json!({"mode":"bootstrap","channel":"book","models":{"Entry":1},"after":0,"until":7})
    );
    // Live traffic flows beside the outstanding historical page.
    h.frame(&socket, &page(7, 8, "live", "streamed"));
    h.run();
    assert_eq!(h.text("live"), Some(json!("streamed")));
    // A push, a direct call and a page, all answered before any step runs.
    h.task("seed", create("e", "seed"));
    h.task(
        "submit",
        json!({"kind":"submitAction","name":"Ping","version":1,"args":{}}),
    );
    h.task("call", rename("direct"));
    h.run();
    let (push, push_body) = h.http("push");
    let (call, call_body) = h.http("action");
    h.ok(&push, &receipt(&client_id, &push_body));
    h.ok(&call, &renamed(&call_body, "direct"));
    h.frame(&socket, &page(8, 9, "live", "again"));
    h.ok(&load, &json!({"mode":"bootstrap","channel":"book","from":0,"to":7,"until":7,"head":9,"records":[]}).to_string());
    let mut events = h.take();
    let mut steps = 0;
    loop {
        let before = h.client().generation();
        if !h.runtime.step(h.now, ENTROPY) {
            break;
        }
        steps += 1;
        let after = h.client().generation();
        assert!(after - before <= 1, "one commit per step");
        events.extend(h.take());
    }
    assert!(steps >= 4);
    assert!(events.iter().any(|e| e["type"] == "callCompleted"));
    assert_eq!(h.completion(&events, "call")["ok"], true);
    assert_eq!(h.text("live"), Some(json!("again")));
    assert_eq!(h.text("e"), Some(json!("direct")));
    // The terminal page fixed the barrier at head 9, which live delivery
    // reaches in its own commits: the run completes whichever came first.
    let runs = signals(&events, "bootstrap");
    assert_eq!(runs[0]["run"]["barrier"], 9);
    assert_eq!(
        runs.last().unwrap()["run"]["state"],
        "complete",
        "delivery reached the barrier: {runs:?}"
    );

    // Duplicate and stale answers change nothing.
    let generation = h.client().generation();
    h.ok(&push, &receipt(&client_id, &push_body));
    h.ok(&call, &renamed(&call_body, "twice"));
    h.ok(&load, "{}");
    assert_eq!(h.run(), Vec::<Value>::new());
    assert_eq!(h.client().generation(), generation);
    // An answer on a socket the runtime cancelled is fenced as well.
    h.task("pause", json!({"kind":"connection","event":"pause"}));
    let events = h.run();
    assert!(cancelled(&events, &socket));
    h.frame(&socket, &page(9, 10, "live", "late"));
    h.answer(&socket, json!({"ok":true,"value":{"event":"closed"}}));
    assert_eq!(h.run(), Vec::<Value>::new());
    assert_eq!(h.text("live"), Some(json!("again")));
}

#[test]
fn pause_abandons_lane_io_without_backoff_resume_asks_again_and_stop_is_final() {
    let mut h = host();
    h.task("subscribe", json!({"kind":"scopeSubscribe","scope":"book"}));
    let events = h.run();
    let subscription = h.completion(&events, "subscribe")["value"]["subscriptionId"].clone();
    // Controls with no connection are no-ops.
    for event in ["pause", "resume", "wake", "stop"] {
        h.task(event, json!({"kind":"connection","event":event}));
        assert_eq!(h.run(), vec![done(event, Value::Null)]);
    }
    h.connect(false);
    h.task(
        "load",
        json!({"kind":"scopeBootstrap","scope":"book","subscriptionId":subscription}),
    );
    h.run();
    let socket = h.socket();
    h.frame(&socket, &ack(&[("book", 5)]));
    h.run();
    let (load, _) = h.http("pull");
    h.frame(&socket, &page(7, 8, "e", "gap"));
    h.run();
    let catch_up = h
        .outstanding("http", Some("pull"))
        .into_iter()
        .find(|(id, _)| *id != load)
        .unwrap()
        .0;
    h.task(
        "submit",
        json!({"kind":"submitAction","name":"Ping","version":1,"args":{}}),
    );
    h.run();
    let (push, body) = h.http("push");

    h.task("pause", json!({"kind":"connection","event":"pause"}));
    let events = h.run();
    assert!(events.contains(&done("pause", Value::Null)));
    for id in [&socket, &load, &catch_up, &push] {
        assert!(cancelled(&events, id), "{id} abandoned: {events:?}");
    }
    assert_eq!(
        errors(&events),
        Vec::<String>::new(),
        "the lane's own abort"
    );
    assert!(
        h.open.is_empty(),
        "no backoff timer, nothing in flight: {:?}",
        h.open
    );
    assert_eq!(signals(&events, "paused").len(), 1);
    assert_eq!(signals(&events, "ended").len(), 1);
    // Paused, nothing is asked for, even after a commit.
    h.task(
        "more",
        json!({"kind":"submitAction","name":"Ping","version":1,"args":{}}),
    );
    h.run();
    assert!(h.open.is_empty(), "{:?}", h.open);

    h.task("resume", json!({"kind":"connection","event":"resume"}));
    let events = h.run();
    assert_eq!(signals(&events, "resumed").len(), 1);
    let (_, resent) = h.http("push");
    assert_eq!(
        resent, body,
        "the abandoned batch is still in flight and is sent again"
    );
    let socket = h.socket();
    let (reloaded, again) = h
        .outstanding("http", Some("pull"))
        .into_iter()
        .next()
        .expect("the bootstrap page is asked for again");
    assert_ne!(reloaded, load);
    assert_eq!(
        serde_json::from_str::<Value>(again["body"].as_str().unwrap()).unwrap()["mode"],
        "bootstrap"
    );

    // Stop: a direct call in flight is unavailable, everything is abandoned.
    h.task("call", rename("x"));
    h.run();
    let (call, _) = h.http("action");
    h.task("stop", json!({"kind":"connection","event":"stop"}));
    let events = h.run();
    assert!(events.contains(&failed("call", "action.unavailable")));
    assert!(cancelled(&events, &call) && cancelled(&events, &socket));
    assert_eq!(signals(&events, "stopped").len(), 1);
    assert!(h.open.is_empty(), "{:?}", h.open);
    // A stopped connection: invoke is unavailable, controls are no-ops, and a
    // new connect starts over.
    h.task("late", rename("y"));
    h.task("wake", json!({"kind":"connection","event":"wake"}));
    let events = h.run();
    assert!(events.contains(&failed("late", "action.unavailable")));
    assert!(events.contains(&done("wake", Value::Null)));
    assert!(h.open.is_empty());
    h.connect(false);
    h.socket();
}

#[test]
fn a_direct_call_times_out_refreshes_once_on_401_and_fails_unavailable_at_close() {
    let mut h = host();
    h.connect(true);
    // The deadline passes first: the request is abandoned, its late answer
    // is fenced.
    h.task("slow", rename("slow"));
    h.run();
    let (http, body) = h.http("action");
    let (timer, _) = h.one("timer", None);
    h.fire(&timer);
    let events = h.run();
    assert!(cancelled(&events, &http));
    assert!(events.contains(&failed("slow", "action.execution_unknown")));
    h.ok(&http, &renamed(&body, "slow"));
    assert_eq!(h.run(), Vec::<Value>::new());
    assert_eq!(h.text("e"), None);

    // A 401: one refresh, then the same body once more under the same timer.
    h.task("auth", rename("auth"));
    h.run();
    let (http, body) = h.http("action");
    let (timer, _) = h.one("timer", None);
    h.fail(&http, "unauthorized", Some(401));
    let events = h.run();
    assert_eq!(
        errors(&events),
        Vec::<String>::new(),
        "the call's own failure"
    );
    let (refresh, _) = h.one("refreshAuth", None);
    assert!(h.outstanding("http", Some("action")).is_empty());
    h.answer(&refresh, json!({"ok":true,"value":null}));
    h.run();
    let (retry, resent) = h.http("action");
    assert_eq!(resent, body);
    assert_eq!(
        h.one("timer", None).0,
        timer,
        "the same deadline keeps running"
    );
    h.ok(&retry, &renamed(&body, "auth"));
    let events = h.run();
    assert_eq!(h.completion(&events, "auth")["ok"], true);
    assert!(cancelled(&events, &timer));

    // A second 401 after the retry is not retried again.
    h.task("twice", rename("twice"));
    h.run();
    let (http, _) = h.http("action");
    h.fail(&http, "unauthorized", Some(401));
    h.run();
    let (refresh, _) = h.one("refreshAuth", None);
    h.answer(&refresh, json!({"ok":true,"value":null}));
    h.run();
    let (http, _) = h.http("action");
    h.fail(&http, "unauthorized", Some(401));
    let events = h.run();
    assert!(events.contains(&failed("twice", "action.execution_unknown")));
    assert!(h.outstanding("refreshAuth", None).is_empty());
    // A failed refresh fails the call as unknown and is reported.
    h.task("refused", rename("refused"));
    h.run();
    let (http, _) = h.http("action");
    h.fail(&http, "unauthorized", Some(401));
    h.run();
    let (refresh, _) = h.one("refreshAuth", None);
    h.fail(&refresh, "no credentials", None);
    let events = h.run();
    assert_eq!(errors(&events), ["no credentials"]);
    assert!(events.contains(&failed("refused", "action.execution_unknown")));
    // Any other failure: unknown at once.
    h.task("down", rename("down"));
    h.run();
    let (http, _) = h.http("action");
    h.fail(&http, "HTTP 503", Some(503));
    assert!(
        h.run()
            .contains(&failed("down", "action.execution_unknown"))
    );

    // Close while a call is out: unavailable, and nothing answers later.
    h.task("closing", rename("closing"));
    h.run();
    let (http, body) = h.http("action");
    h.submit(json!({"type":"close"}));
    let events = h.run();
    assert!(events.contains(&failed("closing", "action.unavailable")));
    assert!(cancelled(&events, &http));
    assert_eq!(signals(&events, "stopped").len(), 1);
    assert_eq!(events.last().unwrap(), &json!({"type":"runtimeClosed"}));
    let late: Input = serde_json::from_value(json!({"type":"effectResult","effectId":http,"outcome":{"ok":true,"value":{"status":200,"body":renamed(&body, "late")}}})).unwrap();
    assert!(h.runtime.receive(late, h.now, ENTROPY).is_err());
}

#[test]
fn query_once_is_decided_fetched_joined_and_released_by_the_runtime() {
    let mut h = host();
    // Refresh without once is not an option.
    h.task("bad", echo(json!({"refresh":true})));
    assert!(h.run().contains(&failed("bad", "action.invalid_options")));
    // No connection: a miss is unavailable and releases its flight.
    h.task("offline", echo(json!({"once":true})));
    assert!(h.run().contains(&failed("offline", "action.unavailable")));
    h.connect(false);
    // Two concurrent calls: one request, two completions.
    h.task("first", echo(json!({"once":true})));
    h.task("joined", echo(json!({"once":true})));
    h.run();
    let (http, body) = h.http("action");
    h.ok(&http, &echoed(&body, "hi"));
    let events = h.run();
    let outcome = json!({"outcome":{"status":"succeeded","result":{"label":"hi"}}});
    assert_eq!(
        h.completion(&events, "first"),
        &done("first", outcome.clone())
    );
    assert_eq!(
        h.completion(&events, "joined"),
        &done("joined", outcome.clone())
    );
    assert!(changes(&events, "axton_query_cache"), "the result is saved");
    // A hit: no effect, no write.
    let generation = h.client().generation();
    h.task("cached", echo(json!({"once":true})));
    let events = h.run();
    assert_eq!(events, vec![done("cached", outcome.clone())]);
    assert_eq!(h.client().generation(), generation);
    // A refresh fetches; its failure keeps the saved result.
    h.task("refresh", echo(json!({"once":true,"refresh":true})));
    h.task("refresh-joined", echo(json!({"once":true,"refresh":true})));
    h.run();
    let (http, _) = h.http("action");
    h.fail(&http, "offline", None);
    let events = h.run();
    assert!(events.contains(&failed("refresh", "action.execution_unknown")));
    assert!(events.contains(&failed("refresh-joined", "action.execution_unknown")));
    h.task("still", echo(json!({"once":true})));
    assert_eq!(h.run(), vec![done("still", outcome.clone())]);
    // The failed flight was released: the next refresh fetches again.
    h.task("next", echo(json!({"once":true,"refresh":true})));
    h.run();
    h.http("action");
    // Once is a task, never a command of an open transaction.
    h.task("tx", json!({"kind":"transaction"}));
    let events = h.run();
    let effect = events
        .iter()
        .find(|e| e["operation"]["kind"] == "callback")
        .unwrap();
    let transaction = effect["operation"]["transactionId"].clone();
    h.submit(json!({"type":"transactionCommand","requestId":"inside","transactionId":transaction,"command":echo(json!({"once":true}))}));
    assert!(
        h.run()
            .contains(&failed("inside", "unsupported transaction command"))
    );
    h.submit(json!({"type":"callbackResult","effectId":effect["effectId"],"transactionId":transaction,"ok":true}));
    h.run();
    // Close with a joined caller: both fail as unavailable.
    h.task("waiting", echo(json!({"once":true,"refresh":true})));
    let events = h.run();
    assert!(!events.iter().any(|e| e["requestId"] == "waiting"));
    h.submit(json!({"type":"close"}));
    let events = h.run();
    assert!(events.contains(&failed("next", "action.unavailable")));
    assert!(events.contains(&failed("waiting", "action.unavailable")));
}

#[test]
fn prerequisites_run_as_effects_and_every_outcome_wakes_the_push_lane() {
    let mut h = host();
    h.connect(false);
    h.task("seed", create("e", "seed"));
    let key = |id: &str| json!({"name":"upload","arguments":{"id":id}}).to_string();
    h.task(
        "edit",
        json!({"kind":"enqueue","mutation":{"name":"Edit","operations":[{"model":"Entry","op":"update","identity":{"id":"e"},"values":{"text":"edited"}}],"prerequisites":[key("a"),key("b")]}}),
    );
    h.run();
    assert!(h.outstanding("http", Some("push")).is_empty(), "blocked");
    h.task(
        "run",
        json!({"kind":"runPrerequisites","handlers":["upload"]}),
    );
    h.task(
        "twice",
        json!({"kind":"runPrerequisites","handlers":["upload"]}),
    );
    let events = h.run();
    assert!(events.contains(&failed("twice", "prerequisites already running")));
    let (first, op) = h.one("prerequisite", None);
    assert_eq!(op["name"], "upload");
    assert_eq!(op["key"], key("a"));
    assert_eq!(op["arguments"], json!({"id":"a"}));
    // A failing handler keeps its reason; the loop goes on.
    h.fail(&first, "disk full", None);
    let events = h.run();
    assert!(changes(&events, "axton_client"));
    let (second, op) = h.one("prerequisite", None);
    assert_eq!(op["key"], key("b"));
    h.task("tasks", json!({"kind":"tasks"}));
    let events = h.run();
    let tasks = h.completion(&events, "tasks")["value"].clone();
    let a = tasks
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == key("a"))
        .unwrap();
    assert_eq!(
        (a["state"].clone(), a["error"].clone()),
        (json!("failed"), json!("disk full"))
    );
    h.answer(&second, json!({"ok":true,"value":null}));
    let events = h.run();
    assert!(events.contains(&done("run", Value::Null)));
    assert!(h.outstanding("prerequisite", None).is_empty());
    // The push lane woke after each outcome and found the mutation still
    // blocked by the failed task.
    assert!(h.outstanding("http", Some("push")).is_empty());
    h.task(
        "ready",
        json!({"kind":"readiness","key":key("a"),"state":"ready"}),
    );
    h.run();
    h.http("push");
}

/// A replica left incompatible with unsent work: rebuild is pending, and
/// `book` is carried over by name.
fn pending_rebuild(dir: &Path) -> ClientRuntime<SqliteStore> {
    let path = dir.join("db");
    {
        let mut runtime = ClientRuntime::open_at(
            &path,
            Schema::from_value(schema_value()).unwrap(),
            factory(),
            false,
        )
        .unwrap();
        let client = runtime.client();
        client
            .transaction(|tx| tx.set_channel("book".into(), true))
            .unwrap();
        client
            .transaction(|tx| {
                tx.enqueue(Mutation::new(
                    "Edit",
                    vec![common::create("Entry", "e", json!({"text":"unsent"}))],
                ))
            })
            .unwrap();
        let subscription = client.ensure_subscription("book").unwrap();
        client
            .request_bootstrap("book", subscription.subscription_id)
            .unwrap();
        client.submit_action("Ping", 1, json!({})).unwrap();
    }
    let mut breaking = schema_value();
    breaking["models"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"due","nullable":false,"type":{"kind":"scalar","name":"string"}}));
    ClientRuntime::open_at(
        &path,
        Schema::from_value(breaking).unwrap(),
        factory(),
        false,
    )
    .unwrap()
}

#[test]
fn a_rebuild_fences_old_lane_io_and_reopens_in_the_same_intent() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Host::of(pending_rebuild(dir.path()), Some(dir));
    let status = {
        h.task("status", json!({"kind":"status"}));
        let events = h.run();
        h.completion(&events, "status")["value"].clone()
    };
    assert!(status["schema"]["pending"].is_object(), "{status}");
    h.connect(false);
    let (old_push, _) = h.http("push");
    let old_socket = h.socket();
    let old_epoch = h.open[&old_socket].clone();
    let events = h.run();
    let _ = events;
    h.frame(&old_socket, &ack(&[("book", 2)]));
    h.run();
    let (old_load, _) = h.http("pull");
    h.frame(&old_socket, &page(3, 4, "e", "gap"));
    h.run();
    let old_pull = h
        .outstanding("http", Some("pull"))
        .into_iter()
        .find(|(id, _)| *id != old_load)
        .unwrap()
        .0;
    let generation = h.client().generation();
    // A refused rebuild changes nothing.
    h.task("refused", json!({"kind":"rebuild"}));
    let events = h.run();
    assert_eq!(h.completion(&events, "refused")["ok"], false);
    assert!(
        !events.iter().any(|e| e["type"] == "cancelEffect"),
        "{events:?}"
    );
    assert_eq!(h.client().generation(), generation);
    assert_eq!(h.socket(), old_socket);

    h.task("rebuild", json!({"kind":"rebuild","discardPending":true}));
    let events = h.run();
    let report = h.completion(&events, "rebuild")["value"].clone();
    assert_eq!(report["leftPending"], 2);
    // The call left behind was never sent (the batch out held the plain
    // mutation): it is rejected, announced before the rebuild's own answer.
    let abandoned = report["abandonedCalls"][0]["callId"].clone();
    assert_eq!(report["abandonedCalls"][0]["frozen"], false);
    let outcome = position(&events, |e| e["type"] == "callCompleted");
    assert_eq!(
        events[outcome],
        json!({"type":"callCompleted","callId":abandoned,"outcome":{"status":"failed","code":"abandoned","execution":"rejected"}})
    );
    assert!(outcome < position(&events, |e| e["requestId"] == "rebuild"));
    for id in [&old_socket, &old_pull, &old_load, &old_push] {
        assert!(cancelled(&events, id), "{id}: {events:?}");
    }
    assert_eq!(
        signals(&events, "ended"),
        [json!({"lane":"ended","epoch":signals_epoch(&events, "ended")})]
    );
    // A new session follows without another connect, with a greater epoch.
    let socket = h.socket();
    assert_ne!(socket, old_socket);
    let opened = signals(&events, "opened");
    assert_eq!(opened.len(), 1);
    let epoch = opened[0]["epoch"].as_u64().unwrap();
    assert!(
        epoch > 1,
        "the epoch allocator survived: {epoch} after {old_epoch}"
    );
    assert!(changes(&events, "Entry"));
    // Old answers change nothing.
    let cursors = h.client().subscriptions().unwrap();
    let generation = h.client().generation();
    h.ok(
        &old_load,
        &json!({"mode":"bootstrap","channel":"book","from":0,"to":2,"until":2,"head":2,"records":[]})
            .to_string(),
    );
    h.ok(&old_push, "{}");
    h.frame(&old_socket, &ack(&[("book", 9)]));
    h.frame(&old_socket, &page(0, 1, "e", "old"));
    h.ok(&old_pull, &page(0, 4, "e", "old"));
    h.fail(&old_pull, "late", Some(503));
    h.answer(&old_socket, json!({"ok":true,"value":{"event":"overflow"}}));
    h.answer(&old_socket, json!({"ok":true,"value":{"event":"closed"}}));
    assert_eq!(h.run(), Vec::<Value>::new());
    assert_eq!(h.client().generation(), generation);
    assert_eq!(h.client().subscriptions().unwrap(), cursors);
    assert_eq!(h.text("e"), None);
    assert_eq!(h.socket(), socket);
    // The new session initializes the carried Scope at its own head.
    h.frame(&socket, &ack(&[("book", 5)]));
    h.run();
    assert_eq!(h.client().cursor("book").unwrap(), Some(5));
}
fn signals_epoch(events: &[Value], lane: &str) -> Value {
    signals(events, lane)[0]["epoch"].clone()
}

#[test]
fn a_paused_lane_stays_paused_through_a_rebuild_until_resume() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Host::of(pending_rebuild(dir.path()), Some(dir));
    h.connect(false);
    let old_socket = h.socket();
    h.task("pause", json!({"kind":"connection","event":"pause"}));
    h.run();
    h.task("rebuild", json!({"kind":"rebuild","discardPending":true}));
    let events = h.run();
    assert_eq!(h.completion(&events, "rebuild")["ok"], true);
    assert!(
        h.open.is_empty(),
        "paused: nothing is asked for: {:?}",
        h.open
    );
    h.task("resume", json!({"kind":"connection","event":"resume"}));
    h.run();
    assert_ne!(h.socket(), old_socket);
}

#[test]
fn a_stopped_lane_stays_stopped_through_a_rebuild_until_connect() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Host::of(pending_rebuild(dir.path()), Some(dir));
    h.connect(false);
    h.task("stop", json!({"kind":"connection","event":"stop"}));
    h.run();
    assert!(h.open.is_empty());
    h.task("rebuild", json!({"kind":"rebuild","discardPending":true}));
    let events = h.run();
    assert_eq!(h.completion(&events, "rebuild")["ok"], true);
    assert!(h.open.is_empty(), "stopped: nothing restarts: {:?}", h.open);
    assert!(signals(&events, "opened").is_empty());
    h.connect(false);
    h.socket();
}
