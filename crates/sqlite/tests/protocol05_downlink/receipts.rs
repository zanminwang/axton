use super::{change, connected_control, socket_offer};
use axton_client::{
    Client, Schema,
    runtime::{ClientRuntime, EffectOutcome, Event, HttpRoute, Operation},
    sync05::{Control, StoreReport},
    v05,
};
use axton_sqlite::SqliteStore;
use serde_json::json;

struct Scenario {
    runtime: ClientRuntime<SqliteStore>,
    control: Control,
    socket: String,
    push: String,
    receipt: v05::BatchAcknowledgement,
    call_id: String,
}

impl Scenario {
    fn new(path: &std::path::Path) -> Self {
        let mut schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/schemas/entry.json")).unwrap();
        schema["actions"] = json!([{
            "name": "Write", "version": 1,
            "inputs": [{"kind": "model", "name": "entry", "model": "Entry",
                "operation": "create", "cardinality": "single"}],
            "outputs": []
        }]);
        let mut client = Client::open05(
            SqliteStore::open(path).unwrap(),
            Schema::from_value(schema).unwrap(),
            "User:u",
        )
        .unwrap();
        let (mut control, socket) = connected_control(&mut client);
        let call = client
            .transaction(|tx| {
                tx.submit_mutation05(
                    "Write",
                    1,
                    json!({"entry": {"id": "e", "text": "optimistic", "note": null}}),
                    vec![],
                )
            })
            .unwrap();
        let batch = client.freeze_batch05().unwrap().unwrap();
        let key = v05::RecordKey {
            model: "Entry".into(),
            identity: json!({"id": "e"}),
        };
        let receipt = v05::BatchAcknowledgement {
            context: batch.context.clone(),
            batch_id: batch.batch_id,
            digest: batch.digest.clone(),
            results: vec![v05::MutationResult {
                mutation_id: call.ordinal,
                outcome: v05::MutationOutcome::Accepted {
                    sync_cursor: 2,
                    result: serde_json::Value::Null,
                    targets: vec![v05::SettlementTarget::Stream {
                        key: key.clone(),
                        cursor: 1,
                        fallback: v05::ReadRecord {
                            key,
                            cursor: (),
                            state: json!({"text": "accepted", "note": null}),
                        },
                    }],
                },
            }],
        };
        control.report(StoreReport::Frozen(Some(batch)), 1).unwrap();
        let push = control
            .events()
            .into_iter()
            .find_map(|event| match event {
                Event::Effect {
                    effect_id,
                    operation:
                        Operation::Http {
                            route: HttpRoute::Push,
                            ..
                        },
                } => Some(effect_id),
                _ => None,
            })
            .unwrap();
        Self {
            runtime: ClientRuntime::new(client),
            control,
            socket,
            push,
            receipt,
            call_id: call.call_id,
        }
    }

    fn run_store(&mut self) {
        for _ in 0..16 {
            let jobs = self.control.jobs();
            if jobs.is_empty() {
                return;
            }
            for job in jobs {
                let report = self.runtime.store_worker05(job).unwrap();
                self.control.report(report, 1).unwrap();
            }
        }
        panic!("Store work did not become idle");
    }

    fn acknowledge(&mut self) {
        self.control
            .receive(
                &self.push,
                EffectOutcome {
                    ok: true,
                    value: Some(json!(serde_json::to_string(&self.receipt).unwrap())),
                    error: None,
                },
                1,
            )
            .unwrap();
        self.run_store();
    }

    fn deliver(&mut self, after: u64, through: u64, changes: Vec<v05::AuthorityChange>) {
        let delivery = v05::freeze_delivery(
            self.receipt.context.clone(),
            format!("live-{after}-{through}"),
            v05::DeliveryPurpose::Sync,
            after,
            through,
            through,
            10000,
            vec![v05::DeliveryUnit {
                index: 0,
                through: Some(through),
                changes,
            }],
            500,
        )
        .unwrap();
        socket_offer(&mut self.control, &self.socket, &delivery).unwrap();
        self.run_store();
    }

    fn assert_no_cursor_pull(&mut self) {
        assert!(
            !self.control.events().iter().any(|event| matches!(
                event,
                Event::Effect {
                    operation: Operation::Http {
                        route: HttpRoute::Pull,
                        ..
                    },
                    ..
                }
            )),
            "a Mutation receipt is a settlement fence, not an observed delivery head"
        );
    }

    fn assert_completed(&mut self) {
        let client = self.runtime.client();
        assert!(client.call_completion05(&self.call_id).unwrap().is_some());
        assert!(client.pending_settlement05().unwrap().is_empty());
        assert_eq!(client.query("Entry", &json!({})).unwrap()[0]["text"], "e");
        assert!(client.settle_ready05().unwrap().completions.is_empty());
    }
}

#[test]
fn receipt_waits_for_committed_delivery_without_requesting_cursor_repair() {
    let directory = tempfile::tempdir().unwrap();
    let mut scenario = Scenario::new(&directory.path().join("db"));
    scenario.acknowledge();
    scenario.assert_no_cursor_pull();
    let client = scenario.runtime.client();
    assert_eq!(client.store_status05().unwrap().cursor, Some(0));
    assert!(
        client
            .call_completion05(&scenario.call_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(client.pending_settlement05().unwrap().len(), 1);

    // Target authority alone cannot complete the receipt's later prefix fence.
    scenario.deliver(0, 1, vec![change("e", 1)]);
    scenario.assert_no_cursor_pull();
    assert!(
        scenario
            .runtime
            .client()
            .call_completion05(&scenario.call_id)
            .unwrap()
            .is_none()
    );
    scenario.deliver(1, 2, vec![]);
    scenario.assert_no_cursor_pull();
    scenario.assert_completed();
}

#[test]
fn delivery_before_receipt_completes_without_another_delivery_or_cursor_repair() {
    let directory = tempfile::tempdir().unwrap();
    let mut scenario = Scenario::new(&directory.path().join("db"));
    scenario.deliver(0, 2, vec![change("e", 1)]);
    scenario.assert_no_cursor_pull();
    assert!(
        scenario
            .runtime
            .client()
            .call_completion05(&scenario.call_id)
            .unwrap()
            .is_none()
    );
    scenario.acknowledge();
    scenario.assert_no_cursor_pull();
    scenario.assert_completed();
}

#[test]
fn reconnect_handshake_still_repairs_the_missing_committed_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let mut scenario = Scenario::new(&directory.path().join("db"));
    scenario.acknowledge();
    scenario.assert_no_cursor_pull();
    scenario.control.connect().unwrap();
    let handshake = scenario
        .control
        .events()
        .into_iter()
        .find_map(|event| match event {
            Event::Effect {
                effect_id,
                operation:
                    Operation::Http {
                        route: HttpRoute::Handshake,
                        ..
                    },
            } => Some(effect_id),
            _ => None,
        })
        .unwrap();
    scenario
        .control
        .receive(
            &handshake,
            EffectOutcome {
                ok: true,
                value: Some(json!(
                    serde_json::to_string(&v05::HandshakeResponse {
                        protocol: 5,
                        store_id: scenario.receipt.context.store_id.clone(),
                        stream: scenario.receipt.context.stream.clone(),
                        head: 2,
                    })
                    .unwrap()
                )),
                error: None,
            },
            2,
        )
        .unwrap();
    scenario.run_store();
    let (_, request) = super::cursor_pull(&mut scenario.control);
    assert_eq!(
        (request.after, request.through, request.bootstrap),
        (0, 2, false)
    );
    assert!(
        scenario
            .runtime
            .client()
            .call_completion05(&scenario.call_id)
            .unwrap()
            .is_none()
    );
}
