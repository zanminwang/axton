//! Batch member execution against a deterministic persistence seam.
use axton_protocols::sync::{self as v05, Mutation, MutationOperation, MutationRequest};
use axton_server::{Config, Host, HostResult};
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Mutex};

// Store progress and saved results, as the database adapter keeps them.
#[derive(Default)]
struct State {
    last_batch: u64,
    progress: u64,
    digest: Option<String>,
    count: Option<u64>,
    last_digest: Option<String>,
    last_count: Option<u64>,
    results: BTreeMap<u64, Value>,
    executions: u64,
}
#[derive(Default)]
struct BatchHost(Mutex<State>);
impl Host for BatchHost {
    fn call(&self, request: Value) -> Pin<Box<dyn Future<Output = HostResult<Value>> + Send + '_>> {
        Box::pin(async move {
            let mut s = self.0.lock().unwrap();
            let q = &request["request"];
            Ok(match (request["op"].as_str(), q["op"].as_str()) {
                (Some("savepoint" | "rollback" | "release"), _) => Value::Null,
                (Some("head"), _) => json!(0),
                (Some("handleAction"), _) => {
                    s.executions += 1;
                    json!({"outputs":{},"changes":[],"declarations":[]})
                }
                (_, Some("claimStore")) => {
                    json!({"principal":"alice","stream":"User:alice","lastProcessedBatchId":s.last_batch,"progress":s.progress,"currentDigest":s.digest,"currentCount":s.count,"lastDigest":s.last_digest,"lastCount":s.last_count})
                }
                (_, Some("admit")) => json!(true),
                (_, Some("beginBatch")) => {
                    s.digest = Some(q["digest"].as_str().unwrap().into());
                    s.count = q["count"].as_u64();
                    s.results.clear();
                    Value::Null
                }
                (_, Some("readResult")) => s
                    .results
                    .get(&q["ordinal"].as_u64().unwrap())
                    .cloned()
                    .unwrap_or(Value::Null),
                (_, Some("targetPositions")) => json!([]),
                (_, Some("saveResult")) => {
                    let ordinal = q["ordinal"].as_u64().unwrap();
                    s.results.insert(ordinal, q["result"].clone());
                    s.progress += 1;
                    if s.progress == s.count.unwrap() {
                        s.last_batch = q["batchId"].as_u64().unwrap();
                        s.last_digest = s.digest.take();
                        s.last_count = s.count.take();
                        s.progress = 0;
                    }
                    Value::Null
                }
                _ => return Err(format!("unexpected host operation {request}")),
            })
        })
    }
}
fn action() -> Value {
    json!({"name":"Say","version":1,"kind":"mutation","inputs":[{"kind":"value","name":"text","type":{"kind":"scalar","name":"string"},"nullable":false}],"outputs":[]})
}
/// The backend at a projection generation; a new generation is an upgrade.
fn config(generation: &str) -> Config {
    Config::decode(json!({"schema":{"enums":[],"models":[],"actions":[action()]},"mutations":[],"loaders":[],"protocol5":{"projectionGeneration":generation}})).unwrap()
}
fn materialization(c: &Config) -> String {
    v05::materialization_id(
        &c.schema,
        &c.protocol5.as_ref().unwrap().projection_generation,
    )
    .unwrap()
}
fn request(c: &Config) -> MutationRequest {
    let descriptor = v05::mutation_descriptor_digest(&c.schema.actions[0]).unwrap();
    let mut r = MutationRequest {
        context: v05::RequestContext {
            protocol: v05::PROTOCOL,
            store_id: "s".into(),
            stream: "User:alice".into(),
            materialization: materialization(c),
        },
        batch_id: 1,
        digest: String::new(),
        mutations: (1..=2)
            .map(|id| Mutation {
                id,
                name: "Say".into(),
                version: 1,
                descriptor: descriptor.clone(),
                operations: vec![MutationOperation {
                    step: 1,
                    input_path: "text".into(),
                    operation: v05::Operation::Argument,
                    model: None,
                    identity: json!(null),
                    value: json!(format!("call {id}")),
                }],
            })
            .collect(),
    };
    r.digest = v05::batch_digest(&r).unwrap();
    r
}
async fn member(c: &Config, h: &BatchHost, r: &MutationRequest, ordinal: u64) -> String {
    let result: v05::MutationResult = serde_json::from_str(
        &axton_server::process_batch_member(c, "alice", &v05::encode(r).unwrap(), ordinal, h)
            .await
            .unwrap(),
    )
    .unwrap();
    match result.outcome {
        v05::MutationOutcome::Accepted {
            materialization, ..
        } => materialization,
        other => panic!("expected acceptance, got {other:?}"),
    }
}
fn run(f: impl Future<Output = ()>) {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(std::pin::pin!(f).as_mut().poll(&mut cx).is_ready());
}
#[test]
fn a_replay_under_another_materialization_returns_the_saved_label() {
    run(async {
        let (before, after) = (config("1"), config("2"));
        let host = BatchHost::default();
        let first = request(&before);
        for ordinal in 0..2 {
            assert_eq!(
                member(&before, &host, &first, ordinal).await,
                materialization(&before)
            );
        }
        let mut resent = first.clone();
        resent.context.materialization = materialization(&after);
        assert_eq!(resent.digest, v05::batch_digest(&resent).unwrap());
        for ordinal in 0..2 {
            assert_eq!(
                member(&after, &host, &resent, ordinal).await,
                materialization(&before)
            );
        }
        assert_eq!(host.0.lock().unwrap().executions, 2);
    });
}
#[test]
fn a_batch_partly_executed_before_an_upgrade_returns_mixed_labels() {
    run(async {
        let (before, after) = (config("1"), config("2"));
        let host = BatchHost::default();
        let first = request(&before);
        assert_eq!(
            member(&before, &host, &first, 0).await,
            materialization(&before)
        );
        let mut resent = first.clone();
        resent.context.materialization = materialization(&after);
        assert_eq!(
            member(&after, &host, &resent, 0).await,
            materialization(&before)
        );
        assert_eq!(
            member(&after, &host, &resent, 1).await,
            materialization(&after)
        );
        let results = vec![
            axton_server::process_batch_member(
                &after,
                "alice",
                &v05::encode(&resent).unwrap(),
                0,
                &host,
            )
            .await
            .unwrap(),
            axton_server::process_batch_member(
                &after,
                "alice",
                &v05::encode(&resent).unwrap(),
                1,
                &host,
            )
            .await
            .unwrap(),
        ];
        let ack: v05::BatchAcknowledgement = v05::decode(
            axton_server::encode_batch_acknowledgement(&v05::encode(&resent).unwrap(), &results)
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(ack.context, resent.context);
        assert_eq!(host.0.lock().unwrap().executions, 2);
    });
}
/// The backend after `Todo` changed its identity: the retained `Write` still
/// shapes its input with the old identity, which the current schema cannot key.
/// (A backend cannot retain a version on a removed Model: its config refuses.)
fn rekeyed() -> Config {
    let field =
        |name: &str| json!({"name":name,"nullable":false,"type":{"kind":"scalar","name":"string"}});
    let old =
        json!({"name":"Todo","version":1,"identity":["id"],"fields":[field("id"),field("title")]});
    let write = json!({"name":"Write","version":1,"kind":"mutation","inputs":[{"kind":"model","name":"todo","model":"Todo","operation":"create","cardinality":"single"}],"outputs":[],"input":{"models":[old],"enums":[]}});
    Config::decode(json!({"schema":{"enums":[],"models":[{"name":"Todo","version":2,"identity":["code"],"fields":[field("code"),field("title")]}],"actions":[action(),write]},"mutations":[],"loaders":["Todo"],"protocol5":{"projectionGeneration":"1"}})).unwrap()
}
#[test]
fn a_member_whose_input_cannot_be_keyed_is_refused_alone() {
    run(async {
        let c = rekeyed();
        let mut r = request(&c);
        let write = Mutation {
            id: 2,
            name: "Write".into(),
            version: 1,
            descriptor: v05::mutation_descriptor_digest(&c.schema.actions[1]).unwrap(),
            operations: vec![MutationOperation {
                step: 1,
                input_path: "todo".into(),
                operation: v05::Operation::Create,
                model: Some("Todo".into()),
                identity: json!({"id":"x"}),
                value: json!({"title":"hello"}),
            }],
        };
        let mut last = r.mutations.pop().unwrap();
        last.id = 3;
        r.mutations.extend([write, last]);
        r.digest = v05::batch_digest(&r).unwrap();
        let host = BatchHost::default();
        let bytes = v05::encode(&r).unwrap();
        let mut outcomes = vec![];
        for ordinal in 0..3 {
            let result: v05::MutationResult = serde_json::from_str(
                &axton_server::process_batch_member(&c, "alice", &bytes, ordinal, &host)
                    .await
                    .unwrap_or_else(|e| panic!("member {ordinal}: {e}")),
            )
            .unwrap();
            outcomes.push(result.outcome);
        }
        assert!(matches!(outcomes[0], v05::MutationOutcome::Accepted { .. }));
        assert_eq!(
            outcomes[1],
            v05::MutationOutcome::Rejected {
                code: axton_server::code::MUTATION_VERSION_UNSUPPORTED.into(),
                message: None,
            }
        );
        assert!(matches!(outcomes[2], v05::MutationOutcome::Accepted { .. }));
        assert_eq!(
            host.0.lock().unwrap().executions,
            2,
            "the refused handler never runs"
        );
    });
}
