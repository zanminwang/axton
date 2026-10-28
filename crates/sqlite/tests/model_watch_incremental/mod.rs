//! Real committed-reader counts distinguish incremental refresh from merely
//! suppressing equal full-query results.
use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct Probe {
    reads: Mutex<Vec<(String, Vec<Value>, usize)>>,
    fail_read: AtomicBool,
    fail_commit: AtomicBool,
}
struct MeasuredStore {
    inner: SqliteStore,
    probe: Arc<Probe>,
}
impl ClientStore for MeasuredStore {
    fn begin(&mut self) -> Result<()> {
        self.inner.begin()
    }
    fn commit(&mut self) -> Result<()> {
        if self.probe.fail_commit.swap(false, Ordering::SeqCst) {
            return Err(invalid("injected commit failure"));
        }
        self.inner.commit()
    }
    fn rollback(&mut self) -> Result<()> {
        self.inner.rollback()
    }
    fn savepoint(&mut self, name: &str) -> Result<()> {
        self.inner.savepoint(name)
    }
    fn release(&mut self, name: &str) -> Result<()> {
        self.inner.release(name)
    }
    fn rollback_to(&mut self, name: &str) -> Result<()> {
        self.inner.rollback_to(name)
    }
    fn execute(&mut self, sql: &str, p: &[Value]) -> Result<usize> {
        self.inner.execute(sql, p)
    }
    fn execute_batch(&mut self, sql: &str) -> Result<()> {
        self.inner.execute_batch(sql)
    }
    fn query(&mut self, sql: &str, p: &[Value]) -> Result<SqlRows> {
        self.inner.query(sql, p)
    }
    fn query_committed(&mut self, sql: &str, p: &[Value]) -> Result<SqlRows> {
        let model = sql.contains(" FROM \"Entry\"")
            || sql.contains(" FROM \"Book\"")
            || sql.contains(" FROM \"Comment\"");
        if model && self.probe.fail_read.swap(false, Ordering::SeqCst) {
            return Err(invalid("injected watch read failure"));
        }
        let rows = self.inner.query_committed(sql, p)?;
        if model {
            self.probe
                .reads
                .lock()
                .unwrap()
                .push((sql.into(), p.to_vec(), rows.rows.len()));
        }
        Ok(rows)
    }
    fn read_tables(&mut self, sql: &str) -> Result<std::collections::BTreeSet<String>> {
        self.inner.read_tables(sql)
    }
}
fn measured(schema: Schema) -> (Harness<MeasuredStore>, Arc<Probe>) {
    let dir = tempfile::tempdir().unwrap();
    let probe = Arc::new(Probe::default());
    let store = MeasuredStore {
        inner: SqliteStore::open(dir.path().join("db")).unwrap(),
        probe: probe.clone(),
    };
    let client = Client::open05(store, schema, "User:u").unwrap();
    (
        Harness {
            runtime: ClientRuntime::new(client),
            _dir: dir,
        },
        probe,
    )
}
fn watch(h: &mut Harness<MeasuredStore>, model: &str, filter: Value) -> (Value, Vec<Value>) {
    let (done, events) = h.call(
        "watch",
        json!({"kind":"watch","model":model,"spec":{"filter":filter}}),
    );
    assert_eq!(done["ok"], true, "{events:?}");
    let id = done["value"]["observerId"].clone();
    assert_eq!(
        rows(&events, &id).len(),
        1,
        "initial result always delivered"
    );
    (id, events)
}
fn op(id: &str, values: Value) -> Operation {
    Operation {
        model: "Entry".into(),
        op: OperationKind::Update,
        identity: json!({"id":id}),
        values: Some(values),
    }
}
fn publish(h: &mut Harness<MeasuredStore>) -> Vec<Value> {
    h.call("publish", json!({"kind":"status"})).1
}
fn clear(probe: &Probe) {
    probe.reads.lock().unwrap().clear();
}
fn read_sizes(probe: &Probe) -> Vec<usize> {
    probe.reads.lock().unwrap().iter().map(|r| r.2).collect()
}

#[test]
fn metadata_does_no_model_read_and_a_changed_identity_does_not_reread_the_result() {
    let (mut h, probe) = measured(schema());
    h.runtime
        .client()
        .transaction(|tx| {
            for i in 0..1000 {
                tx.direct(common05::create(
                    "Entry",
                    &format!("e{i:04}"),
                    json!({"text":"before","note":null}),
                ))?;
            }
            Ok(())
        })
        .unwrap();
    let (observer, initial) = watch(&mut h, "Entry", json!({}));
    assert_eq!(rows(&initial, &observer)[0].as_array().unwrap().len(), 1000);
    assert_eq!(
        read_sizes(&probe),
        [1000],
        "registration reads exactly once"
    );
    clear(&probe);
    for command in [
        sync05::StoreCommand::Needs,
        sync05::StoreCommand::Freeze,
        sync05::StoreCommand::Needs,
    ] {
        h.runtime.store_worker05(command).unwrap();
    }
    assert!(
        read_sizes(&probe).is_empty(),
        "metadata commits must not run Model SELECTs"
    );
    assert!(rows(&h.events(), &observer).is_empty());
    h.runtime
        .client()
        .transaction(|tx| {
            tx.direct(op("e0999", json!({"text":"intermediate"})))?;
            tx.direct(op("e0999", json!({"text":"after"})))
        })
        .unwrap();
    let events = publish(&mut h);
    assert_eq!(
        read_sizes(&probe),
        [1],
        "one deduplicated identity, not 1000 rows"
    );
    assert_eq!(
        rows(&events, &observer),
        [Value::Array(
            h.runtime.client().query("Entry", &json!({})).unwrap()
        )]
    );
    clear(&probe);
    h.runtime
        .client()
        .transaction(|tx| tx.direct(op("e0999", json!({"text":"after"}))))
        .unwrap();
    assert!(
        rows(&publish(&mut h), &observer).is_empty(),
        "equal final row suppressed"
    );
    assert_eq!(read_sizes(&probe), [1]);
}

#[test]
fn filters_enter_and_exit_with_sql_null_rules_and_multiple_commits_keep_identity_order() {
    let (mut h, probe) = measured(schema());
    let (observer, _) = watch(&mut h, "Entry", json!({"note":null}));
    h.runtime
        .client()
        .transaction(|tx| {
            tx.direct(common05::create(
                "Entry",
                "e",
                json!({"text":"excluded","note":"x"}),
            ))
        })
        .unwrap();
    assert!(rows(&publish(&mut h), &observer).is_empty());
    h.runtime
        .client()
        .transaction(|tx| tx.direct(op("e", json!({"note":null}))))
        .unwrap();
    assert_eq!(
        rows(&publish(&mut h), &observer),
        [json!([{"id":"e","text":"excluded","note":null}])]
    );
    h.runtime
        .client()
        .transaction(|tx| tx.direct(op("e", json!({"note":"outside"}))))
        .unwrap();
    assert_eq!(rows(&publish(&mut h), &observer), [json!([])]);
    clear(&probe);
    // UTF-16 compares U+10000 before U+E000, unlike UTF-8 byte order.
    for id in ["\u{e000}", "\u{10000}"] {
        h.runtime
            .client()
            .transaction(|tx| {
                tx.direct(common05::create(
                    "Entry",
                    id,
                    json!({"text":id,"note":null}),
                ))
            })
            .unwrap();
    }
    let events = publish(&mut h);
    assert_eq!(
        read_sizes(&probe),
        [2],
        "both generations accumulated in one key query"
    );
    let result = rows(&events, &observer);
    assert_eq!(
        result[0]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["\u{10000}", "\u{e000}"]
    );
    assert_eq!(
        result,
        [Value::Array(
            h.runtime
                .client()
                .query("Entry", &json!({"note":null}))
                .unwrap()
        )]
    );
}

#[test]
fn failed_commit_and_rolled_back_savepoints_do_not_leak_model_candidates() {
    let (mut h, probe) = measured(schema());
    h.commit("seed", create("e", "base"));
    let (observer, _) = watch(&mut h, "Entry", json!({}));
    clear(&probe);
    h.runtime
        .client()
        .transaction(|tx| {
            let failed = tx.savepoint(|tx| {
                tx.direct(op("e", json!({"text":"discarded"})))?;
                Err::<(), _>(invalid("discard"))
            });
            assert!(failed.is_err());
            Ok(())
        })
        .unwrap();
    assert!(rows(&publish(&mut h), &observer).is_empty());
    assert!(
        read_sizes(&probe).is_empty(),
        "nested savepoint restores candidates"
    );
    h.runtime.client().begin_session().unwrap();
    h.runtime.client().session_savepoint().unwrap();
    h.runtime
        .client()
        .session(|tx| tx.direct(op("e", json!({"text":"discard session scope"}))))
        .unwrap();
    h.runtime.client().session_rollback_savepoint().unwrap();
    h.runtime.client().commit_session().unwrap();
    assert!(rows(&publish(&mut h), &observer).is_empty());
    assert!(read_sizes(&probe).is_empty());
    h.runtime.client().begin_session().unwrap();
    h.runtime
        .client()
        .session(|tx| tx.direct(op("e", json!({"text":"discard session"}))))
        .unwrap();
    h.runtime.client().rollback_session().unwrap();
    probe.fail_commit.store(true, Ordering::SeqCst);
    assert!(
        h.runtime
            .client()
            .transaction(|tx| tx.direct(op("e", json!({"text":"failed commit"}))))
            .is_err()
    );
    assert!(rows(&publish(&mut h), &observer).is_empty());
    assert!(read_sizes(&probe).is_empty());
    h.commit("kept", change("Entry", "e", json!({"text":"kept"})));
    assert_eq!(read_sizes(&probe), [1]);
    assert_eq!(h.entry("e").unwrap()["text"], "kept");
}

#[test]
fn authority_replay_rejection_cache_and_reset_refresh_the_committed_projection() {
    let (mut h, probe) = measured(schema());
    h.runtime.client().initialize_stream05(0).unwrap();
    let context = h.runtime.client().request_context05().unwrap();
    let authority = |cursor, text: &str, note: Value| v05::AuthorityChange::Record {
        key: v05::RecordKey {
            model: "Entry".into(),
            identity: json!({"id":"e"}),
        },
        cursor,
        state: json!({"text":text,"note":note}),
    };
    h.runtime
        .client()
        .install_authority05(&context, &[authority(1, "base", Value::Null)], Some((0, 1)))
        .unwrap();
    let (observer, _) = watch(&mut h, "Entry", json!({}));
    clear(&probe);
    h.runtime
        .client()
        .transaction(|tx| {
            tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"mine"}}), vec![])
        })
        .unwrap();
    assert_eq!(rows(&publish(&mut h), &observer), [json!([row("mine")])]);
    let batch = h.runtime.client().freeze_batch05().unwrap().unwrap();
    h.runtime
        .client()
        .install_authority05(
            &context,
            &[authority(2, "canonical", json!("new"))],
            Some((1, 2)),
        )
        .unwrap();
    assert_eq!(
        rows(&publish(&mut h), &observer),
        [json!([{"id":"e","text":"mine","note":"new"}])],
        "no transient deletion is published"
    );
    h.runtime
        .client()
        .acknowledge_batch05(&v05::BatchAcknowledgement {
            context: batch.context,
            batch_id: batch.batch_id,
            digest: batch.digest,
            results: vec![v05::MutationResult {
                mutation_id: batch.mutations[0].id,
                outcome: v05::MutationOutcome::Rejected {
                    code: "refused".into(),
                    message: None,
                },
            }],
        })
        .unwrap();
    assert_eq!(
        rows(&publish(&mut h), &observer),
        [json!([{"id":"e","text":"canonical","note":"new"}])]
    );
    assert_eq!(read_sizes(&probe), [1, 1, 1]);
    clear(&probe);
    h.runtime
        .client()
        .install_authority05(&context, &[authority(2, "canonical", json!("new"))], None)
        .unwrap();
    assert!(rows(&publish(&mut h), &observer).is_empty());
    assert!(
        read_sizes(&probe).is_empty(),
        "duplicate authority changes no Model row"
    );
    h.runtime
        .client()
        .install_cache05(
            &[v05::ReadRecord {
                key: v05::RecordKey {
                    model: "Entry".into(),
                    identity: json!({"id":"f"}),
                },
                cursor: (),
                state: json!({"text":"cache","note":null}),
            }],
            true,
        )
        .unwrap();
    let events = publish(&mut h);
    assert_eq!(read_sizes(&probe), [1]);
    assert_eq!(rows(&events, &observer)[0].as_array().unwrap().len(), 2);
    clear(&probe);
    let events = h.commit("reset", json!({"kind":"resetStore","discardPending":true}));
    assert_eq!(rows(&events, &observer), [json!([])]);
    let reads = probe.reads.lock().unwrap();
    assert_eq!(reads.len(), 1);
    assert!(
        reads[0].1.is_empty(),
        "bulk reset uses the full-result fallback"
    );
}

#[test]
fn cascading_deletes_include_child_model_keys() {
    let (mut h, probe) = measured(common05::family_schema());
    h.runtime
        .client()
        .transaction(|tx| {
            tx.direct(common05::create("Book", "b", json!({"title":"book"})))?;
            tx.direct(common05::create(
                "Comment",
                "c",
                json!({"bookId":"b","text":"child"}),
            ))
        })
        .unwrap();
    let (observer, _) = watch(&mut h, "Comment", json!({}));
    clear(&probe);
    h.runtime
        .client()
        .transaction(|tx| {
            tx.direct(Operation {
                model: "Book".into(),
                identity: json!({"id":"b"}),
                op: OperationKind::Update,
                values: Some(json!({"title":"unrelated parent update"})),
            })
        })
        .unwrap();
    assert!(rows(&publish(&mut h), &observer).is_empty());
    assert!(
        read_sizes(&probe).is_empty(),
        "an unrelated Model must not be reread"
    );
    h.runtime
        .client()
        .transaction(|tx| {
            tx.direct(Operation {
                model: "Book".into(),
                identity: json!({"id":"b"}),
                op: OperationKind::Delete,
                values: None,
            })
        })
        .unwrap();
    assert_eq!(rows(&publish(&mut h), &observer), [json!([])]);
    assert_eq!(read_sizes(&probe), [0]);
}

#[test]
fn composite_numeric_identity_and_normalized_filters_match_full_queries_across_key_batches() {
    let schema = Schema::from_value(json!({"enums":[],"models":[{
        "name":"Entry","identity":["group","seq"],"fields":[
            {"name":"group","nullable":false,"type":{"kind":"scalar","name":"string"}},
            {"name":"seq","nullable":false,"type":{"kind":"scalar","name":"int"}},
            {"name":"enabled","nullable":false,"type":{"kind":"scalar","name":"boolean"}},
            {"name":"when","nullable":false,"type":{"kind":"scalar","name":"dateTime"}},
            {"name":"tags","nullable":false,"type":{"kind":"list","element":{"kind":"scalar","name":"string"}}}
        ]
    }]})).unwrap();
    let (mut h, probe) = measured(schema);
    let create = |group: &str, seq| Operation {
        model: "Entry".into(),
        identity: json!({"group":group,"seq":seq}),
        op: OperationKind::Create,
        values: Some(json!({"enabled":true,"when":"2026-10-28T04:00:00Z","tags":["initial"]})),
    };
    for (group, seq) in [("a", 10), ("b", -1), ("a", 2)] {
        h.runtime
            .client()
            .transaction(|tx| tx.direct(create(group, seq)))
            .unwrap();
    }
    let filter = json!({"enabled":true,"when":"2026-10-28T05:00:00+01:00"});
    let (observer, initial) = watch(&mut h, "Entry", filter.clone());
    assert_eq!(
        rows(&initial, &observer)[0]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| (row["group"].as_str().unwrap(), row["seq"].as_i64().unwrap()))
            .collect::<Vec<_>>(),
        [("a", 2), ("a", 10), ("b", -1)]
    );
    clear(&probe);
    h.runtime
        .client()
        .transaction(|tx| {
            for seq in 1000..1260 {
                tx.direct(create("a", seq))?;
            }
            Ok(())
        })
        .unwrap();
    let events = publish(&mut h);
    assert_eq!(
        read_sizes(&probe),
        [128, 128, 4],
        "bounded key queries read only the 260 affected rows"
    );
    assert_eq!(
        rows(&events, &observer),
        [Value::Array(
            h.runtime.client().query("Entry", &filter).unwrap()
        )]
    );
    clear(&probe);
    h.runtime
        .client()
        .transaction(|tx| {
            tx.direct(Operation {
                model: "Entry".into(),
                identity: json!({"group":"a","seq":2}),
                op: OperationKind::Update,
                values: Some(json!({"enabled":false,"tags":["changed"]})),
            })
        })
        .unwrap();
    let events = publish(&mut h);
    assert_eq!(
        read_sizes(&probe),
        [0],
        "SQL bool filter removes the one candidate"
    );
    assert_eq!(
        rows(&events, &observer),
        [Value::Array(
            h.runtime.client().query("Entry", &filter).unwrap()
        )]
    );
}

#[test]
fn a_failed_incremental_read_keeps_the_watch_and_recovers_from_a_full_result() {
    let (mut h, probe) = measured(schema());
    h.commit("seed", create("e", "base"));
    let (observer, _) = watch(&mut h, "Entry", json!({}));
    clear(&probe);
    probe.fail_read.store(true, Ordering::SeqCst);
    let events = h.commit("change", change("Entry", "e", json!({"text":"after"})));
    assert!(rows(&events, &observer).is_empty());
    assert!(events.iter().any(|e|e["type"]=="report"&&e.to_string().contains("injected watch read failure")));
    h.runtime
        .store_worker05(sync05::StoreCommand::Needs)
        .unwrap();
    assert_eq!(rows(&h.events(), &observer), [json!([row("after")])]);
    let reads = probe.reads.lock().unwrap();
    assert_eq!(reads.len(), 1);
    assert!(
        reads[0].1.is_empty(),
        "lost invalidation recovers through full refresh"
    );
}
