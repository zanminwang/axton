use axton_client::{Client, Schema, v05};
use axton_sqlite::SqliteStore;
use serde_json::{Value, json};
fn schema() -> Schema {
    let mut s: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    s["actions"] = json!([{"name":"Write","version":1,"inputs":[{"kind":"model","name":"entries","model":"Entry","operation":"create","cardinality":"list"},{"kind":"model","name":"maybe","model":"Entry","operation":"update","cardinality":"optional"}],"outputs":[]}]);
    Schema::from_value(s).unwrap()
}
#[test]
fn fixed_batch_restores_input_presence_and_excludes_new_members() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let call = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]}), vec![]))
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(b.mutations[0].id, call.ordinal);
    assert_eq!(
        v05::reconstruct_input(&b.mutations[0].operations).unwrap(),
        json!({"entries":[]})
    );
    c.transaction(|tx| {
        tx.submit_mutation05("Write", 1, json!({"entries":[],"maybe":null}), vec![])
    })
    .unwrap();
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), b);
    drop(c);
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    assert_eq!(
        v05::encode(&c.freeze_batch05().unwrap().unwrap()).unwrap(),
        v05::encode(&b).unwrap()
    );
}
#[test]
fn no_batch_freezes_before_the_handshake_sets_start() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    let call = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]}), vec![]))
        .unwrap();
    assert!(c.freeze_batch05().unwrap().is_none());
    drop(c);
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    assert!(c.freeze_batch05().unwrap().is_none());
    assert_eq!(c.store_status05().unwrap().last_acknowledged_batch_id, 0);
    c.initialize_stream05(7, "1").unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    assert_eq!((b.batch_id, b.mutations[0].id), (1, call.ordinal));
}
#[test]
fn refuses_unsupported_file_without_touching_rollback_or_wal_bytes() {
    SqliteStore::set_application_data_directory(std::env::temp_dir().join("axton-task2-locks"))
        .unwrap();
    let d = tempfile::tempdir().unwrap();
    for wal in [false, true] {
        let p = d.path().join(if wal { "wal.db" } else { "rollback.db" });
        let db = rusqlite::Connection::open(&p).unwrap();
        if wal {
            db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
                .unwrap();
        }
        db.execute_batch(
            "CREATE TABLE axton_client(id TEXT); INSERT INTO axton_client VALUES('legacy');",
        )
        .unwrap();
        let files = [
            p.clone(),
            std::path::PathBuf::from(format!("{}-wal", p.display())),
            std::path::PathBuf::from(format!("{}-shm", p.display())),
        ];
        let before: Vec<_> = files.iter().map(|p| std::fs::read(p).ok()).collect();
        assert!(SqliteStore::open_exclusive05(&p, "User:u").is_err());
        let after: Vec<_> = files.iter().map(|p| std::fs::read(p).ok()).collect();
        assert_eq!(before, after);
        use sha2::{Digest, Sha256};
        let hashes = |bytes: &[Option<Vec<u8>>]| {
            bytes
                .iter()
                .map(|v| v.as_ref().map(|v| format!("{:x}", Sha256::digest(v))))
                .collect::<Vec<_>>()
        };
        assert_eq!(hashes(&before), hashes(&after));
        println!("unchanged refusal hashes wal={wal}: {:?}", hashes(&after));
    }
}
#[test]
fn nested_rollback_reuses_only_uncommitted_ids_and_nonempty_lists_restore() {
    let d = tempfile::tempdir().unwrap();
    let mut c = Client::open05(
        SqliteStore::open(d.path().join("db")).unwrap(),
        schema(),
        "User:u",
    )
    .unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let call=c.transaction(|tx|{let rolled:axton_client::Result<()>=tx.savepoint(|tx|{tx.submit_mutation05("Write",1,json!({"entries":[{"id":"bad","text":"discarded","note":null}]}),vec![])?;Err(axton_client::invalid("rollback"))});assert!(rolled.is_err());tx.submit_mutation05("Write",1,json!({"entries":[{"id":"e1","text":"one","note":null},{"id":"e2","text":"two","note":null}],"maybe":null}),vec![])}).unwrap();
    assert_eq!(call.ordinal, 1);
    let b = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(b.mutations[0].operations.len(), 3);
    assert_eq!(
        v05::reconstruct_input(&b.mutations[0].operations).unwrap(),
        json!({"entries":[{"id":"e1","text":"one","note":null},{"id":"e2","text":"two","note":null}],"maybe":null})
    );
}
#[test]
fn compatible_descriptor_reopen_preserves_assigned_context_and_input() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    c.transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]}), vec![]))
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    drop(c);
    let mut newer = schema();
    newer.models[0].bootstrap = !newer.models[0].bootstrap;
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), newer, "User:u").unwrap();
    assert_eq!(
        c.request_context05().unwrap().materialization,
        b.context.materialization
    );
    assert!(c.rollover_pending05().unwrap());
    let status = c.status_snapshot05().unwrap();
    assert!(status.get("schema").is_none());
    assert_eq!(status["context"], serde_json::to_value(&b.context).unwrap());
    assert_eq!(status["pending"], 1);
    assert!(c.rollover_pending05().unwrap());
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), b);
}
#[test]
fn value_arguments_have_one_entry_and_do_not_create_model_optimism() {
    let d = tempfile::tempdir().unwrap();
    let mut s = serde_json::to_value(schema()).unwrap();
    s["actions"] = json!([{"name":"Arg","version":1,"inputs":[{"kind":"value","name":"label","type":{"kind":"scalar","name":"string"},"nullable":true},{"kind":"value","name":"tags","type":{"kind":"scalar","name":"string"},"nullable":false,"list":true}],"outputs":[]}]);
    let s = Schema::from_value(s).unwrap();
    let mut c =
        Client::open05(SqliteStore::open(d.path().join("db")).unwrap(), s, "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    c.transaction(|tx| {
        tx.submit_mutation05("Arg", 1, json!({"label":null,"tags":["a","b"]}), vec![])
    })
    .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(b.mutations[0].operations.len(), 2);
    assert!(
        b.mutations[0]
            .operations
            .iter()
            .all(|o| o.operation == v05::Operation::Argument)
    );
    assert_eq!(
        v05::reconstruct_input(&b.mutations[0].operations).unwrap(),
        json!({"label":null,"tags":["a","b"]})
    );
}
#[test]
fn next_batch_preserves_previous_ownership_until_settlement() {
    let d = tempfile::tempdir().unwrap();
    let mut c = Client::open05(
        SqliteStore::open(d.path().join("db")).unwrap(),
        schema(),
        "User:u",
    )
    .unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let first = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entries":[{"id":"e","text":"one","note":null}]}),
                vec![],
            )
        })
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    let dependent = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entries":[],"maybe":{"id":"e","text":"two"}}),
                vec![],
            )
        })
        .unwrap();
    let independent = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]}), vec![]))
        .unwrap();
    let a = v05::BatchAcknowledgement {
        context: b.context.clone(),
        batch_id: b.batch_id,
        digest: b.digest.clone(),
        results: vec![v05::MutationResult {
            mutation_id: first.ordinal,
            outcome: v05::MutationOutcome::Accepted {
                sync_cursor: 9,
                result: Value::Null,
                targets: vec![v05::SettlementTarget::Stream {
                    key: v05::RecordKey {
                        model: "Entry".into(),
                        identity: json!({"id":"e"}),
                    },
                    cursor: 9,
                    fallback: v05::ReadRecord {
                        key: v05::RecordKey {
                            model: "Entry".into(),
                            identity: json!({"id":"e"}),
                        },
                        cursor: (),
                        state: json!({"text":"one","note":null}),
                    },
                }],
            },
        }],
    };
    c.acknowledge_batch05(&a).unwrap();
    assert!(
        c.freeze_batch05().unwrap().is_none(),
        "an acknowledged Batch still owns settlement targets"
    );
    assert!(c.mutation_result05(first.ordinal).unwrap().is_none());
    drop(c);
    let mut c = Client::open05(
        SqliteStore::open(d.path().join("db")).unwrap(),
        schema(),
        "User:u",
    )
    .unwrap();
    assert!(
        c.freeze_batch05().unwrap().is_none(),
        "reopen retains the ownership fence"
    );
    c.install_authority05(
        &b.context,
        &[v05::AuthorityChange::Record {
            key: v05::RecordKey {
                model: "Entry".into(),
                identity: json!({"id":"e"}),
            },
            cursor: 9,
            state: json!({"text":"one","note":null}),
        }],
        Some((0, 9)),
    )
    .unwrap();
    assert!(c.call_completion05(&first.call_id).unwrap().is_some());
    let b2 = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(b2.batch_id, 2);
    assert_eq!(
        b2.mutations.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![dependent.ordinal, independent.ordinal]
    );
}
#[test]
fn descriptor_snapshot_reorder_retains_original_hash_in_identical_read_context() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut s = schema();
    let mut v = serde_json::to_value(&s).unwrap();
    v["actions"][0]["input"] = json!({"models":v["models"].clone(),"enums":[]});
    s = Schema::from_value(v).unwrap();
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), s.clone(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    c.transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]}), vec![]))
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    drop(c);
    s.actions[0].input.as_mut().unwrap().models[0]
        .fields
        .reverse();
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), s, "User:u").unwrap();
    assert_eq!(
        c.request_context05().unwrap().materialization,
        b.context.materialization
    );
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), b);
}
#[test]
fn discard_refuses_assigned_owner_and_dismiss_retains_completed_outcome() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let call = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]}), vec![]))
        .unwrap();
    let report = c
        .transaction(|tx| tx.discard_mutation05(call.ordinal))
        .unwrap();
    assert_eq!(report.completions.len(), 1);
    let completed = c.call_completion05(&call.call_id).unwrap();
    assert!(completed.is_some());
    c.transaction(|tx| tx.dismiss_rejection05(call.ordinal))
        .unwrap();
    assert_eq!(c.call_completion05(&call.call_id).unwrap(), completed);
    let assigned = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]}), vec![]))
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    assert!(
        c.transaction(|tx| tx.discard_mutation05(assigned.ordinal))
            .is_err()
    );
    assert!(
        c.transaction(|tx| tx.dismiss_rejection05(assigned.ordinal))
            .is_err()
    );
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), b);
}
#[test]
fn snapshot_refusal_cost_and_stream_admission_are_observed() {
    SqliteStore::set_application_data_directory(std::env::temp_dir().join("axton-task2-locks"))
        .unwrap();
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("large.db");
    let db = rusqlite::Connection::open(&p).unwrap();
    db.execute_batch("CREATE TABLE axton_client(payload BLOB); INSERT INTO axton_client VALUES(zeroblob(16777216));").unwrap();
    drop(db);
    let bytes = std::fs::metadata(&p).unwrap().len();
    let start = std::time::Instant::now();
    let e = SqliteStore::open_exclusive05(&p, "User:u").err().unwrap();
    assert!(e.to_string().contains("unsupported Store format"));
    println!(
        "locked snapshot refusal: {bytes} bytes, {} microseconds",
        start.elapsed().as_micros()
    );
    let p = d.path().join("current.db");
    let mut c = Client::open05(
        SqliteStore::open_exclusive05(&p, "User:u").unwrap(),
        schema(),
        "User:u",
    )
    .unwrap();
    let id = c.request_context05().unwrap().store_id;
    drop(c);
    let before = std::fs::read(&p).unwrap();
    assert!(
        SqliteStore::open_exclusive05(&p, "User:other")
            .err()
            .unwrap()
            .to_string()
            .contains("Stream mismatch")
    );
    assert_eq!(std::fs::read(&p).unwrap(), before);
    let mut c = Client::open05(
        SqliteStore::open_exclusive05(&p, "User:u").unwrap(),
        schema(),
        "User:u",
    )
    .unwrap();
    assert_eq!(c.request_context05().unwrap().store_id, id);
}
#[test]
fn assigned_payload_and_membership_cannot_be_edited_or_cancelled_in_sql() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let first = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]}), vec![]))
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    let later = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]}), vec![]))
        .unwrap();
    let db = rusqlite::Connection::open(&p).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    assert!(
        db.execute(
            "UPDATE axton_mutation_queue_operation SET value='null' WHERE mutation_id=?",
            [first.ordinal as i64]
        )
        .is_err()
    );
    assert!(
        db.execute(
            "DELETE FROM axton_mutation_queue WHERE id=?",
            [first.ordinal as i64]
        )
        .is_err()
    );
    assert!(
        db.execute(
            "UPDATE axton_mutation_queue SET batch_id=1,batch_materialization=? WHERE id=?",
            rusqlite::params![b.context.materialization, later.ordinal as i64]
        )
        .is_err()
    );
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), b);
}
#[test]
fn same_identity_in_two_slots_keeps_paths_and_binding_failure_is_atomic() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut s = serde_json::to_value(schema()).unwrap();
    s["actions"] = json!([{"name":"Pair","version":1,"inputs":[{"kind":"model","name":"first","model":"Entry","operation":"create","cardinality":"single"},{"kind":"model","name":"second","model":"Entry","operation":"update","cardinality":"optional","bindings":[{"slot":"first","fields":["id"]}]}],"outputs":[]}]);
    let mut c = Client::open05(
        SqliteStore::open(&p).unwrap(),
        Schema::from_value(s).unwrap(),
        "User:u",
    )
    .unwrap();
    c.initialize_stream05(0, "1").unwrap();
    assert!(c.transaction(|tx|tx.submit_mutation05("Pair",1,json!({"first":{"id":"e","text":"one","note":null},"second":{"id":"other","text":"two"}}),vec![])).is_err());
    assert_eq!(c.store_status05().unwrap().next_mutation_id, 1);
    c.transaction(|tx| {
        tx.submit_mutation05(
            "Pair",
            1,
            json!({"first":{"id":"e","text":"one","note":null},"second":{"id":"e","text":"two"}}),
            vec![],
        )
    })
    .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(
        b.mutations[0]
            .operations
            .iter()
            .map(|o| o.input_path.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}
#[test]
fn unsafe_sequence_counter_rolls_back_model_write_and_mutation_id() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    let db = rusqlite::Connection::open(&p).unwrap();
    db.execute(
        "UPDATE axton_store SET next_local_sequence=?",
        [axton_client::MAX_SAFE_INTEGER as i64],
    )
    .unwrap();
    assert!(
        c.transaction(|tx| tx.submit_mutation05(
            "Write",
            1,
            json!({"entries":[{"id":"e","text":"one","note":null}]}),
            vec![]
        ))
        .is_err()
    );
    assert_eq!(c.store_status05().unwrap().next_mutation_id, 1);
    assert!(
        c.read(&axton_client::RecordKey {
            model: "Entry".into(),
            identity: json!({"id":"e"})
        })
        .unwrap()
        .is_none()
    );
    assert!(c.freeze_batch05().unwrap().is_none());
}
#[test]
fn late_cascade_discovery_extends_owned_effects_without_changing_frozen_input() {
    use axton_client::{Operation, OperationKind, RecordKey};
    let mut value = json!({"enums":[],"models":[
        {"name":"Book","identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}},{"name":"title","nullable":false,"type":{"kind":"scalar","name":"string"}}]},
        {"name":"Comment","identity":["id"],"fields":[{"name":"id","nullable":false,"type":{"kind":"scalar","name":"string"}},{"name":"bookId","nullable":false,"type":{"kind":"scalar","name":"string"}},{"name":"text","nullable":false,"type":{"kind":"scalar","name":"string"}}],"relations":[{"name":"book","target":"Book","fields":["bookId"],"targetFields":["id"],"onDelete":"delete"}]}
    ]});
    value["actions"] = json!([{"name":"DeleteBook","version":1,"inputs":[{"kind":"model","name":"book","model":"Book","operation":"delete","cardinality":"single"}],"outputs":[]}]);
    let s = Schema::from_value(value).unwrap();
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), s.clone(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    c.transaction(|tx| {
        tx.direct(Operation {
            model: "Book".into(),
            op: OperationKind::Create,
            identity: json!({"id":"b"}),
            values: Some(json!({"title":"book"})),
        })
    })
    .unwrap();
    let call = c
        .transaction(|tx| tx.submit_mutation05("DeleteBook", 1, json!({"book":{"id":"b"}}), vec![]))
        .unwrap();
    let batch = c.freeze_batch05().unwrap().unwrap();
    c.install_cache05(
        &[v05::ReadRecord {
            key: v05::RecordKey {
                model: "Comment".into(),
                identity: json!({"id":"c"}),
            },
            cursor: (),
            state: json!({"bookId":"b","text":"late child"}),
        }],
        true,
    )
    .unwrap();
    let child = RecordKey {
        model: "Comment".into(),
        identity: json!({"id":"c"}),
    };
    assert!(c.read(&child).unwrap().is_none());
    let ops = c.read_sql("SELECT input_path,kind,model FROM axton_mutation_queue_operation WHERE mutation_id=? ORDER BY step",&[json!(call.ordinal)]).unwrap();
    assert_eq!(ops.len(), 2);
    assert_eq!(ops[1]["input_path"], Value::Null);
    assert_eq!(ops[1]["kind"], "effect");
    assert_eq!(ops[1]["model"], "Comment");
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), batch);
    drop(c);
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), s, "User:u").unwrap();
    assert_eq!(
        v05::encode(&c.freeze_batch05().unwrap().unwrap()).unwrap(),
        v05::encode(&batch).unwrap()
    );
}

fn creation_then_sequenced_edit_schema() -> Schema {
    let mut value = serde_json::to_value(schema()).unwrap();
    value["actions"].as_array_mut().unwrap().push(json!({
        "name":"Edit","version":1,
        "inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"update","cardinality":"single"}],
        "outputs":[],
        "sequence":{"after":[{"name":"Write","arguments":{"entries":"entry"}}]}
    }));
    Schema::from_value(value).unwrap()
}

#[test]
fn offline_create_then_sequenced_update_preserves_lifecycle_dependency() {
    let d = tempfile::tempdir().unwrap();
    let s = creation_then_sequenced_edit_schema();
    let mut c =
        Client::open05(SqliteStore::open(d.path().join("db")).unwrap(), s, "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let (created, edited) = c
        .transaction(|tx| {
            let created = tx.submit_mutation05(
                "Write",
                1,
                json!({"entries":[{"id":"e","text":"created","note":null}]}),
                vec![],
            )?;
            let edited = tx.submit_mutation05(
                "Edit",
                1,
                json!({"entry":{"id":"e","text":"edited"}}),
                vec![],
            )?;
            Ok((created, edited))
        })
        .expect("a lifecycle dependency may also match an explicit sequence predecessor");
    assert_eq!(c.status_snapshot05().unwrap()["pending"], 2);
    let batch = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(
        batch.mutations.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![created.ordinal]
    );
    c.acknowledge_batch05(&v05::BatchAcknowledgement {
        context: batch.context.clone(),
        batch_id: batch.batch_id,
        digest: batch.digest.clone(),
        results: vec![v05::MutationResult {
            mutation_id: created.ordinal,
            outcome: v05::MutationOutcome::Rejected {
                code: "write.denied".into(),
                message: None,
            },
        }],
    })
    .unwrap();
    assert!(
        matches!(c.mutation_result05(edited.ordinal).unwrap().unwrap().outcome,
        v05::MutationOutcome::Rejected { ref code, .. } if code == "dependency.rejected")
    );
    assert!(c.call_completion05(&edited.call_id).unwrap().is_some());
    assert!(
        c.read(&axton_client::RecordKey {
            model: "Entry".into(),
            identity: json!({"id":"e"}),
        })
        .unwrap()
        .is_none()
    );
}

#[test]
fn offline_sequenced_update_waits_for_creation_settlement_and_survives_reopen() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("db");
    let s = creation_then_sequenced_edit_schema();
    let mut c = Client::open05(SqliteStore::open(&path).unwrap(), s.clone(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let created = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entries":[{"id":"e","text":"created","note":null}]}),
                vec![],
            )
        })
        .unwrap();
    let edited = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Edit",
                1,
                json!({"entry":{"id":"e","text":"edited"}}),
                vec![],
            )
        })
        .unwrap();
    let key = axton_client::RecordKey {
        model: "Entry".into(),
        identity: json!({"id":"e"}),
    };
    assert_eq!(c.read(&key).unwrap().unwrap()["text"], "edited");
    let batch = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(
        batch.mutations.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![created.ordinal]
    );
    c.acknowledge_batch05(&v05::BatchAcknowledgement {
        context: batch.context.clone(),
        batch_id: batch.batch_id,
        digest: batch.digest.clone(),
        results: vec![v05::MutationResult {
            mutation_id: created.ordinal,
            outcome: v05::MutationOutcome::Accepted {
                sync_cursor: 9,
                result: Value::Null,
                targets: vec![v05::SettlementTarget::Stream {
                    key: v05::RecordKey {
                        model: "Entry".into(),
                        identity: key.identity.clone(),
                    },
                    cursor: 9,
                    fallback: v05::ReadRecord {
                        key: v05::RecordKey {
                            model: "Entry".into(),
                            identity: key.identity.clone(),
                        },
                        cursor: (),
                        state: json!({"text":"created","note":null}),
                    },
                }],
            },
        }],
    })
    .unwrap();
    assert!(
        c.freeze_batch05().unwrap().is_none(),
        "acceptance still waits for creation settlement"
    );
    drop(c);
    let mut c = Client::open05(SqliteStore::open(&path).unwrap(), s, "User:u").unwrap();
    assert_eq!(c.read(&key).unwrap().unwrap()["text"], "edited");
    assert!(
        c.freeze_batch05().unwrap().is_none(),
        "reopen preserves the dependency fence"
    );
    c.install_authority05(
        &batch.context,
        &[v05::AuthorityChange::Record {
            key: v05::RecordKey {
                model: "Entry".into(),
                identity: key.identity.clone(),
            },
            cursor: 9,
            state: json!({"text":"created","note":null}),
        }],
        Some((0, 9)),
    )
    .unwrap();
    assert_eq!(c.read(&key).unwrap().unwrap()["text"], "edited");
    assert!(c.call_completion05(&created.call_id).unwrap().is_some());
    assert!(c.call_completion05(&edited.call_id).unwrap().is_none());
    let next = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(
        next.mutations.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![edited.ordinal]
    );
}

#[test]
fn rejected_predecessor_releases_sequence_survivor_and_rolls_back_lifecycle_dependent() {
    use axton_client::{Operation, OperationKind, RecordKey};
    for reopen in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("db");
        let mut value = serde_json::to_value(schema()).unwrap();
        value["actions"].as_array_mut().unwrap().push(json!({
            "name":"Edit","version":1,
            "inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"update","cardinality":"single"}],
            "outputs":[],
            "sequence":{"after":[{"name":"Write","arguments":{"maybe":"entry"}}]}
        }));
        let s = Schema::from_value(value).unwrap();
        let mut c = Client::open05(SqliteStore::open(&p).unwrap(), s.clone(), "User:u").unwrap();
        c.initialize_stream05(0, "1").unwrap();
        let existing = RecordKey {
            model: "Entry".into(),
            identity: json!({"id":"e"}),
        };
        let created = RecordKey {
            model: "Entry".into(),
            identity: json!({"id":"n"}),
        };
        c.transaction(|tx| {
            tx.direct(Operation {
                model: "Entry".into(),
                op: OperationKind::Create,
                identity: existing.identity.clone(),
                values: Some(json!({"text":"base","note":null})),
            })
        })
        .unwrap();
        let first = c.transaction(|tx| tx.submit_mutation05("Write",1,json!({"entries":[{"id":"n","text":"created","note":null}],"maybe":{"id":"e","text":"first"}}),vec![])).unwrap();
        let survivor = c
            .transaction(|tx| {
                tx.submit_mutation05(
                    "Edit",
                    1,
                    json!({"entry":{"id":"e","text":"survivor"}}),
                    vec![],
                )
            })
            .unwrap();
        let lifecycle = c
            .transaction(|tx| {
                tx.submit_mutation05(
                    "Edit",
                    1,
                    json!({"entry":{"id":"n","text":"dependent"}}),
                    vec![],
                )
            })
            .unwrap();
        let dependencies = c.read_sql("SELECT ordinal,depends_on,kind FROM axton_mutation_dependency ORDER BY ordinal,kind",&[]).unwrap();
        assert_eq!(
            dependencies,
            vec![
                json!({"ordinal":survivor.ordinal,"depends_on":first.ordinal,"kind":"sequence"}),
                json!({"ordinal":lifecycle.ordinal,"depends_on":first.ordinal,"kind":"lifecycle"})
            ]
        );
        let b1 = c.freeze_batch05().unwrap().unwrap();
        assert_eq!(
            b1.mutations.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![first.ordinal]
        );
        let rejection = v05::MutationOutcome::Rejected {
            code: "write.denied".into(),
            message: Some("retained refusal".into()),
        };
        let report = c
            .acknowledge_batch05(&v05::BatchAcknowledgement {
                context: b1.context.clone(),
                batch_id: b1.batch_id,
                digest: b1.digest.clone(),
                results: vec![v05::MutationResult {
                    mutation_id: first.ordinal,
                    outcome: rejection.clone(),
                }],
            })
            .unwrap();
        assert_eq!(report.completions.len(), 2);
        assert!(c.call_completion05(&first.call_id).unwrap().is_some());
        assert!(c.call_completion05(&lifecycle.call_id).unwrap().is_some());
        assert!(c.call_completion05(&survivor.call_id).unwrap().is_none());
        assert_eq!(c.read(&existing).unwrap().unwrap()["text"], "survivor");
        assert!(c.read(&created).unwrap().is_none());
        if reopen {
            drop(c);
            c = Client::open05(SqliteStore::open(&p).unwrap(), s, "User:u").unwrap();
        }
        assert_eq!(
            c.mutation_result05(first.ordinal).unwrap().unwrap().outcome,
            rejection
        );
        assert!(c.call_completion05(&lifecycle.call_id).unwrap().is_some());
        assert!(c.call_completion05(&survivor.call_id).unwrap().is_none());
        assert_eq!(c.read(&existing).unwrap().unwrap()["text"], "survivor");
        assert!(c.read(&created).unwrap().is_none());
        let b2 = c
            .freeze_batch05()
            .unwrap()
            .expect("terminal refusal releases sequence ordering without dismissal");
        assert_eq!(b2.batch_id, 2);
        assert_eq!(
            b2.mutations.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![survivor.ordinal]
        );
        assert_eq!(
            c.mutation_result05(first.ordinal).unwrap().unwrap().outcome,
            rejection
        );
    }
}

#[test]
fn fresh_store_uses_only_canonical_tables_without_compatibility_views_or_triggers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db");
    let mut client = Client::open05(SqliteStore::open(&path).unwrap(), schema(), "User:u").unwrap();
    client.initialize_stream05(0, "1").unwrap();
    let objects = client.read_sql("SELECT name,type FROM sqlite_schema WHERE name IN ('axton_client','axton_mutation','axton_mutation_operation','axton_queue_diverged')", &[]).unwrap();
    assert!(objects.is_empty(), "retired framework objects: {objects:?}");
    assert_eq!(client.pending_count().unwrap(), 0);
    client
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]}), vec![]))
        .unwrap();
    assert_eq!(client.pending_count().unwrap(), 1);
    drop(client);
    let mut client = Client::open05(SqliteStore::open(&path).unwrap(), schema(), "User:u").unwrap();
    assert_eq!(client.pending_count().unwrap(), 1);
    assert_eq!(client.freeze_batch05().unwrap().unwrap().mutations.len(), 1);
}

fn rollover_schema(extra: bool) -> Schema {
    let mut note = vec![
        json!({"name":"id","type":{"kind":"scalar","name":"string"},"nullable":false}),
        json!({"name":"text","type":{"kind":"scalar","name":"string"},"nullable":false}),
        json!({"name":"blob","type":{"kind":"scalar","name":"string"},"nullable":true}),
    ];
    if extra {
        note.push(json!({"name":"label","type":{"kind":"scalar","name":"string"},"nullable":true}));
    }
    Schema::from_value(json!({"enums":[],
        "models":[
            {"name":"Marked","version":1,"identity":["id"],"bootstrap":true,"fields":[
                {"name":"id","type":{"kind":"scalar","name":"string"},"nullable":false},
                {"name":"text","type":{"kind":"scalar","name":"string"},"nullable":false}]},
            {"name":"Note","version":1,"identity":["id"],"fields":note}],
        "prerequisites":[{"name":"RemoteBlob","fields":[{"name":"key","type":"String"}]}],
        "actions":[
            {"name":"Write","version":1,"outputs":[],
             "inputs":[{"kind":"model","name":"note","model":"Note","operation":"update","cardinality":"single"}],
             "requirements":[{"model":"Note","field":"blob","name":"RemoteBlob","arguments":{"key":"self"}}]},
            {"name":"Create","version":1,"outputs":[],
             "inputs":[{"kind":"model","name":"note","model":"Note","operation":"create","cardinality":"single"}]}
        ]}))
    .unwrap()
}
fn note(id: &str) -> axton_client::RecordKey {
    axton_client::RecordKey {
        model: "Note".into(),
        identity: json!({"id":id}),
    }
}
fn wire(model: &str, id: &str) -> v05::RecordKey {
    v05::RecordKey {
        model: model.into(),
        identity: json!({"id":id}),
    }
}
#[test]
fn rollover_drains_frozen_batches_then_clears_only_server_derived_rows() {
    use axton_client::{Operation, OperationKind};
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(
        SqliteStore::open(&p).unwrap(),
        rollover_schema(false),
        "User:u",
    )
    .unwrap();
    c.initialize_stream05(10, "1").unwrap();
    let old = c.request_context05().unwrap();
    c.transaction(|tx| {
        tx.direct(Operation {
            model: "Note".into(),
            identity: json!({"id":"device"}),
            op: OperationKind::Create,
            values: Some(json!({"text":"device","blob":null})),
        })
    })
    .unwrap();
    c.install_cache05(
        &[v05::ReadRecord {
            key: wire("Note", "cached"),
            cursor: (),
            state: json!({"text":"cached","blob":null}),
        }],
        true,
    )
    .unwrap();
    c.install_authority05(
        &old,
        &[
            v05::AuthorityChange::Record {
                key: wire("Marked", "m"),
                cursor: 3,
                state: json!({"text":"marked"}),
            },
            v05::AuthorityChange::Record {
                key: wire("Note", "stream"),
                cursor: 4,
                state: json!({"text":"stream","blob":null}),
            },
            v05::AuthorityChange::Record {
                key: wire("Note", "dirty"),
                cursor: 5,
                state: json!({"text":"old base","blob":null}),
            },
        ],
        None,
    )
    .unwrap();
    // Prerequisite-blocked, so never frozen and never drained.
    let blocked = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"note":{"id":"dirty","blob":"pending"}}),
                vec![],
            )
        })
        .unwrap();
    let frozen = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Create",
                1,
                json!({"note":{"id":"receipt","text":"optimistic","blob":null}}),
                vec![],
            )
        })
        .unwrap();
    let batch = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(
        batch.mutations.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![frozen.ordinal]
    );
    drop(c);

    let mut c = Client::open05(
        SqliteStore::open(&p).unwrap(),
        rollover_schema(true),
        "User:u",
    )
    .unwrap();
    assert!(c.rollover_pending05().unwrap());
    // Hold: the frozen Batch drains in its context; nothing new freezes.
    let held = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Create",
                1,
                json!({"note":{"id":"held","text":"held","blob":null}}),
                vec![],
            )
        })
        .unwrap();
    let before = c.store_status05().unwrap();
    assert!(
        !c.rollover05().unwrap(),
        "a frozen Batch drains before the clear"
    );
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), batch);
    c.acknowledge_batch05(&v05::BatchAcknowledgement {
        context: batch.context.clone(),
        batch_id: batch.batch_id,
        digest: batch.digest.clone(),
        results: vec![v05::MutationResult {
            mutation_id: frozen.ordinal,
            outcome: v05::MutationOutcome::Accepted {
                sync_cursor: 10,
                result: Value::Null,
                targets: vec![v05::SettlementTarget::Stream {
                    key: wire("Note", "receipt"),
                    cursor: 6,
                    fallback: v05::ReadRecord {
                        key: wire("Note", "receipt"),
                        cursor: (),
                        state: json!({"text":"receipt","blob":null}),
                    },
                }],
            },
        }],
    })
    .unwrap();
    assert_eq!(c.settle_ready05().unwrap().completions.len(), 1);
    assert!(
        c.freeze_batch05().unwrap().is_none(),
        "the hold freezes nothing new"
    );

    assert!(c.rollover05().unwrap());
    let after = c.store_status05().unwrap();
    assert!(!c.rollover_pending05().unwrap());
    assert_ne!(after.context.materialization, old.materialization);
    assert_eq!(after.context.store_id, old.store_id);
    assert_eq!(
        (after.start_cursor, after.bootstrap_cursor, after.cursor),
        (None, None, None)
    );
    assert_eq!(
        (
            after.next_mutation_id,
            after.next_local_sequence,
            after.last_acknowledged_batch_id
        ),
        (before.next_mutation_id, before.next_local_sequence, 1)
    );
    let text = |c: &mut Client<SqliteStore>, key: &axton_client::RecordKey| {
        c.read(key).unwrap().map(|row| row["text"].clone())
    };
    assert_eq!(text(&mut c, &note("device")), Some(json!("device")));
    assert_eq!(text(&mut c, &note("cached")), Some(json!("cached")));
    assert_eq!(text(&mut c, &note("receipt")), Some(json!("receipt")));
    assert_eq!(text(&mut c, &note("held")), Some(json!("held")));
    assert_eq!(text(&mut c, &note("stream")), None);
    let marked = axton_client::RecordKey {
        model: "Marked".into(),
        identity: json!({"id":"m"}),
    };
    assert_eq!(text(&mut c, &marked), None);
    // The dirty row keeps its pending layer; only its evidence is gone.
    assert_eq!(c.read(&note("dirty")).unwrap().unwrap()["blob"], "pending");
    assert_eq!(
        c.record_evidence05(&note("dirty")).unwrap(),
        Default::default()
    );

    // Bootstrap again: the next delivery re-bases the dirty row as Newer.
    c.initialize_stream05(20, "1").unwrap();
    let fresh = c.request_context05().unwrap();
    c.install_authority05(
        &fresh,
        &[v05::AuthorityChange::Record {
            key: wire("Note", "dirty"),
            cursor: 21,
            state: json!({"text":"new base","blob":null,"label":"fresh"}),
        }],
        Some((20, 21)),
    )
    .unwrap();
    let dirty = c.read(&note("dirty")).unwrap().unwrap();
    assert_eq!(
        (&dirty["text"], &dirty["blob"], &dirty["label"]),
        (&json!("new base"), &json!("pending"), &json!("fresh"))
    );
    let next = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(
        (next.batch_id, next.context.materialization.as_str()),
        (2, fresh.materialization.as_str())
    );
    assert_eq!(
        next.mutations.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![held.ordinal]
    );
    assert!(c.mutation_result05(blocked.ordinal).unwrap().is_none());
}
#[test]
fn handshake_generation_rolls_a_fresh_store_over_and_reopen_keeps_it() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    let opened = c.request_context05().unwrap();
    let call = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]}), vec![]))
        .unwrap();
    // Nothing is frozen: the clear commits with the handshake's head.
    c.initialize_stream05(5, "2").unwrap();
    let rolled = c.request_context05().unwrap();
    assert_eq!(
        rolled.materialization,
        v05::materialization_id(&schema(), "2").unwrap()
    );
    assert_ne!(rolled.materialization, opened.materialization);
    assert_eq!(rolled.store_id, opened.store_id);
    let status = c.store_status05().unwrap();
    assert_eq!((status.start_cursor, status.cursor), (Some(5), Some(5)));
    let b = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(b.context, rolled);
    assert_eq!(b.mutations[0].id, call.ordinal);
    drop(c);
    // The generation came from the backend; reopening cannot undo it.
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    assert!(!c.rollover_pending05().unwrap());
    assert_eq!(c.request_context05().unwrap(), rolled);
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), b);
}
#[test]
fn reopen_during_drain_keeps_the_reported_generation_desired() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let old = c.request_context05().unwrap();
    c.transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]}), vec![]))
        .unwrap();
    let frozen = c.freeze_batch05().unwrap().unwrap();
    // The backend moved while a Batch is frozen: drain first.
    c.initialize_stream05(0, "2").unwrap();
    assert!(c.rollover_pending05().unwrap());
    assert_eq!(c.request_context05().unwrap(), old);
    drop(c);
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    assert!(c.rollover_pending05().unwrap());
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), frozen);
}
#[test]
fn stale_frozen_digest_halts_the_uplink_once_without_deleting_anything() {
    use axton_client::{
        runtime::{ClientRuntime, Event, Operation},
        sync05::{Control, StoreCommand, StoreReport},
    };
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    c.transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]}), vec![]))
        .unwrap();
    c.freeze_batch05().unwrap().unwrap();
    drop(c);
    // A Batch frozen under the protocol-5 digest definition looks like this.
    let raw = rusqlite::Connection::open(&p).unwrap();
    raw.execute_batch(&format!(
        "DROP TRIGGER axton_queue_envelope; UPDATE axton_mutation_queue SET batch_digest='{}'",
        "0".repeat(64)
    ))
    .unwrap();
    drop(raw);
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    let error = c.freeze_batch05().unwrap_err();
    assert!(matches!(error, axton_client::Error::Terminal(_)), "{error}");
    assert!(error.to_string().contains("reset the Store"));
    let rows = || {
        let raw = rusqlite::Connection::open(&p).unwrap();
        raw.query_row(
            "SELECT COUNT(*),MAX(batch_id),MAX(batch_digest) FROM axton_mutation_queue",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .unwrap()
    };
    let before = rows();
    assert_eq!(before, (1, 1, "0".repeat(64)));

    // The runtime reports it as a halt, and control stops instead of retrying.
    let mut runtime = ClientRuntime::new(c);
    let status = runtime.client().store_status05().unwrap();
    let report = runtime.store_worker05(StoreCommand::Freeze).unwrap();
    assert!(matches!(report, StoreReport::Halted(_)));
    let mut control = Control::new(status);
    control.connect().unwrap();
    control.events();
    control.report(report, 1).unwrap();
    let events = control.events();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Report { .. }))
            .count(),
        1,
        "{events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::Effect {
                operation: Operation::Timer { .. } | Operation::Http { .. },
                ..
            }
        )),
        "a halt schedules no retry: {events:?}"
    );
    control.wake();
    assert!(control.jobs().is_empty());
    assert_eq!(rows(), before);
}
