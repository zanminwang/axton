pub mod common;
use axton_client::{Client, RecordKey, wire};
use axton_sqlite::SqliteStore;
use serde_json::{Value, json};
fn key(model: &str) -> RecordKey {
    RecordKey {
        model: model.into(),
        identity: json!({"id":if model=="Book"{"b"}else{"c"}}),
    }
}
fn change(model: &str, cursor: u64, state: Value) -> wire::AuthorityChange {
    wire::AuthorityChange::Record {
        key: wire::RecordKey {
            model: model.into(),
            identity: key(model).identity,
        },
        cursor,
        state,
    }
}
#[test]
fn older_parent_absence_cannot_erase_already_installed_later_child_authority() {
    let d = tempfile::tempdir().unwrap();
    let mut c = Client::open(
        SqliteStore::open(d.path().join("db")).unwrap(),
        common::family_schema(),
        "User:u",
    )
    .unwrap();
    let context = c.request_context().unwrap();
    c.install_authority(
        &context,
        &[change(
            "Comment",
            58,
            json!({"bookId":"b","text":"later child authority"}),
        )],
        None,
    )
    .unwrap();
    c.install_authority(&context, &[change("Book", 57, Value::Null)], None)
        .unwrap();
    assert_eq!(
        c.read(&key("Comment")).unwrap().unwrap()["text"],
        "later child authority"
    );
    assert_eq!(
        c.record_evidence(&key("Comment"))
            .unwrap()
            .current
            .unwrap()
            .cursor,
        58
    );
}
#[test]
fn group_cascade_uses_positions_not_transport_array_order() {
    for reverse in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let mut c = Client::open(
            SqliteStore::open(d.path().join("db")).unwrap(),
            common::family_schema(),
            "User:u",
        )
        .unwrap();
        let context = c.request_context().unwrap();
        c.install_cache(
            &[wire::ReadRecord {
                key: wire::RecordKey {
                    model: "Comment".into(),
                    identity: json!({"id":"c"}),
                },
                cursor: (),
                state: json!({"bookId":"b","text":"cached"}),
            }],
            true,
        )
        .unwrap();
        let mut changes = vec![
            change("Book", 57, Value::Null),
            change("Comment", 56, json!({"bookId":"b","text":"older child"})),
        ];
        if reverse {
            changes.reverse()
        };
        c.install_authority(&context, &changes, None).unwrap();
        assert!(c.read(&key("Comment")).unwrap().is_none());
        assert_eq!(
            c.record_evidence(&key("Comment"))
                .unwrap()
                .history
                .values()
                .copied()
                .max(),
            Some(56)
        );
    }
}
