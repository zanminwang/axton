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
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(b.mutations[0].id, call.ordinal);
    assert_eq!(
        v05::reconstruct_input(&b.mutations[0].operations).unwrap(),
        json!({"entries":[]})
    );
    c.transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[],"maybe":null})))
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
fn frozen_batch_is_resent_with_the_same_digest_in_the_store_context() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    c.transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    let raw = rusqlite::Connection::open(&p).unwrap();
    raw.execute("UPDATE axton_store SET materialization='upgraded'", [])
        .unwrap();
    let resent = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(resent.digest, b.digest);
    assert_eq!(resent.context.materialization, "upgraded");
    assert_eq!(resent.mutations, b.mutations);
}
#[test]
fn no_batch_freezes_before_the_handshake_sets_start() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    let call = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
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
    let call=c.transaction(|tx|{let rolled:axton_client::Result<()>=tx.savepoint(|tx|{tx.submit_mutation05("Write",1,json!({"entries":[{"id":"bad","text":"discarded","note":null}]}))?;Err(axton_client::invalid("rollback"))});assert!(rolled.is_err());tx.submit_mutation05("Write",1,json!({"entries":[{"id":"e1","text":"one","note":null},{"id":"e2","text":"two","note":null}],"maybe":null}))}).unwrap();
    assert_eq!(call.ordinal, 1);
    let b = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(b.mutations[0].operations.len(), 3);
    assert_eq!(
        v05::reconstruct_input(&b.mutations[0].operations).unwrap(),
        json!({"entries":[{"id":"e1","text":"one","note":null},{"id":"e2","text":"two","note":null}],"maybe":null})
    );
}
#[test]
fn a_changed_materialization_resends_the_frozen_batch_with_its_digest_in_the_new_context() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    c.transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    drop(c);
    let mut newer = schema();
    newer.models[0].bootstrap = !newer.models[0].bootstrap;
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), newer.clone(), "User:u").unwrap();
    let upgraded = c.request_context05().unwrap();
    assert_eq!(
        upgraded.materialization,
        v05::materialization_id(&newer, "1").unwrap()
    );
    assert_ne!(upgraded.materialization, b.context.materialization);
    let status = c.status_snapshot05().unwrap();
    assert!(status.get("schema").is_none());
    assert_eq!(status["context"], serde_json::to_value(&upgraded).unwrap());
    assert_eq!(status["pending"], 1);
    let resent = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(
        (&resent.digest, &resent.mutations),
        (&b.digest, &b.mutations)
    );
    assert_eq!(resent.context, upgraded);
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
    c.transaction(|tx| tx.submit_mutation05("Arg", 1, json!({"label":null,"tags":["a","b"]})))
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
            )
        })
        .unwrap();
    let independent = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
        .unwrap();
    let a = v05::BatchAcknowledgement {
        context: b.context.clone(),
        batch_id: b.batch_id,
        digest: b.digest.clone(),
        results: vec![v05::MutationResult {
            mutation_id: first.ordinal,
            outcome: v05::MutationOutcome::Accepted {
                sync_cursor: 9,
                materialization: b.context.materialization.clone(),
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
    c.transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
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
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
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
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
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
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    let later = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
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
            "UPDATE axton_mutation_queue SET batch_id=1 WHERE id=?",
            [later.ordinal as i64]
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
    assert!(c.transaction(|tx|tx.submit_mutation05("Pair",1,json!({"first":{"id":"e","text":"one","note":null},"second":{"id":"other","text":"two"}}))).is_err());
    assert_eq!(c.store_status05().unwrap().next_mutation_id, 1);
    c.transaction(|tx| {
        tx.submit_mutation05(
            "Pair",
            1,
            json!({"first":{"id":"e","text":"one","note":null},"second":{"id":"e","text":"two"}}),
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
            json!({"entries":[{"id":"e","text":"one","note":null}]})
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
    use axton_client::RecordKey;
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
    cache_row(&mut c, "Book", "b", json!({"title":"book"}));
    let call = c
        .transaction(|tx| tx.submit_mutation05("DeleteBook", 1, json!({"book":{"id":"b"}})))
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

fn cache_row(c: &mut Client<SqliteStore>, model: &str, id: &str, state: Value) {
    let report = c
        .install_cache05(
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
        .unwrap();
    assert_eq!(report.applied, 1, "{model} {id} installed as cache");
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
            )?;
            let edited =
                tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"edited"}}))?;
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
            )
        })
        .unwrap();
    let edited = c
        .transaction(|tx| {
            tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"edited"}}))
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
                materialization: batch.context.materialization.clone(),
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
    use axton_client::RecordKey;
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
        cache_row(&mut c, "Entry", "e", json!({"text":"base","note":null}));
        let first = c.transaction(|tx| tx.submit_mutation05("Write",1,json!({"entries":[{"id":"n","text":"created","note":null}],"maybe":{"id":"e","text":"first"}}))).unwrap();
        let survivor = c
            .transaction(|tx| {
                tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"survivor"}}))
            })
            .unwrap();
        let lifecycle = c
            .transaction(|tx| {
                tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"n","text":"dependent"}}))
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
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
        .unwrap();
    assert_eq!(client.pending_count().unwrap(), 1);
    drop(client);
    let mut client = Client::open05(SqliteStore::open(&path).unwrap(), schema(), "User:u").unwrap();
    assert_eq!(client.pending_count().unwrap(), 1);
    assert_eq!(client.freeze_batch05().unwrap().unwrap().mutations.len(), 1);
}

fn upgrade_schema(extra: bool) -> Schema {
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
fn an_upgrade_keeps_every_row_and_the_queue_and_freezes_at_once() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(
        SqliteStore::open(&p).unwrap(),
        upgrade_schema(false),
        "User:u",
    )
    .unwrap();
    c.initialize_stream05(10, "1").unwrap();
    let old = c.request_context05().unwrap();
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
    // Prerequisite-blocked, so never frozen.
    let blocked = c
        .transaction(|tx| {
            tx.submit_mutation05("Write", 1, json!({"note":{"id":"dirty","blob":"pending"}}))
        })
        .unwrap();
    let frozen = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Create",
                1,
                json!({"note":{"id":"receipt","text":"optimistic","blob":null}}),
            )
        })
        .unwrap();
    let batch = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(
        batch.mutations.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![frozen.ordinal]
    );
    let before = c.store_status05().unwrap();
    drop(c);

    let mut c = Client::open05(
        SqliteStore::open(&p).unwrap(),
        upgrade_schema(true),
        "User:u",
    )
    .unwrap();
    let after = c.store_status05().unwrap();
    assert_ne!(after.context.materialization, old.materialization);
    assert_eq!(after.context.store_id, old.store_id);
    assert_eq!(
        (after.start_cursor, after.bootstrap_cursor, after.cursor),
        (Some(10), None, Some(10))
    );
    assert_eq!(
        (
            after.next_mutation_id,
            after.next_local_sequence,
            after.last_acknowledged_batch_id
        ),
        (
            before.next_mutation_id,
            before.next_local_sequence,
            before.last_acknowledged_batch_id
        )
    );
    let text = |c: &mut Client<SqliteStore>, key: &axton_client::RecordKey| {
        c.read(key).unwrap().map(|row| row["text"].clone())
    };
    for id in ["cached", "stream", "receipt"] {
        assert!(text(&mut c, &note(id)).is_some(), "{id}");
    }
    let marked = axton_client::RecordKey {
        model: "Marked".into(),
        identity: json!({"id":"m"}),
    };
    assert_eq!(text(&mut c, &marked), Some(json!("marked")));
    assert_eq!(c.read(&note("dirty")).unwrap().unwrap()["blob"], "pending");
    // The frozen Batch is sent again in the new contract; nothing waits.
    let resent = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(
        (&resent.digest, &resent.mutations),
        (&batch.digest, &batch.mutations)
    );
    assert_eq!(resent.context, after.context);
    c.acknowledge_batch05(&v05::BatchAcknowledgement {
        context: resent.context.clone(),
        batch_id: resent.batch_id,
        digest: resent.digest.clone(),
        results: vec![v05::MutationResult {
            mutation_id: frozen.ordinal,
            outcome: v05::MutationOutcome::Accepted {
                sync_cursor: 10,
                materialization: resent.context.materialization.clone(),
                result: Value::Null,
                targets: vec![v05::SettlementTarget::Stream {
                    key: wire("Note", "receipt"),
                    cursor: 6,
                    fallback: v05::ReadRecord {
                        key: wire("Note", "receipt"),
                        cursor: (),
                        state: json!({"text":"receipt","blob":null,"label":null}),
                    },
                }],
            },
        }],
    })
    .unwrap();
    assert_eq!(c.settle_ready05().unwrap().completions.len(), 1);
    assert_eq!(text(&mut c, &note("receipt")), Some(json!("receipt")));
    let next = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Create",
                1,
                json!({"note":{"id":"later","text":"later","blob":null,"label":null}}),
            )
        })
        .unwrap();
    let following = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(following.batch_id, 2);
    assert_eq!(
        following.mutations.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![next.ordinal]
    );
    assert!(c.mutation_result05(blocked.ordinal).unwrap().is_none());
}
#[test]
fn handshake_generation_upgrades_a_fresh_store_and_reopen_keeps_it() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    let opened = c.request_context05().unwrap();
    let call = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
        .unwrap();
    // A fresh Store has no C: the upgrade adopts the contract and the
    // handshake then commits its head.
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
    assert_eq!(c.request_context05().unwrap(), rolled);
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), b);
}
#[test]
fn a_generation_reported_with_a_frozen_batch_upgrades_at_once_and_reopen_keeps_it() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    c.transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
        .unwrap();
    let frozen = c.freeze_batch05().unwrap().unwrap();
    c.initialize_stream05(0, "2").unwrap();
    let upgraded = c.request_context05().unwrap();
    assert_eq!(
        upgraded.materialization,
        v05::materialization_id(&schema(), "2").unwrap()
    );
    drop(c);
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema(), "User:u").unwrap();
    assert_eq!(c.request_context05().unwrap(), upgraded);
    let resent = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(
        (&resent.digest, &resent.context),
        (&frozen.digest, &upgraded)
    );
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
    c.transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entries":[]})))
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

/// How Model `A` changed since a queued Mutation was enqueued; `B` is unchanged.
#[derive(Clone, Copy, Debug)]
enum Change {
    FieldType,
    Identity,
    Removed,
}
fn pair_schema(change: Option<Change>) -> Schema {
    let text =
        |name: &str| json!({"name":name,"nullable":false,"type":{"kind":"scalar","name":"string"}});
    let a = match change {
        None => Some(json!({"name":"A","identity":["id"],"fields":[text("id"),text("title")]})),
        Some(Change::FieldType) => Some(
            json!({"name":"A","identity":["id"],"fields":[text("id"),{"name":"title","nullable":false,"type":{"kind":"scalar","name":"int"}}]}),
        ),
        Some(Change::Identity) => {
            Some(json!({"name":"A","identity":["id","title"],"fields":[text("id"),text("title")]}))
        }
        Some(Change::Removed) => None,
    };
    let b = json!({"name":"B","identity":["id"],"fields":[text("id"),text("title")]});
    let models: Vec<Value> = a.into_iter().chain([b]).collect();
    let touch = json!({"name":"Touch","version":1,"inputs":[
        {"kind":"model","name":"b","model":"B","operation":"update","cardinality":"single","fields":["title"]}],"outputs":[]});
    let actions = if change.is_none() {
        json!([touch, {"name":"Pair","version":1,"inputs":[
            {"kind":"model","name":"a","model":"A","operation":"update","cardinality":"single","fields":["title"]},
            {"kind":"model","name":"b","model":"B","operation":"update","cardinality":"single","fields":["title"]}],"outputs":[]}])
    } else {
        json!([touch])
    };
    Schema::from_value(json!({"enums":[],"models":models,"actions":actions})).unwrap()
}
/// A Store whose queue holds `Pair` (updates of `a` and `b`) enqueued under
/// the old schema, reopened with the schema `change` made.
fn queued_across(
    change: Change,
    freeze: bool,
) -> (
    tempfile::TempDir,
    Client<SqliteStore>,
    Option<v05::MutationRequest>,
) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c =
        Client::open05(SqliteStore::open(&p).unwrap(), pair_schema(None), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    cache_row(&mut c, "A", "a", json!({"title":"A0"}));
    cache_row(&mut c, "B", "b", json!({"title":"B0"}));
    c.transaction(|tx| {
        tx.submit_mutation05(
            "Pair",
            1,
            json!({"a":{"id":"a","title":"A1"},"b":{"id":"b","title":"B1"}}),
        )
    })
    .unwrap();
    let batch = freeze.then(|| c.freeze_batch05().unwrap().unwrap());
    drop(c);
    let c = Client::open05(
        SqliteStore::open(&p).unwrap(),
        pair_schema(Some(change)),
        "User:u",
    )
    .unwrap();
    (d, c, batch)
}
fn pair_key(model: &str, id: &str) -> axton_client::RecordKey {
    axton_client::RecordKey {
        model: model.into(),
        identity: json!({ "id": id }),
    }
}
fn stream(c: &mut Client<SqliteStore>, model: &str, id: &str, cursor: u64, title: Value) {
    let context = c.request_context05().unwrap();
    c.install_authority05(
        &context,
        &[v05::AuthorityChange::Record {
            key: wire(model, id),
            cursor,
            state: json!({ "title": title }),
        }],
        None,
    )
    .unwrap();
}
const CHANGES: [Change; 3] = [Change::FieldType, Change::Identity, Change::Removed];
#[test]
fn replay_skips_an_operation_on_a_model_that_is_not_shared_and_marks_the_mutation_diverged() {
    for change in CHANGES {
        let (_d, mut c, _) = queued_across(change, false);
        // The reset Model shows no optimistic row.
        if !matches!(change, Change::Removed) {
            assert!(c.query("A", &json!({})).unwrap().is_empty(), "{change:?}");
        }
        stream(&mut c, "B", "b", 1, json!("server"));
        assert_eq!(
            c.read(&pair_key("B", "b")).unwrap().unwrap()["title"],
            "B1",
            "{change:?}"
        );
        let status = c.record_status(&pair_key("B", "b")).unwrap();
        assert_eq!(status["pending"][0]["diverged"], true, "{change:?}");
        // It still uploads as enqueued.
        let batch = c.freeze_batch05().unwrap().unwrap();
        assert_eq!(batch.mutations[0].name, "Pair", "{change:?}");
        // A later Mutation derives its dependencies past the skipped operation.
        c.transaction(|tx| tx.submit_mutation05("Touch", 1, json!({"b":{"id":"b","title":"B2"}})))
            .unwrap_or_else(|e| panic!("{change:?}: {e}"));
        assert_eq!(c.read(&pair_key("B", "b")).unwrap().unwrap()["title"], "B2");
        if let Change::FieldType = change {
            stream(&mut c, "A", "a", 1, json!(5));
            assert_eq!(c.read(&pair_key("A", "a")).unwrap().unwrap()["title"], 5);
        }
    }
}
fn private(model: &str, id: &str, title: &str) -> v05::SettlementTarget {
    v05::SettlementTarget::Private {
        record: v05::ReadRecord {
            key: wire(model, id),
            cursor: (),
            state: json!({ "title": title }),
        },
    }
}
fn answer(b: &v05::MutationRequest, outcome: v05::MutationOutcome) -> v05::BatchAcknowledgement {
    v05::BatchAcknowledgement {
        context: b.context.clone(),
        batch_id: b.batch_id,
        digest: b.digest.clone(),
        results: vec![v05::MutationResult {
            mutation_id: b.mutations[0].id,
            outcome,
        }],
    }
}
#[test]
fn acknowledgement_needs_no_target_for_a_model_that_is_not_shared() {
    for change in CHANGES {
        let (_d, mut c, b) = queued_across(change, true);
        let b = b.unwrap();
        // The receipt answers in the Store's current contract.
        let current = c.request_context05().unwrap().materialization;
        c.acknowledge_batch05(&answer(
            &b,
            v05::MutationOutcome::Accepted {
                sync_cursor: 0,
                materialization: current.clone(),
                result: Value::Null,
                targets: vec![private("B", "b", "accepted")],
            },
        ))
        .unwrap_or_else(|e| panic!("{change:?}: {e}"));
        assert_eq!(
            c.settle_ready05().unwrap().completions.len(),
            1,
            "{change:?}"
        );
        assert_eq!(
            c.read(&pair_key("B", "b")).unwrap().unwrap()["title"],
            "accepted"
        );
        c.transaction(|tx| tx.submit_mutation05("Touch", 1, json!({"b":{"id":"b","title":"B2"}})))
            .unwrap();
        assert_eq!(
            c.freeze_batch05().unwrap().unwrap().batch_id,
            2,
            "{change:?}"
        );
    }
}
#[test]
fn rejection_of_a_mutation_on_a_model_that_is_not_shared_completes() {
    for change in CHANGES {
        let (_d, mut c, b) = queued_across(change, true);
        let b = b.unwrap();
        let call = format!("{}:{}", c.client_id(), b.mutations[0].id);
        c.acknowledge_batch05(&answer(
            &b,
            v05::MutationOutcome::Rejected {
                code: "pair.denied".into(),
                message: None,
            },
        ))
        .unwrap_or_else(|e| panic!("{change:?}: {e}"));
        assert!(c.call_completion05(&call).unwrap().is_some(), "{change:?}");
        assert_eq!(c.read(&pair_key("B", "b")).unwrap().unwrap()["title"], "B0");
        assert_eq!(c.store_status05().unwrap().last_acknowledged_batch_id, 1);
    }
}
#[test]
fn settlement_neither_awaits_nor_installs_a_target_that_is_not_shared() {
    for change in CHANGES {
        let (_d, mut c, b) = queued_across(change, true);
        let b = b.unwrap();
        // The receipt answers in the Store's current contract.
        let current = c.request_context05().unwrap().materialization;
        c.acknowledge_batch05(&answer(
            &b,
            v05::MutationOutcome::Accepted {
                sync_cursor: 5,
                materialization: current.clone(),
                result: Value::Null,
                targets: vec![
                    v05::SettlementTarget::Stream {
                        key: wire("A", "a"),
                        cursor: 5,
                        fallback: v05::ReadRecord {
                            key: wire("A", "a"),
                            cursor: (),
                            state: json!({"title":"A5"}),
                        },
                    },
                    private("B", "b", "accepted"),
                ],
            },
        ))
        .unwrap_or_else(|e| panic!("{change:?}: {e}"));
        let context = c.request_context05().unwrap();
        let covered = c.install_authority05(&context, &[], Some((0, 5))).unwrap();
        assert_eq!(covered.completions.len(), 1, "{change:?}");
        assert_eq!(
            c.read(&pair_key("B", "b")).unwrap().unwrap()["title"],
            "accepted"
        );
        if let Change::FieldType = change {
            assert!(c.read(&pair_key("A", "a")).unwrap().is_none());
        }
    }
}
/// `Pair` updates `A` and `B`; `Touch` updates `B` and is sequenced after a
/// `Pair` on the same `B`. `A`'s identity is `identity`.
fn sequenced_schema(identity: &[&str]) -> Schema {
    let text =
        |name: &str| json!({"name":name,"nullable":false,"type":{"kind":"scalar","name":"string"}});
    let a =
        json!({"name":"A","identity":identity,"fields":[text("id"),text("code"),text("title")]});
    let b = json!({"name":"B","identity":["id"],"fields":[text("id"),text("title")]});
    Schema::from_value(json!({"enums":[],"models":[a,b],"actions":[
        {"name":"Pair","version":1,"inputs":[
            {"kind":"model","name":"a","model":"A","operation":"update","cardinality":"single","fields":["title"]},
            {"kind":"model","name":"b","model":"B","operation":"update","cardinality":"single","fields":["title"]}],"outputs":[]},
        {"name":"Touch","version":1,"inputs":[
            {"kind":"model","name":"b","model":"B","operation":"update","cardinality":"single","fields":["title"]}],"outputs":[],
            "sequence":{"after":[{"name":"Pair","arguments":{"b":"b"}}]}}]}))
    .unwrap()
}
#[test]
fn an_action_sequenced_after_a_diverged_mutation_can_be_enqueued() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(
        SqliteStore::open(&p).unwrap(),
        sequenced_schema(&["id"]),
        "User:u",
    )
    .unwrap();
    c.initialize_stream05(0, "1").unwrap();
    cache_row(&mut c, "A", "a", json!({"code":"c","title":"A0"}));
    cache_row(&mut c, "B", "b", json!({"title":"B0"}));
    let pair = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Pair",
                1,
                json!({"a":{"id":"a","title":"A1"},"b":{"id":"b","title":"B1"}}),
            )
        })
        .unwrap();
    // Before the upgrade the sequence edge is derived from Pair's input.
    let first = c
        .transaction(|tx| tx.submit_mutation05("Touch", 1, json!({"b":{"id":"b","title":"B2"}})))
        .unwrap();
    let raw = rusqlite::Connection::open(&p).unwrap();
    let edges = |raw: &rusqlite::Connection, ordinal: u64| -> Vec<i64> {
        let mut s = raw
            .prepare("SELECT depends_on FROM axton_mutation_dependency WHERE ordinal=? AND kind='sequence'")
            .unwrap();
        s.query_map([ordinal as i64], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(edges(&raw, first.ordinal), vec![pair.ordinal as i64]);
    drop(c);
    // `A`'s identity now needs `code`, which Pair's input never carried.
    let mut c = Client::open05(
        SqliteStore::open(&p).unwrap(),
        sequenced_schema(&["id", "code"]),
        "User:u",
    )
    .unwrap();
    let after = c
        .transaction(|tx| tx.submit_mutation05("Touch", 1, json!({"b":{"id":"b","title":"B3"}})))
        .expect("a prior that is not shared takes no part in sequence derivation");
    assert!(edges(&raw, after.ordinal).is_empty());
    assert_eq!(c.read(&pair_key("B", "b")).unwrap().unwrap()["title"], "B3");
}
#[test]
fn a_create_queued_before_a_new_required_field_uploads_and_shows_no_optimistic_row() {
    // `rank` has a creation default, as `@default` compiles; no storage default.
    let entry = |rank: bool| {
        let text = |name: &str| json!({"name":name,"nullable":false,"type":{"kind":"scalar","name":"string"}});
        let mut fields = vec![text("id"), text("text")];
        if rank {
            fields.push(json!({"name":"rank","nullable":false,"type":{"kind":"scalar","name":"int"},"createDefault":{"kind":"literal","value":0}}));
        }
        Schema::from_value(json!({"enums":[],"models":[{"name":"Entry","identity":["id"],"fields":fields}],
            "actions":[{"name":"Add","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"create","cardinality":"single"}],"outputs":[]}]}))
        .unwrap()
    };
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), entry(false), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let call = c
        .transaction(|tx| tx.submit_mutation05("Add", 1, json!({"entry":{"id":"e","text":"A"}})))
        .unwrap();
    let key = axton_client::RecordKey {
        model: "Entry".into(),
        identity: json!({"id":"e"}),
    };
    assert_eq!(c.read(&key).unwrap().unwrap()["text"], "A");
    let before = c.freeze_batch05().unwrap().unwrap();
    drop(c);
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), entry(true), "User:u").unwrap();
    // The Model was reset: its table has the new column and no rows.
    assert!(
        c.read_sql("PRAGMA table_info(Entry)", &[])
            .unwrap()
            .iter()
            .any(|r| r["name"] == "rank")
    );
    assert!(c.query("Entry", &json!({})).unwrap().is_empty());
    assert_eq!(c.read(&key).unwrap(), None);
    // It still uploads as enqueued.
    let resent = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(resent.digest, before.digest);
    assert_eq!(resent.mutations[0].id, call.ordinal);
    assert_eq!(
        v05::reconstruct_input(&resent.mutations[0].operations).unwrap(),
        json!({"entry":{"id":"e","text":"A"}})
    );
}
#[test]
fn the_shared_check_parses_each_retained_descriptor_model_once_per_open() {
    let (_d, mut c, _) = queued_across(Change::FieldType, false);
    stream(&mut c, "B", "b", 1, json!("first"));
    let parsed = c.shared_check_parses();
    assert!(
        parsed > 0,
        "the first rebuild of a dirty row checks sharing"
    );
    // Later rebuilds, reads and status run in new Engines on the same Client.
    stream(&mut c, "B", "b", 2, json!("second"));
    c.record_status(&pair_key("B", "b")).unwrap();
    c.transaction(|tx| tx.submit_mutation05("Touch", 1, json!({"b":{"id":"b","title":"B2"}})))
        .unwrap();
    assert_eq!(
        c.shared_check_parses(),
        parsed,
        "a second rebuild parses nothing"
    );
}
