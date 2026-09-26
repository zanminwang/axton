//! The runtime owns the operation lifecycles
//! ([#134](https://github.com/zanminwang/axton/issues/134), checkpoint 2):
//! the connection lanes and the Downlink worker, over a real SQLite store. The test is
//! the host: it answers every effect the runtime asks for, with a fixed
//! clock that a fired timer advances. No sleeps, no threads.
mod common;
use axton_client::runtime::{ClientRuntime, Input};
use axton_client::*;
use axton_sqlite::SqliteStore;
use common::ack;
use serde_json::{Value, json};
use std::collections::BTreeMap;

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

    // Stop: everything is abandoned, controls are no-ops again, and a new
    // connect starts over.
    h.task("stop", json!({"kind":"connection","event":"stop"}));
    let events = h.run();
    assert!(cancelled(&events, &socket));
    assert_eq!(signals(&events, "stopped").len(), 1);
    assert!(h.open.is_empty(), "{:?}", h.open);
    h.task("wake", json!({"kind":"connection","event":"wake"}));
    assert_eq!(h.run(), vec![done("wake", Value::Null)]);
    assert!(h.open.is_empty());
    h.connect(false);
    h.socket();
}
