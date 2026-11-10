use axton_client::{Client, Schema, sync05::DeliveryQueue, v05};
use axton_sqlite::SqliteStore;
pub mod common05;
#[path = "protocol05_downlink/prefetch.rs"]
mod prefetch;
#[path = "protocol05_downlink/receipts.rs"]
mod receipts;

fn client(path: &std::path::Path) -> Client<SqliteStore> {
    Client::open05(
        SqliteStore::open(path).unwrap(),
        Schema::from_value(
            serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap(),
        )
        .unwrap(),
        "User:u",
    )
    .unwrap()
}
fn plan(
    context: v05::RequestContext,
    id: &str,
    after: u64,
    through: u64,
    bootstrap: bool,
) -> v05::FrozenDelivery {
    v05::freeze_delivery(
        context,
        id.into(),
        if bootstrap {
            v05::DeliveryPurpose::Bootstrap
        } else {
            v05::DeliveryPurpose::Sync
        },
        after,
        through,
        through,
        10000,
        vec![v05::DeliveryUnit {
            index: 0,
            through: Some(through),
            changes: vec![],
        }],
        1,
    )
    .unwrap()
}
#[test]
fn empty_bootstrap_zero_commits_only_verified_final_and_survives_reopen() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = client(&p);
    c.initialize_stream05(0, "1").unwrap();
    assert_eq!(c.store_status05().unwrap().bootstrap_cursor, None);
    let context = c.request_context05().unwrap();
    let f = plan(context.clone(), "p", 0, 0, true);
    let mut q = DeliveryQueue::new(1024 * 1024, 2);
    q.receive(&f.header, &f.parts, &context, 1).unwrap();
    c.apply_next_delivery05(&mut q, 1).unwrap().unwrap();
    assert_eq!(c.store_status05().unwrap().bootstrap_cursor, Some(0));
    drop(c);
    let mut c = client(&p);
    assert_eq!(c.store_status05().unwrap().bootstrap_cursor, Some(0));
    q.receive(&f.header, &f.parts, &context, 2).unwrap();
    assert!(c.apply_next_delivery05(&mut q, 2).unwrap().is_none());
}
#[test]
fn missing_prefix_is_retained_and_repair_capacity_is_reserved() {
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let later = plan(context.clone(), "later", 2, 3, false);
    let early = plan(context.clone(), "early", 0, 2, false);
    let mut q = DeliveryQueue::new(1024 * 1024, 2);
    q.receive(&later.header, &later.parts, &context, 1).unwrap();
    assert!(c.apply_next_delivery05(&mut q, 1).unwrap().is_none());
    q.receive(&early.header, &early.parts, &context, 1).unwrap();
    c.apply_next_delivery05(&mut q, 1).unwrap().unwrap();
    c.apply_next_delivery05(&mut q, 1).unwrap().unwrap();
    assert_eq!(c.store_status05().unwrap().cursor, Some(3));
}
fn fragmented(context: v05::RequestContext, bootstrap: bool) -> v05::FrozenDelivery {
    v05::freeze_delivery(
        context,
        "fragmented".into(),
        if bootstrap {
            v05::DeliveryPurpose::Bootstrap
        } else {
            v05::DeliveryPurpose::Sync
        },
        0,
        2,
        3,
        10000,
        vec![v05::DeliveryUnit {
            index: 0,
            through: Some(2),
            changes: vec![change("a", 1), change("b", 1)],
        }],
        1,
    )
    .unwrap()
}
#[test]
fn socket_loss_discards_incomplete_response_and_repairs_from_committed_cursor() {
    use axton_client::{
        runtime::{EffectOutcome, Event, HttpRoute, Operation},
        sync05::Control,
    };
    let directory = tempfile::tempdir().unwrap();
    let mut c = client(&directory.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let empty = plan(context.clone(), "initial", 0, 0, true);
    let mut queue = DeliveryQueue::new(1000000, 4);
    queue
        .receive(&empty.header, &empty.parts, &context, 1)
        .unwrap();
    c.apply_next_delivery05(&mut queue, 1).unwrap().unwrap();
    let mut control = Control::new(c.store_status05().unwrap());
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
                value: Some(serde_json::Value::String(
                    serde_json::to_string(&v05::HandshakeResponse {
                        protocol: axton_client::v05::PROTOCOL,
                        store_id: context.store_id.clone(),
                        stream: context.stream.clone(),
                        head: 0,
                        projection_generation: "1".into(),
                    })
                    .unwrap(),
                )),
                error: None,
            },
            1,
        )
        .unwrap();
    let socket = control
        .events()
        .into_iter()
        .find_map(|event| match event {
            Event::Effect {
                effect_id,
                operation: Operation::Socket { .. },
            } => Some(effect_id),
            _ => None,
        })
        .unwrap();
    let response = fragmented(context, false);
    control.receive(&socket,EffectOutcome {ok:true,value:Some(serde_json::json!({"event":"message","body":serde_json::to_string(&v05::DeliveryResponse {header:response.header,parts:vec![response.parts[0].clone()]}).unwrap()})),error:None},1).unwrap();
    assert!(control.events().iter().all(|event| !matches!(
        event,
        Event::Effect {
            operation: Operation::Http {
                route: HttpRoute::Pull,
                ..
            },
            ..
        }
    )));
    control
        .receive(
            &socket,
            EffectOutcome {
                ok: true,
                value: Some(serde_json::json!({"event":"closed"})),
                error: None,
            },
            2,
        )
        .unwrap();
    let request = control
        .events()
        .into_iter()
        .find_map(|event| match event {
            Event::Effect {
                operation:
                    Operation::Http {
                        route: HttpRoute::Pull,
                        body,
                    },
                ..
            } => Some(serde_json::from_str::<serde_json::Value>(&body).unwrap()),
            _ => None,
        })
        .expect("socket loss must repair immediately without waiting for old response expiry");
    assert_eq!(request["after"], 0);
    assert_eq!(request["through"], 3);
    assert!(request.get("continuation").is_none());
    assert_eq!(c.store_status05().unwrap().cursor, Some(0));
}
fn change(id: &str, cursor: u64) -> v05::AuthorityChange {
    v05::AuthorityChange::Record {
        key: v05::RecordKey {
            model: "Entry".into(),
            identity: serde_json::json!({"id":id}),
        },
        cursor,
        state: serde_json::json!({"text":id,"note":null}),
    }
}
fn connected_control(c: &mut Client<SqliteStore>) -> (axton_client::sync05::Control, String) {
    use axton_client::{
        runtime::{EffectOutcome, Event, Operation},
        sync05::Control,
    };
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let empty = plan(context.clone(), "initial", 0, 0, true);
    let mut queue = DeliveryQueue::new(1000000, 4);
    queue
        .receive(&empty.header, &empty.parts, &context, 1)
        .unwrap();
    c.apply_next_delivery05(&mut queue, 1).unwrap().unwrap();
    let mut control = Control::new(c.store_status05().unwrap());
    control.connect().unwrap();
    let handshake = control
        .events()
        .into_iter()
        .find_map(|event| match event {
            Event::Effect {
                effect_id,
                operation: Operation::Http { .. },
            } => Some(effect_id),
            _ => None,
        })
        .unwrap();
    control
        .receive(
            &handshake,
            EffectOutcome {
                ok: true,
                value: Some(serde_json::Value::String(
                    serde_json::to_string(&v05::HandshakeResponse {
                        protocol: axton_client::v05::PROTOCOL,
                        store_id: context.store_id,
                        stream: context.stream,
                        head: 0,
                        projection_generation: "1".into(),
                    })
                    .unwrap(),
                )),
                error: None,
            },
            1,
        )
        .unwrap();
    let socket = control
        .events()
        .into_iter()
        .find_map(|event| match event {
            Event::Effect {
                effect_id,
                operation: Operation::Socket { .. },
            } => Some(effect_id),
            _ => None,
        })
        .unwrap();
    control.jobs();
    (control, socket)
}
fn socket_offer(
    control: &mut axton_client::sync05::Control,
    socket: &str,
    offer: &v05::FrozenDelivery,
) -> axton_client::Result<()> {
    control.receive(socket, axton_client::runtime::EffectOutcome {ok:true,value:Some(serde_json::json!({"event":"message","body":serde_json::to_string(&v05::DeliveryResponse {header:offer.header.clone(),parts:offer.parts.clone()}).unwrap()})),error:None},1)
}
fn commit_control_delivery(
    c: &mut Client<SqliteStore>,
    control: &mut axton_client::sync05::Control,
) {
    use axton_client::sync05::{StoreCommand, StoreReport};
    let (id, mut queue, now) = control
        .jobs()
        .into_iter()
        .find_map(|job| match job {
            StoreCommand::Guarded { fence, command } => {
                assert!(fence.current());
                match *command {
                    StoreCommand::Apply {
                        plan_id,
                        queue,
                        now,
                    } => Some((plan_id, queue, now)),
                    _ => None,
                }
            }
            _ => None,
        })
        .expect("a complete response must be dispatched");
    assert_eq!(queue.len(), 1);
    let report = c
        .apply_next_delivery05(&mut queue, now)
        .unwrap()
        .unwrap_or_default();
    assert_eq!(queue.len(), 0, "test response has one complete atomic unit");
    control
        .report(
            StoreReport::Committed {
                status: c.store_status05().unwrap(),
                report,
                plan: Some((id, u64::MAX)),
                active_plans: Some(c.active_delivery_plans05().unwrap()),
            },
            now,
        )
        .unwrap();
}
fn cursor_pull(control: &mut axton_client::sync05::Control) -> (String, v05::DeltaRequest) {
    use axton_client::runtime::{Event, HttpRoute, Operation};
    let events = control.events();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Report { .. })),
        "coalescing is ordinary flow control, not a connection error"
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
    let pull = pulls
        .next()
        .expect("committed progress must drive fresh cursor repair");
    assert!(
        pulls.next().is_none(),
        "only one ordinary cursor request may be in flight"
    );
    pull
}
#[test]
fn stalled_receiver_coalesces_bursts_and_repairs_latest_data_from_committed_cursor() {
    use axton_client::runtime::{EffectOutcome, Event};
    let dir = tempfile::tempdir().unwrap();
    let mut c = client(&dir.path().join("db"));
    let (mut control, socket) = connected_control(&mut c);
    let context = c.request_context05().unwrap();
    let offer = |index| {
        v05::freeze_delivery(
            context.clone(),
            format!("offer{index}"),
            v05::DeliveryPurpose::Sync,
            index - 1,
            index,
            index,
            10000,
            vec![v05::DeliveryUnit {
                index: 0,
                through: Some(index),
                changes: vec![change(&format!("e{index}"), index)],
            }],
            500,
        )
        .unwrap()
    };
    // A malformed idle offer must not become a head hint either.
    let mut malformed_idle = offer(102);
    malformed_idle.parts[0].changes[0] = change("tampered", 102);
    assert!(socket_offer(&mut control, &socket, &malformed_idle).is_err());
    // Hold the Store worker while more than the old 32-response limit arrives.
    for index in 1..=100 {
        socket_offer(&mut control, &socket, &offer(index)).unwrap();
    }
    assert_eq!(c.store_status05().unwrap().cursor, Some(0));
    assert!(c.query("Entry", &serde_json::json!({})).unwrap().is_empty());
    assert!(
        !control
            .events()
            .iter()
            .any(|event| matches!(event, Event::Report { .. }))
    );
    let mut malformed = offer(101);
    malformed.parts[0].changes[0] = change("tampered", 101);
    assert!(
        socket_offer(&mut control, &socket, &malformed)
            .unwrap_err()
            .to_string()
            .contains("fragment correlation mismatch")
    );
    commit_control_delivery(&mut c, &mut control);
    let (pull, request) = cursor_pull(&mut control);
    assert_eq!(
        (request.after, request.through),
        (1, 100),
        "hints cannot advance the committed cursor"
    );
    // A future offer cannot fill the held HTTP repair's gap, so retain its head
    // rather than queueing another payload that is not yet applicable.
    socket_offer(&mut control, &socket, &offer(101)).unwrap();
    let mut changes = (2..=100)
        .map(|index| change(&format!("e{index}"), index))
        .collect::<Vec<_>>();
    let mut latest = change("e1", 100);
    if let v05::AuthorityChange::Record { state, .. } = &mut latest {
        state["text"] = serde_json::json!("latest");
    }
    changes.push(latest);
    let repaired = v05::freeze_delivery(
        context.clone(),
        "repair".into(),
        v05::DeliveryPurpose::Sync,
        1,
        100,
        101,
        10000,
        vec![v05::DeliveryUnit {
            index: 0,
            through: Some(100),
            changes,
        }],
        500,
    )
    .unwrap();
    control
        .receive(
            &pull,
            EffectOutcome {
                ok: true,
                value: Some(serde_json::Value::String(
                    serde_json::to_string(&v05::DeliveryResponse {
                        header: repaired.header,
                        parts: repaired.parts,
                    })
                    .unwrap(),
                )),
                error: None,
            },
            1,
        )
        .unwrap();
    commit_control_delivery(&mut c, &mut control);
    let (pull, request) = cursor_pull(&mut control);
    assert_eq!((request.after, request.through), (100, 101));
    let last = offer(101);
    control
        .receive(
            &pull,
            EffectOutcome {
                ok: true,
                value: Some(serde_json::Value::String(
                    serde_json::to_string(&v05::DeliveryResponse {
                        header: last.header,
                        parts: last.parts,
                    })
                    .unwrap(),
                )),
                error: None,
            },
            1,
        )
        .unwrap();
    commit_control_delivery(&mut c, &mut control);
    assert_eq!(c.store_status05().unwrap().cursor, Some(101));
    let rows = c.query("Entry", &serde_json::json!({})).unwrap();
    assert_eq!(rows.len(), 101);
    for row in rows {
        let id = row["id"].as_str().unwrap();
        assert_eq!(row["text"], if id == "e1" { "latest" } else { id });
    }
    assert!(
        !control
            .events()
            .iter()
            .any(|event| matches!(event, Event::Report { .. }))
    );
}
#[test]
fn admitted_socket_fragments_finish_atomically_while_new_offers_are_coalesced() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = client(&dir.path().join("db"));
    let (mut control, socket) = connected_control(&mut c);
    let context = c.request_context05().unwrap();
    let mut first = fragmented(context.clone(), false);
    let remaining = first.parts.split_off(1);
    socket_offer(&mut control, &socket, &first).unwrap();
    socket_offer(&mut control, &socket, &plan(context, "later", 2, 3, false)).unwrap();
    assert_eq!(c.store_status05().unwrap().cursor, Some(0));
    first.parts = remaining;
    socket_offer(&mut control, &socket, &first).unwrap();
    commit_control_delivery(&mut c, &mut control);
    assert_eq!(c.query("Entry", &serde_json::json!({})).unwrap().len(), 2);
    let (_, request) = cursor_pull(&mut control);
    assert_eq!((request.after, request.through), (2, 3));
}
#[test]
fn held_http_repair_does_not_block_live_prefix_and_late_reply_cannot_undo_it() {
    use axton_client::runtime::EffectOutcome;
    let dir = tempfile::tempdir().unwrap();
    let mut c = client(&dir.path().join("db"));
    let (mut control, socket) = connected_control(&mut c);
    let context = c.request_context05().unwrap();
    let ack = v05::HandshakeResponse {
        protocol: axton_client::v05::PROTOCOL,
        store_id: context.store_id.clone(),
        stream: context.stream.clone(),
        head: 1,
        projection_generation: "1".into(),
    };
    control.receive(&socket,EffectOutcome {ok:true,value:Some(serde_json::json!({"event":"message","body":serde_json::to_string(&ack).unwrap()})),error:None},1).unwrap();
    let (pull, request) = cursor_pull(&mut control);
    assert_eq!((request.after, request.through), (0, 1));
    let live = v05::freeze_delivery(
        context.clone(),
        "live-before-http".into(),
        v05::DeliveryPurpose::Sync,
        0,
        1,
        1,
        10000,
        vec![v05::DeliveryUnit {
            index: 0,
            through: Some(1),
            changes: vec![change("a", 1)],
        }],
        500,
    )
    .unwrap();
    socket_offer(&mut control, &socket, &live).unwrap();
    commit_control_delivery(&mut c, &mut control);
    assert_eq!(
        c.store_status05().unwrap().cursor,
        Some(1),
        "live authority commits while HTTP has not answered"
    );
    let mut stale = change("a", 1);
    if let v05::AuthorityChange::Record { state, .. } = &mut stale {
        state["text"] = serde_json::json!("stale HTTP");
    }
    let late = v05::freeze_delivery(
        context,
        "late-http".into(),
        v05::DeliveryPurpose::Sync,
        0,
        1,
        1,
        10000,
        vec![v05::DeliveryUnit {
            index: 0,
            through: Some(1),
            changes: vec![stale],
        }],
        500,
    )
    .unwrap();
    control
        .receive(
            &pull,
            EffectOutcome {
                ok: true,
                value: Some(serde_json::Value::String(
                    serde_json::to_string(&v05::DeliveryResponse {
                        header: late.header,
                        parts: late.parts,
                    })
                    .unwrap(),
                )),
                error: None,
            },
            1,
        )
        .unwrap();
    commit_control_delivery(&mut c, &mut control);
    assert_eq!(
        c.query("Entry", &serde_json::json!({})).unwrap()[0]["text"],
        "a"
    );
    assert_eq!(c.store_status05().unwrap().cursor, Some(1));
}
#[test]
fn cursor_pages_resume_after_reopen_and_later_sync_installs_current_state_and_deletion() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db");
    let mut c = client(&path);
    c.initialize_stream05(2, "1").unwrap();
    let context = c.request_context05().unwrap();
    let page = |id: &str, bootstrap, after, through, changes| {
        v05::freeze_delivery(
            context.clone(),
            id.into(),
            if bootstrap {
                v05::DeliveryPurpose::Bootstrap
            } else {
                v05::DeliveryPurpose::Sync
            },
            after,
            through,
            4,
            10000,
            vec![v05::DeliveryUnit {
                index: 0,
                through: Some(through),
                changes,
            }],
            500,
        )
        .unwrap()
    };
    let first = page(
        "first",
        true,
        0,
        1,
        vec![change("a", 1), change("deleted", 1)],
    );
    let request = v05::DeltaRequest {
        context: context.clone(),
        after: 0,
        through: 2,
        bootstrap: true,
    };
    let response = v05::DeliveryResponse {
        header: first.header.clone(),
        parts: first.parts.clone(),
    };
    response.admit(&request, &context).unwrap();
    response.require_complete().unwrap();
    let mut queue = DeliveryQueue::new(1000000, 4);
    queue
        .receive(&first.header, &first.parts, &context, 1)
        .unwrap();
    c.apply_next_delivery05(&mut queue, 1).unwrap().unwrap();
    assert_eq!(c.store_status05().unwrap().bootstrap_cursor, Some(1));
    drop(c);
    let mut c = client(&path);
    assert_eq!(c.store_status05().unwrap().bootstrap_cursor, Some(1));
    // The unconsumed b@2 has moved to 3. Completing the fixed Bootstrap
    // prefix does not require retrieving its old payload or a server plan.
    let second = page("fresh-after-reopen", true, 1, 2, vec![]);
    queue
        .receive(&second.header, &second.parts, &context, 1)
        .unwrap();
    c.apply_next_delivery05(&mut queue, 1).unwrap().unwrap();
    assert_eq!(c.store_status05().unwrap().bootstrap_cursor, Some(2));
    let mut latest = change("b", 3);
    if let v05::AuthorityChange::Record { state, .. } = &mut latest {
        *state = serde_json::json!({"text":"latest","note":null});
    }
    let deleted = v05::AuthorityChange::Record {
        key: v05::RecordKey {
            model: "Entry".into(),
            identity: serde_json::json!({"id":"deleted"}),
        },
        cursor: 4,
        state: serde_json::Value::Null,
    };
    let sync = page("latest", false, 2, 4, vec![latest, deleted]);
    queue
        .receive(&sync.header, &sync.parts, &context, 1)
        .unwrap();
    c.apply_next_delivery05(&mut queue, 1).unwrap().unwrap();
    let key = |id| axton_client::RecordKey {
        model: "Entry".into(),
        identity: serde_json::json!({"id":id}),
    };
    assert_eq!(c.read(&key("b")).unwrap().unwrap()["text"], "latest");
    assert!(c.read(&key("deleted")).unwrap().is_none());
    assert_eq!(c.store_status05().unwrap().cursor, Some(4));
    queue
        .receive(&first.header, &first.parts, &context, 1)
        .unwrap();
    assert!(c.apply_next_delivery05(&mut queue, 1).unwrap().is_none());
    assert!(c.read(&key("deleted")).unwrap().is_none());
}
#[test]
fn socket_http_overlap_repeated_fragment_and_changed_payload_are_fenced() {
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let f = fragmented(context.clone(), false);
    let mut q = DeliveryQueue::new(1000000, 4);
    q.receive(&f.header, &f.parts[1..], &context, 1).unwrap();
    assert!(c.apply_next_delivery05(&mut q, 1).unwrap().is_none());
    assert_eq!(c.store_status05().unwrap().cursor, Some(0));
    q.receive(&f.header, &f.parts[1..], &context, 1).unwrap();
    let mut changed = f.parts[1].clone();
    changed.changes[0] = change("b", 2);
    assert!(q.receive(&f.header, &[changed], &context, 1).is_err());
    let mut changed_header = f.header.clone();
    changed_header.expires_at += 1;
    assert!(
        q.receive(&changed_header, &f.parts[..1], &context, 1)
            .is_err()
    );
    q.receive(&f.header, &f.parts[..1], &context, 1).unwrap();
    let report = c.apply_next_delivery05(&mut q, 1).unwrap().unwrap();
    assert_eq!(report.applied, 2);
    assert_eq!(c.store_status05().unwrap().cursor, Some(2));
}
#[test]
fn failed_sql_apply_keeps_unit_and_progress_unchanged_until_explicit_retry() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut c = client(&p);
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let mut units = vec![v05::DeliveryUnit {
        index: 0,
        through: Some(1),
        changes: vec![change("a", 1)],
    }];
    if let v05::AuthorityChange::Record { state, .. } = &mut units[0].changes[0] {
        *state = serde_json::json!({"text":7,"note":null})
    }
    let f = v05::freeze_delivery(
        context.clone(),
        "bad".into(),
        v05::DeliveryPurpose::Sync,
        0,
        1,
        1,
        10000,
        units,
        1,
    )
    .unwrap();
    let mut q = DeliveryQueue::new(1000000, 4);
    q.receive(&f.header, &f.parts, &context, 1).unwrap();
    assert!(c.apply_next_delivery05(&mut q, 1).is_err());
    assert_eq!(q.len(), 1);
    let overlapping = plan(context.clone(), "overlapping-good", 0, 1, false);
    q.receive(&overlapping.header, &overlapping.parts, &context, 1)
        .unwrap();
    // A failed Sync unit blocks the lane, including another verified offer.
    assert!(c.apply_next_delivery05(&mut q, 1).unwrap().is_none());
    assert_eq!(c.store_status05().unwrap().cursor, Some(0));
    drop(c);
    let mut c = client(&p);
    assert_eq!(c.store_status05().unwrap().cursor, Some(0));
    assert_eq!(
        c.read(&axton_client::RecordKey {
            model: "Entry".into(),
            identity: serde_json::json!({"id":"a"})
        })
        .unwrap(),
        None
    );
}
#[test]
fn late_generation_and_expired_plan_leave_progress_unchanged() {
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let f = plan(context.clone(), "old", 0, 1, false);
    let mut q = DeliveryQueue::new(1000000, 4);
    assert!(q.receive(&f.header, &f.parts, &context, 10000).is_err());
    let mut replacement = context.clone();
    replacement.materialization = "replacement".into();
    assert!(q.receive(&f.header, &f.parts, &replacement, 1).is_err());
    assert_eq!(c.store_status05().unwrap().cursor, Some(0));
}
#[test]
fn zero_bootstrap_intermediate_no_progress_does_not_mark_complete() {
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let f = v05::freeze_delivery(
        context.clone(),
        "zero".into(),
        v05::DeliveryPurpose::Bootstrap,
        0,
        0,
        2,
        10000,
        vec![
            v05::DeliveryUnit {
                index: 0,
                through: None,
                changes: vec![change("a", 1)],
            },
            v05::DeliveryUnit {
                index: 1,
                through: Some(0),
                changes: vec![change("b", 2)],
            },
        ],
        1,
    )
    .unwrap();
    let mut q = DeliveryQueue::new(1000000, 4);
    q.receive(&f.header, &f.parts[..1], &context, 1).unwrap();
    c.apply_next_delivery05(&mut q, 1).unwrap().unwrap();
    assert_eq!(c.store_status05().unwrap().bootstrap_cursor, None);
    q.receive(&f.header, &f.parts[1..], &context, 1).unwrap();
    c.apply_next_delivery05(&mut q, 1).unwrap().unwrap();
    assert_eq!(c.store_status05().unwrap().bootstrap_cursor, Some(0));
}
#[test]
fn oversized_atomic_unit_spills_to_disk_and_applies_as_one_commit() {
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let f = fragmented(context.clone(), false);
    let mut q = DeliveryQueue::new(1024, 4);
    q.receive(&f.header, &f.parts, &context, 1).unwrap();
    assert!(c.apply_next_delivery05(&mut q, 1).unwrap().is_some());
    assert_eq!(c.store_status05().unwrap().cursor, Some(2));
}
#[test]
fn duplicate_identity_across_units_cannot_commit_later_prefix() {
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let mut f = v05::freeze_delivery(
        context.clone(),
        "duplicated".into(),
        v05::DeliveryPurpose::Sync,
        0,
        2,
        2,
        10000,
        vec![
            v05::DeliveryUnit {
                index: 0,
                through: Some(1),
                changes: vec![change("a", 1)],
            },
            v05::DeliveryUnit {
                index: 1,
                through: Some(2),
                changes: vec![change("b", 2)],
            },
        ],
        1,
    )
    .unwrap();
    f.parts[1].changes[0] = change("a", 2);
    f.header.units[1].digest = v05::unit_digest(&v05::DeliveryUnit {
        index: 1,
        through: Some(2),
        changes: f.parts[1].changes.clone(),
    })
    .unwrap();
    f.header.units[1].parts[0] = v05::part_digest(&f.parts[1]).unwrap();
    f.header.digest = v05::delivery_digest(&f.header).unwrap();
    for part in &mut f.parts {
        part.plan_digest = f.header.digest.clone();
    }
    let mut q = DeliveryQueue::new(1000000, 4);
    q.receive(&f.header, &f.parts, &context, 1).unwrap();
    c.apply_next_delivery05(&mut q, 1).unwrap().unwrap();
    assert!(c.apply_next_delivery05(&mut q, 1).is_err());
    assert_eq!(c.store_status05().unwrap().cursor, Some(1));
}
struct CrashStore {
    store: SqliteStore,
    armed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl axton_client::ClientStore for CrashStore {
    fn begin(&mut self) -> axton_client::Result<()> {
        self.store.begin()
    }
    fn commit(&mut self) -> axton_client::Result<()> {
        self.store.commit()?;
        if self.armed.load(std::sync::atomic::Ordering::SeqCst) {
            std::process::exit(73)
        }
        Ok(())
    }
    fn rollback(&mut self) -> axton_client::Result<()> {
        self.store.rollback()
    }
    fn savepoint(&mut self, n: &str) -> axton_client::Result<()> {
        self.store.savepoint(n)
    }
    fn release(&mut self, n: &str) -> axton_client::Result<()> {
        self.store.release(n)
    }
    fn rollback_to(&mut self, n: &str) -> axton_client::Result<()> {
        self.store.rollback_to(n)
    }
    fn execute(&mut self, s: &str, p: &[serde_json::Value]) -> axton_client::Result<usize> {
        self.store.execute(s, p)
    }
    fn execute_batch(&mut self, s: &str) -> axton_client::Result<()> {
        self.store.execute_batch(s)
    }
    fn query(
        &mut self,
        s: &str,
        p: &[serde_json::Value],
    ) -> axton_client::Result<axton_client::store::SqlRows> {
        self.store.query(s, p)
    }
    fn query_committed(
        &mut self,
        s: &str,
        p: &[serde_json::Value],
    ) -> axton_client::Result<axton_client::store::SqlRows> {
        self.store.query_committed(s, p)
    }
}
#[test]
#[ignore]
fn crash_after_sql_commit_before_dequeue_child() {
    let path = std::env::var("AXTON_TASK5_CRASH_DB").unwrap();
    let armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let store = CrashStore {
        store: SqliteStore::open(path).unwrap(),
        armed: armed.clone(),
    };
    // The Entry schema with its `Edit` Mutation, as the parent reopens it.
    let mut c = Client::open05(store, common05::schema(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let f = v05::freeze_delivery(
        context.clone(),
        "crash-plan".into(),
        v05::DeliveryPurpose::Sync,
        0,
        1,
        1,
        10000,
        vec![v05::DeliveryUnit {
            index: 0,
            through: Some(1),
            changes: vec![change("a", 1)],
        }],
        1,
    )
    .unwrap();
    let mut q = DeliveryQueue::new(1000000, 4);
    q.receive(&f.header, &f.parts, &context, 1).unwrap();
    armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = c.apply_next_delivery05(&mut q, 1);
    panic!("crash commit hook not reached");
}
#[test]
fn crash_after_commit_before_dequeue_reopens_without_reapplying_under_later_optimism() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("db");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "crash_after_sql_commit_before_dequeue_child",
        ])
        .env("AXTON_TASK5_CRASH_DB", &path)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(73));
    let mut c = common05::open(&path);
    assert_eq!(c.store_status05().unwrap().cursor, Some(1));
    let key = axton_client::RecordKey {
        model: "Entry".into(),
        identity: serde_json::json!({"id":"a"}),
    };
    assert_eq!(c.read(&key).unwrap().unwrap()["text"], "a");
    c.transaction(|tx| {
        tx.submit_mutation05(
            "Edit",
            1,
            serde_json::json!({"entry":{"id":"a","text":"later"}}),
            vec![],
        )
    })
    .unwrap();
    let context = c.request_context05().unwrap();
    let f = v05::freeze_delivery(
        context.clone(),
        "crash-plan".into(),
        v05::DeliveryPurpose::Sync,
        0,
        1,
        1,
        10000,
        vec![v05::DeliveryUnit {
            index: 0,
            through: Some(1),
            changes: vec![change("a", 1)],
        }],
        1,
    )
    .unwrap();
    let mut q = DeliveryQueue::new(1000000, 4);
    q.receive(&f.header, &f.parts, &context, 1).unwrap();
    assert!(c.apply_next_delivery05(&mut q, 1).unwrap().is_none());
    assert_eq!(c.read(&key).unwrap().unwrap()["text"], "later");
}
#[test]
fn foreign_live_context_is_only_a_head_hint_for_retained_context_repair() {
    use axton_client::{
        runtime::{EffectOutcome, Event, HttpRoute, Operation},
        sync05::{Control, StoreReport},
    };
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let f = plan(context.clone(), "boot", 0, 0, true);
    let mut q = DeliveryQueue::new(10000, 4);
    q.receive(&f.header, &f.parts, &context, 1).unwrap();
    c.apply_next_delivery05(&mut q, 1).unwrap();
    let status = c.store_status05().unwrap();
    let mut control = Control::new(status.clone());
    control.connect().unwrap();
    let events = control.events();
    let h = events
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
    control.receive(&h,EffectOutcome{ok:true,value:Some(serde_json::json!({"protocol":axton_client::v05::PROTOCOL,"storeId":context.store_id,"stream":context.stream,"head":0,"projectionGeneration":"1"}).to_string().into()),error:None},1).unwrap();
    let socket = control
        .events()
        .into_iter()
        .find_map(|e| match e {
            Event::Effect {
                effect_id,
                operation: Operation::Socket { .. },
            } => Some(effect_id),
            _ => None,
        })
        .unwrap();
    let _ = control.jobs();
    control.report(StoreReport::Snapshot(status), 1).unwrap();
    let _ = control.jobs();
    let _ = control.events();
    let mut foreign = context.clone();
    foreign.materialization = "foreign".into();
    let f = plan(foreign, "foreign-live", 0, 5, false);
    control.receive(&socket,EffectOutcome{ok:true,value:Some(serde_json::json!({"event":"message","body":serde_json::json!({"header":f.header,"parts":f.parts}).to_string()})),error:None},1).unwrap();
    assert!(
        control
            .jobs()
            .into_iter()
            .all(|j| !matches!(j, axton_client::sync05::StoreCommand::Apply { .. }))
    );
    let repair = control
        .events()
        .into_iter()
        .find_map(|e| match e {
            Event::Effect {
                operation:
                    Operation::Http {
                        route: HttpRoute::Pull,
                        body,
                    },
                ..
            } => Some(serde_json::from_str::<v05::DeltaRequest>(&body).unwrap()),
            _ => None,
        })
        .unwrap();
    assert_eq!(repair.context, context);
    assert_eq!((repair.after, repair.through), (0, 5));
    assert_eq!(c.store_status05().unwrap().cursor, Some(0));
}

#[test]
fn slow_observer_receiver_does_not_hold_apply_and_notices_only_commits() {
    use std::collections::BTreeSet;
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let observer = c.watch(BTreeSet::from(["Entry".into()]));
    let first = v05::freeze_delivery(
        context.clone(),
        "observer-one".into(),
        v05::DeliveryPurpose::Sync,
        0,
        1,
        1,
        10000,
        vec![v05::DeliveryUnit {
            index: 0,
            through: Some(1),
            changes: vec![change("a", 1)],
        }],
        1,
    )
    .unwrap();
    let second = v05::freeze_delivery(
        context.clone(),
        "observer-two".into(),
        v05::DeliveryPurpose::Sync,
        1,
        2,
        2,
        10000,
        vec![v05::DeliveryUnit {
            index: 0,
            through: Some(2),
            changes: vec![change("b", 2)],
        }],
        1,
    )
    .unwrap();
    let mut q = DeliveryQueue::new(100000, 4);
    q.receive(&first.header, &first.parts, &context, 1).unwrap();
    assert!(observer.try_recv().is_err());
    c.apply_next_delivery05(&mut q, 1).unwrap();
    // The observer deliberately does not consume the first committed notice.
    q.receive(&second.header, &second.parts, &context, 1)
        .unwrap();
    c.apply_next_delivery05(&mut q, 1).unwrap();
    assert_eq!(c.store_status05().unwrap().cursor, Some(2));
    assert_eq!(observer.try_iter().count(), 2);
}

#[test]
fn manifest_over_capacity_is_refused_without_data_or_progress() {
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let before = c.store_status05().unwrap();
    let f = plan(before.context.clone(), "too-large", 0, 1, false);
    let mut q = DeliveryQueue::new(16, 4);
    assert!(q.receive(&f.header, &f.parts, &before.context, 1).is_err());
    assert!(q.is_empty());
    assert_eq!(
        serde_json::to_value(c.store_status05().unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}

#[test]
fn terminal_admission_refusal_cancels_control_without_auth_or_backoff() {
    use axton_client::{
        runtime::{Diagnostic, EffectError, EffectOutcome, Event, Operation},
        sync05::Control,
    };
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    let status = c.store_status05().unwrap();
    let mut control = Control::new(status.clone());
    control.set_refresh_auth(true);
    control.connect().unwrap();
    let events = control.events();
    let id = events
        .iter()
        .find_map(|e| {
            if let Event::Effect { effect_id, .. } = e {
                Some(effect_id.clone())
            } else {
                None
            }
        })
        .unwrap();
    control
        .receive(
            &id,
            EffectOutcome {
                ok: false,
                value: None,
                error: Some(EffectError {
                    message: "minimum build".into(),
                    status: Some(426),
                    retry: false,
                    refusal: Some("{\"minimumBuild\":7}".into()),
                }),
            },
            1,
        )
        .unwrap();
    let events = control.events();
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Report {
            diagnostic: Diagnostic::Refused { status: 426, .. }
        }
    )));
    assert!(!events.iter().any(|e| matches!(
        e,
        Event::Effect {
            operation: Operation::Timer { .. } | Operation::RefreshAuth,
            ..
        }
    )));
    assert!(control.jobs().is_empty());
    assert_eq!(
        serde_json::to_value(c.store_status05().unwrap()).unwrap(),
        serde_json::to_value(status).unwrap()
    );
}

#[test]
fn explicit_reset_retires_frozen_batch_and_reused_ids_have_new_store_owner() {
    use serde_json::json;
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("db");
    let mut s: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    s["actions"] = json!([{"name":"Write","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"create","cardinality":"single"}],"outputs":[]}]);
    let schema = Schema::from_value(s).unwrap();
    let mut c =
        Client::open05(SqliteStore::open(&path).unwrap(), schema.clone(), "User:u").unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let old = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"old","note":null}}),
                vec![],
            )
        })
        .unwrap();
    let batch = c.freeze_batch05().unwrap().unwrap();
    assert!(c.reset_store05(false).is_err());
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), batch);
    let report = c.reset_store05(true).unwrap();
    assert_ne!(report.context.store_id, batch.context.store_id);
    assert_eq!(
        report.abandoned_calls,
        vec![axton_client::AbandonedCall {
            call_id: old.call_id,
            frozen: true
        }]
    );
    assert!(
        c.read(&axton_client::RecordKey {
            model: "Entry".into(),
            identity: json!({"id":"e"})
        })
        .unwrap()
        .is_none()
    );
    let late = v05::BatchAcknowledgement {
        context: batch.context.clone(),
        batch_id: batch.batch_id,
        digest: batch.digest,
        results: vec![v05::MutationResult {
            mutation_id: old.ordinal,
            outcome: v05::MutationOutcome::Rejected {
                code: "write.denied".into(),
                message: None,
            },
        }],
    };
    assert!(c.acknowledge_batch05(&late).is_err());
    let call = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                json!({"entry":{"id":"e","text":"new","note":null}}),
                vec![],
            )
        })
        .unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let fresh = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(fresh.batch_id, batch.batch_id);
    assert_eq!(call.ordinal, old.ordinal);
    assert_ne!(fresh.context.store_id, batch.context.store_id);
    drop(c);
    let mut c = Client::open05(SqliteStore::open(&path).unwrap(), schema, "User:u").unwrap();
    assert_eq!(c.freeze_batch05().unwrap().unwrap(), fresh);
}

#[test]
fn completed_and_expired_transfer_staging_is_cleared() {
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let f = fragmented(context.clone(), false);
    let mut q = DeliveryQueue::new(100000, 4);
    q.receive(&f.header, &f.parts, &context, 1).unwrap();
    c.apply_next_delivery05(&mut q, 1).unwrap();
    assert_eq!(
        c.read_sql("SELECT COUNT(*) AS n FROM axton_delivery_progress", &[])
            .unwrap()[0]["n"],
        0
    );
    assert_eq!(
        c.read_sql("SELECT COUNT(*) AS n FROM axton_delivery_key", &[])
            .unwrap()[0]["n"],
        0
    );
}

#[test]
fn earlier_missing_prefix_can_replace_near_capacity_future_manifest() {
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let later = plan(context.clone(), "future", 2, 3, false);
    let early = plan(context.clone(), "repair", 0, 2, false);
    let cap = serde_json::to_vec(&later.header).unwrap().len()
        + serde_json::to_vec(&early.header).unwrap().len()
        - 1;
    let mut q = DeliveryQueue::new(cap, 4);
    q.receive(&later.header, &later.parts, &context, 1).unwrap();
    assert!(q.receive(&early.header, &early.parts, &context, 1).is_ok());
    c.apply_next_delivery05(&mut q, 1).unwrap();
    assert_eq!(c.store_status05().unwrap().cursor, Some(2));
}

#[test]
fn stopped_control_ignores_already_dispatched_freeze_report() {
    use axton_client::{
        runtime::{ClientRuntime, Event, HttpRoute, Operation},
        sync05::Control,
    };
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("db");
    let mut s: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    s["actions"] = serde_json::json!([{"name":"Write","version":1,"inputs":[],"outputs":[]}]);
    let mut c = Client::open05(
        SqliteStore::open(&path).unwrap(),
        Schema::from_value(s).unwrap(),
        "User:u",
    )
    .unwrap();
    c.transaction(|tx| tx.submit_mutation05("Write", 1, serde_json::json!({}), vec![]))
        .unwrap();
    let mut control = Control::new(c.store_status05().unwrap());
    control.connect().unwrap();
    control.events();
    control.wake();
    let command = control.jobs().into_iter().next().unwrap();
    let mut runtime = ClientRuntime::new(c);
    let report = runtime.store_worker05(command).unwrap();
    control.stop();
    control.events();
    control.report(report, 1).unwrap();
    assert!(!control.events().iter().any(|e| matches!(
        e,
        Event::Effect {
            operation: Operation::Http {
                route: HttpRoute::Push,
                ..
            },
            ..
        }
    )));
}

#[test]
fn partial_transfer_retention_is_bounded_and_expiry_clears_keys() {
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    for index in 0..35 {
        let f = v05::freeze_delivery(
            context.clone(),
            format!("partial{index}"),
            v05::DeliveryPurpose::Sync,
            0,
            2,
            2,
            10000,
            vec![
                v05::DeliveryUnit {
                    index: 0,
                    through: Some(1),
                    changes: vec![change(&format!("row{index}"), 1)],
                },
                v05::DeliveryUnit {
                    index: 1,
                    through: Some(2),
                    changes: vec![],
                },
            ],
            1,
        )
        .unwrap();
        let mut q = DeliveryQueue::new(100000, 4);
        q.receive(&f.header, &f.parts[..1], &context, 1).unwrap();
        c.apply_next_delivery05(&mut q, 1).unwrap().unwrap();
        assert!(c.active_delivery_plans05().unwrap().len() <= 1);
    }
    assert_eq!(c.active_delivery_plans05().unwrap().len(), 1);
    assert_eq!(c.store_status05().unwrap().cursor, Some(1));
    assert_eq!(
        c.read_sql("SELECT COUNT(*) AS n FROM axton_delivery_key", &[])
            .unwrap()[0]["n"],
        1
    );
    assert!(c.cleanup_delivery05(10000).unwrap().is_empty());
    assert_eq!(
        c.read_sql("SELECT COUNT(*) AS n FROM axton_delivery_key", &[])
            .unwrap()[0]["n"],
        0
    );
    let repair = plan(context.clone(), "fresh-prefix", 1, 2, false);
    let mut q = DeliveryQueue::new(100000, 4);
    q.receive(&repair.header, &repair.parts, &context, 10001)
        .unwrap_err();
    // Repair must be newly frozen, rather than reviving an expired transfer.
    let repair = plan(context.clone(), "fresh-prefix", 1, 2, false);
    q.receive(&repair.header, &repair.parts, &context, 2)
        .unwrap();
    c.apply_next_delivery05(&mut q, 2).unwrap().unwrap();
    assert_eq!(c.store_status05().unwrap().cursor, Some(2));
}

#[test]
fn earlier_missing_prefix_can_replace_near_capacity_payload() {
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let later = plan(context.clone(), "future", 2, 3, false);
    let early = plan(context.clone(), "repair", 0, 2, false);
    let budget = serde_json::to_vec(&later.parts[0]).unwrap().len()
        + serde_json::to_vec(&early.parts[0]).unwrap().len()
        - 1;
    let mut q = DeliveryQueue::with_limits(100000, 4, budget);
    q.receive(&later.header, &later.parts, &context, 1).unwrap();
    q.receive(&early.header, &early.parts, &context, 1).unwrap();
    c.apply_next_delivery05(&mut q, 1).unwrap().unwrap();
    assert_eq!(c.store_status05().unwrap().cursor, Some(2));
}

#[test]
#[ignore]
fn crash_spill_has_no_named_remnants_child() {
    let dir = std::env::var("AXTON_TASK5_SPOOL_DIR").unwrap();
    let context = v05::RequestContext {
        protocol: axton_client::v05::PROTOCOL,
        store_id: "spool".into(),
        stream: "User:u".into(),
        materialization: "m".into(),
    };
    let mut value = change("a", 1);
    if let v05::AuthorityChange::Record { state, .. } = &mut value {
        state["text"] = serde_json::json!("x".repeat(100000));
    }
    let f = v05::freeze_delivery(
        context.clone(),
        "spilled".into(),
        v05::DeliveryPurpose::Sync,
        0,
        1,
        1,
        10000,
        vec![v05::DeliveryUnit {
            index: 0,
            through: Some(1),
            changes: vec![value],
        }],
        1,
    )
    .unwrap();
    let mut q = DeliveryQueue::new(10000, 4);
    q.receive(&f.header, &f.parts, &context, 1).unwrap();
    assert_eq!(std::fs::read_dir(dir).unwrap().count(), 0);
    std::process::exit(74);
}

#[test]
fn process_crash_spill_reclaims_without_sweeping_other_clients() {
    let d = tempfile::tempdir().unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "crash_spill_has_no_named_remnants_child",
        ])
        .env("TMPDIR", d.path())
        .env("AXTON_TASK5_SPOOL_DIR", d.path())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(74));
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
}

#[test]
fn repair_continuation_can_replace_near_capacity_future_payload() {
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let future = plan(context.clone(), "future", 2, 3, false);
    let repair = fragmented(context.clone(), false);
    let budget = serde_json::to_vec(&future.parts[0]).unwrap().len()
        + repair
            .parts
            .iter()
            .map(|p| serde_json::to_vec(p).unwrap().len())
            .sum::<usize>()
        - 1;
    let mut q = DeliveryQueue::with_limits(100000, 4, budget);
    q.receive(&future.header, &future.parts, &context, 1)
        .unwrap();
    q.receive(&repair.header, &repair.parts[..1], &context, 1)
        .unwrap();
    assert!(c.apply_next_delivery05(&mut q, 1).unwrap().is_none());
    assert_eq!(c.store_status05().unwrap().cursor, Some(0));
    q.receive(&repair.header, &repair.parts[1..], &context, 1)
        .unwrap();
    c.apply_next_delivery05(&mut q, 1).unwrap().unwrap();
    assert_eq!(c.store_status05().unwrap().cursor, Some(2));
}

#[test]
fn background_network_failures_keep_status_during_retry_and_auth_refresh() {
    use axton_client::{
        runtime::{Diagnostic, EffectError, EffectOutcome, Event},
        sync05::Control,
    };
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    for status in [503, 401] {
        let mut control = Control::new(c.store_status05().unwrap());
        control.set_refresh_auth(true);
        control.connect().unwrap();
        let id = control
            .events()
            .into_iter()
            .find_map(|e| match e {
                Event::Effect { effect_id, .. } => Some(effect_id),
                _ => None,
            })
            .unwrap();
        control
            .receive(
                &id,
                EffectOutcome {
                    ok: false,
                    value: None,
                    error: Some(EffectError {
                        message: "network failed".into(),
                        status: Some(status),
                        refusal: None,
                        retry: false,
                    }),
                },
                1,
            )
            .unwrap();
        assert!(control.events().iter().any(|e|matches!(e,Event::Report{diagnostic:Diagnostic::Error{status:Some(value),..}} if *value==status)));
    }
}

#[test]
fn settlement_progress_wakes_uplink_once_and_waiting_needs_stays_idle() {
    use axton_client::sync05::{Control, StoreCommand, StoreReport};
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let status = c.store_status05().unwrap();
    let mut control = Control::new(status.clone());
    control.connect().unwrap();
    control.events();
    control.report(StoreReport::Frozen(None), 1).unwrap();
    control.jobs();
    control
        .report(StoreReport::Needs { status: None }, 1)
        .unwrap();
    assert!(
        control.jobs().is_empty(),
        "waiting settlement must not spin Freeze/Needs"
    );
    control
        .report(
            StoreReport::Needs {
                status: Some(status),
            },
            1,
        )
        .unwrap();
    let jobs = control.jobs();
    assert_eq!(jobs.len(), 1);
    assert!(
        matches!(&jobs[0], StoreCommand::Guarded { command, .. } if matches!(**command, StoreCommand::Freeze))
    );
    control
        .report(StoreReport::Needs { status: None }, 1)
        .unwrap();
    assert!(control.jobs().is_empty());
}

fn pending_settlement_control(
    path: &std::path::Path,
) -> (Client<SqliteStore>, axton_client::sync05::Control) {
    use axton_client::sync05::{Control, StoreReport};
    let mut schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    schema["actions"] = serde_json::json!([{"name":"Write","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"create","cardinality":"single"}],"outputs":[]}]);
    let mut c = Client::open05(
        SqliteStore::open(path).unwrap(),
        Schema::from_value(schema).unwrap(),
        "User:u",
    )
    .unwrap();
    c.initialize_stream05(0, "1").unwrap();
    let context = c.request_context05().unwrap();
    let bootstrap = plan(context.clone(), "initial", 0, 0, true);
    let mut queue = DeliveryQueue::new(1000000, 4);
    queue
        .receive(&bootstrap.header, &bootstrap.parts, &context, 1)
        .unwrap();
    c.apply_next_delivery05(&mut queue, 1).unwrap();
    let call = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                serde_json::json!({"entry":{"id":"e","text":"optimistic","note":null}}),
                vec![],
            )
        })
        .unwrap();
    let batch = c.freeze_batch05().unwrap().unwrap();
    let key = v05::RecordKey {
        model: "Entry".into(),
        identity: serde_json::json!({"id":"e"}),
    };
    c.acknowledge_batch05(&v05::BatchAcknowledgement {
        context: context.clone(),
        batch_id: batch.batch_id,
        digest: batch.digest,
        results: vec![v05::MutationResult {
            mutation_id: call.ordinal,
            outcome: v05::MutationOutcome::Accepted {
                sync_cursor: 1,
                materialization: context.materialization.clone(),
                result: serde_json::Value::Null,
                targets: vec![v05::SettlementTarget::Stream {
                    key: key.clone(),
                    cursor: 1,
                    fallback: v05::ReadRecord {
                        key,
                        cursor: (),
                        state: serde_json::json!({"text":"accepted","note":null}),
                    },
                }],
            },
        }],
    })
    .unwrap();
    assert!(c.call_completion05(&call.call_id).unwrap().is_none());
    let mut control = Control::new(c.store_status05().unwrap());
    control.connect().unwrap();
    control.events();
    control
        .report(StoreReport::Needs { status: None }, 1)
        .unwrap();
    control.events();
    (c, control)
}

#[test]
fn settlement_commit_reconciles_before_freezing_next_batch() {
    use axton_client::{
        ApplyReport,
        sync05::{StoreCommand, StoreReport},
    };
    let dir = tempfile::tempdir().unwrap();
    let (mut c, mut control) = pending_settlement_control(&dir.path().join("db"));
    control.jobs();
    // Stream authority commits the accepted Mutation.
    let context = c.request_context05().unwrap();
    let delivery = plan(context.clone(), "settled-prefix", 0, 1, false);
    c.install_authority05(&context, &[change("e", 1)], None)
        .unwrap();
    let mut queue = DeliveryQueue::new(1000000, 4);
    queue
        .receive(&delivery.header, &delivery.parts, &context, 2)
        .unwrap();
    assert_eq!(
        c.apply_next_delivery05(&mut queue, 2)
            .unwrap()
            .unwrap()
            .completions
            .len(),
        1
    );
    control
        .report(
            StoreReport::Committed {
                status: c.store_status05().unwrap(),
                report: ApplyReport::default(),
                plan: None,
                active_plans: None,
            },
            2,
        )
        .unwrap();
    let jobs = control.jobs();
    assert!(jobs.iter().any(|job| matches!(job,StoreCommand::Guarded {command,..} if matches!(**command,StoreCommand::Needs))));
    assert!(!jobs.iter().any(|job| matches!(job,StoreCommand::Guarded {command,..} if matches!(**command,StoreCommand::Freeze))),"next Batch cannot freeze before durable settlement reconciliation");
    control
        .report(StoreReport::Needs { status: None }, 3)
        .unwrap();
    assert!(control.jobs().iter().any(|job| matches!(job,StoreCommand::Guarded {command,..} if matches!(**command,StoreCommand::Freeze))),"reconciliation must release the committed wake even without another Call completion");
}

#[test]
fn already_dispatched_freeze_waits_for_reconciliation_before_push() {
    use axton_client::{
        ApplyReport,
        runtime::{Event, HttpRoute, Operation},
        sync05::StoreReport,
    };
    for mode in [0, 1, 2] {
        let dir = tempfile::tempdir().unwrap();
        let (mut c, mut control) = pending_settlement_control(&dir.path().join("db"));
        let context = c.request_context05().unwrap();
        c.install_authority05(&context, &[change("e", 1)], None)
            .unwrap();
        let delivery = plan(context.clone(), "settled-prefix", 0, 1, false);
        let mut queue = DeliveryQueue::new(1000000, 4);
        queue
            .receive(&delivery.header, &delivery.parts, &context, 2)
            .unwrap();
        c.apply_next_delivery05(&mut queue, 2).unwrap();
        c.transaction(|tx| {
            tx.submit_mutation05(
                "Write",
                1,
                serde_json::json!({"entry":{"id":"next","text":"next","note":null}}),
                vec![],
            )
        })
        .unwrap();
        let batch = c.freeze_batch05().unwrap().unwrap();
        control
            .report(
                StoreReport::Committed {
                    status: c.store_status05().unwrap(),
                    report: ApplyReport::default(),
                    plan: None,
                    active_plans: None,
                },
                2,
            )
            .unwrap();
        control.report(StoreReport::Frozen(Some(batch)), 2).unwrap();
        assert!(
            !control.events().iter().any(|e| matches!(
                e,
                Event::Effect {
                    operation: Operation::Http {
                        route: HttpRoute::Push,
                        ..
                    },
                    ..
                }
            )),
            "already dispatched Freeze must hold Push until reconciliation"
        );
        if mode == 1 {
            control.stop();
            control.events();
        } else if mode == 2 {
            control.connect().unwrap();
            control.events();
        }
        control
            .report(StoreReport::Needs { status: None }, 3)
            .unwrap();
        let events = control.events();
        let pushes: Vec<_> = events
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                matches!(
                    e,
                    Event::Effect {
                        operation: Operation::Http {
                            route: HttpRoute::Push,
                            ..
                        },
                        ..
                    }
                )
            })
            .collect();
        if mode != 0 {
            assert!(
                pushes.is_empty(),
                "stop and handshake must discard held Push"
            );
        } else {
            assert_eq!(pushes.len(), 1);
            control
                .report(StoreReport::Needs { status: None }, 4)
                .unwrap();
            control
                .report(StoreReport::Snapshot(c.store_status05().unwrap()), 4)
                .unwrap();
            control
                .report(StoreReport::Needs { status: None }, 4)
                .unwrap();
            assert!(
                !control.events().iter().any(|e| matches!(
                    e,
                    Event::Effect {
                        operation: Operation::Http {
                            route: HttpRoute::Push,
                            ..
                        },
                        ..
                    }
                )),
                "repeated reports must not send a held Batch twice"
            );
        }
    }
}

fn handshake_effect(control: &mut axton_client::sync05::Control) -> Option<String> {
    use axton_client::runtime::{Event, HttpRoute, Operation};
    control.events().into_iter().find_map(|event| match event {
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
}
fn answer_handshake(
    control: &mut axton_client::sync05::Control,
    id: &str,
    context: &v05::RequestContext,
    head: u64,
    generation: &str,
) {
    let body = serde_json::to_string(&v05::HandshakeResponse {
        protocol: v05::PROTOCOL,
        store_id: context.store_id.clone(),
        stream: context.stream.clone(),
        head,
        projection_generation: generation.into(),
    })
    .unwrap();
    control
        .receive(
            id,
            axton_client::runtime::EffectOutcome {
                ok: true,
                value: Some(serde_json::Value::String(body)),
                error: None,
            },
            1,
        )
        .unwrap();
}
fn freezes(jobs: &[axton_client::sync05::StoreCommand]) -> bool {
    use axton_client::sync05::StoreCommand;
    jobs.iter().any(|job| {
        matches!(job, StoreCommand::Guarded { command, .. } if matches!(**command, StoreCommand::Freeze))
    })
}
#[test]
fn freeze_in_flight_across_a_generation_upgrade_does_not_stall_the_uplink() {
    use axton_client::sync05::{Control, StoreReport};
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let old = c.store_status05().unwrap();
    let mut control = Control::new(old.clone());
    control.connect().unwrap();
    let id = handshake_effect(&mut control).unwrap();
    // The handshake reports generation 2; its Initialize is dispatched.
    answer_handshake(&mut control, &id, &old.context, 0, "2");
    control.jobs();
    // The application commits a Mutation: wake dispatches Freeze in the old context.
    control
        .report(StoreReport::Snapshot(old.clone()), 1)
        .unwrap();
    control.jobs();
    control
        .report(StoreReport::Needs { status: None }, 1)
        .unwrap();
    assert!(freezes(&control.jobs()));
    // Initialize commits the upgrade: same Store, new materialization.
    let mut upgraded = old.clone();
    upgraded.context.materialization = "upgraded".into();
    control
        .report(StoreReport::Snapshot(upgraded.clone()), 1)
        .unwrap();
    assert!(
        handshake_effect(&mut control).is_none(),
        "an upgrade sends no second handshake"
    );
    // The old context's Freeze comes back obsolete; the uplink still wakes.
    control.report(StoreReport::Obsolete, 1).unwrap();
    control.jobs();
    control
        .report(StoreReport::Needs { status: None }, 1)
        .unwrap();
    assert!(freezes(&control.jobs()), "uplink stalled after the upgrade");
}
#[test]
fn pull_parked_for_auth_refresh_is_not_resent_after_a_generation_upgrade() {
    use axton_client::{
        runtime::{EffectError, EffectOutcome, Event, HttpRoute, Operation},
        sync05::{Control, StoreReport},
    };
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    c.initialize_stream05(0, "1").unwrap();
    let old = c.store_status05().unwrap();
    let mut control = Control::new(old.clone());
    control.set_refresh_auth(true);
    control.connect().unwrap();
    let id = handshake_effect(&mut control).unwrap();
    answer_handshake(&mut control, &id, &old.context, 5, "1");
    control.jobs();
    control
        .report(StoreReport::Snapshot(old.clone()), 1)
        .unwrap();
    let (pull, request) = cursor_pull(&mut control);
    assert_eq!(request.context, old.context);
    control
        .receive(
            &pull,
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
            1,
        )
        .unwrap();
    let refresh = control
        .events()
        .into_iter()
        .find_map(|event| match event {
            Event::Effect {
                effect_id,
                operation: Operation::RefreshAuth,
            } => Some(effect_id),
            _ => None,
        })
        .unwrap();
    // A handshake-time upgrade commits while the pull waits for the refresh.
    let mut upgraded = old.clone();
    upgraded.context.materialization = "upgraded".into();
    control
        .report(StoreReport::Snapshot(upgraded.clone()), 1)
        .unwrap();
    control.events();
    control
        .receive(
            &refresh,
            EffectOutcome {
                ok: true,
                value: None,
                error: None,
            },
            1,
        )
        .unwrap();
    let stale: Vec<_> = control
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::Effect {
                operation:
                    Operation::Http {
                        route: HttpRoute::Pull,
                        body,
                    },
                ..
            } => Some(serde_json::from_str::<v05::DeltaRequest>(&body).unwrap()),
            _ => None,
        })
        .filter(|request| request.context != upgraded.context)
        .collect();
    assert!(stale.is_empty(), "old-context pull resent: {stale:?}");
}

fn upgrade_read(c: &mut Client<SqliteStore>, model: &str, id: &str) -> Option<serde_json::Value> {
    c.read(&axton_client::RecordKey {
        model: model.into(),
        identity: serde_json::json!({ "id": id }),
    })
    .unwrap()
}
#[test]
fn bootstrap_after_an_upgrade_overwrites_held_rows_and_sync_from_c_deletes_and_removes_kept_rows() {
    use common05::{before_upgrade, deliver, upgrade_key, upgrade_record, upgrade_schema};
    use serde_json::json;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    before_upgrade(&p);
    let mut c = Client::open05(
        SqliteStore::open(&p).unwrap(),
        upgrade_schema(true, false),
        "User:u",
    )
    .unwrap();
    let upgraded = c.request_context05().unwrap().materialization;
    // Bootstrap over (0, S] in the new contract, at the row's unchanged position.
    deliver(
        &mut c,
        true,
        0,
        40,
        vec![upgrade_record(
            "Marked",
            "m",
            10,
            json!({"text":"marked","label":"filled"}),
        )],
    );
    assert_eq!(
        upgrade_read(&mut c, "Marked", "m").unwrap()["label"],
        "filled"
    );
    let key = upgrade_key("Marked", "m");
    let evidence = c
        .record_evidence05(&axton_client::RecordKey {
            model: key.model,
            identity: key.identity,
        })
        .unwrap();
    assert_eq!(
        evidence.history.into_iter().collect::<Vec<_>>(),
        vec![(upgraded, 10)]
    );
    let status = c.store_status05().unwrap();
    assert_eq!(
        (status.bootstrap_cursor, status.start_cursor),
        (Some(40), Some(40))
    );
    // Sync continues from C: a deletion after it deletes a kept row, and a
    // Remove after it releases the other kept row as cache.
    deliver(
        &mut c,
        false,
        40,
        45,
        vec![
            upgrade_record("Note", "gone", 44, serde_json::Value::Null),
            v05::AuthorityChange::Remove {
                key: upgrade_key("Note", "n"),
                cursor: 45,
            },
        ],
    );
    assert!(upgrade_read(&mut c, "Note", "gone").is_none());
    assert_eq!(upgrade_read(&mut c, "Note", "n").unwrap()["text"], "note");
    let key = upgrade_key("Note", "n");
    let removed = c
        .record_evidence05(&axton_client::RecordKey {
            model: key.model,
            identity: key.identity,
        })
        .unwrap()
        .membership
        .unwrap();
    assert_eq!((removed.cursor, removed.live), (45, false));
}
#[test]
fn a_unique_change_resets_the_model_and_bootstrap_fills_it_again() {
    use common05::{before_upgrade, deliver, upgrade_record, upgrade_schema};
    use serde_json::json;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let before = before_upgrade(&p);
    let mut c = Client::open05(
        SqliteStore::open(&p).unwrap(),
        upgrade_schema(false, true),
        "User:u",
    )
    .unwrap();
    let status = c.store_status05().unwrap();
    assert_eq!(status.context, before.context);
    assert_eq!(
        (status.start_cursor, status.bootstrap_cursor, status.cursor),
        (Some(40), None, Some(40))
    );
    assert!(upgrade_read(&mut c, "Marked", "m").is_none());
    assert_eq!(upgrade_read(&mut c, "Note", "n").unwrap()["text"], "note");
    deliver(
        &mut c,
        true,
        0,
        40,
        vec![upgrade_record("Marked", "m", 10, json!({"text":"marked"}))],
    );
    assert_eq!(
        upgrade_read(&mut c, "Marked", "m").unwrap()["text"],
        "marked"
    );
}
#[test]
fn a_handshake_reporting_a_new_generation_upgrades_in_its_commit_and_a_push_sent_before_it_acknowledges()
 {
    use common05::{before_upgrade, upgrade_key, upgrade_schema};
    use serde_json::json;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let before = before_upgrade(&p);
    let mut c = Client::open05(
        SqliteStore::open(&p).unwrap(),
        upgrade_schema(false, false),
        "User:u",
    )
    .unwrap();
    let call = c
        .transaction(|tx| {
            tx.submit_mutation05(
                "Edit",
                1,
                json!({"note":{"id":"n","text":"edited"}}),
                vec![],
            )
        })
        .unwrap();
    let sent = c.freeze_batch05().unwrap().unwrap();
    assert_eq!(sent.context, before.context);
    c.initialize_stream05(50, "2").unwrap();
    let status = c.store_status05().unwrap();
    assert_eq!(
        status.context.materialization,
        v05::materialization_id(&upgrade_schema(false, false), "2").unwrap()
    );
    assert_eq!(
        (status.start_cursor, status.bootstrap_cursor, status.cursor),
        (Some(40), None, Some(40))
    );
    assert_eq!(
        upgrade_read(&mut c, "Marked", "m").unwrap()["text"],
        "marked"
    );
    assert_eq!(upgrade_read(&mut c, "Note", "n").unwrap()["text"], "edited");
    // The receipt of the push sent before the upgrade answers in its context.
    c.acknowledge_batch05(&v05::BatchAcknowledgement {
        context: sent.context.clone(),
        batch_id: sent.batch_id,
        digest: sent.digest.clone(),
        results: vec![v05::MutationResult {
            mutation_id: call.ordinal,
            outcome: v05::MutationOutcome::Accepted {
                sync_cursor: 45,
                materialization: sent.context.materialization.clone(),
                result: serde_json::Value::Null,
                targets: vec![v05::SettlementTarget::Private {
                    record: v05::ReadRecord {
                        key: upgrade_key("Note", "n"),
                        cursor: (),
                        state: json!({"text":"old contract"}),
                    },
                }],
            },
        }],
    })
    .unwrap();
    assert_eq!(c.store_status05().unwrap().last_acknowledged_batch_id, 1);
    let context = c.request_context05().unwrap();
    let settled = c
        .install_authority05(&context, &[], Some((40, 45)))
        .unwrap();
    assert_eq!(settled.completions.len(), 1);
    // The snapshot was loaded in the previous contract and is not installed.
    assert_ne!(
        upgrade_read(&mut c, "Note", "n").unwrap()["text"],
        "old contract"
    );
}
#[test]
fn an_upgrade_drops_staging_of_the_old_context_and_pulls_bootstrap_without_a_handshake() {
    use axton_client::sync05::{StoreCommand, StoreReport};
    let d = tempfile::tempdir().unwrap();
    let mut c = client(&d.path().join("db"));
    let (mut control, socket) = connected_control(&mut c);
    control
        .report(StoreReport::Snapshot(c.store_status05().unwrap()), 1)
        .unwrap();
    control.jobs();
    control.events();
    let context = c.request_context05().unwrap();
    let offer = v05::freeze_delivery(
        context.clone(),
        "old".into(),
        v05::DeliveryPurpose::Sync,
        0,
        3,
        3,
        10000,
        vec![v05::DeliveryUnit {
            index: 0,
            through: Some(3),
            changes: vec![change("a", 3)],
        }],
        100000,
    )
    .unwrap();
    socket_offer(&mut control, &socket, &offer).unwrap();
    // The handshake's Initialize commits the upgrade before that Apply runs.
    c.initialize_stream05(3, "2").unwrap();
    let upgraded = c.store_status05().unwrap();
    assert_ne!(upgraded.context, context);
    control
        .report(StoreReport::Snapshot(upgraded.clone()), 1)
        .unwrap();
    assert!(
        !control.jobs().into_iter().any(|job| matches!(
            job,
            StoreCommand::Guarded { command, .. } if matches!(*command, StoreCommand::Apply { .. })
        )),
        "an Apply of the old context survived the upgrade"
    );
    use axton_client::runtime::{Event, HttpRoute, Operation};
    let requests: Vec<(HttpRoute, String)> = control
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::Effect {
                operation: Operation::Http { route, body },
                ..
            } => Some((route, body)),
            _ => None,
        })
        .collect();
    assert!(
        !requests
            .iter()
            .any(|(route, _)| matches!(route, HttpRoute::Handshake)),
        "an upgrade sends no second handshake"
    );
    let pulls: Vec<v05::DeltaRequest> = requests
        .iter()
        .filter(|(route, _)| matches!(route, HttpRoute::Pull))
        .map(|(_, body)| serde_json::from_str(body).unwrap())
        .collect();
    assert_eq!(pulls.len(), 1, "{pulls:?}");
    assert_eq!(pulls[0].context, upgraded.context);
    assert!(pulls[0].bootstrap);
    assert!(
        c.read(&axton_client::RecordKey {
            model: "Entry".into(),
            identity: serde_json::json!({"id":"a"}),
        })
        .unwrap()
        .is_none()
    );
}
/// Run Store work until the worker is idle, answering every push with the
/// receipt `answer` gives its request. Returns the pushes sent.
fn run_worker(
    runtime: &mut axton_client::runtime::ClientRuntime<SqliteStore>,
    control: &mut axton_client::sync05::Control,
    answer: impl Fn(&v05::MutationRequest) -> v05::BatchAcknowledgement,
) -> Vec<v05::MutationRequest> {
    use axton_client::runtime::{EffectOutcome, Event, HttpRoute, Operation};
    let mut pushes = vec![];
    for _ in 0..32 {
        let jobs = control.jobs();
        let sent: Vec<(String, v05::MutationRequest)> = control
            .events()
            .into_iter()
            .filter_map(|event| match event {
                Event::Effect {
                    effect_id,
                    operation:
                        Operation::Http {
                            route: HttpRoute::Push,
                            body,
                        },
                } => Some((effect_id, serde_json::from_str(&body).unwrap())),
                _ => None,
            })
            .collect();
        if jobs.is_empty() && sent.is_empty() {
            return pushes;
        }
        for job in jobs {
            let report = runtime.store_worker05(job).unwrap();
            control.report(report, 1).unwrap();
        }
        for (id, request) in sent {
            let body = serde_json::to_string(&answer(&request)).unwrap();
            control
                .receive(
                    &id,
                    EffectOutcome {
                        ok: true,
                        value: Some(serde_json::Value::String(body)),
                        error: None,
                    },
                    1,
                )
                .unwrap();
            pushes.push(request);
        }
    }
    panic!("Store work did not become idle");
}
#[test]
fn a_push_answered_after_a_generation_upgrade_is_acknowledged_through_the_sync_worker() {
    use axton_client::{
        runtime::{ClientRuntime, EffectOutcome},
        sync05::StoreReport,
    };
    use serde_json::json;
    let mut schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/schemas/entry.json")).unwrap();
    schema["actions"] = json!([{"name":"Write","version":1,"inputs":[{"kind":"model","name":"entry","model":"Entry","operation":"create","cardinality":"single"}],"outputs":[]}]);
    let schema = Schema::from_value(schema).unwrap();
    // `late`: the receipt arrives after the upgrade committed but before the
    // worker reported it, so its Acknowledge is fenced with the old context.
    for late in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let mut c = Client::open05(
            SqliteStore::open(d.path().join("db")).unwrap(),
            schema.clone(),
            "User:u",
        )
        .unwrap();
        let (mut control, socket) = connected_control(&mut c);
        let call = c
            .transaction(|tx| {
                tx.submit_mutation05(
                    "Write",
                    1,
                    json!({"entry":{"id":"e","text":"optimistic","note":null}}),
                    vec![],
                )
            })
            .unwrap();
        let mut runtime = ClientRuntime::new(c);
        control.wake();
        let receipt = |request: &v05::MutationRequest, label: &str| v05::BatchAcknowledgement {
            context: request.context.clone(),
            batch_id: request.batch_id,
            digest: request.digest.clone(),
            results: vec![v05::MutationResult {
                mutation_id: call.ordinal,
                outcome: v05::MutationOutcome::Accepted {
                    sync_cursor: 0,
                    materialization: label.into(),
                    result: serde_json::Value::Null,
                    targets: vec![v05::SettlementTarget::Private {
                        record: v05::ReadRecord {
                            key: v05::RecordKey {
                                model: "Entry".into(),
                                identity: json!({"id":"e"}),
                            },
                            cursor: (),
                            state: json!({"text":"accepted","note":null}),
                        },
                    }],
                },
            }],
        };
        // Freeze and send; the receipt is held back.
        let mut held = None;
        let jobs = control.jobs();
        for job in jobs {
            let report = runtime.store_worker05(job).unwrap();
            control.report(report, 1).unwrap();
        }
        for event in control.events() {
            if let axton_client::runtime::Event::Effect {
                effect_id,
                operation:
                    axton_client::runtime::Operation::Http {
                        route: axton_client::runtime::HttpRoute::Push,
                        body,
                    },
            } = event
            {
                held = Some((
                    effect_id,
                    serde_json::from_str::<v05::MutationRequest>(&body).unwrap(),
                ));
            }
        }
        let (push, sent) = held.expect("the frozen Batch is pushed");
        let old = sent.context.clone();
        // A socket handshake reports generation 2; its Initialize upgrades.
        let handshake = serde_json::to_string(&v05::HandshakeResponse {
            protocol: v05::PROTOCOL,
            store_id: old.store_id.clone(),
            stream: old.stream.clone(),
            head: 0,
            projection_generation: "2".into(),
        })
        .unwrap();
        control
            .receive(
                &socket,
                EffectOutcome {
                    ok: true,
                    value: Some(json!({"event":"message","body":handshake})),
                    error: None,
                },
                1,
            )
            .unwrap();
        let initialize = control.jobs();
        let reports: Vec<StoreReport> = initialize
            .into_iter()
            .map(|job| runtime.store_worker05(job).unwrap())
            .collect();
        let upgraded = runtime.client().request_context05().unwrap();
        assert_ne!(upgraded.materialization, old.materialization);
        let answer = EffectOutcome {
            ok: true,
            value: Some(serde_json::Value::String(
                serde_json::to_string(&receipt(&sent, &old.materialization)).unwrap(),
            )),
            error: None,
        };
        if late {
            control.receive(&push, answer, 1).unwrap();
            let acknowledge = control.jobs();
            for report in reports {
                control.report(report, 1).unwrap();
            }
            for job in acknowledge {
                let report = runtime.store_worker05(job).unwrap();
                assert!(matches!(report, StoreReport::Obsolete), "{late}");
                control.report(report, 1).unwrap();
            }
        } else {
            for report in reports {
                control.report(report, 1).unwrap();
            }
            control.receive(&push, answer, 1).unwrap();
        }
        // No further trigger: the worker acknowledges, or resends once.
        let resent = run_worker(&mut runtime, &mut control, |request| {
            receipt(request, &old.materialization)
        });
        if late {
            assert_eq!(resent.len(), 1, "one resend");
            assert_eq!(resent[0].digest, sent.digest);
            assert_eq!(resent[0].context, upgraded);
        } else {
            assert!(resent.is_empty(), "acknowledged directly");
        }
        let client = runtime.client();
        assert_eq!(
            client.store_status05().unwrap().last_acknowledged_batch_id,
            1,
            "{late}"
        );
        assert!(
            client.call_completion05(&call.call_id).unwrap().is_some(),
            "{late}"
        );
    }
}
