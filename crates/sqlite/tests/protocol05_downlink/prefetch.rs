use super::{change, client};
use axton_client::{
    runtime::{ClientRuntime, EffectError, EffectOutcome, Event, HttpRoute, Operation},
    sync05::{Control, StoreCommand, StoreReport},
    v05,
};
use axton_sqlite::SqliteStore;
use serde_json::json;

struct Harness {
    runtime: ClientRuntime<SqliteStore>,
    control: Control,
    first: String,
}
impl Harness {
    fn new(path: &std::path::Path) -> Self {
        Self::configured(path, true)
    }
    fn configured(path: &std::path::Path, enabled: bool) -> Self {
        let mut client = client(path);
        client.initialize_stream05(3).unwrap();
        let context = client.request_context05().unwrap();
        let status = client.store_status05().unwrap();
        let mut control = if enabled {
            Control::with_bootstrap_prefetch(status.clone(), true)
        } else {
            Control::new(status.clone())
        };
        control.connect().unwrap();
        let handshake = control
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
        control
            .receive(
                &handshake,
                EffectOutcome {
                    ok: true,
                    value: Some(json!(
                        serde_json::to_string(&v05::HandshakeResponse {
                            protocol: 5,
                            store_id: context.store_id,
                            stream: context.stream,
                            head: 3,
                        })
                        .unwrap()
                    )),
                    error: None,
                },
                1,
            )
            .unwrap();
        control.report(StoreReport::Snapshot(status), 1).unwrap();
        let first = Self::pull_events(&mut control).unwrap().0;
        control.jobs();
        Self {
            runtime: ClientRuntime::new(client),
            control,
            first,
        }
    }
    fn pull_events(control: &mut Control) -> Option<(String, v05::DeltaRequest)> {
        let events = control.events();
        assert!(
            !events.iter().any(|e| matches!(e, Event::Report { .. })),
            "unexpected connection diagnostic"
        );
        let mut pulls = events.into_iter().filter_map(|event| match event {
            Event::Effect {
                effect_id,
                operation:
                    Operation::Http {
                        route: HttpRoute::Pull,
                        body,
                    },
            } => Some((effect_id, serde_json::from_str(&body).unwrap())),
            _ => None,
        });
        let first = pulls.next();
        assert!(pulls.next().is_none(), "one cursor HTTP request at a time");
        first
    }
    fn page(&mut self, id: &str, after: u64, through: u64, bad: bool) {
        let mut record = change(&format!("e{through}"), through);
        if bad && let v05::AuthorityChange::Record { state, .. } = &mut record {
            *state = json!({"text":7,"note":null});
        }
        let frozen = v05::freeze_delivery(
            self.runtime.client().request_context05().unwrap(),
            format!("page-{after}-{through}"),
            v05::DeliveryPurpose::Bootstrap,
            after,
            through,
            3,
            10000,
            vec![v05::DeliveryUnit {
                index: 0,
                through: Some(through),
                changes: vec![record],
            }],
            500,
        )
        .unwrap();
        self.control
            .receive(
                id,
                EffectOutcome {
                    ok: true,
                    value: Some(json!(
                        serde_json::to_string(&v05::DeliveryResponse {
                            header: frozen.header,
                            parts: frozen.parts,
                        })
                        .unwrap()
                    )),
                    error: None,
                },
                1,
            )
            .unwrap();
    }
    fn take_apply(&mut self) -> Option<StoreCommand> {
        let mut applies=self.control.jobs().into_iter().filter(|job|matches!(job,StoreCommand::Guarded {command,..} if matches!(**command,StoreCommand::Apply {..})));
        let first = applies.next();
        assert!(applies.next().is_none(), "one Store apply at a time");
        first
    }
    fn apply(&mut self, job: StoreCommand) {
        let report = self.runtime.store_worker05(job).unwrap();
        self.control.report(report, 1).unwrap();
    }
    fn progress(&mut self) -> Option<u64> {
        self.runtime
            .client()
            .store_status05()
            .unwrap()
            .bootstrap_cursor
    }
    fn unauthorized(&mut self, id: &str) -> String {
        self.control.set_refresh_auth(true);
        self.control
            .receive(
                id,
                EffectOutcome {
                    ok: false,
                    value: None,
                    error: Some(EffectError {
                        message: "expired token".into(),
                        status: Some(401),
                        refusal: None,
                        retry: false,
                    }),
                },
                2,
            )
            .unwrap();
        self.control
            .events()
            .into_iter()
            .find_map(|event| match event {
                Event::Effect {
                    effect_id,
                    operation: Operation::RefreshAuth,
                } => Some(effect_id),
                _ => None,
            })
            .unwrap()
    }
    fn refreshed(&mut self, id: &str) {
        self.control
            .receive(
                id,
                EffectOutcome {
                    ok: true,
                    value: None,
                    error: None,
                },
                3,
            )
            .unwrap();
    }
}

#[test]
fn auth_refresh_keeps_the_successor_as_the_only_pending_pull() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(&dir.path().join("db"));
    h.page(&h.first.clone(), 0, 1, false);
    let first = h.take_apply().unwrap();
    let (future, _) = Harness::pull_events(&mut h.control).unwrap();
    let refresh = h.unauthorized(&future);
    h.apply(first);
    assert!(
        Harness::pull_events(&mut h.control).is_none(),
        "a request waiting for auth still occupies the one HTTP slot"
    );
    h.refreshed(&refresh);
    let (retried, request) = Harness::pull_events(&mut h.control).unwrap();
    assert_ne!(retried, future);
    assert_eq!((request.after, request.through), (1, 3));
    h.page(&retried, 1, 2, false);
    let second = h.take_apply().unwrap();
    h.apply(second);
    assert_eq!(h.progress(), Some(2));
}

#[test]
fn failed_predecessor_retires_prefetch_waiting_for_auth() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(&dir.path().join("db"));
    h.page(&h.first.clone(), 0, 1, true);
    let first = h.take_apply().unwrap();
    let (future, _) = Harness::pull_events(&mut h.control).unwrap();
    let refresh = h.unauthorized(&future);
    h.apply(first);
    h.control.events();
    h.refreshed(&refresh);
    assert!(Harness::pull_events(&mut h.control).is_none());
    assert_eq!(h.progress(), None);
    assert!(h.take_apply().is_none());
}

#[test]
fn reconnect_retires_prefetch_waiting_for_auth() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(&dir.path().join("db"));
    h.page(&h.first.clone(), 0, 1, false);
    h.take_apply().unwrap();
    let (future, _) = Harness::pull_events(&mut h.control).unwrap();
    let refresh = h.unauthorized(&future);
    h.control.connect().unwrap();
    h.control.events();
    h.refreshed(&refresh);
    assert!(Harness::pull_events(&mut h.control).is_none());
    assert_eq!(h.progress(), None);
}

#[test]
fn default_controller_waits_for_committed_bootstrap() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::configured(&dir.path().join("db"), false);
    h.page(&h.first.clone(), 0, 1, false);
    let first = h.take_apply().unwrap();
    assert!(Harness::pull_events(&mut h.control).is_none());
    h.apply(first);
    let (_, request) = Harness::pull_events(&mut h.control).unwrap();
    assert_eq!((request.after, request.through), (1, 3));
}

#[test]
fn commit_before_prefetch_reply_keeps_only_one_http_request() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(&dir.path().join("db"));
    h.page(&h.first.clone(), 0, 1, false);
    let first = h.take_apply().unwrap();
    let (second, _) = Harness::pull_events(&mut h.control).unwrap();
    h.apply(first);
    assert!(Harness::pull_events(&mut h.control).is_none());
    assert!(h.take_apply().is_none());
    h.page(&second, 1, 2, false);
    assert!(h.take_apply().is_some());
    let (_, request) = Harness::pull_events(&mut h.control).unwrap();
    assert_eq!(request.after, 2);
}

#[test]
fn reconnect_retires_future_offer_and_reuses_only_the_admitted_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(&dir.path().join("db"));
    h.page(&h.first.clone(), 0, 1, false);
    let first = h.take_apply().unwrap();
    let (future, _) = Harness::pull_events(&mut h.control).unwrap();
    h.control.connect().unwrap();
    let events = h.control.events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e,Event::CancelEffect {effect_id} if effect_id==&future))
    );
    let handshake = events
        .into_iter()
        .find_map(|e| match e {
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
    h.apply(first);
    h.page(&future, 1, 2, false);
    assert_eq!(h.progress(), None);
    let status = h.runtime.client().store_status05().unwrap();
    h.control
        .receive(
            &handshake,
            EffectOutcome {
                ok: true,
                value: Some(json!(
                    serde_json::to_string(&v05::HandshakeResponse {
                        protocol: 5,
                        store_id: status.context.store_id.clone(),
                        stream: status.context.stream.clone(),
                        head: 3,
                    })
                    .unwrap()
                )),
                error: None,
            },
            2,
        )
        .unwrap();
    h.control.report(StoreReport::Snapshot(status), 2).unwrap();
    let (fresh, request) = Harness::pull_events(&mut h.control).unwrap();
    assert_ne!(fresh, future);
    assert_eq!((request.after, request.through), (1, 3));
    assert_eq!(
        h.progress(),
        None,
        "retained receipt is not committed progress"
    );
    let prefix = h
        .take_apply()
        .expect("retained current page gets a fresh Store fence");
    h.apply(prefix);
    assert_eq!(h.progress(), Some(1));
}

#[test]
fn multi_unit_page_prefetches_after_the_whole_page_without_skipping_units() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(&dir.path().join("db"));
    let frozen = v05::freeze_delivery(
        h.runtime.client().request_context05().unwrap(),
        "multi-unit".into(),
        v05::DeliveryPurpose::Bootstrap,
        0,
        2,
        3,
        10000,
        vec![
            v05::DeliveryUnit {
                index: 0,
                through: Some(1),
                changes: vec![change("e1", 1)],
            },
            v05::DeliveryUnit {
                index: 1,
                through: Some(2),
                changes: vec![change("e2", 2)],
            },
        ],
        500,
    )
    .unwrap();
    h.control
        .receive(
            &h.first,
            EffectOutcome {
                ok: true,
                value: Some(json!(
                    serde_json::to_string(&v05::DeliveryResponse {
                        header: frozen.header,
                        parts: frozen.parts,
                    })
                    .unwrap()
                )),
                error: None,
            },
            1,
        )
        .unwrap();
    let first = h.take_apply().unwrap();
    let (future, request) = Harness::pull_events(&mut h.control).unwrap();
    assert_eq!(request.after, 2);
    h.page(&future, 2, 3, false);
    h.apply(first);
    assert_eq!(h.progress(), Some(1));
    let second = h.take_apply().unwrap();
    assert!(Harness::pull_events(&mut h.control).is_none());
    h.apply(second);
    assert_eq!(h.progress(), Some(2));
    let third = h.take_apply().unwrap();
    h.apply(third);
    assert_eq!(h.progress(), Some(3));
}

#[test]
fn one_page_can_arrive_ahead_but_never_commit_ahead() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(&dir.path().join("db"));
    h.page(&h.first.clone(), 0, 1, false);
    let first = h.take_apply().unwrap();
    let (second, request) = Harness::pull_events(&mut h.control)
        .expect("prefetch must overlap the uncommitted first page");
    assert_eq!(
        (request.after, request.through, request.bootstrap),
        (1, 3, true)
    );
    assert_eq!(h.progress(), None);
    h.page(&second, 1, 2, false);
    assert!(h.take_apply().is_none());
    assert!(
        Harness::pull_events(&mut h.control).is_none(),
        "a third page must wait for a commit"
    );
    assert!(
        h.runtime
            .client()
            .query("Entry", &json!({}))
            .unwrap()
            .is_empty()
    );
    h.apply(first);
    assert_eq!(h.progress(), Some(1));
    let second = h.take_apply().unwrap();
    let (third, request) = Harness::pull_events(&mut h.control).unwrap();
    assert_eq!((request.after, request.through), (2, 3));
    h.page(&third, 2, 3, false);
    assert!(h.take_apply().is_none());
    h.apply(second);
    assert_eq!(h.progress(), Some(2));
    let third = h.take_apply().unwrap();
    h.apply(third);
    assert_eq!(h.progress(), Some(3));
    assert_eq!(
        h.runtime.client().query("Entry", &json!({})).unwrap().len(),
        3
    );
    assert!(Harness::pull_events(&mut h.control).is_none());
}

#[test]
fn failed_predecessor_retires_future_work_without_advancing_bootstrap() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(&dir.path().join("db"));
    h.page(&h.first.clone(), 0, 1, true);
    let first = h.take_apply().unwrap();
    let (future, _) = Harness::pull_events(&mut h.control).unwrap();
    h.page(&future, 1, 2, false);
    let report = h.runtime.store_worker05(first).unwrap();
    h.control.report(report, 1).unwrap();
    assert_eq!(h.progress(), None);
    assert!(h.take_apply().is_none());
    assert!(
        h.runtime
            .client()
            .query("Entry", &json!({}))
            .unwrap()
            .is_empty()
    );
    h.control.connect().unwrap();
    assert!(h.take_apply().is_none());
}

#[test]
fn close_cancels_prefetch_and_fences_dispatched_apply() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(&dir.path().join("db"));
    h.page(&h.first.clone(), 0, 1, false);
    let first = h.take_apply().unwrap();
    let (future, _) = Harness::pull_events(&mut h.control).unwrap();
    h.control.stop();
    assert!(
        h.control
            .events()
            .iter()
            .any(|e| matches!(e,Event::CancelEffect {effect_id} if effect_id==&future))
    );
    h.page(&future, 1, 2, false);
    h.apply(first);
    assert_eq!(h.progress(), None);
    assert!(h.take_apply().is_none());
}
