pub mod common05;
use axton_client::ddl::reconcile;
use axton_client::{Client, ClientStore, RecordKey, wire};
use axton_core::Schema;
use axton_sqlite::SqliteStore;
use serde_json::{Value, json};

fn schema(fields: Value) -> Schema {
    Schema::from_value(json!({"enums":[],"models":[{"name":"Task","identity":["id"],"fields":fields,"unique":[["title"]]}]})).unwrap()
}
fn base() -> Value {
    json!([{"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}},
           {"name":"title","nullable":false,"type":{"kind":"scalar","name":"string"}},
           {"name":"done","nullable":false,"type":{"kind":"scalar","name":"boolean"}}])
}
fn columns(s: &mut SqliteStore, table: &str) -> Vec<(String, String, i64)> {
    s.query_committed(&format!("PRAGMA table_info(\"{table}\")"), &[])
        .unwrap()
        .rows
        .into_iter()
        .map(|r| {
            (
                r[1].as_str().unwrap().into(),
                r[2].as_str().unwrap().into(),
                r[5].as_i64().unwrap(),
            )
        })
        .collect()
}
fn open(
    dir: &tempfile::TempDir,
    stored: Option<&Schema>,
    schema: &Schema,
) -> axton_core::Result<SqliteStore> {
    let mut s = SqliteStore::open(dir.path().join("db")).unwrap();
    s.begin().unwrap();
    match reconcile(&mut s, stored, schema) {
        Ok(_) => {
            s.commit().unwrap();
            Ok(s)
        }
        Err(e) => {
            s.rollback().unwrap();
            Err(e)
        }
    }
}

#[test]
fn creates_model_before_and_framework_tables() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = open(&dir, None, &schema(base())).unwrap();
    assert_eq!(
        columns(&mut s, "Task"),
        vec![
            ("id".into(), "TEXT".into(), 1),
            ("title".into(), "TEXT".into(), 0),
            ("done".into(), "INTEGER".into(), 0)
        ]
    );
    assert_eq!(
        columns(&mut s, "axton_before_Task"),
        columns(&mut s, "Task")
    );
    for table in ["Task", "axton_before_Task"] {
        assert!(!columns(&mut s, table).is_empty(), "{table}");
    }
    let indexes = s.query_committed("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='Task' AND name='Task_title_unique'", &[]).unwrap();
    assert_eq!(indexes.rows.len(), 1);
    assert!(
        s.execute("INSERT INTO \"Task\" VALUES ('a','same',0)", &[])
            .is_ok()
    );
    assert!(
        s.execute("INSERT INTO \"Task\" VALUES ('b','same',0)", &[])
            .is_err()
    );
}

#[test]
fn adds_missing_columns_to_both_tables_and_keeps_unknown_ones() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = open(&dir, None, &schema(base())).unwrap();
    s.execute("INSERT INTO \"Task\" VALUES ('a','t',1)", &[])
        .unwrap();
    s.execute_batch("ALTER TABLE \"Task\" ADD COLUMN legacy TEXT")
        .unwrap();
    drop(s);
    let mut fields = base().as_array().unwrap().clone();
    fields.push(json!({"name":"note","nullable":true,"type":{"kind":"scalar","name":"string"}}));
    fields.push(
        json!({"name":"rank","nullable":false,"type":{"kind":"scalar","name":"int"},"default":3}),
    );
    let mut s = open(&dir, Some(&schema(base())), &schema(Value::Array(fields))).unwrap();
    let names: Vec<String> = columns(&mut s, "Task").into_iter().map(|c| c.0).collect();
    assert_eq!(names, vec!["id", "title", "done", "legacy", "note", "rank"]);
    let before: Vec<String> = columns(&mut s, "axton_before_Task")
        .into_iter()
        .map(|c| c.0)
        .collect();
    assert_eq!(before, vec!["id", "title", "done", "note", "rank"]);
    assert_eq!(
        s.query_committed("SELECT rank, note FROM \"Task\"", &[])
            .unwrap()
            .rows,
        vec![vec![json!(3), Value::Null]]
    );
}

#[test]
fn a_table_whose_shape_differs_from_the_schema_is_rebuilt_empty() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = open(&dir, None, &schema(base())).unwrap();
    s.execute("INSERT INTO \"Task\" VALUES ('a','t',1)", &[])
        .unwrap();
    drop(s);
    // No stored schema: the table itself shows the changed type.
    let retyped = json!([{"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}},
                         {"name":"title","nullable":false,"type":{"kind":"scalar","name":"string"}},
                         {"name":"done","nullable":false,"type":{"kind":"scalar","name":"string"}}]);
    let mut s = SqliteStore::open(dir.path().join("db")).unwrap();
    s.begin().unwrap();
    let reset = reconcile(&mut s, None, &schema(retyped)).unwrap();
    s.commit().unwrap();
    assert_eq!(reset, vec!["Task".to_string()]);
    assert_eq!(columns(&mut s, "Task")[2].1, "TEXT");
    assert!(
        s.query_committed("SELECT * FROM \"Task\"", &[])
            .unwrap()
            .rows
            .is_empty()
    );
}

/// Why Model `Task` needs a reset; `Other` never changes.
#[derive(Clone, Copy, Debug)]
enum Reset {
    RemovedField,
    ChangedType,
    ChangedNullability,
    ChangedIdentity,
    ChangedUnique,
    ChangedRelations,
    ChangedEnumValues,
    RequiredWithoutDefault,
    RemovedModel,
}
const RESETS: [Reset; 9] = [
    Reset::RemovedField,
    Reset::ChangedType,
    Reset::ChangedNullability,
    Reset::ChangedIdentity,
    Reset::ChangedUnique,
    Reset::ChangedRelations,
    Reset::ChangedEnumValues,
    Reset::RequiredWithoutDefault,
    Reset::RemovedModel,
];
fn field(name: &str, kind: &str, nullable: bool) -> Value {
    json!({"name":name,"nullable":nullable,"type":{"kind":"scalar","name":kind}})
}
fn pair(reset: Option<Reset>) -> Schema {
    let mut task = json!({"name":"Task","identity":["id"],
        "fields":[field("id","string",false),field("title","string",false),
                  {"name":"mood","nullable":true,"type":{"kind":"enum","name":"Mood"}},
                  field("otherId","string",true)],
        "unique":[["title"]],
        "relations":[{"name":"other","target":"Other","fields":["otherId"],"targetFields":["id"],"onDelete":"none"}]});
    let mut mood = json!(["calm", "busy"]);
    match reset {
        None => {}
        Some(Reset::RemovedField) => {
            task["fields"].as_array_mut().unwrap().remove(2);
        }
        Some(Reset::ChangedType) => task["fields"][1] = field("title", "int", false),
        Some(Reset::ChangedNullability) => task["fields"][1] = field("title", "string", true),
        Some(Reset::ChangedIdentity) => task["identity"] = json!(["id", "title"]),
        Some(Reset::ChangedUnique) => task["unique"] = json!([]),
        Some(Reset::ChangedRelations) => task["relations"] = json!([]),
        Some(Reset::ChangedEnumValues) => mood = json!(["calm", "busy", "tired"]),
        Some(Reset::RequiredWithoutDefault) => task["fields"]
            .as_array_mut()
            .unwrap()
            .push(field("rank", "int", false)),
        Some(Reset::RemovedModel) => task = Value::Null,
    }
    let other = json!({"name":"Other","identity":["id"],"fields":[field("id","string",false),field("title","string",false)]});
    let models: Vec<Value> = [task, other].into_iter().filter(|m| !m.is_null()).collect();
    let mut actions = vec![json!({"name":"TouchOther","version":1,"inputs":[
        {"kind":"model","name":"other","model":"Other","operation":"update","cardinality":"single","fields":["title"]}],"outputs":[]})];
    if reset.is_none() {
        actions.push(json!({"name":"TouchTask","version":1,"inputs":[
            {"kind":"model","name":"task","model":"Task","operation":"update","cardinality":"single","fields":["title"]}],"outputs":[]}));
    }
    Schema::from_value(
        json!({"enums":[{"name":"Mood","values":mood}],"models":models,"actions":actions}),
    )
    .unwrap()
}
fn key(model: &str, id: &str) -> wire::RecordKey {
    wire::RecordKey {
        model: model.into(),
        identity: json!({ "id": id }),
    }
}
/// Rows per engine table for one Model: the table, its before-image,
/// authority evidence and local writes.
fn state(path: &std::path::Path, model: &str) -> Vec<i64> {
    let raw = rusqlite::Connection::open(path).unwrap();
    let count = |sql: &str| -> i64 {
        raw.query_row(sql, [model], |r| r.get(0))
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
    };
    let table = |name: &str| -> i64 {
        let exists: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name=?",
                [name],
                |r| r.get(0),
            )
            .unwrap();
        if exists == 0 {
            return -1;
        }
        raw.query_row(&format!("SELECT COUNT(*) FROM \"{name}\""), [], |r| {
            r.get(0)
        })
        .unwrap()
    };
    vec![
        table(model),
        table(&format!("axton_before_{model}")),
        count("SELECT COUNT(*) FROM axton_authority WHERE model=?"),
        count("SELECT COUNT(*) FROM axton_local_write WHERE model=?"),
    ]
}
fn queue(path: &std::path::Path) -> Vec<Vec<Value>> {
    let raw = rusqlite::Connection::open(path).unwrap();
    let mut rows = vec![];
    for sql in [
        "SELECT id,name,batch_id,batch_digest,reconciled FROM axton_mutation_queue ORDER BY id",
        "SELECT mutation_id,step,model,identity,value FROM axton_mutation_queue_operation ORDER BY mutation_id,step",
    ] {
        let mut statement = raw.prepare(sql).unwrap();
        let width = statement.column_count();
        let mut found = statement.query([]).unwrap();
        while let Some(row) = found.next().unwrap() {
            rows.push(
                (0..width)
                    .map(|i| match row.get_ref(i).unwrap() {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(n) => json!(n),
                        rusqlite::types::ValueRef::Text(t) => {
                            json!(String::from_utf8_lossy(t))
                        }
                        other => json!(format!("{other:?}")),
                    })
                    .collect(),
            );
        }
    }
    rows
}
/// A Store holding Stream rows of both Models, a cache install on `Task` and
/// queued Mutations on each Model.
fn populated(path: &std::path::Path) {
    let mut c = Client::open(SqliteStore::open(path).unwrap(), pair(None), "User:u").unwrap();
    c.initialize_stream05(10, "1").unwrap();
    let context = c.request_context05().unwrap();
    c.install_authority05(
        &context,
        &[
            wire::AuthorityChange::Record {
                key: key("Task", "t"),
                cursor: 3,
                state: json!({"title":"stream","mood":"calm","otherId":"o"}),
            },
            wire::AuthorityChange::Record {
                key: key("Other", "o"),
                cursor: 4,
                state: json!({"title":"other"}),
            },
        ],
        None,
    )
    .unwrap();
    c.install_cache05(
        &[wire::ReadRecord {
            key: key("Task", "cached"),
            cursor: (),
            state: json!({"title":"cached","mood":null,"otherId":null}),
        }],
        true,
    )
    .unwrap();
    c.transaction(|tx| {
        tx.submit_mutation05("TouchTask", 1, json!({"task":{"id":"t","title":"queued"}}))?;
        tx.submit_mutation05(
            "TouchOther",
            1,
            json!({"other":{"id":"o","title":"queued"}}),
        )
    })
    .unwrap();
    c.freeze_batch05().unwrap().unwrap();
}
#[test]
fn reopen_resets_only_the_model_whose_storage_changed() {
    for reset in RESETS {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        populated(&path);
        let task = state(&path, "Task");
        assert!(task[..3].iter().all(|n| *n > 0), "{reset:?}: {task:?}");
        let other = state(&path, "Other");
        let queued = queue(&path);
        let mut c = Client::open(
            SqliteStore::open(&path).unwrap(),
            pair(Some(reset)),
            "User:u",
        )
        .unwrap_or_else(|e| panic!("{reset:?}: {e}"));
        let empty = if let Reset::RemovedModel = reset {
            -1
        } else {
            0
        };
        assert_eq!(state(&path, "Task"), vec![empty, empty, 0, 0], "{reset:?}");
        assert_eq!(state(&path, "Other"), other, "{reset:?}");
        assert_eq!(queue(&path), queued, "{reset:?}");
        assert_eq!(
            c.read(&RecordKey {
                model: "Other".into(),
                identity: json!({"id":"o"}),
            })
            .unwrap()
            .unwrap()["title"],
            "queued",
            "{reset:?}"
        );
        if !matches!(reset, Reset::RemovedModel) {
            assert!(c.query("Task", &json!({})).unwrap().is_empty(), "{reset:?}");
        }
    }
}
#[test]
fn a_reset_with_an_unchanged_materialization_asks_for_bootstrap_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    populated(&path);
    let before = {
        let mut c = Client::open(SqliteStore::open(&path).unwrap(), pair(None), "User:u").unwrap();
        c.store_status05().unwrap()
    };
    let mut c = Client::open(
        SqliteStore::open(&path).unwrap(),
        pair(Some(Reset::ChangedUnique)),
        "User:u",
    )
    .unwrap();
    let after = c.store_status05().unwrap();
    assert_eq!(after.context, before.context);
    assert_eq!(
        (after.start_cursor, after.bootstrap_cursor, after.cursor),
        (before.cursor, None, before.cursor)
    );
}
#[test]
fn a_model_version_that_goes_backwards_still_refuses_to_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut newer = pair(None);
    newer.models[0].version = 2;
    drop(Client::open(SqliteStore::open(&path).unwrap(), newer, "User:u").unwrap());
    let error = Client::open(SqliteStore::open(&path).unwrap(), pair(None), "User:u")
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("Model version regressed"),
        "{error}"
    );
}

#[test]
fn reopen_with_an_added_nullable_field_keeps_rows_and_asks_for_bootstrap_from_c() {
    use common05::{before_upgrade, upgrade_key, upgrade_schema};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let before = before_upgrade(&path);
    let mut c = Client::open(
        SqliteStore::open(&path).unwrap(),
        upgrade_schema(true, false),
        "User:u",
    )
    .unwrap();
    let after = c.store_status05().unwrap();
    assert_eq!(
        (after.start_cursor, after.bootstrap_cursor, after.cursor),
        (Some(40), None, Some(40))
    );
    assert_ne!(
        after.context.materialization,
        before.context.materialization
    );
    assert_eq!(
        after.context.materialization,
        wire::materialization_id(&upgrade_schema(true, false), "1").unwrap()
    );
    assert_eq!(after.context.store_id, before.context.store_id);
    // No connection: stored reads answer from the kept rows.
    for (model, id, text) in [
        ("Marked", "m", "marked"),
        ("Note", "n", "note"),
        ("Note", "gone", "gone"),
    ] {
        let key = upgrade_key(model, id);
        let row = c
            .read(&RecordKey {
                model: key.model,
                identity: key.identity,
            })
            .unwrap()
            .unwrap();
        assert_eq!((&row["text"], &row["label"]), (&json!(text), &Value::Null));
    }
    assert_eq!(c.query("Note", &json!({})).unwrap().len(), 2);
    c.transaction(|tx| tx.submit_mutation05("Edit", 1, json!({"note":{"id":"n","text":"edited"}})))
        .unwrap();
    let batch = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(batch.context, after.context);
}
