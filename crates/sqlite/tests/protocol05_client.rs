pub mod common05;
use axton_client::*;
use axton_sqlite::SqliteStore;
use common05::*;
use serde_json::json;
use std::collections::BTreeSet;
fn table_count(c: &mut Client<SqliteStore>, table: &str) -> u64 {
    c.read_sql(&format!("SELECT COUNT(*) AS n FROM {table}"), &[])
        .unwrap()[0]["n"]
        .as_u64()
        .unwrap()
}
#[test]
fn open_creates_tables_persists_identity_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut c = open(&path);
    let id = c.client_id().to_string();
    seed(&mut c, "A");
    assert_eq!(
        c.read(&key()).unwrap().unwrap(),
        json!({"id":"e","text":"A","note":null})
    );
    drop(c);
    let mut c = open(&path);
    assert_eq!(c.client_id(), id);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "A");
    assert_eq!(table_count(&mut c, "axton_before_Entry"), 0);
}

#[test]
fn local_transaction_and_mutation_savepoint_have_independent_fate() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    seed(&mut c, "A");
    let events = c.watch(BTreeSet::from(["Entry".to_string()]));
    let result: Result<()> = c.transaction(|tx| {
        tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"B"}}), vec![])?;
        Err(invalid("rollback"))
    });
    assert!(result.is_err());
    assert_eq!(c.pending_count().unwrap(), 0);
    assert!(events.try_recv().is_err());
    c.transaction(|tx| {
        tx.direct(update("LOCAL"))?;
        let failed: Result<()> = tx.savepoint(|tx| {
            tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"bad"}}), vec![])?;
            Err(invalid("refuse"))
        });
        assert!(failed.is_err());
        Ok(())
    })
    .unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "LOCAL");
    assert_eq!(c.pending_count().unwrap(), 0);
    assert!(events.try_recv().is_ok());
    assert!(events.try_recv().is_err());
}

#[test]
fn session_reads_own_writes_without_notifying_until_commit_and_blocks_other_writes() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    seed(&mut c, "A");
    let events = c.watch(BTreeSet::from(["Entry".to_string()]));
    c.begin_session().unwrap();
    c.session(|tx| {
        tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"B"}}), vec![])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        c.session(|tx| tx.read(&key())).unwrap().unwrap()["text"],
        "B"
    );
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "A");
    assert!(events.try_recv().is_err());
    assert!(
        c.transaction(|tx| tx.direct(update("X"))).is_err(),
        "no second transaction during a session"
    );
    c.session_savepoint().unwrap();
    c.session(|tx| tx.direct(update("C"))).unwrap();
    c.session_rollback_savepoint().unwrap();
    assert_eq!(
        c.session(|tx| tx.read(&key())).unwrap().unwrap()["text"],
        "B"
    );
    c.commit_session().unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    assert!(events.try_recv().is_ok());
    c.begin_session().unwrap();
    c.session(|tx| tx.direct(update("Z"))).unwrap();
    c.rollback_session().unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
}

#[test]
fn stale_writer_cannot_overwrite_committed_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut a = open(&path);
    let mut b = open(&path);
    seed(&mut a, "A");
    let error = b
        .transaction(|tx| tx.direct(update("B")))
        .expect_err("the stale handle must be fenced out");
    assert!(
        error.to_string().contains("stale client writer"),
        "the write itself would succeed; only the fence refuses it: {error}"
    );
    assert_eq!(open(&path).read(&key()).unwrap().unwrap()["text"], "A");
}

#[test]
fn declared_unique_constraint_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Client::open05(
        SqliteStore::open(dir.path().join("db")).unwrap(),
        family_schema(),
        "User:u",
    )
    .unwrap();
    let result = c.transaction(|tx| {
        tx.direct(create("Comment", "c1", json!({"bookId":"b","text":"same"})))?;
        tx.direct(create("Comment", "c2", json!({"bookId":"b","text":"same"})))
    });
    assert!(result.is_err());
    assert!(c.query("Comment", &json!({})).unwrap().is_empty());
}

#[test]
fn direct_cascade_handles_cyclic_relationships_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut value = serde_json::to_value(family_schema()).unwrap();
    value["models"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"commentId","nullable":true,"type":{"kind":"scalar","name":"string"}}));
    value["models"][0]["relations"] = json!([{"name":"comment","target":"Comment","fields":["commentId"],"targetFields":["id"],"onDelete":"delete"}]);
    let mut c = Client::open05(
        SqliteStore::open(dir.path().join("db")).unwrap(),
        Schema::from_value(value).unwrap(),
        "User:u",
    )
    .unwrap();
    c.transaction(|tx| {
        tx.direct(create("Book", "b", json!({"title":"B","commentId":"c"})))?;
        tx.direct(create("Comment", "c", json!({"bookId":"b","text":"C"})))?;
        tx.direct(Operation {
            model: "Book".into(),
            op: OperationKind::Delete,
            identity: json!({"id":"b"}),
            values: None,
        })
    })
    .unwrap();
    assert!(c.query("Book", &json!({})).unwrap().is_empty());
    assert!(c.query("Comment", &json!({})).unwrap().is_empty());
}

#[test]
fn committing_with_an_unclosed_savepoint_is_refused_and_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    c.begin_session().unwrap();
    c.session(|tx| tx.direct(create("Entry", "e", json!({"text":"inside","note":null}))))
        .unwrap();
    c.session_savepoint().unwrap();
    c.session(|tx| {
        tx.submit_mutation05(
            "Edit",
            1,
            json!({"entry":{"id":"e","text":"edited"}}),
            vec![],
        )
        .map(|_| ())
    })
    .unwrap();
    let err = c.commit_session().unwrap_err();
    assert!(err.to_string().contains("unclosed savepoint"), "{err}");
    assert!(!c.session_active(), "the refused commit closes the session");
    assert!(
        c.read(&key()).unwrap().is_none(),
        "nothing from the session committed"
    );
    assert_eq!(c.pending_count().unwrap(), 0);
    c.begin_session().unwrap();
    c.session(|tx| tx.direct(create("Entry", "e", json!({"text":"again","note":null}))))
        .unwrap();
    c.session_savepoint().unwrap();
    c.session_release().unwrap();
    c.commit_session().unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "again");
}

#[test]
fn watch_fires_only_for_declared_tables() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = open(&dir.path().join("db"));
    let entry = c.watch(BTreeSet::from(["Entry".into()]));
    let queue = c.watch(BTreeSet::from(["axton_mutation_queue".into()]));
    let other = c.watch(BTreeSet::from(["Other".into()]));
    seed(&mut c, "A");
    assert!(entry.try_recv().is_ok());
    assert!(queue.try_recv().is_err());
    assert!(other.try_recv().is_err());
    c.transaction(|tx| {
        tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"B"}}), vec![])
    })
    .unwrap();
    assert!(entry.try_recv().is_ok());
    assert!(queue.try_recv().is_ok());
    assert!(other.try_recv().is_err());
}
#[test]
fn file_with_batch_materialization_opens_and_keeps_digest_and_outcome_labels() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut c = open(&path);
    c.initialize_stream05(0, "1").unwrap();
    seed(&mut c, "A");
    let call = c
        .transaction(|tx| {
            tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"P"}}), vec![])
        })
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    c.acknowledge_batch05(&v05::BatchAcknowledgement {
        context: b.context.clone(),
        batch_id: b.batch_id,
        digest: b.digest.clone(),
        results: vec![v05::MutationResult {
            mutation_id: call.ordinal,
            outcome: v05::MutationOutcome::Accepted {
                sync_cursor: 1,
                materialization: b.context.materialization.clone(),
                result: serde_json::Value::Null,
                targets: vec![v05::SettlementTarget::Private {
                    record: v05::ReadRecord {
                        key: v05::RecordKey {
                            model: "Entry".into(),
                            identity: json!({"id":"e"}),
                        },
                        cursor: (),
                        state: json!({"text":"server","note":null}),
                    },
                }],
            },
        }],
    })
    .unwrap();
    drop(c);
    // The queue table and envelope trigger as files written before the Batch
    // stopped carrying the schema version have them.
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch(
        "DROP TRIGGER axton_queue_envelope;
         ALTER TABLE axton_mutation_queue ADD COLUMN batch_materialization TEXT;
         UPDATE axton_mutation_queue SET batch_materialization=outcome_materialization;
         ALTER TABLE axton_mutation_queue DROP COLUMN outcome_materialization;
         CREATE TRIGGER axton_queue_envelope BEFORE UPDATE OF batch_materialization,batch_digest ON axton_mutation_queue WHEN OLD.batch_digest IS NOT NULL AND (NEW.batch_digest IS NOT OLD.batch_digest OR NEW.batch_materialization IS NOT OLD.batch_materialization) BEGIN SELECT RAISE(ABORT,'Batch envelope is immutable'); END;",
    )
    .unwrap();
    drop(raw);
    let mut c = open(&path);
    let columns: Vec<String> = c
        .read_sql(
            "SELECT name FROM pragma_table_info('axton_mutation_queue')",
            &[],
        )
        .unwrap()
        .into_iter()
        .map(|r| r["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(!columns.iter().any(|n| n == "batch_materialization"));
    drop(c);
    let raw = rusqlite::Connection::open(&path).unwrap();
    let error = raw
        .execute(
            "UPDATE axton_mutation_queue SET batch_digest=?",
            [b.digest.replace(&b.digest[..1], "x")],
        )
        .unwrap_err();
    assert!(error.to_string().contains("Batch envelope is immutable"));
    drop(raw);
    let mut c = open(&path);
    let context = c.request_context05().unwrap();
    c.install_authority05(&context, &[], Some((0, 1))).unwrap();
    c.settle_ready05().unwrap();
    assert!(c.call_completion05(&call.call_id).unwrap().is_some());
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "server");
}
