//! The Rust-owned client runtime over a real SQLite store, driven step by step
//! with fixed clock and entropy facts: task correlation, transaction ownership
//! across the application callback, savepoint scopes, close and commit
//! failure ([#134](https://github.com/zanminwang/axton/issues/134)).
mod common;
use axton_client::runtime::{BridgeError, ClientRuntime, Event, Input};
use axton_client::*;
use axton_sqlite::SqliteStore;
use common::{key, schema};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const NOW: u64 = 1_000;
const ENTROPY: u64 = 7;

struct Harness<S: ClientStore + 'static> {
    runtime: ClientRuntime<S>,
    _dir: tempfile::TempDir,
}
fn harness() -> Harness<SqliteStore> {
    let dir = tempfile::tempdir().unwrap();
    let client = Client::open(SqliteStore::open(dir.path().join("db")).unwrap(), schema()).unwrap();
    Harness {
        runtime: ClientRuntime::new(client),
        _dir: dir,
    }
}
impl<S: ClientStore + 'static> Harness<S> {
    fn submit(&mut self, input: Value) -> std::result::Result<(), BridgeError> {
        let input: Input = serde_json::from_value(input).unwrap();
        self.runtime.receive(input, NOW, ENTROPY)
    }
    fn task(&mut self, id: &str, command: Value) {
        self.submit(json!({"type":"task","requestId":id,"command":command}))
            .unwrap();
    }
    fn command(&mut self, id: &str, transaction: &str, scope: Option<&str>, command: Value) {
        let mut input = json!({"type":"transactionCommand","requestId":id,"transactionId":transaction,"command":command});
        if let Some(scope) = scope {
            input["scope"] = json!(scope);
        }
        self.submit(input).unwrap();
    }
    fn callback(&mut self, open: &Open, ok: bool, error: Option<&str>) {
        let mut input = json!({"type":"callbackResult","effectId":open.effect,"transactionId":open.transaction,"ok":ok});
        if let Some(error) = error {
            input["error"] = json!(error);
        }
        self.submit(input).unwrap();
    }
    /// Step until the runtime has nothing runnable, then hand over what it said.
    fn run(&mut self) -> Vec<Value> {
        while self.runtime.step(NOW, ENTROPY) {}
        self.events()
    }
    fn events(&mut self) -> Vec<Value> {
        self.runtime
            .take_events()
            .into_iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect()
    }
    /// Submit a `transaction` task and answer its callback effect's identities.
    fn begin(&mut self, id: &str) -> Open {
        self.task(id, json!({"kind":"transaction"}));
        let events = self.run();
        assert_eq!(events.len(), 1, "{events:?}");
        let effect = &events[0];
        assert_eq!(effect["type"], "effect");
        assert_eq!(effect["operation"]["kind"], "callback");
        assert_eq!(effect["operation"]["requestId"], id);
        Open {
            effect: effect["effectId"].as_str().unwrap().to_string(),
            transaction: effect["operation"]["transactionId"]
                .as_str()
                .unwrap()
                .to_string(),
        }
    }
    fn committed(&mut self) -> Option<Value> {
        self.runtime.client().read(&key()).unwrap()
    }
}
struct Open {
    effect: String,
    transaction: String,
}
fn create(id: &str, text: &str) -> Value {
    json!({"kind":"direct","operation":{"model":"Entry","op":"create","identity":{"id":id},"values":{"text":text}}})
}
fn read(id: &str) -> Value {
    json!({"kind":"read","key":{"model":"Entry","identity":{"id":id}}})
}
fn missing_model() -> Value {
    json!({"kind":"direct","operation":{"model":"Nope","op":"create","identity":{"id":"x"},"values":{}}})
}
fn done(id: &str, value: Value) -> Value {
    json!({"type":"taskCompleted","requestId":id,"ok":true,"value":value})
}
fn failed(id: &str, error: &str) -> Value {
    json!({"type":"taskCompleted","requestId":id,"ok":false,"value":null,"error":error})
}
fn is_changed(event: &Value) -> bool {
    event["type"] == "changed"
        && event["tables"]
            .as_array()
            .unwrap()
            .contains(&json!("Entry"))
}
fn row(text: &str) -> Value {
    json!({"id":"e","text":text,"note":null})
}

#[test]
fn tasks_complete_in_order_to_their_own_request_and_a_duplicate_never_runs_twice() {
    let mut h = harness();
    h.task("1", read("e"));
    h.task("2", create("e", "hi"));
    h.task("3", missing_model());
    h.task("4", read("e"));
    // A second submission of a routed id is a protocol report, not a task.
    h.task("4", create("e", "twice"));
    let reported = h.events();
    assert_eq!(
        reported,
        vec![
            json!({"type":"report","diagnostic":{"kind":"protocol","message":"duplicate request id 4"}})
        ]
    );
    let events = h.run();
    assert_eq!(events.len(), 5, "{events:?}");
    assert_eq!(events[0], done("1", Value::Null));
    assert!(is_changed(&events[1]), "{events:?}");
    assert_eq!(events[2], done("2", Value::Null));
    assert_eq!(events[3]["requestId"], "3");
    assert_eq!(events[3]["ok"], false);
    assert!(!events[3]["error"].as_str().unwrap().is_empty());
    assert_eq!(events[4], done("4", row("hi")));
    // Nothing else ran: the duplicate's write never happened.
    assert_eq!(h.committed(), Some(row("hi")));
    // Once completed, an id no longer routes anything; the runtime is idle.
    assert!(!h.runtime.step(NOW, ENTROPY));
    h.task("5", json!({"kind":"nope"}));
    assert_eq!(h.run(), vec![failed("5", "unknown client command nope")]);
}

#[test]
fn a_callback_transaction_owns_the_writer_until_its_result_commits() {
    let mut h = harness();
    let a = h.begin("1");
    // An ordinary read waits outside the open transaction.
    h.task("2", read("e"));
    assert_eq!(h.run(), Vec::<Value>::new());
    // The callback's own write runs on the continuation lane.
    h.command("3", &a.transaction, None, create("e", "hi"));
    assert_eq!(h.run(), vec![done("3", Value::Null)]);
    // A wrong token joins nothing.
    h.command("4", "tx999", None, read("e"));
    assert_eq!(h.run(), vec![failed("4", "transaction_closed")]);
    h.command("5", &a.transaction, None, read("e"));
    assert_eq!(h.run(), vec![done("5", row("hi"))]);
    // Nothing is committed yet: the committed reader does not see the row.
    assert_eq!(h.committed(), None);
    h.callback(&a, true, None);
    let events = h.run();
    assert_eq!(events.len(), 3, "{events:?}");
    assert!(is_changed(&events[0]), "{events:?}");
    assert_eq!(events[1], done("1", Value::Null));
    assert_eq!(events[2], done("2", row("hi")));
    // After the result the transaction is closed to its own token too.
    h.command("6", &a.transaction, None, read("e"));
    assert_eq!(h.run(), vec![failed("6", "transaction_closed")]);
    // Identities are strings from one counter and are never reused.
    let b = h.begin("7");
    assert_ne!(a.transaction, b.transaction);
    assert_ne!(a.effect, b.effect);
    h.callback(&b, true, None);
    let events = h.run();
    assert_eq!(events.last(), Some(&done("7", Value::Null)));
}

#[test]
fn savepoint_scopes_admit_only_the_innermost_open_scope() {
    // A command naming the wrong scope fails structurally and poisons the unit.
    let mut h = harness();
    let a = h.begin("1");
    h.command("2", &a.transaction, None, json!({"kind":"savepoint"}));
    let events = h.run();
    let scope = events[0]["value"]["scope"].as_str().unwrap().to_string();
    assert_eq!(events, vec![done("2", json!({"scope":scope}))]);
    assert!(scope.starts_with("sp"));
    h.command("3", &a.transaction, None, create("e", "outer"));
    assert_eq!(h.run(), vec![failed("3", "invalid transaction scope")]);
    h.command("4", &a.transaction, Some(&scope), create("e", "inner"));
    h.command(
        "5",
        &a.transaction,
        Some(&scope),
        json!({"kind":"release","scope":scope}),
    );
    assert_eq!(
        h.run(),
        vec![done("4", Value::Null), done("5", Value::Null)]
    );
    h.callback(&a, true, None);
    assert_eq!(h.run(), vec![failed("1", "invalid transaction scope")]);
    assert_eq!(h.committed(), None);

    // A failure inside a savepoint that rolls back leaves the outer unit
    // committable; nested scopes release in order.
    let mut h = harness();
    let a = h.begin("1");
    h.command("2", &a.transaction, None, create("e", "outer"));
    h.command("3", &a.transaction, None, json!({"kind":"savepoint"}));
    let events = h.run();
    assert_eq!(events[0], done("2", Value::Null));
    let outer = events[1]["value"]["scope"].as_str().unwrap().to_string();
    h.command(
        "4",
        &a.transaction,
        Some(&outer),
        json!({"kind":"savepoint"}),
    );
    let inner = h.run()[0]["value"]["scope"].as_str().unwrap().to_string();
    assert_ne!(inner, outer);
    h.command("5", &a.transaction, Some(&inner), missing_model());
    let events = h.run();
    assert_eq!(events[0]["requestId"], "5");
    assert_eq!(events[0]["ok"], false);
    h.command(
        "6",
        &a.transaction,
        Some(&inner),
        json!({"kind":"rollbackSavepoint","scope":inner}),
    );
    h.command(
        "7",
        &a.transaction,
        Some(&outer),
        json!({"kind":"release","scope":outer}),
    );
    assert_eq!(
        h.run(),
        vec![done("6", Value::Null), done("7", Value::Null)]
    );
    h.callback(&a, true, None);
    let events = h.run();
    assert!(is_changed(&events[0]), "{events:?}");
    assert_eq!(events[1], done("1", Value::Null));
    assert_eq!(h.committed(), Some(row("outer")));

    // A failure caught at the top level still poisons the commit.
    let mut h = harness();
    let a = h.begin("1");
    h.command("2", &a.transaction, None, create("e", "kept?"));
    h.command("3", &a.transaction, None, missing_model());
    let events = h.run();
    assert_eq!(events[0], done("2", Value::Null));
    let error = events[1]["error"].as_str().unwrap().to_string();
    h.callback(&a, true, None);
    assert_eq!(h.run(), vec![failed("1", &error)]);
    assert_eq!(h.committed(), None);

    // `release` pops only the top scope.
    let mut h = harness();
    let a = h.begin("1");
    h.command("2", &a.transaction, None, json!({"kind":"savepoint"}));
    let outer = h.run()[0]["value"]["scope"].as_str().unwrap().to_string();
    h.command(
        "3",
        &a.transaction,
        Some(&outer),
        json!({"kind":"savepoint"}),
    );
    let inner = h.run()[0]["value"]["scope"].as_str().unwrap().to_string();
    h.command(
        "4",
        &a.transaction,
        Some(&inner),
        json!({"kind":"release","scope":outer}),
    );
    assert_eq!(h.run(), vec![failed("4", "invalid transaction scope")]);
    // The inner scope is still the top one.
    h.command("5", &a.transaction, Some(&inner), create("e", "x"));
    assert_eq!(h.run(), vec![done("5", Value::Null)]);
    h.callback(&a, true, None);
    assert_eq!(h.run(), vec![failed("1", "invalid transaction scope")]);

    // An unclosed savepoint at a successful result rolls back.
    let mut h = harness();
    let a = h.begin("1");
    h.command("2", &a.transaction, None, create("e", "x"));
    h.command("3", &a.transaction, None, json!({"kind":"savepoint"}));
    assert_eq!(h.run().len(), 2);
    h.callback(&a, true, None);
    assert_eq!(h.run(), vec![failed("1", "unclosed savepoint")]);
    assert_eq!(h.committed(), None);
    // The runtime is usable afterwards.
    h.task("4", read("e"));
    assert_eq!(h.run(), vec![done("4", Value::Null)]);
}

#[test]
fn callback_failure_rolls_back_and_stale_or_late_messages_join_nothing() {
    let mut h = harness();
    let a = h.begin("1");
    h.command("2", &a.transaction, None, create("e", "hi"));
    assert_eq!(h.run(), vec![done("2", Value::Null)]);
    // A result naming another effect or transaction is ignored.
    h.submit(
        json!({"type":"callbackResult","effectId":"999","transactionId":a.transaction,"ok":true}),
    )
    .unwrap();
    h.submit(
        json!({"type":"callbackResult","effectId":a.effect,"transactionId":"tx999","ok":true}),
    )
    .unwrap();
    // So is an effect result for the callback: only `callbackResult` ends it.
    h.submit(json!({"type":"effectResult","effectId":a.effect,"outcome":{"ok":true,"value":null}}))
        .unwrap();
    assert_eq!(h.run(), Vec::<Value>::new());
    h.command("3", &a.transaction, None, read("e"));
    assert_eq!(h.run(), vec![done("3", row("hi"))]);
    h.callback(&a, false, Some("boom"));
    assert_eq!(h.run(), vec![failed("1", "boom")]);
    assert_eq!(h.committed(), None);
    h.command("4", &a.transaction, None, read("e"));
    assert_eq!(h.run(), vec![failed("4", "transaction_closed")]);
    // A second result for the finished transaction is ignored as well.
    h.callback(&a, true, None);
    assert_eq!(h.run(), Vec::<Value>::new());

    // A failure without a message still fails the parent.
    let b = h.begin("5");
    h.callback(&b, false, None);
    assert_eq!(h.run(), vec![failed("5", "transaction failed")]);

    // A result that arrives while submitted commands are still queued means
    // the application did not await them: nothing commits.
    let c = h.begin("6");
    h.command("7", &c.transaction, None, create("e", "unawaited"));
    h.callback(&c, true, None);
    // Commands after the result are closed at once.
    h.command("8", &c.transaction, None, read("e"));
    assert_eq!(h.events(), vec![failed("8", "transaction_closed")]);
    assert_eq!(
        h.run(),
        vec![
            failed("7", "transaction_closed"),
            failed("6", "unawaited transaction operation")
        ]
    );
    assert_eq!(h.committed(), None);
}

#[test]
fn close_during_a_callback_rolls_back_and_releases_every_waiter() {
    let mut h = harness();
    let a = h.begin("1");
    h.command("2", &a.transaction, None, create("e", "hi"));
    assert_eq!(h.run(), vec![done("2", Value::Null)]);
    h.task("3", read("e"));
    h.command("4", &a.transaction, None, read("e"));
    h.submit(json!({"type":"close"})).unwrap();
    // Close is admitted as control: nothing more is.
    assert_eq!(
        h.submit(json!({"type":"task","requestId":"5","command":read("e")})),
        Err(BridgeError::Closed)
    );
    assert_eq!(
        h.run(),
        vec![
            json!({"type":"cancelEffect","effectId":a.effect}),
            failed("1", "client_closed"),
            failed("4", "client_closed"),
            failed("3", "client_closed"),
            json!({"type":"runtimeClosed"}),
        ]
    );
    assert!(h.runtime.closed());
    assert!(!h.runtime.step(NOW, ENTROPY));
    assert_eq!(h.submit(json!({"type":"close"})), Err(BridgeError::Closed));
    assert_eq!(h.committed(), None);
    // The rollback released the writer: another client can write the file.
    let dir = h._dir.path().join("db");
    let mut other = Client::open(SqliteStore::open(&dir).unwrap(), schema()).unwrap();
    let _ = other.generation();
    assert!(other.read(&key()).unwrap().is_none());
}

/// A SQLite store whose next `commit` fails once when armed.
struct FailingCommit {
    inner: SqliteStore,
    armed: Arc<AtomicBool>,
}
impl ClientStore for FailingCommit {
    fn begin(&mut self) -> Result<()> {
        self.inner.begin()
    }
    fn commit(&mut self) -> Result<()> {
        if self.armed.swap(false, Ordering::SeqCst) {
            return Err(invalid("injected commit failure"));
        }
        self.inner.commit()
    }
    fn rollback(&mut self) -> Result<()> {
        self.inner.rollback()
    }
    fn savepoint(&mut self, name: &str) -> Result<()> {
        self.inner.savepoint(name)
    }
    fn release(&mut self, name: &str) -> Result<()> {
        self.inner.release(name)
    }
    fn rollback_to(&mut self, name: &str) -> Result<()> {
        self.inner.rollback_to(name)
    }
    fn execute(&mut self, sql: &str, parameters: &[Value]) -> Result<usize> {
        self.inner.execute(sql, parameters)
    }
    fn execute_batch(&mut self, sql: &str) -> Result<()> {
        self.inner.execute_batch(sql)
    }
    fn query(&mut self, sql: &str, parameters: &[Value]) -> Result<SqlRows> {
        self.inner.query(sql, parameters)
    }
    fn query_committed(&mut self, sql: &str, parameters: &[Value]) -> Result<SqlRows> {
        self.inner.query_committed(sql, parameters)
    }
}

#[test]
fn a_failed_commit_fails_the_task_and_lets_no_success_or_change_escape() {
    let dir = tempfile::tempdir().unwrap();
    let armed = Arc::new(AtomicBool::new(false));
    let store = FailingCommit {
        inner: SqliteStore::open(dir.path().join("db")).unwrap(),
        armed: armed.clone(),
    };
    let mut h = Harness {
        runtime: ClientRuntime::new(Client::open(store, schema()).unwrap()),
        _dir: dir,
    };
    let generation = h.runtime.client().generation();
    // The callback transaction.
    let a = h.begin("1");
    h.command("2", &a.transaction, None, create("e", "hi"));
    assert_eq!(h.run(), vec![done("2", Value::Null)]);
    armed.store(true, Ordering::SeqCst);
    h.callback(&a, true, None);
    assert_eq!(h.run(), vec![failed("1", "injected commit failure")]);
    assert_eq!(h.runtime.client().generation(), generation);
    assert_eq!(h.committed(), None);
    // An ordinary write task.
    armed.store(true, Ordering::SeqCst);
    h.task("3", create("e", "hi"));
    assert_eq!(h.run(), vec![failed("3", "injected commit failure")]);
    assert_eq!(h.committed(), None);
    // The writer is free again: a later transaction commits.
    let c = h.begin("6");
    h.command("7", &c.transaction, None, create("e", "later"));
    assert_eq!(h.run(), vec![done("7", Value::Null)]);
    h.callback(&c, true, None);
    let events = h.run();
    assert!(is_changed(&events[0]), "{events:?}");
    assert_eq!(events[1], done("6", Value::Null));
    assert_eq!(h.committed(), Some(row("later")));
}

#[test]
fn envelope_fixtures_decode_and_re_encode_unchanged() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/bridge/envelopes.json")).unwrap();
    let inputs = fixture["inputs"].as_array().unwrap();
    let events = fixture["events"].as_array().unwrap();
    assert!(inputs.len() >= 10 && events.len() >= 15);
    for wire in inputs {
        let typed: Input =
            serde_json::from_value(wire.clone()).unwrap_or_else(|e| panic!("{wire}: {e}"));
        assert_eq!(&serde_json::to_value(&typed).unwrap(), wire);
    }
    for wire in events {
        let typed: Event =
            serde_json::from_value(wire.clone()).unwrap_or_else(|e| panic!("{wire}: {e}"));
        assert_eq!(&serde_json::to_value(&typed).unwrap(), wire);
    }
}

/// The id and body of the one `http` effect among `events`.
fn http_effect(events: &[Value]) -> (String, String) {
    let effects: Vec<&Value> = events
        .iter()
        .filter(|e| e["type"] == "effect" && e["operation"]["kind"] == "http")
        .collect();
    assert_eq!(effects.len(), 1, "{events:?}");
    (
        effects[0]["effectId"].as_str().unwrap().to_string(),
        effects[0]["operation"]["body"]
            .as_str()
            .unwrap()
            .to_string(),
    )
}

#[test]
fn a_direct_apply_that_fails_to_commit_fails_the_call_and_lets_nothing_escape() {
    let dir = tempfile::tempdir().unwrap();
    let armed = Arc::new(AtomicBool::new(false));
    let store = FailingCommit {
        inner: SqliteStore::open(dir.path().join("db")).unwrap(),
        armed: armed.clone(),
    };
    let mut raw = serde_json::to_value(schema()).unwrap();
    raw["actions"] = json!([{"name":"Rename","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"update","cardinality":"single"}],"outputs":[]}]);
    let client = Client::open(store, Schema::from_value(raw).unwrap()).unwrap();
    let mut h = Harness {
        runtime: ClientRuntime::new(client),
        _dir: dir,
    };
    h.task("connect", json!({"kind":"connect"}));
    assert!(h.run().contains(&done("connect", Value::Null)));
    let rename = json!({"kind":"invoke","name":"Rename","version":1,"args":{"entry":{"id":"e","text":"server"}}});
    let answer = |body: &str| {
        let call: Value = serde_json::from_str(body).unwrap();
        json!({"ok":true,"value":{"status":200,"body":json!({"completion":{"callId":call["call"]["callId"],"outcome":{"status":"succeeded","result":null}},"records":[{"model":"Entry","identity":{"id":"e"},"stamp":1,"state":{"text":"server","note":null}}]}).to_string()}})
    };
    h.task("1", rename.clone());
    let (effect, body) = http_effect(&h.run());
    let generation = h.runtime.client().generation();
    armed.store(true, Ordering::SeqCst);
    h.submit(json!({"type":"effectResult","effectId":effect,"outcome":answer(&body)}))
        .unwrap();
    let events = h.run();
    assert!(
        events.contains(&failed("1", "action.execution_unknown")),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| e["type"] == "changed" || e["type"] == "callCompleted"),
        "{events:?}"
    );
    assert_eq!(h.runtime.client().generation(), generation);
    assert_eq!(h.committed(), None);
    // The writer is free: a later call applies and succeeds.
    h.task("2", rename);
    let (effect, body) = http_effect(&h.run());
    h.submit(json!({"type":"effectResult","effectId":effect,"outcome":answer(&body)}))
        .unwrap();
    let events = h.run();
    assert!(events.iter().any(|e| e["type"] == "changed"));
    assert!(events.contains(&done(
        "2",
        json!({"outcome":{"status":"succeeded","result":null}})
    )));
    assert_eq!(h.committed().unwrap()["text"], "server");
}

/// The protocol seams settle calls too: `drop` and `ack` announce every
/// completion as `callCompleted`, after the commit and before the task's own
/// answer, which keeps its value.
#[test]
fn drop_and_ack_announce_their_completions_as_call_completed() {
    let mut h = harness();
    let schema = {
        let mut schema = serde_json::to_value(common::schema()).unwrap();
        schema["actions"] = json!([{"name":"Ping","version":1,"inputs":[],"outputs":[]}]);
        schema
    };
    let dir = tempfile::tempdir().unwrap();
    h.runtime = ClientRuntime::new(
        Client::open(
            SqliteStore::open(dir.path().join("db")).unwrap(),
            Schema::from_value(schema).unwrap(),
        )
        .unwrap(),
    );
    let ping = json!({"kind":"submitAction","name":"Ping","version":1,"args":{}});
    h.task("dropped", ping.clone());
    let events = h.run();
    let dropped = events[events.len() - 1]["value"].clone();
    h.task("drop", json!({"kind":"drop","ordinal":dropped["ordinal"]}));
    let events = h.run();
    let announced = position(&events, |e| e["type"] == "callCompleted");
    assert_eq!(events[announced]["callId"], dropped["callId"]);
    let answered = position(&events, |e| e["requestId"] == "drop");
    assert!(announced < answered);
    assert_eq!(
        events[answered]["value"]["completions"][0]["callId"], dropped["callId"],
        "the value is unchanged"
    );

    h.task("sent", ping);
    let events = h.run();
    let sent = events[events.len() - 1]["value"].clone();
    h.task("freeze", json!({"kind":"freeze"}));
    let events = h.run();
    let push: Value =
        serde_json::from_str(events[events.len() - 1]["value"].as_str().unwrap()).unwrap();
    let client_id = h.runtime.client().client_id().to_string();
    let receipt = json!({"clientId":client_id,"batchSequence":push["batchSequence"],"rejections":[],
        "completions":[{"callId":sent["callId"],"outcome":{"status":"succeeded","result":null}}],"records":[]});
    h.task(
        "ack",
        json!({"kind":"ack","sequence":push["batchSequence"],"receipt":receipt}),
    );
    let events = h.run();
    let changed = position(&events, |e| e["type"] == "changed");
    let announced = position(&events, |e| e["type"] == "callCompleted");
    let answered = position(&events, |e| e["requestId"] == "ack");
    assert!(changed < announced && announced < answered, "{events:?}");
    assert_eq!(
        events[announced],
        json!({"type":"callCompleted","callId":sent["callId"],"outcome":{"status":"succeeded","result":null}})
    );
    assert_eq!(
        events[answered]["value"]["completions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
fn position(events: &[Value], matches: impl Fn(&Value) -> bool) -> usize {
    events
        .iter()
        .position(matches)
        .unwrap_or_else(|| panic!("not found in {events:?}"))
}
