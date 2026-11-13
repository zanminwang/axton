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
    let client = Client::open05(store, common05::with_crud(schema), "User:u").unwrap();
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
/// Entry `id` with `values`, as the `row` of a test Mutation.
fn op(id: &str, mut values: Value) -> Value {
    values["id"] = json!(id);
    values
}
/// Submit the `with_crud` Mutation `{verb}{model}` of `row` inside `tx`: the
/// write each change below commits.
fn submit<S: ClientStore>(
    tx: &mut ClientTransaction<'_, S>,
    verb: &str,
    model: &str,
    row: Value,
) -> Result<()> {
    tx.submit_mutation05(&format!("{verb}{model}"), 1, json!({ "row": row }))
        .map(|_| ())
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
                submit(
                    tx,
                    "Create",
                    "Entry",
                    op(&format!("e{i:04}"), json!({"text":"before","note":null})),
                )?;
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
            submit(
                tx,
                "Update",
                "Entry",
                op("e0999", json!({"text":"intermediate"})),
            )?;
            submit(tx, "Update", "Entry", op("e0999", json!({"text":"after"})))
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
        .transaction(|tx| submit(tx, "Update", "Entry", op("e0999", json!({"text":"after"}))))
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
            submit(
                tx,
                "Create",
                "Entry",
                op("e", json!({"text":"excluded","note":"x"})),
            )
        })
        .unwrap();
    assert!(rows(&publish(&mut h), &observer).is_empty());
    h.runtime
        .client()
        .transaction(|tx| submit(tx, "Update", "Entry", op("e", json!({"note":null}))))
        .unwrap();
    assert_eq!(
        rows(&publish(&mut h), &observer),
        [json!([{"id":"e","text":"excluded","note":null}])]
    );
    h.runtime
        .client()
        .transaction(|tx| submit(tx, "Update", "Entry", op("e", json!({"note":"outside"}))))
        .unwrap();
    assert_eq!(rows(&publish(&mut h), &observer), [json!([])]);
    clear(&probe);
    // UTF-16 compares U+10000 before U+E000, unlike UTF-8 byte order.
    for id in ["\u{e000}", "\u{10000}"] {
        h.runtime
            .client()
            .transaction(|tx| {
                submit(
                    tx,
                    "Create",
                    "Entry",
                    op(id, json!({"text":id,"note":null})),
                )
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
    h.commit("seed", put("Entry", "e", json!({"text":"base"})));
    let (observer, _) = watch(&mut h, "Entry", json!({}));
    clear(&probe);
    h.runtime
        .client()
        .transaction(|tx| {
            let failed = tx.savepoint(|tx| {
                submit(tx, "Update", "Entry", op("e", json!({"text":"discarded"})))?;
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
        .session(|tx| {
            submit(
                tx,
                "Update",
                "Entry",
                op("e", json!({"text":"discard session scope"})),
            )
        })
        .unwrap();
    h.runtime.client().session_rollback_savepoint().unwrap();
    h.runtime.client().commit_session().unwrap();
    assert!(rows(&publish(&mut h), &observer).is_empty());
    assert!(read_sizes(&probe).is_empty());
    h.runtime.client().begin_session().unwrap();
    h.runtime
        .client()
        .session(|tx| {
            submit(
                tx,
                "Update",
                "Entry",
                op("e", json!({"text":"discard session"})),
            )
        })
        .unwrap();
    h.runtime.client().rollback_session().unwrap();
    probe.fail_commit.store(true, Ordering::SeqCst);
    assert!(
        h.runtime
            .client()
            .transaction(|tx| submit(
                tx,
                "Update",
                "Entry",
                op("e", json!({"text":"failed commit"}))
            ))
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
    h.runtime.client().initialize_stream05(0, "1").unwrap();
    let context = h.runtime.client().request_context05().unwrap();
    let authority = |cursor, text: &str, note: Value| wire::AuthorityChange::Record {
        key: wire::RecordKey {
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
            tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"mine"}}))
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
        .acknowledge_batch05(&wire::BatchAcknowledgement {
            context: batch.context,
            batch_id: batch.batch_id,
            digest: batch.digest,
            results: vec![wire::MutationResult {
                mutation_id: batch.mutations[0].id,
                outcome: wire::MutationOutcome::Rejected {
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
            &[wire::ReadRecord {
                key: wire::RecordKey {
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
            submit(tx, "Create", "Book", op("b", json!({"title":"book"})))?;
            submit(
                tx,
                "Create",
                "Comment",
                op("c", json!({"bookId":"b","text":"child"})),
            )
        })
        .unwrap();
    let (observer, _) = watch(&mut h, "Comment", json!({}));
    clear(&probe);
    h.runtime
        .client()
        .transaction(|tx| {
            submit(
                tx,
                "Update",
                "Book",
                json!({"id":"b","title":"unrelated parent update"}),
            )
        })
        .unwrap();
    assert!(rows(&publish(&mut h), &observer).is_empty());
    assert!(
        read_sizes(&probe).is_empty(),
        "an unrelated Model must not be reread"
    );
    h.runtime
        .client()
        .transaction(|tx| submit(tx, "Delete", "Book", json!({"id":"b"})))
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
    let create = |group: &str, seq| json!({"group":group,"seq":seq,"enabled":true,"when":"2026-10-28T04:00:00Z","tags":["initial"]});
    for (group, seq) in [("a", 10), ("b", -1), ("a", 2)] {
        h.runtime
            .client()
            .transaction(|tx| submit(tx, "Create", "Entry", create(group, seq)))
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
                submit(tx, "Create", "Entry", create("a", seq))?;
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
            submit(
                tx,
                "Update",
                "Entry",
                json!({"group":"a","seq":2,"enabled":false,"tags":["changed"]}),
            )
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
    h.commit("seed", put("Entry", "e", json!({"text":"base"})));
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

fn snapshots(events: &[Value], observer: &Value) -> Vec<Value> {
    events
        .iter()
        .filter(|e| e["type"] == "observerChanged" && &e["observerId"] == observer)
        .map(|e| e["snapshot"].clone())
        .collect()
}
fn delta_watch(h: &mut Harness<MeasuredStore>, filter: Value) -> (Value, Value) {
    let (done, events) = h.call(
        "delta-watch",
        json!({"kind":"watch","model":"Entry",
        "spec":{"filter":filter,"delivery":"splice-v1"}}),
    );
    assert_eq!(done["ok"], true, "{events:?}");
    let id = done["value"]["observerId"].clone();
    let first = snapshots(&events, &id).remove(0);
    assert_eq!(first["protocol"], "splice-v1");
    assert_eq!(first["version"], 1);
    (id, first)
}
fn apply_splices(result: &mut Vec<Value>, snapshot: &Value, version: &mut u64) {
    assert_eq!(snapshot["protocol"], "splice-v1");
    assert_eq!(snapshot["version"], *version + 1);
    if let Some(rows) = snapshot["rows"].as_array() {
        *result = rows.clone();
    } else {
        assert_eq!(snapshot["baseVersion"], *version);
        for splice in snapshot["splices"].as_array().unwrap() {
            let index = splice["index"].as_u64().unwrap() as usize;
            let count = splice["deleteCount"].as_u64().unwrap() as usize;
            result.splice(
                index..index + count,
                splice["rows"].as_array().unwrap().clone(),
            );
        }
    }
    *version += 1;
}
#[test]
fn delta_delivery_moves_only_changed_rows_and_preserves_order_and_legacy_callers() {
    let (mut h, probe) = measured(schema());
    h.runtime
        .client()
        .transaction(|tx| {
            for i in 0..1000 {
                submit(
                    tx,
                    "Create",
                    "Entry",
                    op(
                        &format!("e{i:04}"),
                        json!({"text":"x".repeat(512),"note":null}),
                    ),
                )?;
            }
            Ok(())
        })
        .unwrap();
    let (id, first) = delta_watch(&mut h, json!({}));
    let (legacy, _) = watch(&mut h, "Entry", json!({}));
    let mut result = first["rows"].as_array().unwrap().clone();
    let retained = result.clone();
    let mut version = 1;
    clear(&probe);
    h.runtime
        .client()
        .transaction(|tx| {
            submit(
                tx,
                "Update",
                "Entry",
                op("e0523", json!({"text":"changed"})),
            )?;
            submit(tx, "Delete", "Entry", json!({"id":"e0000"}))?;
            submit(
                tx,
                "Create",
                "Entry",
                op("a", json!({"text":"new","note":null})),
            )
        })
        .unwrap();
    let events = publish(&mut h);
    let patch = snapshots(&events, &id).remove(0);
    assert!(
        patch.get("rows").is_none(),
        "no complete result after key update"
    );
    assert!(
        serde_json::to_vec(&patch).unwrap().len() < 1000,
        "bounded output, not 512KB"
    );
    assert_eq!(patch["splices"].as_array().unwrap().len(), 3);
    apply_splices(&mut result, &patch, &mut version);
    let authoritative = h.runtime.client().query("Entry", &json!({})).unwrap();
    assert_eq!(result, authoritative);
    assert_eq!(retained[523]["text"], "x".repeat(512));
    let full = snapshots(&events, &legacy).remove(0);
    assert!(full.get("protocol").is_none());
    assert_eq!(full["rows"], json!(authoritative));
    assert_eq!(read_sizes(&probe), [2, 2, 1000]); // each watch reads only changed keys; explicit comparison reads all
    h.runtime
        .client()
        .transaction(|tx| {
            submit(
                tx,
                "Update",
                "Entry",
                op("e0523", json!({"text":"changed"})),
            )
        })
        .unwrap();
    assert!(
        snapshots(&publish(&mut h), &id).is_empty(),
        "same committed result suppressed"
    );
}
#[test]
fn delta_full_recovery_reset_and_close_do_not_require_a_second_query() {
    let (mut h, probe) = measured(schema());
    h.commit("seed", put("Entry", "e", json!({"text":"before"})));
    let (id, first) = delta_watch(&mut h, json!({}));
    let mut result = first["rows"].as_array().unwrap().clone();
    let mut version = 1;
    clear(&probe);
    let (done, events) = h.call("recover", json!({"kind":"watchSnapshot","observerId":id}));
    assert_eq!(done["ok"], true, "{events:?}");
    let reset = snapshots(&events, &id).remove(0);
    assert!(reset["rows"].is_array());
    apply_splices(&mut result, &reset, &mut version);
    assert!(
        read_sizes(&probe).is_empty(),
        "recovery uses authoritative cache"
    );
    probe.fail_read.store(true, Ordering::SeqCst);
    h.runtime
        .client()
        .transaction(|tx| submit(tx, "Update", "Entry", op("e", json!({"text":"after"}))))
        .unwrap();
    let failed = publish(&mut h);
    assert!(snapshots(&failed, &id).is_empty());
    h.runtime
        .client()
        .transaction(|tx| submit(tx, "Update", "Entry", op("e", json!({"text":"recovered"}))))
        .unwrap();
    let events = publish(&mut h);
    let recovered = snapshots(&events, &id).remove(0);
    assert!(
        recovered["rows"].is_array(),
        "failed query recovers with full result"
    );
    apply_splices(&mut result, &recovered, &mut version);
    assert_eq!(result, [row("recovered")]);
    let reset = h.commit("reset", json!({"kind":"resetStore","discardPending":true}));
    let reset = snapshots(&reset, &id).remove(0);
    assert!(reset["rows"].is_array());
    apply_splices(&mut result, &reset, &mut version);
    assert!(result.is_empty());
    h.submit(json!({"type":"close"})).unwrap();
    let events = h.run();
    let terminal = snapshots(&events, &id).remove(0);
    assert_eq!(terminal, json!({"kind":"watch","closed":true}));
}
#[test]
fn delta_splice_sequences_match_committed_filter_results() {
    let (mut h, _) = measured(schema());
    // Deliveries, not Mutations: a key is deleted and created again many
    // times, which only the Stream does without lifecycle dependencies.
    h.runtime.client().initialize_stream05(0, "1").unwrap();
    let context = h.runtime.client().request_context05().unwrap();
    let mut cursor = 0;
    let filter = json!({"note":null});
    let (id, first) = delta_watch(&mut h, filter.clone());
    let mut result = first["rows"].as_array().unwrap().clone();
    let mut version = 1;
    let mut present = std::collections::BTreeSet::new();
    let mut rng = 17_u64;
    for step in 0..150 {
        for _ in 0..4 {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let key = format!("{:02}", (rng >> 32) % 30);
            let pick = (rng >> 40) % 4;
            let values = json!({"text":format!("step {step}"),"note":if pick == 0 {json!("out")} else {Value::Null}});
            let state = if present.contains(&key) && pick == 1 {
                present.remove(&key);
                Value::Null
            } else {
                present.insert(key.clone());
                values
            };
            cursor += 1;
            h.runtime
                .client()
                .install_authority05(
                    &context,
                    &[wire::AuthorityChange::Record {
                        key: wire::RecordKey {
                            model: "Entry".into(),
                            identity: json!({ "id": key }),
                        },
                        cursor,
                        state,
                    }],
                    None,
                )
                .unwrap();
        }
        for snapshot in snapshots(&publish(&mut h), &id) {
            apply_splices(&mut result, &snapshot, &mut version);
        }
        assert_eq!(
            result,
            h.runtime.client().query("Entry", &filter).unwrap(),
            "step {step}"
        );
    }
    let _ = h.call("unwatch", json!({"kind":"unwatch","observerId":id}));
    h.commit("late", put("Entry", "late", json!({"text":"ignored"})));
    assert!(snapshots(&h.events(), &id).is_empty());
}
