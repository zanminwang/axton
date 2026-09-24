use axton_server::{Config, Host, HostResult, process_action_push};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll, Waker},
};

fn run<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
    }
}

struct HostState(Mutex<Vec<Value>>);
impl Host for HostState {
    fn call(&self, request: Value) -> Pin<Box<dyn Future<Output = HostResult<Value>> + Send + '_>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(request.clone());
            Ok(match request["op"].as_str().unwrap() {
                "claim" => json!({"clientId":"device","owner":"alice","sequence":0,"receipt":null}),
                "claimCall" => json!({"fresh":true,"request":request["request"],"response":null}),
                "handleAction" => {
                    let outputs = match request["name"].as_str().unwrap() {
                        "Void" | "Broken" => json!({}),
                        "Values" => json!({"maybe":null,"items":["a","b"]}),
                        _ => json!({"message":"ok"}),
                    };
                    json!({"outputs":outputs,"changes":[],"publications":[]})
                }
                _ => Value::Null,
            })
        })
    }
}

#[test]
fn ordinary_action_returns_its_handler_result_and_saves_it() {
    let config = Config::decode(json!({"schema":{"enums":[],"models":[],"actions":[{"name":"Send","version":1,"inputs":[],"outputs":[{"name":"message","kind":"value","type":{"kind":"scalar","name":"string"},"cardinality":"single","source":"handlerValue"}]}]},"mutations":[],"loaders":[]})).unwrap();
    let host = HostState(Mutex::new(vec![]));
    let request = json!({"clientId":"device","batchSequence":1,"models":{},"mutations":[{"ordinal":1,"callId":"01890f47-1234-7123-8123-123456789abc","name":"Send","version":1,"args":{}}]});
    let receipt = run(process_action_push(
        &config,
        "alice",
        request.to_string().as_bytes(),
        &host,
    ))
    .unwrap();
    let receipt: Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(
        receipt["completions"][0]["outcome"]["result"],
        json!({"message":"ok"})
    );
    assert!(
        host.0
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["op"] == "saveCall")
    );
}

#[test]
fn ordinary_list_nullable_void_and_missing_explicit_outputs_are_independent() {
    let scalar = json!({"kind":"scalar","name":"string"});
    let config = Config::decode(json!({"schema":{"enums":[],"models":[],"actions":[
        {"name":"Values","version":1,"inputs":[],"outputs":[{"name":"maybe","kind":"value","type":scalar,"cardinality":"optional","source":"handlerValue"},{"name":"items","kind":"value","type":scalar,"cardinality":"list","source":"handlerValue"}]},
        {"name":"Broken","version":1,"inputs":[],"outputs":[{"name":"message","kind":"value","type":scalar,"cardinality":"single","source":"handlerValue"}]},
        {"name":"Void","version":1,"inputs":[],"outputs":[]}
    ]},"mutations":[],"loaders":[]})).unwrap();
    let host = HostState(Mutex::new(vec![]));
    let calls = ["Values","Broken","Void"].into_iter().enumerate().map(|(index,name)| json!({"ordinal":index+1,"callId":format!("01890f47-1234-7123-8123-123456789ab{}", index+1),"name":name,"version":1,"args":{}})).collect::<Vec<_>>();
    let request = json!({"clientId":"device","batchSequence":1,"models":{},"mutations":calls});
    let receipt: Value = serde_json::from_str(
        &run(process_action_push(
            &config,
            "alice",
            request.to_string().as_bytes(),
            &host,
        ))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        receipt["completions"][0]["outcome"]["result"],
        json!({"maybe":null,"items":["a","b"]})
    );
    assert_eq!(
        receipt["rejections"],
        json!([{"ordinal":2,"code":"handler.invalid"}])
    );
    assert_eq!(receipt["completions"][2]["outcome"]["result"], Value::Null);
    let ops = host.0.lock().unwrap();
    assert_eq!(ops.iter().filter(|op| op["op"] == "rollback").count(), 1);
    assert_eq!(ops.iter().filter(|op| op["op"] == "saveCall").count(), 3);
}

struct ModelHost(Mutex<Vec<Value>>);
impl Host for ModelHost {
    fn call(&self, request: Value) -> Pin<Box<dyn Future<Output = HostResult<Value>> + Send + '_>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(request.clone());
            Ok(match request["op"].as_str().unwrap() {
                "claim" => {
                    json!({"clientId":"device-1","owner":"alice","sequence":3,"receipt":null})
                }
                "claimCall" => json!({"fresh":true,"request":request["request"],"response":null}),
                "handleAction" => {
                    json!({"outputs":{"todo":{"id":"01890f47-1234-7123-8123-123456789abc"}},"changes":[],"publications":[]})
                }
                "ensureStamp" => json!(1),
                "load" if request["version"] == 1 => {
                    json!([{"id":"01890f47-1234-7123-8123-123456789abc","title":"A"}])
                }
                "load" => {
                    json!([{"id":"01890f47-1234-7123-8123-123456789abc","title":"A","done":false}])
                }
                _ => Value::Null,
            })
        })
    }
}

#[test]
fn explicit_model_output_uses_old_result_loader_and_current_authority_loader() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../fixtures/protocol/action-results.json"
    ))
    .unwrap();
    let config = Config::decode(json!({"schema":fixture["schema"],"mutations":[],"loaders":["Todo"],"models":[
        {"name":"Todo","version":1,"identity":["id"],"fields":fixture["schema"]["resultModels"][0]["fields"],"enums":[]},
        {"name":"Todo","version":2,"identity":["id"],"fields":fixture["schema"]["models"][0]["fields"],"enums":[]}
    ]})).unwrap();
    let host = ModelHost(Mutex::new(vec![]));
    let mut request = fixture["request"].clone();
    request["mutations"].as_array_mut().unwrap().truncate(1);
    let receipt = run(process_action_push(
        &config,
        "alice",
        request.to_string().as_bytes(),
        &host,
    ))
    .unwrap();
    let receipt: Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(
        receipt["completions"][0]["outcome"]["result"]["todo"],
        json!({"id":"01890f47-1234-7123-8123-123456789abc","title":"A"})
    );
    assert_eq!(
        receipt["records"][0]["state"],
        json!({"title":"A","done":false})
    );
    let ops = host.0.lock().unwrap();
    let versions: Vec<u64> = ops
        .iter()
        .filter(|op| op["op"] == "load")
        .map(|op| op["version"].as_u64().unwrap())
        .collect();
    assert_eq!(versions, vec![1, 2]);
    assert!(
        ops.iter().position(|op| op["op"] == "ensureStamp")
            < ops.iter().position(|op| op["op"] == "load")
    );
}

#[derive(Default)]
struct Stateful {
    row: Option<String>,
    stamp: u64,
    savepoint: Option<(Option<String>, u64)>,
    calls: BTreeMap<String, (String, Option<String>)>,
    receipts: BTreeMap<String, (u64, String)>,
    handlers: usize,
    loaders: usize,
    delete_on_handle: bool,
    log: Vec<Value>,
}
struct StatefulHost(Mutex<Stateful>);
impl StatefulHost {
    fn new() -> Self {
        Self(Mutex::new(Stateful::default()))
    }
}
impl Host for StatefulHost {
    fn call(&self, request: Value) -> Pin<Box<dyn Future<Output = HostResult<Value>> + Send + '_>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            state.log.push(request.clone());
            let answer = match request["op"].as_str().unwrap() {
                "claim" => {
                    let (sequence, receipt) = state
                        .receipts
                        .get(request["clientId"].as_str().unwrap())
                        .cloned()
                        .unwrap_or((0, String::new()));
                    json!({"clientId":request["clientId"],"owner":"alice","sequence":sequence,"receipt":if receipt.is_empty() {Value::Null} else {json!(receipt)}})
                }
                "saveReceipt" => {
                    state.receipts.insert(
                        request["clientId"].as_str().unwrap().into(),
                        (
                            request["sequence"].as_u64().unwrap(),
                            request["receipt"].as_str().unwrap().into(),
                        ),
                    );
                    Value::Null
                }
                "claimCall" => {
                    let id = request["callId"].as_str().unwrap();
                    if let Some((stored, response)) = state.calls.get(id) {
                        json!({"fresh":false,"request":stored,"response":response})
                    } else {
                        let incoming = request["request"].as_str().unwrap().to_string();
                        state.calls.insert(id.into(), (incoming.clone(), None));
                        json!({"fresh":true,"request":incoming,"response":null})
                    }
                }
                "saveCall" => {
                    let id = request["callId"].as_str().unwrap();
                    state.calls.get_mut(id).unwrap().1 =
                        Some(request["response"].as_str().unwrap().into());
                    Value::Null
                }
                "savepoint" => {
                    state.savepoint = Some((state.row.clone(), state.stamp));
                    Value::Null
                }
                "rollback" => {
                    let (row, stamp) = state.savepoint.clone().unwrap();
                    state.row = row;
                    state.stamp = stamp;
                    Value::Null
                }
                "release" => {
                    state.savepoint = None;
                    Value::Null
                }
                "handleAction" => {
                    state.handlers += 1;
                    if request["name"] == "Read" {
                        return Ok(
                            json!({"outputs":{"todo":{"id":"t"}},"changes":[],"publications":[]}),
                        );
                    }
                    if request["name"] == "Delete" {
                        if state.delete_on_handle {
                            state.row = None;
                        }
                        return Ok(json!({"outputs":{},"changes":[],"publications":[]}));
                    }
                    if request["arguments"]["todo"].is_null() {
                        return Ok(json!({"outputs":{},"changes":[],"publications":[]}));
                    }
                    let title = request["arguments"]["todo"]["title"].as_str().unwrap();
                    match title {
                        "refuse" => json!({"rejection":"todo.refused"}),
                        "crash" => json!({"error":"application fault"}),
                        "missing" => json!({"outputs":{},"changes":[],"publications":[]}),
                        _ => {
                            state.row = Some(title.into());
                            json!({"outputs":{},"changes":[],"publications":[]})
                        }
                    }
                }
                "advanceStamp" => {
                    state.stamp += 1;
                    json!(state.stamp)
                }
                "ensureStamp" => {
                    if state.stamp == 0 {
                        state.stamp = 1;
                    }
                    json!(state.stamp)
                }
                "load" => {
                    state.loaders += 1;
                    match &state.row {
                        Some(title) => json!([{"id":"t","title":title}]),
                        None => json!([null]),
                    }
                }
                _ => return Err(format!("unexpected host operation {}", request["op"])),
            };
            Ok(answer)
        })
    }
}

fn stateful_config() -> Config {
    stateful_config_with_note(false)
}
fn stateful_config_with_note(note: bool) -> Config {
    let mut fields = json!([{"name":"id","type":{"kind":"scalar","name":"string"},"nullable":false},{"name":"title","type":{"kind":"scalar","name":"string"},"nullable":false}]);
    if note {
        fields
            .as_array_mut()
            .unwrap()
            .push(json!({"name":"note","type":{"kind":"scalar","name":"string"},"nullable":true}));
    }
    Config::decode(json!({"schema":{"enums":[],"models":[{"name":"Todo","version":1,"identity":["id"],"fields":fields}],"resultModels":[{"name":"Todo","version":1,"identity":["id"],"fields":fields,"enums":[]}],"actions":[{"name":"Edit","version":1,"inputs":[{"kind":"model","name":"todo","model":"Todo","operation":"update","cardinality":"single","allowedPatchFields":["title"]}],"outputs":[{"name":"todo","kind":"model","modelReadVersion":1,"model":"Todo","cardinality":"single","source":{"inputIdentity":"todo"}}]},{"name":"Read","version":1,"inputs":[],"outputs":[{"name":"todo","kind":"model","modelReadVersion":1,"model":"Todo","cardinality":"single","source":"handlerIdentity","handlerType":{"kind":"identity","model":"Todo","fields":[{"name":"id","type":{"kind":"scalar","name":"string"}}]}}]}]},"mutations":[],"loaders":["Todo"]})).unwrap()
}

#[test]
fn supplied_optional_implicit_model_requires_a_row_but_explicit_nullable_identity_may_be_missing() {
    let mut raw = serde_json::to_value(stateful_config()).unwrap();
    raw["schema"]["actions"][0]["inputs"][0]["cardinality"] = json!("optional");
    raw["schema"]["actions"][0]["outputs"][0]["cardinality"] = json!("optional");
    raw["schema"]["actions"][1]["outputs"][0]["cardinality"] = json!("optional");
    let config = Config::decode(raw).unwrap();
    let host = StatefulHost::new();
    let present = stateful_push(
        &config,
        &host,
        1,
        vec![call(1, "01890f47-1234-7123-8123-123456789abc", "missing")],
    );
    assert_eq!(
        present["rejections"],
        json!([{"ordinal":1,"code":"loader.invalid"}])
    );
    let absent = json!({"ordinal":1,"callId":"01890f47-1234-7123-8123-123456789abd","name":"Edit","version":1,"args":{}});
    let selected = json!({"ordinal":2,"callId":"01890f47-1234-7123-8123-123456789abe","name":"Read","version":1,"args":{}});
    let next = stateful_push(&config, &host, 2, vec![absent, selected]);
    assert_eq!(
        next["completions"][0]["outcome"]["result"],
        json!({"todo":null})
    );
    assert_eq!(
        next["completions"][1]["outcome"]["result"],
        json!({"todo":null})
    );
    assert_eq!(next["records"][0]["state"], Value::Null);
}

#[test]
fn delete_confirmation_requires_loader_absence_and_rolls_back_a_forgotten_delete() {
    let mut raw = serde_json::to_value(stateful_config()).unwrap();
    raw["schema"]["actions"][0]["name"] = json!("Delete");
    raw["schema"]["actions"][0]["inputs"][0]["operation"] = json!("delete");
    raw["schema"]["actions"][0]["inputs"][0]
        .as_object_mut()
        .unwrap()
        .remove("allowedPatchFields");
    raw["schema"]["actions"][0]["outputs"][0]["kind"] = json!("deleteIdentity");
    raw["schema"]["actions"][0]["outputs"][0]
        .as_object_mut()
        .unwrap()
        .remove("modelReadVersion");
    let config = Config::decode(raw).unwrap();
    let host = StatefulHost::new();
    host.0.lock().unwrap().row = Some("A".into());
    let invoke = |ordinal, id| json!({"ordinal":ordinal,"callId":id,"name":"Delete","version":1,"args":{"todo":{"id":"t"}}});
    let failed = stateful_push(
        &config,
        &host,
        1,
        vec![invoke(1, "01890f47-1234-7123-8123-123456789abc")],
    );
    assert_eq!(
        failed["rejections"],
        json!([{"ordinal":1,"code":"loader.invalid"}])
    );
    assert_eq!(host.0.lock().unwrap().row.as_deref(), Some("A"));
    host.0.lock().unwrap().delete_on_handle = true;
    let succeeded = stateful_push(
        &config,
        &host,
        2,
        vec![invoke(1, "01890f47-1234-7123-8123-123456789abd")],
    );
    assert_eq!(
        succeeded["completions"][0]["outcome"]["result"],
        json!({"todo":{"id":"t"}})
    );
    assert_eq!(succeeded["records"][0]["state"], Value::Null);
}

#[test]
fn mismatched_model_binding_rejects_only_its_action_before_handler() {
    let mut raw = serde_json::to_value(stateful_config()).unwrap();
    raw["schema"]["models"].as_array_mut().unwrap().extend([
        json!({"name":"Parent","version":1,"identity":["id"],"fields":[{"name":"id","type":{"kind":"scalar","name":"string"},"nullable":false}]}),
        json!({"name":"Child","version":1,"identity":["id"],"fields":[{"name":"id","type":{"kind":"scalar","name":"string"},"nullable":false},{"name":"parentId","type":{"kind":"scalar","name":"string"},"nullable":false}],"relations":[{"name":"parent","target":"Parent","fields":["parentId"],"targetFields":["id"],"onDelete":"none"}]}),
    ]);
    raw["schema"]["actions"].as_array_mut().unwrap().push(json!({"name":"AddPair","version":1,"inputs":[
        {"kind":"model","name":"parent","model":"Parent","operation":"create","cardinality":"single"},
        {"kind":"model","name":"child","model":"Child","operation":"create","cardinality":"single","bindings":[{"slot":"parent","fields":["parentId"]}]}
    ],"outputs":[]}));
    raw["loaders"] = json!(["Todo", "Parent", "Child"]);
    raw["models"] = json!([]);
    let config = Config::decode(raw).unwrap();
    let host = StatefulHost::new();
    let invalid = json!({"ordinal":1,"callId":"01890f47-1234-7123-8123-123456789abc","name":"AddPair","version":1,"args":{"parent":{"id":"p"},"child":{"id":"c","parentId":"other"}}});
    let receipt = stateful_push(
        &config,
        &host,
        1,
        vec![
            invalid,
            call(2, "01890f47-1234-7123-8123-123456789abd", "B"),
        ],
    );
    assert_eq!(
        receipt["rejections"],
        json!([{"ordinal":1,"code":"action.invalid"}])
    );
    assert_eq!(
        receipt["completions"][1]["outcome"]["result"]["todo"]["title"],
        "B"
    );
    assert_eq!(host.0.lock().unwrap().handlers, 1);
}

#[test]
fn compatible_added_nullable_field_does_not_conflict_with_old_saved_stamp() {
    let host = StatefulHost::new();
    let old = "01890f47-1234-7123-8123-123456789abc";
    let reader = "01890f47-1234-7123-8123-123456789abd";
    stateful_push(&stateful_config(), &host, 1, vec![call(1, old, "A")]);
    let current = stateful_config_with_note(true);
    let read = json!({"ordinal":1,"callId":reader,"name":"Read","version":1,"args":{}});
    let receipt = stateful_push(&current, &host, 2, vec![read, call(2, old, "A")]);
    assert_eq!(
        receipt["completions"][1]["outcome"]["result"]["todo"],
        json!({"id":"t","title":"A"})
    );
    assert_eq!(
        receipt["records"][0]["state"],
        json!({"title":"A","note":null})
    );
    assert_eq!(receipt["records"][0]["stamp"], 1);
}

#[test]
fn unequal_content_at_equal_stamp_is_a_storage_fault() {
    let config = stateful_config();
    let host = StatefulHost::new();
    let old = "01890f47-1234-7123-8123-123456789abc";
    let reader = "01890f47-1234-7123-8123-123456789abd";
    stateful_push(&config, &host, 1, vec![call(1, old, "A")]);
    {
        let mut state = host.0.lock().unwrap();
        let response = &mut state.calls.get_mut(old).unwrap().1;
        let mut saved: Value = serde_json::from_str(response.as_ref().unwrap()).unwrap();
        saved["records"][0]["state"]["title"] = json!("OTHER");
        *response = Some(saved.to_string());
    }
    let read = json!({"ordinal":1,"callId":reader,"name":"Read","version":1,"args":{}});
    let body = json!({"clientId":"device","batchSequence":2,"models":{"Todo":1},"mutations":[read,call(2,old,"A")]});
    let error = run(process_action_push(
        &config,
        "alice",
        body.to_string().as_bytes(),
        &host,
    ))
    .unwrap_err();
    assert_eq!(error.code, "storage.invalid");
    assert!(
        host.0
            .lock()
            .unwrap()
            .receipts
            .get("device")
            .is_some_and(|(sequence, _)| *sequence == 1)
    );
}
fn call(ordinal: u64, id: &str, title: &str) -> Value {
    json!({"ordinal":ordinal,"callId":id,"name":"Edit","version":1,"args":{"todo":{"id":"t","title":title}}})
}
fn stateful_push(config: &Config, host: &StatefulHost, sequence: u64, calls: Vec<Value>) -> Value {
    let body =
        json!({"clientId":"device","batchSequence":sequence,"models":{"Todo":1},"mutations":calls});
    serde_json::from_str(
        &run(process_action_push(
            config,
            "alice",
            body.to_string().as_bytes(),
            host,
        ))
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn saved_old_result_after_fresh_new_result_keeps_newest_authority() {
    let config = stateful_config();
    let host = StatefulHost::new();
    let old = "01890f47-1234-7123-8123-123456789abc";
    let new = "01890f47-1234-7123-8123-123456789abd";
    let first = stateful_push(&config, &host, 1, vec![call(1, old, "A")]);
    let second = stateful_push(
        &config,
        &host,
        2,
        vec![call(1, new, "B"), call(2, old, "A")],
    );
    assert_eq!(
        first["completions"][0]["outcome"]["result"]["todo"]["title"],
        "A"
    );
    assert_eq!(
        second["completions"][0]["outcome"]["result"]["todo"]["title"],
        "B"
    );
    assert_eq!(
        second["completions"][1]["outcome"]["result"]["todo"]["title"],
        "A"
    );
    assert_eq!(second["records"][0]["stamp"], 2);
    assert_eq!(second["records"][0]["state"]["title"], "B");
    let state = host.0.lock().unwrap();
    assert_eq!((state.handlers, state.loaders), (2, 2));
}

#[test]
fn invalid_and_refused_calls_do_not_replace_a_successful_call_or_original_claim() {
    let config = stateful_config();
    let host = StatefulHost::new();
    let old = "01890f47-1234-7123-8123-123456789abc";
    let fresh = "01890f47-1234-7123-8123-123456789abd";
    let invalid = "01890f47-1234-7123-8123-123456789abe";
    let refused = "01890f47-1234-7123-8123-123456789abf";
    stateful_push(&config, &host, 1, vec![call(1, old, "A")]);
    let mut unsupported = call(2, invalid, "Z");
    unsupported["version"] = json!(2);
    let receipt = stateful_push(
        &config,
        &host,
        2,
        vec![
            call(1, old, "DIFFERENT"),
            unsupported,
            call(3, refused, "refuse"),
            call(4, fresh, "B"),
        ],
    );
    assert_eq!(
        receipt["rejections"],
        json!([{"ordinal":1,"code":"call.identity_conflict"},{"ordinal":2,"code":"action_version_unsupported"},{"ordinal":3,"code":"todo.refused"}])
    );
    assert_eq!(
        receipt["completions"][3]["outcome"]["result"]["todo"]["title"],
        "B"
    );
    let state = host.0.lock().unwrap();
    assert_eq!(state.row.as_deref(), Some("B"));
    assert_eq!(state.handlers, 3);
    assert!(
        state
            .calls
            .get(old)
            .unwrap()
            .1
            .as_ref()
            .unwrap()
            .contains("\"A\"")
    );
}
