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
pub fn seed(c: &mut Client<SqliteStore>, text: &str) {
    c.transaction(|tx| {
        tx.direct(Operation {
            model: "Entry".into(),
            op: OperationKind::Create,
            identity: json!({"id":"e"}),
            values: Some(json!({"text":text,"note":null})),
        })
    })
    .unwrap();
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
pub fn upgrade_key(model: &str, id: &str) -> v05::RecordKey {
    v05::RecordKey {
        model: model.into(),
        identity: json!({ "id": id }),
    }
}
pub fn upgrade_record(model: &str, id: &str, cursor: u64, state: Value) -> v05::AuthorityChange {
    v05::AuthorityChange::Record {
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
    changes: Vec<v05::AuthorityChange>,
) {
    let context = c.request_context05().unwrap();
    let plan = v05::freeze_delivery(
        context.clone(),
        format!("{bootstrap}-{after}-{through}"),
        if bootstrap {
            v05::DeliveryPurpose::Bootstrap
        } else {
            v05::DeliveryPurpose::Sync
        },
        after,
        through,
        through,
        10000,
        vec![v05::DeliveryUnit {
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
