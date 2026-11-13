use axton_client::{Client, RecordKey, Schema, wire};
use axton_sqlite::SqliteStore;
use serde_json::{Value, json};
fn schema() -> Schema {
    let mut s: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    s["actions"] = json!([
        {"name":"Write","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"create","cardinality":"single"}],"outputs":[]},
        {"name":"Edit","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"update","cardinality":"single"}],"outputs":[]}
    ]);
    Schema::from_value(s).unwrap()
}
fn open(p: &std::path::Path) -> Client<SqliteStore> {
    Client::open05(SqliteStore::open(p).unwrap(), schema(), "User:u").unwrap()
}
fn key() -> RecordKey {
    RecordKey {
        model: "Entry".into(),
        identity: json!({"id":"e"}),
    }
}
// Later local work on the record: an `Edit` queued after the frozen Batch
// and never sent. Its ordinal discards it.
fn later(c: &mut Client<SqliteStore>, text: &str) -> u64 {
    c.transaction(|tx| tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":text}})))
        .unwrap()
        .ordinal
}
fn discard(c: &mut Client<SqliteStore>, ordinal: u64) {
    c.transaction(|tx| tx.discard_mutation05(ordinal)).unwrap();
}
fn ack(b: &wire::MutationRequest, outcome: wire::MutationOutcome) -> wire::BatchAcknowledgement {
    wire::BatchAcknowledgement {
        context: b.context.clone(),
        batch_id: b.batch_id,
        digest: b.digest.clone(),
        results: vec![wire::MutationResult {
            mutation_id: b.mutations[0].id,
            outcome,
        }],
    }
}
#[test]
fn rejected_owner_completes_after_reopen_beneath_a_later_pending_mutation() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = open(&p);
    c.initialize_stream05(0, "1").unwrap();
    let call = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"A","note":null}}),
            )
        })
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    later(&mut c, "B");
    let a = ack(
        &b,
        wire::MutationOutcome::Rejected {
            code: "write.denied".into(),
            message: None,
        },
    );
    c.acknowledge_batch05(&a).unwrap();
    // The later Edit of the refused creation is its lifecycle dependent.
    assert_eq!(c.read(&key()).unwrap(), None);
    assert_eq!(c.pending_count().unwrap(), 0);
    drop(c);
    let mut c = open(&p);
    assert!(c.call_completion05(&call.call_id).unwrap().is_some());
    assert!(c.freeze_batch05().unwrap().is_none());
    c.acknowledge_batch05(&a).unwrap();
}
#[test]
fn accepted_private_result_waits_for_coverage_beneath_a_later_pending_mutation() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = open(&p);
    c.initialize_stream05(0, "1").unwrap();
    let call = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"A","note":null}}),
            )
        })
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    later(&mut c, "B");
    let a = ack(
        &b,
        wire::MutationOutcome::Accepted {
            sync_cursor: 1,
            materialization: b.context.materialization.clone(),
            result: Value::Null,
            targets: vec![wire::SettlementTarget::Private {
                record: wire::ReadRecord {
                    key: wire::RecordKey {
                        model: "Entry".into(),
                        identity: json!({"id":"e"}),
                    },
                    cursor: (),
                    state: json!({"text":"accepted","note":null}),
                },
            }],
        },
    );
    c.acknowledge_batch05(&a).unwrap();
    assert!(c.call_completion05(&call.call_id).unwrap().is_none());
    assert!(c.settle_ready05().unwrap().completions.is_empty());
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    let context = c.request_context05().unwrap();
    c.install_authority05(&context, &[], Some((0, 1))).unwrap();
    c.settle_ready05().unwrap();
    assert!(c.call_completion05(&call.call_id).unwrap().is_some());
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    assert_eq!(c.pending_count().unwrap(), 1);
}
fn cache_row(c: &mut Client<SqliteStore>, model: &str, id: &str, state: Value) {
    let report = c
        .install_cache05(
            &[wire::ReadRecord {
                key: wire::RecordKey {
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
fn stream_key(id: &str) -> wire::RecordKey {
    wire::RecordKey {
        model: "Entry".into(),
        identity: json!({"id":id}),
    }
}
fn state(text: &str) -> Value {
    json!({"text":text,"note":null})
}
fn install(c: &mut Client<SqliteStore>, id: &str, cursor: u64, text: Option<&str>) {
    let context = c.request_context05().unwrap();
    c.install_authority05(
        &context,
        &[wire::AuthorityChange::Record {
            key: stream_key(id),
            cursor,
            state: text.map(state).unwrap_or(Value::Null),
        }],
        None,
    )
    .unwrap();
}
#[test]
fn newer_stream_supersedes_private_fallback_beneath_a_later_pending_mutation() {
    let d = tempfile::tempdir().unwrap();
    let mut c = open(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let call = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"A","note":null}}),
            )
        })
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    install(&mut c, "e", 5, Some("server"));
    let edit = later(&mut c, "B");
    let a = ack(
        &b,
        wire::MutationOutcome::Accepted {
            sync_cursor: 5,
            materialization: b.context.materialization.clone(),
            result: Value::Null,
            targets: vec![wire::SettlementTarget::Private {
                record: wire::ReadRecord {
                    key: stream_key("e"),
                    cursor: (),
                    state: state("old-private"),
                },
            }],
        },
    );
    c.acknowledge_batch05(&a).unwrap();
    let context = c.request_context05().unwrap();
    c.install_authority05(&context, &[], Some((0, 5))).unwrap();
    c.settle_ready05().unwrap();
    assert!(c.call_completion05(&call.call_id).unwrap().is_some());
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    // Beneath the later Edit, the Stream's newer state stayed the base.
    discard(&mut c, edit);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "server");
}
// Two Edits of one cached record share a Batch. The later one's Private
// receipt settles first, while the earlier one waits for the Stream: its accepted
// state is retained at its own position above the earlier pending Edit and
// outlives its settlement, until the earlier Edit settles beneath it.
#[test]
fn an_accepted_state_outlives_its_settlement_above_an_earlier_pending_mutation() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = open(&p);
    c.initialize_stream05(0, "1").unwrap();
    cache_row(&mut c, "Entry", "e", state("base"));
    let first = later(&mut c, "first");
    let second = later(&mut c, "second");
    let b = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(
        b.mutations.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![first, second]
    );
    let accepted = |sync_cursor, target| wire::MutationOutcome::Accepted {
        sync_cursor,
        materialization: b.context.materialization.clone(),
        result: Value::Null,
        targets: vec![target],
    };
    c.acknowledge_batch05(&wire::BatchAcknowledgement {
        context: b.context.clone(),
        batch_id: b.batch_id,
        digest: b.digest.clone(),
        results: vec![
            wire::MutationResult {
                mutation_id: first,
                outcome: accepted(
                    5,
                    wire::SettlementTarget::Stream {
                        key: stream_key("e"),
                        cursor: 5,
                        fallback: wire::ReadRecord {
                            key: stream_key("e"),
                            cursor: (),
                            state: state("first fallback"),
                        },
                    },
                ),
            },
            wire::MutationResult {
                mutation_id: second,
                outcome: accepted(
                    1,
                    wire::SettlementTarget::Private {
                        record: wire::ReadRecord {
                            key: stream_key("e"),
                            cursor: (),
                            state: state("second accepted"),
                        },
                    },
                ),
            },
        ],
    })
    .unwrap();
    axton_sim::scenario05::assert_projection05(&mut c, &schema());
    let context = c.request_context05().unwrap();
    c.install_authority05(&context, &[], Some((0, 1))).unwrap();
    c.settle_ready05().unwrap();
    let retained = "SELECT COUNT(*) AS n FROM axton_local_write WHERE disposition='accepted'";
    assert_eq!(c.pending_count().unwrap(), 1);
    assert_eq!(c.read_sql(retained, &[]).unwrap()[0]["n"], 1);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "second accepted");
    axton_sim::scenario05::assert_projection05(&mut c, &schema());
    drop(c);
    let mut c = open(&p);
    axton_sim::scenario05::assert_projection05(&mut c, &schema());
    install(&mut c, "e", 5, Some("first server"));
    let context = c.request_context05().unwrap();
    c.install_authority05(&context, &[], Some((1, 5))).unwrap();
    c.settle_ready05().unwrap();
    assert_eq!(c.pending_count().unwrap(), 0);
    assert_eq!(c.read_sql(retained, &[]).unwrap()[0]["n"], 0);
    axton_sim::scenario05::assert_projection05(&mut c, &schema());
}
#[test]
fn rejected_update_keeps_a_later_pending_mutation_on_the_protected_base() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut s = schema();
    let mut value = serde_json::to_value(&s).unwrap();
    value["actions"][0]["inputs"][0]["operation"] = json!("update");
    s = Schema::from_value(value).unwrap();
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), s.clone(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    install(&mut c, "e", 3, Some("base"));
    let call = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entry":{"id":"e","text":"A"}})))
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    later(&mut c, "B");
    let a = ack(
        &b,
        wire::MutationOutcome::Rejected {
            code: "write.denied".into(),
            message: Some("No".into()),
        },
    );
    c.acknowledge_batch05(&a).unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    assert_eq!(
        c.record_evidence05(&key()).unwrap().current.unwrap().cursor,
        3
    );
    assert_eq!(c.pending_count().unwrap(), 1);
    drop(c);
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), s, "User:u").unwrap();
    c.acknowledge_batch05(&a).unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    assert!(c.call_completion05(&call.call_id).unwrap().is_some());
}
#[test]
fn private_acceptance_finalizes_at_original_position_beneath_a_later_pending_mutation() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = open(&p);
    c.initialize_stream05(0, "1").unwrap();
    let call = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"A","note":null}}),
            )
        })
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    let edit = later(&mut c, "B");
    let a = ack(
        &b,
        wire::MutationOutcome::Accepted {
            sync_cursor: 0,
            materialization: b.context.materialization.clone(),
            result: Value::Null,
            targets: vec![wire::SettlementTarget::Private {
                record: wire::ReadRecord {
                    key: stream_key("e"),
                    cursor: (),
                    state: state("accepted"),
                },
            }],
        },
    );
    c.acknowledge_batch05(&a).unwrap();
    let context = c.request_context05().unwrap();
    c.install_authority05(&context, &[], Some((0, 0))).unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    assert!(c.call_completion05(&call.call_id).unwrap().is_some());
    drop(c);
    let mut c = open(&p);
    assert!(c.call_completion05(&call.call_id).unwrap().is_some());
    c.acknowledge_batch05(&a).unwrap();
    let count = |sql: &str| {
        rusqlite::Connection::open(&p)
            .unwrap()
            .query_row(sql, [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    let pending = "SELECT COUNT(*) FROM axton_mutation_queue_operation o JOIN axton_mutation_queue q ON q.id=o.mutation_id WHERE o.model IS NOT NULL AND q.reconciled=0 AND q.rejection_code IS NULL";
    let accepted = "SELECT COUNT(*) FROM axton_local_write WHERE disposition='accepted'";
    // No earlier pending work needs the receipt's state at its position, so
    // it became the base beneath the Edit.
    assert_eq!((count(pending), count(accepted)), (1, 0));
    discard(&mut c, edit);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "accepted");
    assert_eq!((count(pending), count(accepted)), (0, 0));
}
use axton_client::{ClientStore, Result, SqlRows, invalid};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
struct FaultStore {
    inner: SqliteStore,
    fail_commit: Arc<AtomicBool>,
    crash_ack: bool,
}
impl ClientStore for FaultStore {
    fn begin(&mut self) -> Result<()> {
        self.inner.begin()
    }
    fn commit(&mut self) -> Result<()> {
        if self.fail_commit.swap(false, Ordering::SeqCst) {
            Err(invalid("injected commit failure"))
        } else {
            self.inner.commit()
        }
    }
    fn rollback(&mut self) -> Result<()> {
        self.inner.rollback()
    }
    fn savepoint(&mut self, n: &str) -> Result<()> {
        self.inner.savepoint(n)
    }
    fn release(&mut self, n: &str) -> Result<()> {
        self.inner.release(n)
    }
    fn rollback_to(&mut self, n: &str) -> Result<()> {
        self.inner.rollback_to(n)
    }
    fn execute(&mut self, s: &str, p: &[Value]) -> Result<usize> {
        let r = self.inner.execute(s, p)?;
        if std::env::var("AXTON_TASK2_ASSIGNMENT_CRASH")
            .ok()
            .as_deref()
            == Some("before")
            && s == "UPDATE axton_mutation_queue SET batch_digest=? WHERE batch_id=?"
        {
            std::fs::write(
                std::env::var("AXTON_TASK2_ASSIGNMENT_EVIDENCE").unwrap(),
                p[0].as_str().unwrap(),
            )
            .unwrap();
            std::process::exit(80)
        }
        if self.crash_ack && s == "UPDATE axton_store SET last_acknowledged_batch_id=?" {
            std::process::exit(77)
        }
        Ok(r)
    }
    fn execute_batch(&mut self, s: &str) -> Result<()> {
        self.inner.execute_batch(s)
    }
    fn query(&mut self, s: &str, p: &[Value]) -> Result<SqlRows> {
        self.inner.query(s, p)
    }
    fn query_committed(&mut self, s: &str, p: &[Value]) -> Result<SqlRows> {
        self.inner.query_committed(s, p)
    }
}
#[test]
fn commit_failure_rolls_back_assignment_and_acknowledgement() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let failure = Arc::new(AtomicBool::new(false));
    let store = FaultStore {
        inner: SqliteStore::open(&p).unwrap(),
        fail_commit: failure.clone(),
        crash_ack: false,
    };
    let mut c = Client::open05(store, schema(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let call = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"A","note":null}}),
            )
        })
        .unwrap();
    failure.store(true, Ordering::SeqCst);
    assert!(c.freeze_batch05().is_err());
    assert_eq!(c.store_status05().unwrap().last_acknowledged_batch_id, 0);
    let b = c.freeze_batch05().unwrap().unwrap();
    let a = ack(
        &b,
        wire::MutationOutcome::Rejected {
            code: "write.denied".into(),
            message: None,
        },
    );
    failure.store(true, Ordering::SeqCst);
    assert!(c.acknowledge_batch05(&a).is_err());
    assert!(c.call_completion05(&call.call_id).unwrap().is_none());
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), b);
    drop(c);
    let mut c = open(&p);
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), b);
    c.acknowledge_batch05(&a).unwrap();
    assert!(c.call_completion05(&call.call_id).unwrap().is_some());
}
#[test]
fn partial_duplicate_or_foreign_acknowledgement_changes_nothing() {
    let d = tempfile::tempdir().unwrap();
    let mut c = open(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    c.transaction(|tx| {
        tx.submit_mutation05(
            "Write",
            1,
            json!({"entry":{"id":"e","text":"A","note":null}}),
        )
    })
    .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    let a = ack(
        &b,
        wire::MutationOutcome::Rejected {
            code: "write.denied".into(),
            message: None,
        },
    );
    let mut bad = a.clone();
    bad.results.clear();
    assert!(c.acknowledge_batch05(&bad).is_err());
    bad = a.clone();
    bad.results.push(bad.results[0].clone());
    assert!(c.acknowledge_batch05(&bad).is_err());
    bad = a.clone();
    bad.context.store_id = "foreign".into();
    assert!(c.acknowledge_batch05(&bad).is_err());
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), b);
    assert_eq!(c.store_status05().unwrap().last_acknowledged_batch_id, 0);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "A");
}
fn update_schema() -> Schema {
    let mut s = serde_json::to_value(schema()).unwrap();
    s["actions"][0]["inputs"][0]["operation"] = json!("update");
    Schema::from_value(s).unwrap()
}
#[test]
#[ignore]
fn abrupt_ack_child() {
    let p = std::env::var("AXTON_TASK2_CRASH_DB").unwrap();
    let store = FaultStore {
        inner: SqliteStore::open(&p).unwrap(),
        fail_commit: Arc::new(AtomicBool::new(false)),
        crash_ack: true,
    };
    let mut c = Client::open05(store, update_schema(), "User:u").unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    c.acknowledge_batch05(&ack(
        &b,
        wire::MutationOutcome::Rejected {
            code: "write.denied".into(),
            message: None,
        },
    ))
    .unwrap();
    panic!("fault boundary did not terminate child")
}
#[test]
fn abrupt_process_exit_before_ack_commit_restores_exact_batch_beneath_later_work() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), update_schema(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    c.install_authority05(
        &context,
        &[wire::AuthorityChange::Record {
            key: stream_key("e"),
            cursor: 1,
            state: state("base"),
        }],
        None,
    )
    .unwrap();
    let call = c
        .transaction(|tx| tx.submit_mutation05("Write", 1, json!({"entry":{"id":"e","text":"A"}})))
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    later(&mut c, "B");
    drop(c);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "abrupt_ack_child", "--ignored", "--nocapture"])
        .env("AXTON_TASK2_CRASH_DB", &p)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(77));
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), update_schema(), "User:u").unwrap();
    assert_eq!(
        wire::encode(&c.freeze_batch05().unwrap().unwrap()).unwrap(),
        wire::encode(&b).unwrap()
    );
    assert_eq!(c.store_status05().unwrap().last_acknowledged_batch_id, 0);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    let a = ack(
        &b,
        wire::MutationOutcome::Rejected {
            code: "write.denied".into(),
            message: None,
        },
    );
    c.acknowledge_batch05(&a).unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "B");
    assert_eq!(c.pending_count().unwrap(), 1);
    assert!(c.call_completion05(&call.call_id).unwrap().is_some());
}
#[test]
fn pending_optimism_keeps_authoritative_base_protection() {
    let d = tempfile::tempdir().unwrap();
    let mut c = Client::open05(
        SqliteStore::open(d.path().join("db")).unwrap(),
        update_schema(),
        "User:u",
    )
    .unwrap();
    let ctx = c.request_context05().unwrap();
    c.install_authority05(
        &ctx,
        &[wire::AuthorityChange::Record {
            key: stream_key("e"),
            cursor: 2,
            state: state("base"),
        }],
        None,
    )
    .unwrap();
    c.transaction(|tx| {
        tx.submit_mutation05("Write", 1, json!({"entry":{"id":"e","text":"optimistic"}}))
    })
    .unwrap();
    assert_eq!(
        c.record_evidence05(&key()).unwrap().current.unwrap().cursor,
        2
    );
    c.install_cache05(
        &[wire::ReadRecord {
            key: stream_key("e"),
            cursor: (),
            state: state("cache"),
        }],
        true,
    )
    .unwrap();
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "optimistic");
}
#[test]
fn target_moved_past_sync_cursor_keeps_local_layer_until_newer_position_lands() {
    let d = tempfile::tempdir().unwrap();
    let mut c = open(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let call = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"A","note":null}}),
            )
        })
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    let a = ack(
        &b,
        wire::MutationOutcome::Accepted {
            sync_cursor: 5,
            materialization: b.context.materialization.clone(),
            result: Value::Null,
            targets: vec![wire::SettlementTarget::Stream {
                key: stream_key("e"),
                cursor: 5,
                fallback: wire::ReadRecord {
                    key: stream_key("e"),
                    cursor: (),
                    state: state("frozen"),
                },
            }],
        },
    );
    c.acknowledge_batch05(&a).unwrap();
    let context = c.request_context05().unwrap();
    c.install_authority05(&context, &[], Some((0, 40))).unwrap();
    // Coverage of syncCursor alone does not settle: compaction moved the
    // target to a later unit, so the pending layer stays over the base.
    assert!(c.call_completion05(&call.call_id).unwrap().is_none());
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "A");
    assert_eq!(c.pending_count().unwrap(), 1);
    install(&mut c, "e", 45, Some("latest"));
    assert!(c.call_completion05(&call.call_id).unwrap().is_some());
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "latest");
}
#[test]
fn lifecycle_dependents_are_rejected_atomically_and_retained_but_not_replayed() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = open(&p);
    c.initialize_stream05(0, "1").unwrap();
    let first = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"A","note":null}}),
            )
        })
        .unwrap();
    let second = c
        .transaction(|tx| {
            tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"edit"}}))
        })
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(b.mutations.len(), 1);
    let report = c
        .acknowledge_batch05(&ack(
            &b,
            wire::MutationOutcome::Rejected {
                code: "write.denied".into(),
                message: None,
            },
        ))
        .unwrap();
    assert_eq!(report.completions.len(), 2);
    assert!(c.call_completion05(&first.call_id).unwrap().is_some());
    assert!(c.call_completion05(&second.call_id).unwrap().is_some());
    assert!(c.freeze_batch05().unwrap().is_none());
    assert!(c.read(&key()).unwrap().is_none());
    let db = rusqlite::Connection::open(&p).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM axton_mutation_queue_operation",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM axton_mutation_queue_operation o JOIN axton_mutation_queue q ON q.id=o.mutation_id WHERE o.model IS NOT NULL AND q.reconciled=0 AND q.rejection_code IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
#[test]
fn settlement_constraint_failure_retains_acceptance_and_retries_locally() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut s = schema();
    s.models[0].unique.push(vec!["text".into()]);
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), s.clone(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    cache_row(&mut c, "Entry", "other", state("taken"));
    let call = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"optimistic","note":null}}),
            )
        })
        .unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    let a = ack(
        &b,
        wire::MutationOutcome::Accepted {
            sync_cursor: 1,
            materialization: b.context.materialization.clone(),
            result: Value::Null,
            targets: vec![wire::SettlementTarget::Private {
                record: wire::ReadRecord {
                    key: stream_key("e"),
                    cursor: (),
                    state: state("taken"),
                },
            }],
        },
    );
    c.acknowledge_batch05(&a).unwrap();
    let ctx = c.request_context05().unwrap();
    assert!(c.install_authority05(&ctx, &[], Some((0, 1))).is_err());
    assert!(c.call_completion05(&call.call_id).unwrap().is_none());
    assert_eq!(c.store_status05().unwrap().last_acknowledged_batch_id, 1);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "optimistic");
    drop(c);
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), s, "User:u").unwrap();
    assert!(c.settle_ready05().unwrap().completions.is_empty());
    cache_row(&mut c, "Entry", "other", state("free"));
    let ctx = c.request_context05().unwrap();
    c.install_authority05(&ctx, &[], Some((0, 1))).unwrap();
    assert!(c.call_completion05(&call.call_id).unwrap().is_some());
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "taken");
}
#[test]
#[ignore]
fn abrupt_assignment_child() {
    let p = std::env::var("AXTON_TASK2_CRASH_DB").unwrap();
    let store = FaultStore {
        inner: SqliteStore::open(&p).unwrap(),
        fail_commit: Arc::new(AtomicBool::new(false)),
        crash_ack: false,
    };
    let mut c = Client::open05(store, schema(), "User:u").unwrap();
    let b = c.freeze_batch05().unwrap().unwrap();
    std::fs::write(
        std::env::var("AXTON_TASK2_ASSIGNMENT_EVIDENCE").unwrap(),
        wire::encode(&b).unwrap(),
    )
    .unwrap();
    std::process::exit(81)
}
#[test]
fn abrupt_assignment_commit_boundaries_preserve_canonical_retry() {
    let d = tempfile::tempdir().unwrap();
    for mode in ["before", "after"] {
        let p = d.path().join(format!("{mode}.db"));
        let evidence = d.path().join(format!("{mode}.evidence"));
        let mut c = open(&p);
        c.initialize_stream05(0, "1").unwrap();
        c.transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"A","note":null}}),
            )
        })
        .unwrap();
        drop(c);
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "abrupt_assignment_child",
                "--ignored",
                "--nocapture",
            ])
            .env("AXTON_TASK2_CRASH_DB", &p)
            .env("AXTON_TASK2_ASSIGNMENT_CRASH", mode)
            .env("AXTON_TASK2_ASSIGNMENT_EVIDENCE", &evidence)
            .status()
            .unwrap();
        assert_eq!(result.code(), Some(if mode == "before" { 80 } else { 81 }));
        let mut c = open(&p);
        let b = c.freeze_batch05().unwrap().unwrap();
        if mode == "before" {
            assert_eq!(b.digest, std::fs::read_to_string(&evidence).unwrap());
        } else {
            assert_eq!(wire::encode(&b).unwrap(), std::fs::read(&evidence).unwrap());
        }
        drop(c);
        let mut c = open(&p);
        assert_eq!(c.freeze_batch05().unwrap().unwrap(), b);
    }
}

#[test]
fn zero_start_handshake_does_not_complete_bootstrap() {
    use axton_client::{ClientStore, engine::Engine};
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    drop(open(&p));
    let mut store = SqliteStore::open(&p).unwrap();
    let s = schema();
    let mut changed = std::collections::BTreeSet::new();
    store.begin().unwrap();
    Engine::new(&mut store, &s, &mut changed, false)
        .initialize_stream05(0)
        .unwrap();
    store.commit().unwrap();
    drop(store);
    let mut c = open(&p);
    let status = c.store_status05().unwrap();
    assert_eq!(status.start_cursor, Some(0));
    assert_eq!(status.cursor, Some(0));
    assert_eq!(status.bootstrap_cursor, None);
    drop(c);
    let mut store = SqliteStore::open(&p).unwrap();
    store.begin().unwrap();
    Engine::new(&mut store, &s, &mut changed, false)
        .initialize_stream05(10)
        .unwrap();
    store.commit().unwrap();
    drop(store);
    let mut c = open(&p);
    assert_eq!(c.store_status05().unwrap().start_cursor, Some(0));
    assert_eq!(c.store_status05().unwrap().bootstrap_cursor, None);
}

#[test]
fn discarded_mutation_and_lifecycle_dependent_keep_public_codes_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db");
    let mut descriptor: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    descriptor["actions"] = json!([
        {"name":"Write","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"create","cardinality":"single"}],"outputs":[]},
        {"name":"Edit","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"update","fields":["text"],"cardinality":"single"}],"outputs":[]}
    ]);
    let schema = Schema::from_value(descriptor).unwrap();
    let mut client =
        Client::open05(SqliteStore::open(&path).unwrap(), schema.clone(), "User:u").unwrap();
    let created = client
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"created","note":null}}),
            )
        })
        .unwrap();
    let dependent = client
        .transaction(|tx| {
            tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"dependent"}}))
        })
        .unwrap();
    let report = client
        .transaction(|tx| tx.discard_mutation05(created.ordinal))
        .unwrap();
    assert_eq!(report.completions.len(), 2);
    drop(client);
    {
        use axton_client::ClientStore;
        let mut store = SqliteStore::open(&path).unwrap();
        assert_eq!(
            store
                .query(
                    "SELECT id,rejection_acknowledged FROM axton_mutation_queue ORDER BY id",
                    &[]
                )
                .unwrap()
                .rows,
            vec![
                vec![json!(created.ordinal), json!(1)],
                vec![json!(dependent.ordinal), json!(0)]
            ]
        );
    }

    let mut client = Client::open05(SqliteStore::open(&path).unwrap(), schema, "User:u").unwrap();
    for (call, expected) in [(&created, "dropped"), (&dependent, "dependency.rejected")] {
        let completion = client.call_completion05(&call.call_id).unwrap().unwrap();
        assert!(
            matches!(completion.outcome, axton_client::ActionOutcome::Failed { code, .. } if code == expected)
        );
        assert!(
            matches!(client.mutation_result05(call.ordinal).unwrap().unwrap().outcome, wire::MutationOutcome::Rejected { code, .. } if code == expected)
        );
    }
    assert!(client.freeze_batch05().unwrap().is_none());
    assert_eq!(client.read(&key()).unwrap(), None);
}

#[test]
fn dismissed_rejection_keeps_durable_completion_but_retires_input_after_reopen() {
    use axton_client::ClientStore;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db");
    let mut client = open(&path);
    client.initialize_stream05(0, "1").unwrap();
    let call = client
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"denied","note":null}}),
            )
        })
        .unwrap();
    let batch = client.freeze_batch05().unwrap().unwrap();
    client
        .acknowledge_batch05(&ack(
            &batch,
            wire::MutationOutcome::Rejected {
                code: "write.denied".into(),
                message: Some("retained outcome".into()),
            },
        ))
        .unwrap();
    client
        .transaction(|tx| tx.dismiss_rejection05(call.ordinal))
        .unwrap();
    drop(client);
    let mut store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.query("SELECT rejection_acknowledged,rejection_code,rejection_message FROM axton_mutation_queue WHERE id=?", &[json!(call.ordinal)]).unwrap().rows,
        vec![vec![json!(1),json!("write.denied"),json!("retained outcome")]]);
    for table in [
        "axton_mutation_queue_operation",
        "axton_mutation_prerequisite",
        "axton_mutation_dependency",
    ] {
        assert!(
            store
                .query(&format!("SELECT * FROM {table}"), &[])
                .unwrap()
                .rows
                .is_empty()
        );
    }
    drop(store);
    let mut client = open(&path);
    let completion = client.call_completion05(&call.call_id).unwrap().unwrap();
    assert!(
        matches!(completion.outcome, axton_client::ActionOutcome::Failed {code, ..}
        if code == "write.denied")
    );
    assert!(client.mutation_result05(call.ordinal).unwrap().is_some());
    client
        .transaction(|tx| tx.dismiss_rejection05(call.ordinal))
        .unwrap();
    assert!(client.freeze_batch05().unwrap().is_none());
}

#[test]
fn incomplete_format5_is_refused_without_upgrade_or_file_changes() {
    use axton_client::ClientStore;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("incomplete.db");
    let mut client = open(&path);
    client
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"unsent","note":null}}),
            )
        })
        .unwrap();
    drop(client);
    let mut store = SqliteStore::open(&path).unwrap();
    let metadata = store.query("SELECT * FROM axton_store", &[]).unwrap().rows;
    let queue = store
        .query(
            "SELECT id,name,descriptor,result FROM axton_mutation_queue",
            &[],
        )
        .unwrap()
        .rows;
    store
        .execute_batch("ALTER TABLE axton_mutation_queue DROP COLUMN rejection_acknowledged")
        .unwrap();
    drop(store);
    let before = std::fs::read(&path).unwrap();
    for stream in ["User:other", "User:u"] {
        assert!(Client::open05(SqliteStore::open(&path).unwrap(), schema(), stream).is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "admission must not upgrade a prior provisional file"
        );
    }
    let mut store = SqliteStore::open(&path).unwrap();
    assert!(
        !store
            .query("PRAGMA table_info(axton_mutation_queue)", &[])
            .unwrap()
            .rows
            .iter()
            .any(|r| r[1] == "rejection_acknowledged")
    );
    assert_eq!(
        store.query("SELECT * FROM axton_store", &[]).unwrap().rows,
        metadata
    );
    assert_eq!(
        store
            .query(
                "SELECT id,name,descriptor,result FROM axton_mutation_queue",
                &[]
            )
            .unwrap()
            .rows,
        queue
    );
}

#[test]
fn legacy_drop_retains_own_refusal_until_explicit_acknowledgement() {
    use axton_client::ClientStore;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db");
    let mut client = open(&path);
    let call = client
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"drop","note":null}}),
            )
        })
        .unwrap();
    client
        .transaction(|tx| tx.drop_mutation05(call.ordinal))
        .unwrap();
    drop(client);
    let mut store = SqliteStore::open(&path).unwrap();
    assert_eq!(
        store
            .query(
                "SELECT rejection_acknowledged,rejection_code FROM axton_mutation_queue WHERE id=?",
                &[json!(call.ordinal)]
            )
            .unwrap()
            .rows,
        vec![vec![json!(0), json!("dropped")]]
    );
    assert!(
        !store
            .query(
                "SELECT * FROM axton_mutation_queue_operation WHERE mutation_id=?",
                &[json!(call.ordinal)]
            )
            .unwrap()
            .rows
            .is_empty()
    );
    drop(store);
    let mut client = open(&path);
    let completed = client.call_completion05(&call.call_id).unwrap().unwrap();
    assert!(
        matches!(&completed.outcome, axton_client::ActionOutcome::Failed {code,..} if code=="dropped")
    );
    client
        .transaction(|tx| tx.dismiss_rejection05(call.ordinal))
        .unwrap();
    assert_eq!(
        client.call_completion05(&call.call_id).unwrap(),
        Some(completed)
    );
    assert!(client.freeze_batch05().unwrap().is_none());
}

#[test]
fn dismiss_missing_is_noop_but_pending_and_accepted_outcomes_are_protected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db");
    let mut descriptor: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    descriptor["actions"] = json!([{"name":"Ping","version":1,"inputs":[],"outputs":[]}]);
    let schema = Schema::from_value(descriptor).unwrap();
    let mut client =
        Client::open05(SqliteStore::open(&path).unwrap(), schema.clone(), "User:u").unwrap();
    client.initialize_stream05(0, "1").unwrap();
    client.transaction(|tx| tx.dismiss_rejection05(99)).unwrap();
    let call = client
        .transaction(|tx| tx.submit_mutation05("Ping", 1, json!({})))
        .unwrap();
    assert!(
        client
            .transaction(|tx| tx.dismiss_rejection05(call.ordinal))
            .is_err()
    );
    let batch = client.freeze_batch05().unwrap().unwrap();
    client
        .acknowledge_batch05(&ack(
            &batch,
            wire::MutationOutcome::Accepted {
                sync_cursor: 0,
                materialization: batch.context.materialization.clone(),
                result: Value::Null,
                targets: vec![],
            },
        ))
        .unwrap();
    assert!(
        client
            .transaction(|tx| tx.dismiss_rejection05(call.ordinal))
            .is_err()
    );
    client.settle_ready05().unwrap();
    let completed = client.call_completion05(&call.call_id).unwrap();
    assert!(completed.is_some());
    assert!(
        client
            .transaction(|tx| tx.dismiss_rejection05(call.ordinal))
            .is_err()
    );
    drop(client);
    let mut client = Client::open05(SqliteStore::open(&path).unwrap(), schema, "User:u").unwrap();
    client.transaction(|tx| tx.dismiss_rejection05(99)).unwrap();
    assert_eq!(client.call_completion05(&call.call_id).unwrap(), completed);
    assert!(
        client
            .transaction(|tx| tx.dismiss_rejection05(call.ordinal))
            .is_err()
    );
}

#[test]
fn failed_reset_keeps_old_store_rows_authority_and_prefix_after_real_reopen() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("db");
    let failure = Arc::new(AtomicBool::new(false));
    let mut c = Client::open05(
        FaultStore {
            inner: SqliteStore::open(&path).unwrap(),
            fail_commit: failure.clone(),
            crash_ack: false,
        },
        schema(),
        "User:u",
    )
    .unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let old = c.request_context05().unwrap();
    c.install_authority05(
        &old,
        &[wire::AuthorityChange::Record {
            key: stream_key("e"),
            cursor: 57,
            state: state("authority"),
        }],
        Some((0, 57)),
    )
    .unwrap();
    failure.store(true, Ordering::SeqCst);
    assert!(c.reset_store05(true).is_err());
    assert_eq!(c.request_context05().unwrap(), old);
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "authority");
    assert_eq!(c.store_status05().unwrap().cursor, Some(57));
    drop(c);
    let mut c = Client::open05(
        FaultStore {
            inner: SqliteStore::open(&path).unwrap(),
            fail_commit: failure,
            crash_ack: false,
        },
        schema(),
        "User:u",
    )
    .unwrap();
    assert_eq!(c.request_context05().unwrap(), old);
    assert_eq!(c.store_status05().unwrap().cursor, Some(57));
    assert_eq!(
        c.record_evidence05(&key()).unwrap().current.unwrap().cursor,
        57
    );
    assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "authority");
}

#[test]
fn retained_private_snapshot_never_evaluates_new_creation_defaults() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = open(&p);
    c.initialize_stream05(0, "1").unwrap();
    let call = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"queued","note":null}}),
            )
        })
        .unwrap();
    let batch = c.freeze_batch05().unwrap().unwrap();
    let context = c.request_context05().unwrap();
    c.install_authority05(&context, &[], Some((0, 0))).unwrap();
    drop(c);
    let mut desired = serde_json::to_value(schema()).unwrap();
    let fields = desired["models"][0]["fields"].as_array_mut().unwrap();
    fields.extend([
        json!({"name":"token","nullable":true,"type":{"kind":"scalar","name":"uuid"},"createDefault":{"kind":"uuid"}}),
        json!({"name":"time","nullable":true,"type":{"kind":"scalar","name":"dateTime"},"createDefault":{"kind":"now"}}),
        json!({"name":"label","nullable":true,"type":{"kind":"scalar","name":"string"},"createDefault":{"kind":"literal","value":"fresh"}}),
    ]);
    let desired = Schema::from_value(desired).unwrap();
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), desired.clone(), "User:u").unwrap();
    // The upgrade sends the same Batch in the new contract, and the receipt
    // answers in it.
    let resent = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(
        (&resent.digest, &resent.mutations),
        (&batch.digest, &batch.mutations)
    );
    assert_ne!(resent.context, batch.context);
    let receipt = ack(
        &resent,
        wire::MutationOutcome::Accepted {
            sync_cursor: 0,
            materialization: resent.context.materialization.clone(),
            result: Value::Null,
            targets: vec![wire::SettlementTarget::Private {
                record: wire::ReadRecord {
                    key: stream_key("e"),
                    cursor: (),
                    state: state("accepted"),
                },
            }],
        },
    );
    c.acknowledge_batch05(&receipt).unwrap();
    c.settle_ready05().unwrap();
    assert!(c.call_completion05(&call.call_id).unwrap().is_some());
    let expected =
        json!({"id":"e","text":"accepted","note":null,"token":null,"time":null,"label":null});
    assert_eq!(c.read(&key()).unwrap(), Some(expected.clone()));
    drop(c);
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), desired, "User:u").unwrap();
    c.acknowledge_batch05(&receipt).unwrap();
    c.settle_ready05().unwrap();
    assert_eq!(c.read(&key()).unwrap(), Some(expected));
}

#[test]
fn delayed_tracked_receipt_after_remove_finalizes_original_fallback_beneath_later_work() {
    use axton_client::sync05::DeliveryQueue;
    for later_work in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("db");
        let mut c =
            Client::open05(SqliteStore::open(&p).unwrap(), update_schema(), "User:u").unwrap();
        c.initialize_stream05(0, "1").unwrap();
        let context = c.request_context05().unwrap();
        let mut q = DeliveryQueue::new(100_000, 8);
        let initial = wire::freeze_delivery(
            context.clone(),
            "initial".into(),
            wire::DeliveryPurpose::Bootstrap,
            0,
            0,
            1,
            10000,
            vec![wire::DeliveryUnit {
                index: 0,
                through: Some(0),
                changes: vec![wire::AuthorityChange::Record {
                    key: stream_key("e"),
                    cursor: 1,
                    state: state("original"),
                }],
            }],
            10,
        )
        .unwrap();
        q.receive(&initial.header, &initial.parts, &context, 1)
            .unwrap();
        c.apply_next_delivery05(&mut q, 1).unwrap().unwrap();
        let call = c
            .transaction(|tx| {
                tx.submit_mutation05(
                    "Write",
                    1,
                    json!({"entry":{"id":"e","text":"optimistic","note":null}}),
                )
            })
            .unwrap();
        let b = c.freeze_batch05().unwrap().unwrap();
        let a = ack(
            &b,
            wire::MutationOutcome::Accepted {
                sync_cursor: 2,
                materialization: b.context.materialization.clone(),
                result: Value::Null,
                targets: vec![wire::SettlementTarget::Stream {
                    key: stream_key("e"),
                    cursor: 2,
                    fallback: wire::ReadRecord {
                        key: stream_key("e"),
                        cursor: (),
                        state: state("accepted"),
                    },
                }],
            },
        );
        // The newer Remove arrives before the held HTTP acknowledgement.
        let removed = wire::freeze_delivery(
            context.clone(),
            "remove".into(),
            wire::DeliveryPurpose::Sync,
            0,
            3,
            3,
            10000,
            vec![wire::DeliveryUnit {
                index: 0,
                through: Some(3),
                changes: vec![wire::AuthorityChange::Remove {
                    key: stream_key("e"),
                    cursor: 3,
                }],
            }],
            10,
        )
        .unwrap();
        q.receive(&removed.header, &removed.parts, &context, 1)
            .unwrap();
        c.apply_next_delivery05(&mut q, 1).unwrap().unwrap();
        if later_work {
            later(&mut c, "later");
        }
        c.acknowledge_batch05(&a).unwrap();
        assert_eq!(c.settle_ready05().unwrap().completions.len(), 1);
        assert_eq!(
            c.read(&key()).unwrap().unwrap()["text"],
            if later_work { "later" } else { "accepted" }
        );
        let e = c.record_evidence05(&key()).unwrap();
        assert!(e.current.is_none());
        assert_eq!(e.history[&context.materialization], 1);
        assert_eq!(e.membership.as_ref().unwrap().cursor, 3);
        assert!(!e.membership.unwrap().live);
        assert_eq!(c.store_status05().unwrap().cursor, Some(3));
        assert_eq!(c.pending_count().unwrap(), usize::from(later_work));
        assert!(c.call_completion05(&call.call_id).unwrap().is_some());
        assert!(c.settle_ready05().unwrap().completions.is_empty());
        drop(c);
        let mut c =
            Client::open05(SqliteStore::open(&p).unwrap(), update_schema(), "User:u").unwrap();
        assert_eq!(
            c.read(&key()).unwrap().unwrap()["text"],
            if later_work { "later" } else { "accepted" }
        );
        assert!(c.call_completion05(&call.call_id).unwrap().is_some());
        c.acknowledge_batch05(&a).unwrap();
        assert!(c.settle_ready05().unwrap().completions.is_empty());
    }
}

#[test]
fn historical_target_finalizes_from_receipt_unless_bootstrap_will_deliver_it() {
    use axton_client::sync05::DeliveryQueue;
    for bootstrap in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("db");
        let mut s = serde_json::to_value(schema()).unwrap();
        s["models"][0]["bootstrap"] = json!(bootstrap);
        let schema = Schema::from_value(s).unwrap();
        let mut c =
            Client::open05(SqliteStore::open(&p).unwrap(), schema.clone(), "User:u").unwrap();
        c.initialize_stream05(80, "1").unwrap();
        let context = c.request_context05().unwrap();
        let call = c
            .transaction(|tx| {
                tx.submit_mutation05(
                    "Write",
                    1,
                    json!({"entry":{"id":"e","text":"optimistic","note":null}}),
                )
            })
            .unwrap();
        let b = c.freeze_batch05().unwrap().unwrap();
        c.acknowledge_batch05(&ack(
            &b,
            wire::MutationOutcome::Accepted {
                sync_cursor: 80,
                materialization: b.context.materialization.clone(),
                result: Value::Null,
                targets: vec![wire::SettlementTarget::Stream {
                    key: stream_key("e"),
                    cursor: 57,
                    fallback: wire::ReadRecord {
                        key: stream_key("e"),
                        cursor: (),
                        state: state("fallback"),
                    },
                }],
            },
        ))
        .unwrap();
        let settled = c.settle_ready05().unwrap().completions.len();
        if !bootstrap {
            // The Stream never delivers an unmarked position at or below S.
            assert_eq!(settled, 1);
            assert!(c.call_completion05(&call.call_id).unwrap().is_some());
            assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "fallback");
            let e = c.record_evidence05(&key()).unwrap();
            assert!(e.history.is_empty() && e.membership.is_none() && e.current.is_none());
            drop(c);
            let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema, "User:u").unwrap();
            assert!(c.call_completion05(&call.call_id).unwrap().is_some());
            assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "fallback");
            continue;
        }
        // A marked target arrives when Bootstrap passes it.
        assert_eq!(settled, 0);
        assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "optimistic");
        let public = wire::freeze_delivery(
            context.clone(),
            "public-bootstrap".into(),
            wire::DeliveryPurpose::Bootstrap,
            0,
            80,
            80,
            10000,
            vec![wire::DeliveryUnit {
                index: 0,
                through: Some(80),
                changes: vec![wire::AuthorityChange::Record {
                    key: stream_key("e"),
                    cursor: 57,
                    state: state("canonical"),
                }],
            }],
            10,
        )
        .unwrap();
        let mut q = DeliveryQueue::new(100_000, 8);
        q.receive(&public.header, &public.parts, &context, 1)
            .unwrap();
        let applied = c.apply_next_delivery05(&mut q, 1).unwrap().unwrap();
        assert_eq!(applied.completions.len(), 1);
        assert_eq!(c.read(&key()).unwrap().unwrap()["text"], "canonical");
        assert_eq!(
            c.record_evidence05(&key()).unwrap().history[&context.materialization],
            57
        );
        assert_eq!(c.store_status05().unwrap().bootstrap_cursor, Some(80));
    }
}

#[test]
fn failed_local_settlement_retains_previous_batch_window_after_reopen() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let failure = Arc::new(AtomicBool::new(false));
    let mut c = Client::open05(
        FaultStore {
            inner: SqliteStore::open(&p).unwrap(),
            fail_commit: failure.clone(),
            crash_ack: false,
        },
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
                json!({"entry":{"id":"e","text":"optimistic","note":null}}),
            )
        })
        .unwrap();
    let batch = c.freeze_batch05().unwrap().unwrap();
    c.acknowledge_batch05(&ack(
        &batch,
        wire::MutationOutcome::Accepted {
            sync_cursor: 0,
            materialization: batch.context.materialization.clone(),
            result: Value::Null,
            targets: vec![wire::SettlementTarget::Private {
                record: wire::ReadRecord {
                    key: stream_key("e"),
                    cursor: (),
                    state: state("canonical"),
                },
            }],
        },
    ))
    .unwrap();
    let next = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"other","text":"independent","note":null}}),
            )
        })
        .unwrap();
    failure.store(true, Ordering::SeqCst);
    assert!(c.settle_ready05().is_err());
    assert!(c.call_completion05(&first.call_id).unwrap().is_none());
    assert!(
        c.freeze_batch05().unwrap().is_none(),
        "failed settlement cannot release the window"
    );
    drop(c);
    let mut c = open(&p);
    assert!(c.freeze_batch05().unwrap().is_none());
    assert_eq!(c.settle_ready05().unwrap().completions.len(), 1);
    assert!(c.call_completion05(&first.call_id).unwrap().is_some());
    let batch = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(batch.batch_id, 2);
    assert_eq!(
        batch.mutations.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![next.ordinal]
    );
}

#[test]
fn receipt_snapshot_installs_only_when_labelled_with_the_store_materialization() {
    for private in [true, false] {
        for same in [true, false] {
            let d = tempfile::tempdir().unwrap();
            let p = d.path().join("db");
            let mut s = serde_json::to_value(schema()).unwrap();
            s["models"][0]["bootstrap"] = json!(false);
            let schema = Schema::from_value(s).unwrap();
            let mut c = Client::open05(SqliteStore::open(&p).unwrap(), schema, "User:u").unwrap();
            c.initialize_stream05(80, "1").unwrap();
            let call = c
                .transaction(|tx| {
                    tx.submit_mutation05(
                        "Write",
                        1,
                        json!({"entry":{"id":"e","text":"optimistic","note":null}}),
                    )
                })
                .unwrap();
            let b = c.freeze_batch05().unwrap().unwrap();
            let snapshot = wire::ReadRecord {
                key: stream_key("e"),
                cursor: (),
                state: state("snapshot"),
            };
            c.acknowledge_batch05(&ack(
                &b,
                wire::MutationOutcome::Accepted {
                    sync_cursor: 80,
                    materialization: if same {
                        b.context.materialization.clone()
                    } else {
                        "earlier".into()
                    },
                    result: Value::Null,
                    targets: vec![if private {
                        wire::SettlementTarget::Private { record: snapshot }
                    } else {
                        // A historical target of an unmarked Model.
                        wire::SettlementTarget::Stream {
                            key: stream_key("e"),
                            cursor: 57,
                            fallback: snapshot,
                        }
                    }],
                },
            ))
            .unwrap();
            assert_eq!(c.settle_ready05().unwrap().completions.len(), 1);
            assert!(c.call_completion05(&call.call_id).unwrap().is_some());
            let row = c.read(&key()).unwrap();
            if same {
                assert_eq!(row.unwrap()["text"], "snapshot", "{private}");
            } else {
                assert_eq!(row, None, "{private}");
            }
        }
    }
}

/// `Entry` with a nullable `label`, which changes the materialization.
fn labelled() -> Schema {
    let mut s = serde_json::to_value(schema()).unwrap();
    s["models"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"label","nullable":true,"type":{"kind":"scalar","name":"string"}}));
    Schema::from_value(s).unwrap()
}
#[test]
fn a_batch_partly_executed_before_an_upgrade_settles_each_outcome_by_its_own_label() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = open(&p);
    c.initialize_stream05(0, "1").unwrap();
    let ids = ["first", "second"];
    for id in ids {
        c.transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":id,"text":"optimistic","note":null}}),
            )
        })
        .unwrap();
    }
    let before = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(before.mutations.len(), 2);
    drop(c);
    let mut c = Client::open05(SqliteStore::open(&p).unwrap(), labelled(), "User:u").unwrap();
    let resent = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(resent.digest, before.digest);
    // The first member ran before the upgrade and replays its saved label.
    let labels = [
        before.context.materialization.clone(),
        resent.context.materialization.clone(),
    ];
    c.acknowledge_batch05(&wire::BatchAcknowledgement {
        context: resent.context.clone(),
        batch_id: resent.batch_id,
        digest: resent.digest.clone(),
        results: resent
            .mutations
            .iter()
            .zip(ids.iter().zip(labels))
            .map(|(m, (id, materialization))| wire::MutationResult {
                mutation_id: m.id,
                outcome: wire::MutationOutcome::Accepted {
                    sync_cursor: 0,
                    materialization,
                    result: Value::Null,
                    targets: vec![wire::SettlementTarget::Private {
                        record: wire::ReadRecord {
                            key: stream_key(id),
                            cursor: (),
                            state: state("receipt"),
                        },
                    }],
                },
            })
            .collect(),
    })
    .unwrap();
    assert_eq!(c.settle_ready05().unwrap().completions.len(), 2);
    let read = |c: &mut Client<SqliteStore>, id: &str| {
        c.read(&RecordKey {
            model: "Entry".into(),
            identity: json!({ "id": id }),
        })
        .unwrap()
    };
    assert_eq!(read(&mut c, "first"), None);
    assert_eq!(read(&mut c, "second").unwrap()["text"], "receipt");
}
#[test]
fn a_target_protected_only_under_the_previous_materialization_finalizes_from_the_receipt() {
    for upgrade in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("db");
        let edit = |schema: Schema| {
            let mut s = serde_json::to_value(schema).unwrap();
            s["actions"] = json!([{"name":"Edit","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"update","cardinality":"single","fields":["text"]}],"outputs":[]}]);
            Schema::from_value(s).unwrap()
        };
        let mut c =
            Client::open05(SqliteStore::open(&p).unwrap(), edit(schema()), "User:u").unwrap();
        c.initialize_stream05(80, "1").unwrap();
        install(&mut c, "e", 57, Some("stream"));
        let call = c
            .transaction(|tx| {
                tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"edited"}}))
            })
            .unwrap();
        c.freeze_batch05().unwrap().unwrap();
        drop(c);
        let after = edit(if upgrade { labelled() } else { schema() });
        let mut c = Client::open05(SqliteStore::open(&p).unwrap(), after, "User:u").unwrap();
        // The target (60) is past the held delivery (57), so it is not
        // installed; at or below S in an unmarked Model it settles from the
        // receipt. (A target at 57 would settle on the held delivery in any
        // contract.)
        let resent = c.freeze_batch05().unwrap().unwrap();
        c.acknowledge_batch05(&ack(
            &resent,
            wire::MutationOutcome::Accepted {
                sync_cursor: 80,
                materialization: resent.context.materialization.clone(),
                result: Value::Null,
                targets: vec![wire::SettlementTarget::Stream {
                    key: stream_key("e"),
                    cursor: 60,
                    fallback: wire::ReadRecord {
                        key: stream_key("e"),
                        cursor: (),
                        state: state("fallback"),
                    },
                }],
            },
        ))
        .unwrap();
        assert_eq!(c.settle_ready05().unwrap().completions.len(), 1);
        assert!(c.call_completion05(&call.call_id).unwrap().is_some());
        // Current protection holds only in the contract that installed it.
        let text = if upgrade { "fallback" } else { "stream" };
        assert_eq!(c.read(&key()).unwrap().unwrap()["text"], text);
    }
}
/// `Entry` marked `@@bootstrap` with an `Edit` update, optionally `labelled`.
fn marked_edit(labelled_schema: bool) -> Schema {
    let mut s = serde_json::to_value(if labelled_schema {
        labelled()
    } else {
        schema()
    })
    .unwrap();
    s["models"][0]["bootstrap"] = json!(true);
    s["actions"] = json!([{"name":"Edit","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"update","cardinality":"single","fields":["text"]}],"outputs":[]}]);
    Schema::from_value(s).unwrap()
}
#[test]
fn a_target_the_stream_delivered_before_an_upgrade_settles_after_it_without_bootstrap() {
    for deleted in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("db");
        let mut c =
            Client::open05(SqliteStore::open(&p).unwrap(), marked_edit(false), "User:u").unwrap();
        c.initialize_stream05(10, "1").unwrap();
        let context = c.request_context05().unwrap();
        let record = |cursor: u64, text: Option<&str>| wire::AuthorityChange::Record {
            key: stream_key("e"),
            cursor,
            state: text.map(state).unwrap_or(Value::Null),
        };
        c.install_authority05(&context, &[record(12, Some("stream"))], Some((10, 12)))
            .unwrap();
        let call = c
            .transaction(|tx| {
                tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"edited"}}))
            })
            .unwrap();
        let b = c.freeze_batch05().unwrap().unwrap();
        // Accepted at 20; the Stream delivers the target before the upgrade,
        // but the Store has not yet covered the receipt's head.
        c.acknowledge_batch05(&ack(
            &b,
            wire::MutationOutcome::Accepted {
                sync_cursor: 30,
                materialization: b.context.materialization.clone(),
                result: Value::Null,
                targets: vec![wire::SettlementTarget::Stream {
                    key: stream_key("e"),
                    cursor: 20,
                    fallback: wire::ReadRecord {
                        key: stream_key("e"),
                        cursor: (),
                        state: state("fallback"),
                    },
                }],
            },
        ))
        .unwrap();
        let server = if deleted { None } else { Some("server") };
        c.install_authority05(&context, &[record(20, server)], Some((12, 20)))
            .unwrap();
        assert!(c.call_completion05(&call.call_id).unwrap().is_none());
        drop(c);
        let mut c =
            Client::open05(SqliteStore::open(&p).unwrap(), marked_edit(true), "User:u").unwrap();
        let upgraded = c.request_context05().unwrap();
        assert_ne!(upgraded.materialization, context.materialization);
        assert_eq!(c.store_status05().unwrap().bootstrap_cursor, None);
        // Sync reaches the receipt's head; no Bootstrap unit is applied.
        let settled = c
            .install_authority05(&upgraded, &[], Some((20, 30)))
            .unwrap();
        assert_eq!(settled.completions.len(), 1, "deleted={deleted}");
        assert!(c.call_completion05(&call.call_id).unwrap().is_some());
        let row = c.read(&key()).unwrap();
        if deleted {
            assert_eq!(row, None);
        } else {
            assert_eq!(row.unwrap()["text"], "server");
        }
    }
}
#[test]
fn a_noncanonical_target_identity_is_refused_whatever_the_outcome_label() {
    let tally = Schema::from_value(json!({"enums":[],"models":[{"name":"Tally","identity":["n"],"fields":[
        {"name":"n","nullable":false,"type":{"kind":"scalar","name":"int"}},
        {"name":"text","nullable":false,"type":{"kind":"scalar","name":"string"}}]}],
        "actions":[{"name":"Add","version":1,"inputs":[{"kind":"model","name":"tally","model":"Tally","operation":"create","cardinality":"single"}],"outputs":[]}]}))
    .unwrap();
    for label in ["current", "earlier"] {
        let d = tempfile::tempdir().unwrap();
        let mut c = Client::open05(
            SqliteStore::open(d.path().join("db")).unwrap(),
            tally.clone(),
            "User:u",
        )
        .unwrap();
        c.initialize_stream05(0, "1").unwrap();
        c.transaction(|tx| tx.submit_mutation05("Add", 1, json!({"tally":{"n":1,"text":"one"}})))
            .unwrap();
        let b = c.freeze_batch05().unwrap().unwrap();
        let target = |n: Value| wire::SettlementTarget::Private {
            record: wire::ReadRecord {
                key: wire::RecordKey {
                    model: "Tally".into(),
                    identity: json!({ "n": n }),
                },
                cursor: (),
                state: json!({"text":"receipt"}),
            },
        };
        let refused = c.acknowledge_batch05(&ack(
            &b,
            wire::MutationOutcome::Accepted {
                sync_cursor: 0,
                materialization: if label == "current" {
                    b.context.materialization.clone()
                } else {
                    "earlier".into()
                },
                result: Value::Null,
                // `2.0` names record 2, whose canonical identity is `2`.
                targets: vec![target(json!(1)), target(json!(2.0))],
            },
        ));
        assert!(
            refused
                .as_ref()
                .is_err_and(|e| e.to_string().contains("noncanonical settlement identity")),
            "{label}: {refused:?}"
        );
        assert_eq!(c.store_status05().unwrap().last_acknowledged_batch_id, 0);
    }
}
