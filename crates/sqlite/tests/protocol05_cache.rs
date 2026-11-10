use axton_client::{Client, Operation, OperationKind, RecordKey, Schema, v05};
use axton_sqlite::SqliteStore;
use serde_json::json;
fn schema(on_delete: &str) -> Schema {
    Schema::from_value(json!({"enums":[],"models":[
 {"name":"Parent","identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}}]},
 {"name":"Child","identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}},{"name":"parentId","nullable":true,"type":{"kind":"scalar","name":"string"}}],"relations":[{"name":"parent","target":"Parent","fields":["parentId"],"targetFields":["id"],"onDelete":on_delete}]}
 ]})).unwrap()
}
fn key(model: &str, id: &str) -> RecordKey {
    RecordKey {
        model: model.into(),
        identity: json!({"id":id}),
    }
}
fn parent(c: &mut Client<SqliteStore>, cursor: u64, deleted: bool) {
    let context = c.request_context05().unwrap();
    c.install_authority05(
        &context,
        &[v05::AuthorityChange::Record {
            key: v05::RecordKey {
                model: "Parent".into(),
                identity: json!({"id":"p"}),
            },
            cursor,
            state: if deleted {
                serde_json::Value::Null
            } else {
                json!({})
            },
        }],
        None,
    )
    .unwrap();
}
fn child(parent_id: serde_json::Value) -> v05::ReadRecord {
    v05::ReadRecord {
        key: v05::RecordKey {
            model: "Child".into(),
            identity: json!({"id":"c"}),
        },
        cursor: (),
        state: json!({"parentId":parent_id}),
    }
}
#[test]
fn cascading_reference_cache_guard_uses_only_current_parent_tombstone() {
    let d = tempfile::tempdir().unwrap();
    let mut c = Client::open05(
        SqliteStore::open(d.path().join("db")).unwrap(),
        schema("delete"),
        "User:u",
    )
    .unwrap();
    let record = child(json!("p"));
    assert_eq!(
        c.install_cache05(std::slice::from_ref(&record), true)
            .unwrap()
            .applied,
        1
    ); // unloaded parent
    parent(&mut c, 57, false);
    c.transaction(|tx| {
        tx.direct(Operation {
            model: "Parent".into(),
            op: OperationKind::Delete,
            identity: json!({"id":"p"}),
            values: None,
        })
    })
    .unwrap();
    assert_eq!(
        c.install_cache05(std::slice::from_ref(&record), true)
            .unwrap()
            .applied,
        1
    ); // local absence has no authority
    parent(&mut c, 58, true);
    assert!(c.read(&key("Child", "c")).unwrap().is_none());
    assert_eq!(
        c.install_cache05(std::slice::from_ref(&record), true)
            .unwrap()
            .applied,
        0
    );
    assert_eq!(
        c.install_cache05(std::slice::from_ref(&record), false)
            .unwrap()
            .applied,
        0
    );
    assert_eq!(
        c.install_cache05(&[child(serde_json::Value::Null)], true)
            .unwrap()
            .applied,
        1
    ); // no relation target
    parent(&mut c, 59, false);
    assert_eq!(c.install_cache05(&[record], true).unwrap().applied, 1);
    assert!(
        c.record_evidence05(&key("Child", "c"))
            .unwrap()
            .history
            .is_empty()
    );
}
#[test]
fn noncascading_reference_does_not_borrow_parent_tombstone() {
    let d = tempfile::tempdir().unwrap();
    let mut c = Client::open05(
        SqliteStore::open(d.path().join("db")).unwrap(),
        schema("none"),
        "User:u",
    )
    .unwrap();
    parent(&mut c, 58, true);
    assert_eq!(
        c.install_cache05(&[child(json!("p"))], true)
            .unwrap()
            .applied,
        1
    );
    assert!(
        c.record_evidence05(&key("Child", "c"))
            .unwrap()
            .history
            .is_empty()
    );
}
fn install(
    c: &mut Client<SqliteStore>,
    model: &str,
    id: &str,
    cursor: u64,
    state: serde_json::Value,
) {
    let context = c.request_context05().unwrap();
    c.install_authority05(
        &context,
        &[v05::AuthorityChange::Record {
            key: v05::RecordKey {
                model: model.into(),
                identity: json!({ "id": id }),
            },
            cursor,
            state,
        }],
        None,
    )
    .unwrap();
}
fn cached(c: &mut Client<SqliteStore>, model: &str, id: &str, state: serde_json::Value) -> usize {
    c.install_cache05(
        &[v05::ReadRecord {
            key: v05::RecordKey {
                model: model.into(),
                identity: json!({ "id": id }),
            },
            cursor: (),
            state,
        }],
        true,
    )
    .unwrap()
    .applied
}
#[test]
fn protection_holds_only_in_the_materialization_that_installed_it() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema("delete"), "User:u").unwrap();
    install(&mut c, "Parent", "live", 57, json!({}));
    install(&mut c, "Parent", "gone", 58, serde_json::Value::Null);
    let protected = |c: &mut Client<SqliteStore>| {
        [
            cached(c, "Parent", "live", json!({})),
            cached(c, "Parent", "gone", json!({})),
            cached(c, "Child", "c", json!({"parentId":"gone"})),
        ]
    };
    assert_eq!(protected(&mut c), [0, 0, 0]);
    let db = rusqlite::Connection::open(&p).unwrap();
    db.execute("UPDATE axton_store SET materialization='m2'", [])
        .unwrap();
    assert_eq!(protected(&mut c), [1, 1, 1]);
    assert!(c.read(&key("Parent", "gone")).unwrap().is_some());
    assert!(c.read(&key("Child", "c")).unwrap().is_some());
}
