use axton_client::*;
use axton_sqlite::SqliteStore;
use serde_json::{Value, json};
pub fn family_schema() -> Schema {
    Schema::from_value(json!({"enums":[],"models":[
 {"name":"Book","identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}},{"name":"title","nullable":false,"type":{"kind":"scalar","name":"string"}}]},
 {"name":"Comment","identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}},{"name":"bookId","nullable":false,"type":{"kind":"scalar","name":"string"}},{"name":"text","nullable":false,"type":{"kind":"scalar","name":"string"}}],"relations":[{"name":"book","target":"Book","fields":["bookId"],"targetFields":["id"],"onDelete":"delete"}],"unique":[["bookId","text"]]}
]})).unwrap()
}
pub fn create(model: &str, id: &str, values: Value) -> Operation {
    Operation {
        model: model.into(),
        op: OperationKind::Create,
        identity: json!({ "id": id }),
        values: Some(values),
    }
}
/// Entry `e` at `text`, unprotected and unpending.
pub fn seed(c: &mut Client<SqliteStore>, text: &str) {
    cached(c, "Entry", "e", json!({"text":text,"note":null}));
}
pub fn key() -> RecordKey {
    schema().record_key("Entry", &json!({"id":"e"})).unwrap()
}
pub fn update(text: &str) -> Operation {
    Operation {
        model: "Entry".into(),
        op: OperationKind::Update,
        identity: json!({"id":"e"}),
        values: Some(json!({ "text": text })),
    }
}
pub fn schema() -> Schema {
    let mut s: Value =
        serde_json::from_str(include_str!("../../../../fixtures/schemas/entry.json")).unwrap();
    s["actions"] = json!([{"name":"Edit","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"update","cardinality":"single","fields":["text"]}],"outputs":[]}]);
    Schema::from_value(s).unwrap()
}
pub fn open(path: &std::path::Path) -> Client<SqliteStore> {
    Client::open05(SqliteStore::open(path).unwrap(), schema(), "User:u").unwrap()
}

/// `Marked` is `@@bootstrap`, `Note` is not. `label` adds a nullable field to
/// both; `unique` adds a unique constraint to `Marked`, which leaves the
/// materialization unchanged.
pub fn upgrade_schema(label: bool, unique: bool) -> Schema {
    let text =
        |name: &str| json!({"name":name,"nullable":false,"type":{"kind":"scalar","name":"string"}});
    let mut fields = vec![text("id"), text("text")];
    if label {
        fields
            .push(json!({"name":"label","nullable":true,"type":{"kind":"scalar","name":"string"}}));
    }
    let mut marked = json!({"name":"Marked","identity":["id"],"bootstrap":true,"fields":fields});
    if unique {
        marked["unique"] = json!([["text"]]);
    }
    Schema::from_value(json!({"enums":[],"models":[
        marked,
        {"name":"Note","identity":["id"],"fields":fields}],
        "actions":[
            {"name":"Edit","version":1,"outputs":[],"inputs":[{"kind":"model","name":"note","model":"Note","operation":"update","cardinality":"single","fields":["text"]}]},
            {"name":"Find","version":1,"kind":"query","inputs":[],"outputs":[]}]}))
    .unwrap()
}
pub fn upgrade_key(model: &str, id: &str) -> wire::RecordKey {
    wire::RecordKey {
        model: model.into(),
        identity: json!({ "id": id }),
    }
}
pub fn upgrade_record(model: &str, id: &str, cursor: u64, state: Value) -> wire::AuthorityChange {
    wire::AuthorityChange::Record {
        key: upgrade_key(model, id),
        cursor,
        state,
    }
}
/// Apply one complete delivery unit over `(after, through]` in the Store's
/// current context.
pub fn deliver(
    c: &mut Client<SqliteStore>,
    bootstrap: bool,
    after: u64,
    through: u64,
    changes: Vec<wire::AuthorityChange>,
) {
    let context = c.request_context05().unwrap();
    let plan = wire::freeze_delivery(
        context.clone(),
        format!("{bootstrap}-{after}-{through}"),
        if bootstrap {
            wire::DeliveryPurpose::Bootstrap
        } else {
            wire::DeliveryPurpose::Sync
        },
        after,
        through,
        through,
        10000,
        vec![wire::DeliveryUnit {
            index: 0,
            through: Some(through),
            changes,
        }],
        100000,
    )
    .unwrap();
    let mut queue = sync05::DeliveryQueue::new(1 << 20, 4);
    queue
        .receive(&plan.header, &plan.parts, &context, 1)
        .unwrap();
    c.apply_next_delivery05(&mut queue, 1).unwrap().unwrap();
}
/// A Store before an upgrade: `S = B = 25`, `C = 40`, Bootstrap row `m`
/// at 10, Sync rows `n` at 30 and `gone` at 35.
pub fn before_upgrade(path: &std::path::Path) -> store05::StoreStatus05 {
    let mut c = Client::open05(
        SqliteStore::open(path).unwrap(),
        upgrade_schema(false, false),
        "User:u",
    )
    .unwrap();
    c.initialize_stream05(25, "1").unwrap();
    deliver(
        &mut c,
        true,
        0,
        25,
        vec![upgrade_record("Marked", "m", 10, json!({"text":"marked"}))],
    );
    deliver(
        &mut c,
        false,
        25,
        40,
        vec![
            upgrade_record("Note", "n", 30, json!({"text":"note"})),
            upgrade_record("Note", "gone", 35, json!({"text":"gone"})),
        ],
    );
    let status = c.store_status05().unwrap();
    assert_eq!(
        (status.start_cursor, status.bootstrap_cursor, status.cursor),
        (Some(25), Some(25), Some(40))
    );
    status
}

// Seed routes that survive the removal of direct Model writes. Pick the one
// whose evidence state the test needs.

/// Start the Stream at 0 unless the Store already has a start.
pub fn start_stream(c: &mut Client<SqliteStore>) {
    if c.store_status05().unwrap().start_cursor.is_none() {
        c.initialize_stream05(0, "1").unwrap();
    }
}
/// An authority change for `model` row `id` at `cursor`; a null `state`
/// is a delivered deletion.
pub fn record(model: &str, id: &str, cursor: u64, state: Value) -> wire::AuthorityChange {
    wire::AuthorityChange::Record {
        key: wire::RecordKey {
            model: model.into(),
            identity: json!({ "id": id }),
        },
        cursor,
        state,
    }
}
/// A protected row: the Stream delivers `changes` in the Store's context,
/// with no coverage. Starts the Stream first when it has no start.
pub fn protected(c: &mut Client<SqliteStore>, changes: &[wire::AuthorityChange]) -> ApplyReport {
    start_stream(c);
    let context = c.request_context05().unwrap();
    c.install_authority05(&context, changes, None).unwrap()
}
/// An unprotected, unpending row: a store-mode Query or Fetch read installs
/// `state` (every required field) as cache, as a direct write left it.
pub fn cached(c: &mut Client<SqliteStore>, model: &str, id: &str, state: Value) {
    let report = c
        .install_cache05(
            &[wire::ReadRecord {
                key: wire::RecordKey {
                    model: model.into(),
                    identity: json!({ "id": id }),
                },
                cursor: (),
                state,
            }],
            true,
        )
        .unwrap();
    assert_eq!(report.applied, 1, "cache install of {model} {id} applied");
}
/// An offline Mutation: queued and applied optimistically, never sent while
/// the test holds no connection.
pub fn offline(c: &mut Client<SqliteStore>, name: &str, input: Value) -> SubmittedCall {
    c.transaction(|tx| tx.submit_mutation05(name, 1, input))
        .unwrap()
}
/// `schema` with `Create<Model>`, `Update<Model>` and `Delete<Model>`
/// Mutations for every Model, each taking one record as `row`.
pub fn with_crud(schema: Schema) -> Schema {
    let mut value = serde_json::to_value(&schema).unwrap();
    let models: Vec<String> = value["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap().to_owned())
        .collect();
    let mut actions = value["actions"].as_array().cloned().unwrap_or_default();
    for model in models {
        for (verb, operation) in [
            ("Create", "create"),
            ("Update", "update"),
            ("Delete", "delete"),
        ] {
            actions.push(json!({"name":format!("{verb}{model}"),"version":1,"outputs":[],
                "inputs":[{"kind":"model","name":"row","model":model,"operation":operation,"cardinality":"single"}]}));
        }
    }
    value["actions"] = Value::Array(actions);
    Schema::from_value(value).unwrap()
}
fn row(id: &str, values: Value) -> Value {
    let mut row = values;
    row["id"] = json!(id);
    json!({ "row": row })
}
/// Inside a transaction, the `with_crud` Mutation creating `model` row `id`.
pub fn create_call<S: ClientStore>(
    tx: &mut ClientTransaction<'_, S>,
    model: &str,
    id: &str,
    values: Value,
) -> Result<SubmittedCall> {
    tx.submit_mutation05(&format!("Create{model}"), 1, row(id, values))
}
/// Inside a transaction, the `with_crud` Mutation updating `model` row `id`.
pub fn update_call<S: ClientStore>(
    tx: &mut ClientTransaction<'_, S>,
    model: &str,
    id: &str,
    values: Value,
) -> Result<SubmittedCall> {
    tx.submit_mutation05(&format!("Update{model}"), 1, row(id, values))
}
/// Inside a transaction, the `with_crud` Mutation deleting `model` row `id`.
pub fn delete_call<S: ClientStore>(
    tx: &mut ClientTransaction<'_, S>,
    model: &str,
    id: &str,
) -> Result<SubmittedCall> {
    tx.submit_mutation05(&format!("Delete{model}"), 1, row(id, json!({})))
}
