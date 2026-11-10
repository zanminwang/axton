//! Deterministic recovery orders against real protocol-5 SQLite state.
//! Cloud acknowledgements are fixtures; the actual PostgreSQL/SDK join lives
//! in integration/v05-sdk and must pass independently.
use axton_client::{Client, RecordKey, Schema};
use axton_protocols::sync as v05;
use axton_sqlite::SqliteStore;
use serde_json::{Value, json};
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
/// Reopen after every durable boundary. `authority_first` permutes the only
/// two independent sources: the Batch receipt and content authority.
pub fn run_recovery05(path: &Path, authority_first: bool, private: bool) -> RecoveryTrace05 {
    let key = RecordKey {
        model: "Entry".into(),
        identity: json!({"id":"e"}),
    };
    let mut client = open(path);
    client.initialize_stream05(0, "1").unwrap();
    let call = client
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"optimistic","note":null}}),
                vec![],
            )
        })
        .unwrap();
    drop(client);
    let mut boundaries = vec![Boundary05::Queued];
    client = open(path);
    let batch = client.freeze_batch05().unwrap().unwrap();
    let bytes = v05::encode(&batch).unwrap();
    drop(client);
    boundaries.push(Boundary05::Frozen);
    client = open(path);
    let exact_retry = v05::encode(&client.freeze_batch05().unwrap().unwrap()).unwrap() == bytes;
    // A later Mutation on the same record, queued after the frozen Batch and
    // never sent: it stays pending on top of whatever settles beneath it.
    let later = client
        .transaction(|tx| {
            tx.submit_mutation05(
                "Edit",
                1,
                json!({"entry":{"id":"e","text":"later"}}),
                vec![],
            )
        })
        .unwrap();
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
    let unfinished_after_receipt = client.call_completion05(&call.call_id).unwrap().is_none();
    drop(client);
    boundaries.push(Boundary05::SavedAcceptance);
    client = open(path);
    if !authority_first && !private {
        client
            .install_authority05(&context, std::slice::from_ref(&authority), None)
            .unwrap();
    }
    client
        .install_authority05(&context, &[], Some((0, 7)))
        .unwrap();
    drop(client);
    boundaries.push(Boundary05::Covered);
    client = open(path);
    client.settle_ready05().unwrap();
    drop(client);
    boundaries.push(Boundary05::Settled);
    client = open(path);
    let durable_completion = client.call_completion05(&call.call_id).unwrap().is_some();
    let optimistic = client.read(&key).unwrap().unwrap();
    client
        .transaction(|tx| tx.discard_mutation05(later.ordinal))
        .unwrap();
    RecoveryTrace05 {
        exact_retry,
        unfinished_after_receipt,
        durable_completion,
        optimistic,
        visible: client.read(&key).unwrap().unwrap(),
        boundaries,
    }
}
