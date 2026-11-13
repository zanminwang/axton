pub mod common05;
use axton_client::{Client, wire};
use axton_sqlite::SqliteStore;
use common05::{key, open};
use serde_json::{Value, json};
fn state(text: Option<&str>) -> Value {
    text.map(|t| json!({"text":t,"note":null}))
        .unwrap_or(Value::Null)
}
fn stream(c: &mut Client<SqliteStore>, cursor: u64, text: Option<&str>) {
    let context = c.request_context05().unwrap();
    c.install_authority05(
        &context,
        &[wire::AuthorityChange::Record {
            key: wire::RecordKey {
                model: "Entry".into(),
                identity: json!({"id":"e"}),
            },
            cursor,
            state: state(text),
        }],
        None,
    )
    .unwrap();
}
fn cache(c: &mut Client<SqliteStore>, text: Option<&str>, store: bool) {
    c.install_cache05(
        &[wire::ReadRecord {
            key: wire::RecordKey {
                model: "Entry".into(),
                identity: json!({"id":"e"}),
            },
            cursor: (),
            state: state(text),
        }],
        store,
    )
    .unwrap();
}
#[test]
fn stream_history_orders_deliveries_and_a_tombstone_guards_later_cache_installs() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = open(&p);
    let context = c.request_context05().unwrap().clone();
    stream(&mut c, 57, Some("A"));
    stream(&mut c, 56, Some("old"));
    stream(&mut c, 57, Some("equal but different"));
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "A");
    cache(&mut c, Some("cache"), true);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "A");
    stream(&mut c, 58, Some("new"));
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "new");
    stream(&mut c, 59, None);
    cache(&mut c, Some("late"), true);
    assert!(c.read(&key()).unwrap().is_none());
    assert!(
        c.record_evidence05(&key())
            .unwrap()
            .current
            .unwrap()
            .deleted
    );
    drop(c);
    let mut c = open(&p);
    cache(&mut c, Some("after-reopen"), true);
    assert!(c.read(&key()).unwrap().is_none());
    assert_eq!(
        c.record_evidence05(&key()).unwrap().history[&context.materialization],
        59
    );
}
#[test]
fn pending_optimism_does_not_release_base_protection_and_null_reads_never_delete() {
    let d = tempfile::tempdir().unwrap();
    let mut c = open(&d.path().join("db"));
    stream(&mut c, 57, Some("A"));
    c.transaction(|tx| tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"P"}})))
        .unwrap();
    assert_eq!(
        c.record_evidence05(&key()).unwrap().current.unwrap().cursor,
        57
    );
    cache(&mut c, Some("ordinary"), true);
    cache(&mut c, None, true);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "P");
    stream(&mut c, 58, Some("new"));
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "P");
    assert_eq!(
        c.record_evidence05(&key()).unwrap().current.unwrap().cursor,
        58
    );
}
#[test]
fn cache_mode_false_changes_no_rows_or_evidence_and_true_is_best_effort_without_g() {
    let d = tempfile::tempdir().unwrap();
    let mut c = open(&d.path().join("db"));
    cache(&mut c, Some("snapshot"), false);
    assert!(c.read(&key()).unwrap().is_none());
    assert_eq!(
        c.record_evidence05(&key()).unwrap(),
        axton_core::authority::RecordEvidence::default()
    );
    cache(&mut c, Some("newer request finished first"), true);
    cache(&mut c, Some("older request finished last"), true);
    assert_eq!(
        c.read(&key()).unwrap().unwrap()["text"],
        "older request finished last"
    );
    assert_eq!(
        c.record_evidence05(&key()).unwrap(),
        axton_core::authority::RecordEvidence::default()
    );
    cache(&mut c, None, true);
    assert!(c.read(&key()).unwrap().is_some());
}

#[test]
fn a_return_to_an_earlier_materialization_overwrites_the_row_each_time() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = open(&p);
    let a = c.request_context05().unwrap().materialization;
    let db = rusqlite::Connection::open(&p).unwrap();
    let adopt = |m: &str| {
        db.execute("UPDATE axton_store SET materialization=?", [m])
            .unwrap();
    };
    stream(&mut c, 57, Some("A"));
    adopt("b");
    stream(&mut c, 57, Some("B"));
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    adopt(&a);
    stream(&mut c, 57, Some("A again"));
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "A again");
    assert_eq!(
        c.record_evidence05(&key()).unwrap().history,
        [(a, 57)].into()
    );
}
