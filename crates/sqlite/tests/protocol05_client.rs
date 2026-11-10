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
        tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"B"}}))?;
        Err(invalid("rollback"))
    });
    assert!(result.is_err());
    assert_eq!(c.pending_count().unwrap(), 0);
    assert!(events.try_recv().is_err());
    c.transaction(|tx| {
        tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"LOCAL"}}))?;
        let failed: Result<()> = tx.savepoint(|tx| {
            tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"bad"}}))?;
            Err(invalid("refuse"))
        });
        assert!(failed.is_err());
        Ok(())
    })
    .unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "LOCAL");
    assert_eq!(
        c.pending_count().unwrap(),
        1,
        "only the outer Mutation is queued"
    );
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
        tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"B"}}))?;
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
        c.transaction(|tx| tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"X"}})))
            .is_err(),
        "no second transaction during a session"
    );
    c.session_savepoint().unwrap();
    c.session(|tx| tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"C"}})))
        .unwrap();
    c.session_rollback_savepoint().unwrap();
    assert_eq!(
        c.session(|tx| tx.read(&key())).unwrap().unwrap()["text"],
        "B"
    );
    c.commit_session().unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    assert!(events.try_recv().is_ok());
    c.begin_session().unwrap();
    c.session(|tx| tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"Z"}})))
        .unwrap();
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
        .transaction(|tx| tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"B"}})))
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
        with_crud(family_schema()),
        "User:u",
    )
    .unwrap();
    let result = c.transaction(|tx| {
        create_call(tx, "Comment", "c1", json!({"bookId":"b","text":"same"}))?;
        create_call(tx, "Comment", "c2", json!({"bookId":"b","text":"same"}))
    });
    assert!(result.is_err());
    assert!(c.query("Comment", &json!({})).unwrap().is_empty());
}

#[test]
fn delete_cascade_handles_cyclic_relationships_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut value = serde_json::to_value(family_schema()).unwrap();
    value["models"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"commentId","nullable":true,"type":{"kind":"scalar","name":"string"}}));
    value["models"][0]["relations"] = json!([{"name":"comment","target":"Comment","fields":["commentId"],"targetFields":["id"],"onDelete":"delete"}]);
    let mut c = Client::open05(
        SqliteStore::open(dir.path().join("db")).unwrap(),
        with_crud(Schema::from_value(value).unwrap()),
        "User:u",
    )
    .unwrap();
    c.transaction(|tx| {
        create_call(tx, "Book", "b", json!({"title":"B","commentId":"c"}))?;
        create_call(tx, "Comment", "c", json!({"bookId":"b","text":"C"}))?;
        delete_call(tx, "Book", "b")
    })
    .unwrap();
    assert!(c.query("Book", &json!({})).unwrap().is_empty());
    assert!(c.query("Comment", &json!({})).unwrap().is_empty());
}

#[test]
fn committing_with_an_unclosed_savepoint_is_refused_and_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Client::open05(
        SqliteStore::open(dir.path().join("db")).unwrap(),
        with_crud(schema()),
        "User:u",
    )
    .unwrap();
    c.begin_session().unwrap();
    c.session(|tx| create_call(tx, "Entry", "e", json!({"text":"inside","note":null})))
        .unwrap();
    c.session_savepoint().unwrap();
    c.session(|tx| {
        tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"edited"}}))
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
    c.session(|tx| create_call(tx, "Entry", "e", json!({"text":"again","note":null})))
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
    c.transaction(|tx| tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"B"}})))
        .unwrap();
    assert!(entry.try_recv().is_ok());
    assert!(queue.try_recv().is_ok());
    assert!(other.try_recv().is_err());
}
#[test]
fn a_file_written_by_0_6_0_opens_without_its_three_dead_columns_and_keeps_digest_and_outcome_labels()
 {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut c = open(&path);
    c.initialize_stream05(0, "1").unwrap();
    seed(&mut c, "A");
    let call = c
        .transaction(|tx| tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"P"}})))
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
    // The Store and queue tables and the envelope trigger as 0.6.0 wrote them.
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch(
        "ALTER TABLE axton_store ADD COLUMN desired_materialization TEXT NOT NULL DEFAULT '';
         ALTER TABLE axton_store ADD COLUMN enabled_descriptor TEXT NOT NULL DEFAULT '';
         UPDATE axton_store SET desired_materialization=materialization,enabled_descriptor=schema_descriptor;
         DROP TRIGGER axton_queue_envelope;
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
    let store: Vec<String> = c
        .read_sql("SELECT name FROM pragma_table_info('axton_store')", &[])
        .unwrap()
        .into_iter()
        .map(|r| r["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !store
            .iter()
            .any(|n| n == "desired_materialization" || n == "enabled_descriptor"),
        "{store:?}"
    );
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

// A file written while Mutation companion writes and direct Model writes
// existed may hold a queued Mutation with a `companion` operation and an
// `independent` local write after it, with the rows they wrote. Open purges
// both and rebuilds the records they touched; the Mutation stays queued and
// uploads alone. Opening again changes nothing.
#[test]
fn an_old_file_s_companion_operations_and_independent_writes_are_purged_at_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut c = open(&path);
    start_stream(&mut c);
    seed(&mut c, "A");
    cached(&mut c, "Entry", "c", json!({"text":"C","note":null}));
    let call = offline(&mut c, "Edit", json!({"entry":{"id":"e","text":"B"}}));
    drop(c);
    // What the engine that had them left behind: the companion's row over
    // a held base on `c`, and the independent write's row on `e`.
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch(
        "INSERT INTO axton_mutation_queue_operation(mutation_id,step,local_sequence,input_path,kind,model,identity,operation,value,owner_history) \
         SELECT mutation_id,step+1,local_sequence+1000,NULL,'companion',model,'{\"id\":\"c\"}',operation,'{\"text\":\"companion\"}',owner_history \
         FROM axton_mutation_queue_operation WHERE kind='wire';
         INSERT INTO axton_before_Entry SELECT * FROM Entry WHERE id='c';
         UPDATE Entry SET text='companion' WHERE id='c';
         INSERT INTO axton_local_write(sequence,ordinal,position,disposition,model,identity,op,\"values\") \
         VALUES(1000,1,NULL,'independent','Entry','{\"id\":\"e\"}','update','{\"text\":\"independent\"}');
         UPDATE Entry SET text='independent' WHERE id='e';",
    )
    .unwrap();
    drop(raw);
    let other = schema().record_key("Entry", &json!({"id":"c"})).unwrap();
    for _ in 0..2 {
        let mut c = open(&path);
        assert_eq!(
            c.read_sql(
                "SELECT COUNT(*) AS n FROM axton_mutation_queue_operation WHERE kind<>'wire'",
                &[]
            )
            .unwrap()[0]["n"],
            0
        );
        assert_eq!(table_count(&mut c, "axton_local_write"), 0);
        assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
        assert_eq!(c.read(&other).unwrap().unwrap()["text"], "C");
        assert_eq!(table_count(&mut c, "axton_before_Entry"), 1);
        assert_eq!(c.record_status(&other).unwrap()["pending"], json!([]));
        assert_eq!(c.pending_count().unwrap(), 1);
        let batch = c.freeze_batch05().unwrap().unwrap();
        assert_eq!(batch.mutations.len(), 1);
        assert_eq!(batch.mutations[0].id, call.ordinal);
        let body = String::from_utf8(v05::encode(&batch).unwrap()).unwrap();
        assert!(!body.contains("companion"), "{body}");
        assert!(!body.contains("independent"), "{body}");
        axton_sim::scenario05::assert_projection05(&mut c, &schema());
    }
}

// A refused Mutation the app has not dismissed keeps its operation rows,
// and in an old file a `companion` may be among them. The refusal already
// restored every record the Mutation touched and dropped their before
// images, so the purge deletes the companion and leaves the record as the
// Stream delivered it, with its evidence.
#[test]
fn the_purge_leaves_a_record_a_refused_mutation_s_companion_touched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut c = open(&path);
    start_stream(&mut c);
    seed(&mut c, "A");
    protected(&mut c, &[record("Entry", "r", 1, json!({"text":"R","note":null}))]);
    let call = offline(&mut c, "Edit", json!({"entry":{"id":"e","text":"B"}}));
    let batch = c.freeze_batch05().unwrap().unwrap();
    c.acknowledge_batch05(&v05::BatchAcknowledgement {
        context: batch.context.clone(),
        batch_id: batch.batch_id,
        digest: batch.digest.clone(),
        results: vec![v05::MutationResult {
            mutation_id: call.ordinal,
            outcome: v05::MutationOutcome::Rejected {
                code: "write.denied".into(),
                message: None,
            },
        }],
    })
    .unwrap();
    let delivered = schema().record_key("Entry", &json!({"id":"r"})).unwrap();
    let evidence = "SELECT evidence FROM axton_authority WHERE model='Entry' AND identity='{\"id\":\"r\"}'";
    let before = c.read_sql(evidence, &[]).unwrap();
    assert_eq!(before.len(), 1);
    drop(c);
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch(
        "INSERT INTO axton_mutation_queue_operation(mutation_id,step,local_sequence,input_path,kind,model,identity,operation,value,owner_history) \
         SELECT mutation_id,step+1,local_sequence+1000,NULL,'companion',model,'{\"id\":\"r\"}',operation,'{\"text\":\"companion\"}',owner_history \
         FROM axton_mutation_queue_operation WHERE kind='wire';",
    )
    .unwrap();
    drop(raw);
    for _ in 0..2 {
        let mut c = open(&path);
        assert_eq!(
            c.read_sql(
                "SELECT COUNT(*) AS n FROM axton_mutation_queue_operation WHERE kind<>'wire'",
                &[]
            )
            .unwrap()[0]["n"],
            0
        );
        assert_eq!(
            c.read(&delivered).unwrap(),
            Some(json!({"id":"r","text":"R","note":null}))
        );
        assert_eq!(c.read_sql(evidence, &[]).unwrap(), before);
        assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "A");
        assert!(matches!(
            c.mutation_result05(call.ordinal).unwrap().unwrap().outcome,
            v05::MutationOutcome::Rejected { .. }
        ));
        axton_sim::scenario05::assert_projection05(&mut c, &schema());
    }
}

/// `schema()` with a second Model `Extra`; `title` is a string, or an int
/// (`incompatible`), and `removed` leaves `Extra` out.
fn with_extra(removed: bool, incompatible: bool) -> Schema {
    let mut value = serde_json::to_value(schema()).unwrap();
    if !removed {
        let title = if incompatible { "int" } else { "string" };
        value["models"].as_array_mut().unwrap().push(json!({"name":"Extra","identity":["id"],"fields":[
            {"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}},
            {"name":"title","nullable":false,"type":{"kind":"scalar","name":title}}]}));
    }
    Schema::from_value(value).unwrap()
}
fn open_with(path: &std::path::Path, schema: Schema) -> Client<SqliteStore> {
    Client::open05(SqliteStore::open(path).unwrap(), schema, "User:u").unwrap()
}
/// What an engine with companion and direct writes left on `Extra` `x`:
/// a companion of the queued Edit and an independent write after it, over a
/// held base when `held`.
fn retired_writes_on_extra(path: &std::path::Path, held: bool) {
    let raw = rusqlite::Connection::open(path).unwrap();
    raw.execute_batch(
        "INSERT INTO axton_mutation_queue_operation(mutation_id,step,local_sequence,input_path,kind,model,identity,operation,value,owner_history) \
         SELECT mutation_id,step+1,local_sequence+1000,NULL,'companion','Extra','{\"id\":\"x\"}','update','{\"title\":\"companion\"}',owner_history \
         FROM axton_mutation_queue_operation WHERE kind='wire';
         INSERT INTO axton_local_write(sequence,ordinal,position,disposition,model,identity,op,\"values\") \
         VALUES(1000,1,NULL,'independent','Extra','{\"id\":\"x\"}','update','{\"title\":\"independent\"}');",
    )
    .unwrap();
    if held {
        raw.execute_batch(
            "INSERT INTO axton_before_Extra SELECT * FROM Extra WHERE id='x';
             UPDATE Extra SET title='independent' WHERE id='x';",
        )
        .unwrap();
    }
}
/// A Store with `Extra` `x` delivered by the Stream and an offline Edit of
/// `e`, written under `with_extra(false, false)`.
fn queued_with_extra(path: &std::path::Path) -> SubmittedCall {
    let mut c = open_with(path, with_extra(false, false));
    start_stream(&mut c);
    seed(&mut c, "A");
    protected(&mut c, &[record("Extra", "x", 1, json!({"title":"X"}))]);
    offline(&mut c, "Edit", json!({"entry":{"id":"e","text":"B"}}))
}
fn assert_purged(c: &mut Client<SqliteStore>, schema: &Schema, call: &SubmittedCall) {
    assert_eq!(
        c.read_sql(
            "SELECT COUNT(*) AS n FROM axton_mutation_queue_operation WHERE kind<>'wire'",
            &[]
        )
        .unwrap()[0]["n"],
        0
    );
    assert_eq!(table_count(c, "axton_local_write"), 0);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    assert_eq!(c.pending_count().unwrap(), 1);
    assert_eq!(
        c.freeze_batch05().unwrap().unwrap().mutations[0].id,
        call.ordinal
    );
    axton_sim::scenario05::assert_projection05(c, schema);
}
// The purge runs after open reconciled the Model tables in the same
// transaction. A companion or independent write on a Model the new schema
// removed names a record that has no key and no table: both are deleted and
// nothing is rebuilt.
#[test]
fn the_purge_deletes_retired_writes_on_a_removed_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let call = queued_with_extra(&path);
    retired_writes_on_extra(&path, true);
    for _ in 0..2 {
        let mut c = open_with(&path, with_extra(true, false));
        assert_purged(&mut c, &with_extra(true, false), &call);
        assert!(
            c.read_sql(
                "SELECT name FROM sqlite_schema WHERE name LIKE '%Extra%'",
                &[]
            )
            .unwrap()
            .is_empty()
        );
    }
}
// A Model the new schema resets as incompatible is emptied before the purge,
// and the queued Mutation no longer shares it: its companion on that Model
// holds nothing, both retired rows are deleted and nothing is rebuilt.
#[test]
fn the_purge_deletes_retired_writes_on_a_reset_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let call = queued_with_extra(&path);
    retired_writes_on_extra(&path, true);
    for _ in 0..2 {
        let mut c = open_with(&path, with_extra(false, true));
        assert_purged(&mut c, &with_extra(false, true), &call);
        assert_eq!(table_count(&mut c, "Extra"), 0);
        assert_eq!(table_count(&mut c, "axton_before_Extra"), 0);
    }
}
// An older engine that reset `Extra` kept the queued Mutation's companion on
// it, and the Stream then delivered `x` again under the new shape. The
// Mutation does not share `Extra` with the current schema, so the purge
// deletes the companion and leaves `x` as delivered.
#[test]
fn the_purge_leaves_a_record_of_a_model_the_mutation_does_not_share() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let call = queued_with_extra(&path);
    let reset = with_extra(false, true);
    let mut c = open_with(&path, reset.clone());
    let context = c.request_context05().unwrap();
    c.install_authority05(&context, &[record("Extra", "x", 2, json!({"title":7}))], None)
        .unwrap();
    drop(c);
    retired_writes_on_extra(&path, false);
    let x = reset.record_key("Extra", &json!({"id":"x"})).unwrap();
    for _ in 0..2 {
        let mut c = open_with(&path, reset.clone());
        assert_purged(&mut c, &reset, &call);
        assert_eq!(c.read(&x).unwrap(), Some(json!({"id":"x","title":7})));
    }
}

// A file written while direct Model writes existed has
// `axton_local_replica_layer`, maybe with rows. Open drops it; the file stays
// format 5 and its rows are unchanged.
#[test]
fn an_old_file_s_local_replica_layer_is_dropped_at_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut c = open(&path);
    seed(&mut c, "A");
    drop(c);
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch(
        "CREATE TABLE IF NOT EXISTS axton_local_replica_layer(model TEXT NOT NULL,identity TEXT NOT NULL,operations TEXT NOT NULL,PRIMARY KEY(model,identity));
         INSERT INTO axton_local_replica_layer VALUES('Entry','{\"id\":\"e\"}','[]');",
    )
    .unwrap();
    drop(raw);
    for _ in 0..2 {
        let mut c = open(&path);
        assert_eq!(
            c.read_sql(
                "SELECT COUNT(*) AS n FROM sqlite_schema WHERE name='axton_local_replica_layer'",
                &[]
            )
            .unwrap()[0]["n"],
            0
        );
        assert_eq!(
            c.read_sql("SELECT format FROM axton_store", &[]).unwrap()[0]["format"],
            5
        );
        assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "A");
        assert_eq!(c.pending_count().unwrap(), 0);
    }
}
