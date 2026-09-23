//! Reproducible diagnostic, not a throughput guarantee. Each enqueue is a real SQLite commit.
use axton_client::{Client, Mutation, Operation, OperationKind};
use axton_core::{AuthorityRecord, CursorRange, PullPage, Schema};
use axton_sqlite::SqliteStore;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Instant;
fn main() {
    let schema = Schema::from_value(
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap(),
    )
    .unwrap();
    for count in [10, 1000] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capacity.sqlite");
        let mut client = Client::open(SqliteStore::open(&path).unwrap(), schema.clone()).unwrap();
        let page = |from, to| PullPage {
            cursors: BTreeMap::from([("book".to_string(), CursorRange { from, to, head: to })]),
            changes: vec![AuthorityRecord {
                model: "Entry".into(),
                identity: json!({"id":"one"}),
                stamp: to,
                state: json!({"text":"authority","note":null}),
                error: None,
            }],
        };
        client
            .transaction(|tx| tx.set_channel("book".into(), true))
            .unwrap();
        client.apply_page(page(0, 1)).unwrap();
        let mut samples = Vec::new();
        for i in 0..count {
            let start = Instant::now();
            client
                .transaction(|tx| {
                    tx.enqueue(Mutation::new(
                        "Edit",
                        vec![Operation {
                            model: "Entry".into(),
                            op: OperationKind::Update,
                            identity: json!({"id":"one"}),
                            values: Some(json!({"text":format!("local-{i}")})),
                        }],
                    ))?;
                    Ok(())
                })
                .unwrap();
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(f64::total_cmp);
        let start = Instant::now();
        client.apply_page(page(1, 2)).unwrap();
        let replay_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(client.pending_count().unwrap(), count);
        let key = schema.record_key("Entry", &json!({"id":"one"})).unwrap();
        assert_eq!(
            client.read(&key).unwrap().unwrap()["text"],
            format!("local-{}", count - 1)
        );
        println!(
            "{}",
            json!({"queue":count,"enqueue_p50_ms":samples[(count-1)/2],"enqueue_p95_ms":samples[(count*95/100).min(count-1)],"page_replay_ms":replay_ms,"sqlite_commits":count+2})
        );
    }
}
