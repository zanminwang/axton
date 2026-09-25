use axton_client::ddl::{FRAMEWORK_DDL, reconcile};
use axton_client::engine::Engine;
use axton_client::queue::OpKind;
use axton_client::rows::{decode_row, merge_identity};
use axton_client::{ClientStore, Mutation, Operation, OperationKind};
use axton_core::{RecordKey, Schema};
use axton_sqlite::SqliteStore;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn schema() -> Schema {
    Schema::from_value(json!({"enums":[],"models":[{"name":"Task","identity":["id"],"fields":[
        {"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}},
        {"name":"title","nullable":false,"type":{"kind":"scalar","name":"string"}},
        {"name":"done","nullable":false,"type":{"kind":"scalar","name":"boolean"}},
        {"name":"tags","nullable":false,"type":{"kind":"list","element":{"kind":"scalar","name":"string"}}}]}]})).unwrap()
}
fn store() -> (tempfile::TempDir, SqliteStore) {
    let dir = tempfile::tempdir().unwrap();
    let mut s = SqliteStore::open(dir.path().join("db")).unwrap();
    s.execute_batch(FRAMEWORK_DDL).unwrap();
    s.execute("INSERT INTO axton_client (client_id, next_ordinal, next_push, generation) VALUES ('c', 1, 1, 1)", &[])
        .unwrap();
    s.begin().unwrap();
    reconcile(&mut s, &schema()).unwrap();
    s.commit().unwrap();
    (dir, s)
}
fn key(id: &str) -> RecordKey {
    schema().record_key("Task", &json!({"id":id})).unwrap()
}
fn row(id: &str, title: &str) -> Value {
    json!({"id":id,"title":title,"done":false,"tags":["a","b"]})
}

#[test]
fn model_rows_round_trip_booleans_lists_and_copy_aside() {
    let (_d, mut s) = store();
    let schema = schema();
    let model = schema.model("Task").unwrap();
    let mut changed = BTreeSet::new();
    s.begin().unwrap();
    let mut e = Engine::new(&mut s, &schema, &mut changed, false);
    e.row_insert("Task", model, &row("t1", "A")).unwrap();
    assert!(e.row_insert("Task", model, &row("t1", "A")).is_err());
    e.row_upsert("Task", model, &row("t1", "B")).unwrap();
    assert_eq!(
        e.row_get("Task", model, &json!({"id":"t1"})).unwrap(),
        Some(row("t1", "B"))
    );
    e.copy_aside(model, &json!({"id":"t1"})).unwrap();
    e.copy_aside(model, &json!({"id":"missing"})).unwrap();
    assert_eq!(
        e.row_get("axton_before_Task", model, &json!({"id":"t1"}))
            .unwrap(),
        Some(row("t1", "B"))
    );
    assert_eq!(e.count("axton_before_Task").unwrap(), 1);
    assert_eq!(
        e.identities_where("Task", model, &[("done".into(), json!(false))])
            .unwrap(),
        vec![json!({"id":"t1"})]
    );
    assert!(
        e.identities_where("Task", model, &[("title".into(), json!("nope"))])
            .unwrap()
            .is_empty()
    );
    e.row_delete("Task", model, &json!({"id":"t1"})).unwrap();
    assert_eq!(e.row_get("Task", model, &json!({"id":"t1"})).unwrap(), None);
    assert_eq!(
        changed,
        BTreeSet::from(["Task".to_string(), "axton_before_Task".to_string()])
    );
    s.rollback().unwrap();
    assert_eq!(
        decode_row(
            model,
            &["done".into(), "tags".into()],
            &[json!(1), json!("[\"x\"]")]
        )
        .unwrap(),
        json!({"done":true,"tags":["x"]})
    );
    assert_eq!(
        merge_identity(&json!({"id":"t1"}), &json!({"title":"T"})),
        json!({"id":"t1","title":"T"})
    );
}

#[test]
fn ledger_tracks_stamps_and_subscriptions() {
    let (_d, mut s) = store();
    let schema = schema();
    let mut changed = BTreeSet::new();
    s.begin().unwrap();
    let mut e = Engine::new(&mut s, &schema, &mut changed, false);
    assert_eq!(e.record_stamp(&key("t1")).unwrap(), 0);
    e.set_record_stamp(&key("t1"), 7).unwrap();
    e.set_record_stamp(&key("t1"), 9).unwrap();
    assert_eq!(e.record_stamp(&key("t1")).unwrap(), 9);
    assert_eq!(e.cursor("x").unwrap(), None);
    let (x, created) = e.ensure_subscription("x").unwrap();
    assert!(created);
    assert_eq!(
        e.cursor("x").unwrap(),
        None,
        "a registration is not a delivery position"
    );
    assert!(
        e.advance_cursor("x", x.subscription_id, 4).is_err(),
        "an uninitialized subscription has no cursor to advance"
    );
    assert!(
        e.initialize_subscription("x", x.subscription_id, 0)
            .unwrap()
    );
    assert!(
        !e.initialize_subscription("x", x.subscription_id, 3)
            .unwrap(),
        "only the first initialization writes the origin"
    );
    e.advance_cursor("x", x.subscription_id, 4).unwrap();
    let (y, _) = e.ensure_subscription("y").unwrap();
    e.initialize_subscription("y", y.subscription_id, 2)
        .unwrap();
    assert_eq!(
        e.subscriptions().unwrap(),
        vec![("x".to_string(), 4), ("y".to_string(), 2)]
    );
    assert!(
        e.advance_cursor("x", y.subscription_id, 5).is_err(),
        "an advance is fenced by the identity the caller read"
    );
    assert_eq!(
        e.cursor("x").unwrap(),
        Some(4),
        "the stale advance moved nothing"
    );
    assert!(
        !e.remove_subscription("x", Some(y.subscription_id)).unwrap(),
        "removal is fenced by the identity it names"
    );
    assert!(e.remove_subscription("x", Some(x.subscription_id)).unwrap());
    assert_eq!(e.cursor("x").unwrap(), None);
    assert!(
        e.advance_cursor("x", x.subscription_id, 5).is_err(),
        "a removed subscription cannot be resurrected by a page"
    );
    s.rollback().unwrap();
}

#[test]
fn queue_rows_reconstruct_mutations_and_cascade_on_delete() {
    let (_d, mut s) = store();
    let schema = schema();
    let mut changed = BTreeSet::new();
    s.begin().unwrap();
    let mut e = Engine::new(&mut s, &schema, &mut changed, false);
    let first = e.allocate_ordinal().unwrap();
    assert_eq!(first, 1);
    e.insert_mutation(
        first,
        &Mutation::new(
            "First",
            vec![Operation {
                model: "Task".into(),
                op: OperationKind::Create,
                identity: json!({"id":"t3"}),
                values: Some(row("t3", "C")),
            }],
        ),
    )
    .unwrap();
    let mut m = Mutation::new(
        "Edit",
        vec![Operation {
            model: "Task".into(),
            op: OperationKind::Update,
            identity: json!({"id":"t1"}),
            values: Some(json!({"title":"B"})),
        }],
    );
    m.companion.push(Operation {
        model: "Task".into(),
        op: OperationKind::Delete,
        identity: json!({"id":"t2"}),
        values: None,
    });
    m.prerequisites.push("upload:1".into());
    m.lifecycle_dependencies.push(first);
    let ordinal = e.allocate_ordinal().unwrap();
    assert_eq!(ordinal, 2);
    e.insert_mutation(ordinal, &m).unwrap();
    e.add_effect(
        ordinal,
        &Operation {
            model: "Task".into(),
            op: OperationKind::Delete,
            identity: json!({"id":"t9"}),
            values: None,
        },
    )
    .unwrap();
    let queued = e.queued().unwrap();
    assert_eq!(queued.len(), 2);
    let q = &queued[1];
    assert_eq!(
        (q.ordinal, q.push, q.mutation.name.as_str()),
        (2, None, "Edit")
    );
    assert_eq!(q.mutation.operations.len(), 1);
    assert_eq!(q.mutation.companion.len(), 1);
    assert_eq!(q.mutation.effects[0].identity, json!({"id":"t9"}));
    assert_eq!(q.mutation.prerequisites, vec!["upload:1"]);
    assert_eq!(q.mutation.lifecycle_dependencies, vec![1]);
    let ops = e.ops_for(&key("t2")).unwrap();
    assert_eq!((ops[0].ordinal, ops[0].kind), (2, OpKind::Companion));
    assert!(e.dirty(&key("t9")).unwrap());
    assert!(!e.dirty(&key("t8")).unwrap());
    assert_eq!(
        e.prerequisite_keys().unwrap(),
        vec![("upload:1".to_string(), None)]
    );
    e.fail_prerequisite("upload:1", "timeout").unwrap();
    assert_eq!(
        e.prerequisite_keys().unwrap(),
        vec![("upload:1".to_string(), Some("timeout".to_string()))]
    );
    e.reset_prerequisite("upload:1").unwrap();
    assert_eq!(e.resolve_prerequisite("upload:1").unwrap(), 1);
    assert!(e.prerequisite_keys().unwrap().is_empty());
    let push = e.allocate_push().unwrap();
    assert_eq!(push, 1);
    e.assign_push(&[1, 2], push).unwrap();
    assert_eq!(e.queued_one(2).unwrap().unwrap().push, Some(1));
    assert_eq!(e.in_flight().unwrap(), Some(1));
    assert_eq!(e.push_models().unwrap(), None);
    e.set_push_models(&json!({"Task":1})).unwrap();
    assert_eq!(e.push_models().unwrap(), Some(json!({"Task":1})));
    assert_eq!(e.last_completed_push().unwrap(), 0);
    e.insert_rejection(2, "Edit", "denied", &json!({"records":[]}))
        .unwrap();
    assert_eq!(e.rejections().unwrap()[0].code, "denied");
    assert_eq!(e.rejection_details().unwrap()[0]["records"], json!([]));
    e.delete_mutations(&[2]).unwrap();
    assert!(
        e.ops_for(&key("t2")).unwrap().is_empty(),
        "operations cascade with the mutation"
    );
    assert!(e.queued_one(2).unwrap().is_none());
    e.set_last_completed_push(push).unwrap();
    assert_eq!(e.last_completed_push().unwrap(), 1);
    assert_eq!(
        e.push_models().unwrap(),
        None,
        "completion releases the frozen declaration"
    );
    e.delete_rejection(2).unwrap();
    assert!(e.rejections().unwrap().is_empty());
    s.rollback().unwrap();
}
