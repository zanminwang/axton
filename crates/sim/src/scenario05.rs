//! Deterministic recovery orders against real protocol-5 SQLite state.
//! Cloud acknowledgements are fixtures; the actual PostgreSQL/SDK join lives
//! in integration/v05-sdk and must pass independently.
use axton_client::{Client, RecordKey, Schema};
use axton_protocols::sync as v05;
use axton_sqlite::SqliteStore;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub enum Boundary05 {
    Queued,
    Frozen,
    SavedAcceptance,
    Covered,
    Settled,
}
#[derive(Debug)]
pub struct RecoveryTrace05 {
    pub exact_retry: bool,
    pub unfinished_after_receipt: bool,
    pub durable_completion: bool,
    /// The row with the later pending Mutation applied on top.
    pub optimistic: Value,
    /// The row once that later Mutation is discarded: what the receipt and
    /// authority left.
    pub visible: Value,
    pub boundaries: Vec<Boundary05>,
}
fn schema() -> Schema {
    let mut value: Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    value["actions"] = json!([
        {"name":"Write","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"create","cardinality":"single"}],"outputs":[]},
        {"name":"Edit","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"update","cardinality":"single"}],"outputs":[]}
    ]);
    Schema::from_value(value).unwrap()
}
fn open(path: &Path) -> Client<SqliteStore> {
    Client::open05(SqliteStore::open(path).unwrap(), schema(), "User:scenario").unwrap()
}
// One entry of a record's history above its base, as stored.
#[derive(Debug)]
struct Step {
    // `(ordinal, position, sequence)`: a pending operation at its Mutation's
    // position, an accepted write at its wire operation's position after
    // the operations that share it.
    order: (u64, u64, u64),
    op: String,
    values: Option<Value>,
    pending: bool,
}
fn fold(row: &mut Option<Value>, step: &Step) -> bool {
    match step.op.as_str() {
        "create" if row.is_some() && step.pending => return false,
        "create" => {
            let mut created = row.take().unwrap_or_else(|| json!({}));
            for (field, value) in step.values.as_ref().unwrap().as_object().unwrap() {
                created[field] = value.clone();
            }
            *row = Some(created);
        }
        "update" => match row.as_mut() {
            Some(current) => {
                for (field, value) in step.values.as_ref().unwrap().as_object().unwrap() {
                    current[field] = value.clone();
                }
            }
            None => return !step.pending,
        },
        "delete" => *row = None,
        other => panic!("unknown operation {other}"),
    }
    true
}
fn decoded(value: &Value) -> Option<Value> {
    value
        .as_str()
        .map(|text| serde_json::from_str(text).unwrap())
}
/// The projection invariant: every Model row equals the rebuild of its key
/// from its base, its pending operations and its accepted writes, in
/// committed local order. A record nothing is pending on has no before image
/// and no retained write: its row is its own base. A pending operation that
/// no longer replays leaves the base with the accepted writes visible.
/// `schema` is the one `client` was opened with.
pub fn assert_projection05(client: &mut Client<SqliteStore>, schema: &Schema) {
    let mut histories: BTreeMap<(String, String), Vec<Step>> = BTreeMap::new();
    for row in client
        .read_sql(
            "SELECT o.mutation_id,o.step,o.kind,o.model,o.identity,o.operation,o.value FROM axton_mutation_queue_operation o JOIN axton_mutation_queue q ON q.id=o.mutation_id WHERE o.model IS NOT NULL AND q.reconciled=0 AND q.rejection_code IS NULL",
            &[],
        )
        .unwrap()
    {
        let kind = row["kind"].as_str().unwrap();
        assert!(matches!(kind, "wire" | "effect"), "operation kind {kind}");
        histories
            .entry((
                row["model"].as_str().unwrap().into(),
                row["identity"].as_str().unwrap().into(),
            ))
            .or_default()
            .push(Step {
                order: (
                    row["mutation_id"].as_u64().unwrap(),
                    row["step"].as_u64().unwrap(),
                    0,
                ),
                op: row["operation"].as_str().unwrap().into(),
                values: decoded(&row["value"]),
                pending: true,
            });
    }
    let accepted = client
        .read_sql(
            "SELECT sequence,ordinal,position,disposition,model,identity,op,\"values\" FROM axton_local_write",
            &[],
        )
        .unwrap();
    for row in accepted {
        assert_eq!(row["disposition"], "accepted", "{row}");
        let key = (
            row["model"].as_str().unwrap().to_string(),
            row["identity"].as_str().unwrap().to_string(),
        );
        assert!(
            histories.contains_key(&key),
            "an accepted write outlived the pending work beneath it: {row}"
        );
        histories.get_mut(&key).unwrap().push(Step {
            order: (
                row["ordinal"].as_u64().unwrap(),
                row["position"].as_u64().unwrap(),
                row["sequence"].as_u64().unwrap(),
            ),
            op: row["op"].as_str().unwrap().into(),
            values: decoded(&row["values"]),
            pending: false,
        });
    }
    for model in &schema.models {
        let identity = |row: &Value| {
            let fields: serde_json::Map<String, Value> = model
                .identity
                .iter()
                .map(|field| (field.clone(), row[field].clone()))
                .collect();
            serde_json::to_string(&Value::Object(fields)).unwrap()
        };
        let before: BTreeMap<String, Value> = client
            .read_sql(
                &format!("SELECT * FROM \"axton_before_{}\"", model.name),
                &[],
            )
            .unwrap()
            .into_iter()
            .map(|row| (identity(&row), row))
            .collect();
        let mut keys: Vec<String> = before.keys().cloned().collect();
        keys.extend(
            histories
                .keys()
                .filter(|(m, _)| *m == model.name)
                .map(|(_, i)| i.clone()),
        );
        keys.sort();
        keys.dedup();
        for encoded in keys {
            let steps = histories.get_mut(&(model.name.clone(), encoded.clone()));
            let key = RecordKey {
                model: model.name.clone(),
                identity: serde_json::from_str(&encoded).unwrap(),
            };
            let Some(steps) = steps else {
                panic!("{key:?} has a before image and nothing pending");
            };
            steps.sort_by_key(|step| step.order);
            let base = before.get(&encoded).cloned();
            let mut row = base.clone();
            if !steps.iter().all(|step| fold(&mut row, step)) {
                row = base;
                for step in steps.iter().filter(|step| !step.pending) {
                    fold(&mut row, step);
                }
            }
            let row = row.map(|mut row| {
                for (field, value) in key.identity.as_object().unwrap() {
                    row[field] = value.clone();
                }
                row
            });
            assert_eq!(client.read(&key).unwrap(), row, "{key:?} {steps:?}");
        }
    }
}
/// Reopen after every durable boundary. `authority_first` permutes the only
/// two independent sources: the Batch receipt and content authority.
pub fn run_recovery05(path: &Path, authority_first: bool, private: bool) -> RecoveryTrace05 {
    let schema = schema();
    let key = RecordKey {
        model: "Entry".into(),
        identity: json!({"id":"e"}),
    };
    let mut client = open(path);
    assert_projection05(&mut client, &schema);
    client.initialize_stream05(0, "1").unwrap();
    assert_projection05(&mut client, &schema);
    let call = client
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"optimistic","note":null}}),
            )
        })
        .unwrap();
    assert_projection05(&mut client, &schema);
    drop(client);
    let mut boundaries = vec![Boundary05::Queued];
    client = open(path);
    assert_projection05(&mut client, &schema);
    let batch = client.freeze_batch05().unwrap().unwrap();
    assert_projection05(&mut client, &schema);
    let bytes = v05::encode(&batch).unwrap();
    drop(client);
    boundaries.push(Boundary05::Frozen);
    client = open(path);
    assert_projection05(&mut client, &schema);
    let exact_retry = v05::encode(&client.freeze_batch05().unwrap().unwrap()).unwrap() == bytes;
    // A later Mutation on the same record, queued after the frozen Batch and
    // never sent: it stays pending on top of whatever settles beneath it.
    let later = client
        .transaction(|tx| {
            tx.submit_mutation05("Edit", 1, json!({"entry":{"id":"e","text":"later"}}))
        })
        .unwrap();
    assert_projection05(&mut client, &schema);
    let authority = v05::AuthorityChange::Record {
        key: v05::RecordKey {
            model: key.model.clone(),
            identity: key.identity.clone(),
        },
        cursor: 7,
        state: json!({"text":"canonical","note":null}),
    };
    let context = client.request_context05().unwrap();
    if authority_first && !private {
        client
            .install_authority05(&context, std::slice::from_ref(&authority), None)
            .unwrap();
        assert_projection05(&mut client, &schema);
    }
    let receipt = v05::BatchAcknowledgement {
        context: batch.context.clone(),
        batch_id: batch.batch_id,
        digest: batch.digest.clone(),
        results: vec![v05::MutationResult {
            mutation_id: call.ordinal,
            outcome: v05::MutationOutcome::Accepted {
                sync_cursor: 7,
                materialization: batch.context.materialization.clone(),
                result: Value::Null,
                targets: vec![if private {
                    v05::SettlementTarget::Private {
                        record: v05::ReadRecord {
                            key: v05::RecordKey {
                                model: key.model.clone(),
                                identity: key.identity.clone(),
                            },
                            cursor: (),
                            state: json!({"text":"canonical","note":null}),
                        },
                    }
                } else {
                    v05::SettlementTarget::Stream {
                        key: v05::RecordKey {
                            model: key.model.clone(),
                            identity: key.identity.clone(),
                        },
                        cursor: 7,
                        fallback: v05::ReadRecord {
                            key: v05::RecordKey {
                                model: key.model.clone(),
                                identity: key.identity.clone(),
                            },
                            cursor: (),
                            state: json!({"text":"canonical","note":null}),
                        },
                    }
                }],
            },
        }],
    };
    client.acknowledge_batch05(&receipt).unwrap();
    assert_projection05(&mut client, &schema);
    let unfinished_after_receipt = client.call_completion05(&call.call_id).unwrap().is_none();
    drop(client);
    boundaries.push(Boundary05::SavedAcceptance);
    client = open(path);
    assert_projection05(&mut client, &schema);
    if !authority_first && !private {
        client
            .install_authority05(&context, std::slice::from_ref(&authority), None)
            .unwrap();
        assert_projection05(&mut client, &schema);
    }
    client
        .install_authority05(&context, &[], Some((0, 7)))
        .unwrap();
    assert_projection05(&mut client, &schema);
    drop(client);
    boundaries.push(Boundary05::Covered);
    client = open(path);
    assert_projection05(&mut client, &schema);
    client.settle_ready05().unwrap();
    assert_projection05(&mut client, &schema);
    drop(client);
    boundaries.push(Boundary05::Settled);
    client = open(path);
    assert_projection05(&mut client, &schema);
    let durable_completion = client.call_completion05(&call.call_id).unwrap().is_some();
    let optimistic = client.read(&key).unwrap().unwrap();
    client
        .transaction(|tx| tx.discard_mutation05(later.ordinal))
        .unwrap();
    assert_projection05(&mut client, &schema);
    RecoveryTrace05 {
        exact_retry,
        unfinished_after_receipt,
        durable_completion,
        optimistic,
        visible: client.read(&key).unwrap().unwrap(),
        boundaries,
    }
}
