//! Shared settlement: one stamp per changed record, persistent Channel
//! membership reduced to its final state, and one publication position per
//! affected Channel/record pair, on the Action, legacy and external paths
//! ([spec §4-§6](../../../docs/superpowers/specs/2026-09-26-140-touch-publish-design.md)).
mod support;
use axton_server::host::HostRequest;
use serde_json::{Value, json};
use support::*;

fn todo_row(id: &str, title: &str) -> Value {
    json!({"id":id,"title":title})
}
fn project_row(id: &str) -> Value {
    json!({"id":id,"name":"P"})
}
fn effects(changes: Vec<Value>, memberships: Vec<Value>) -> Value {
    json!({"outputs":{},"changes":changes,"memberships":memberships})
}
fn succeeded(receipt: &Value) {
    assert_eq!(receipt["rejections"], json!([]), "{receipt}");
}
fn publishes(backend: &Backend) -> Vec<(String, String, u64)> {
    backend
        .log()
        .into_iter()
        .filter_map(|request| match request {
            HostRequest::Publish {
                channel,
                identity,
                stamp,
                ..
            } => Some((channel, identity["id"].as_str().unwrap().to_string(), stamp)),
            _ => None,
        })
        .collect()
}

/// Spec §4's example: a Todo at stamp 7 in Channels A and B changes once. It
/// advances to 8 exactly once and each Channel gets one new position at 8.
#[test]
fn a_changed_member_advances_once_and_gets_one_new_position_per_channel() {
    let backend = Backend::new();
    backend.seed("Todo", "t", todo_row("t", "old"), Some(7));
    backend.enroll("A", "Todo", "t", 120);
    backend.enroll("B", "Todo", "t", 45);
    let receipt = push(
        &backend,
        1,
        json!({"Todo":1}),
        vec![edit(1, 1, "Edit", "t", "new")],
    );
    succeeded(&receipt);
    assert_eq!(authority(&receipt), [("Todo".into(), "t".into(), 8)]);
    assert_eq!(backend.count("advanceStamp"), 1);
    assert_eq!(backend.stamp("Todo", "t"), Some(8));
    assert_eq!(
        publishes(&backend),
        [("A".into(), "t".into(), 8), ("B".into(), "t".into(), 8)]
    );
    assert_eq!((backend.head("A"), backend.head("B")), (121, 46));
    assert_eq!(backend.invalidation("A", "Todo", "t"), Some((121, 8)));
    assert_eq!(backend.invalidation("B", "Todo", "t"), Some((46, 8)));
    // A record with no membership changes, stamps and reads back, published nowhere.
    backend.seed("Todo", "lone", todo_row("lone", "old"), None);
    backend.clear_log();
    let receipt = push(
        &backend,
        2,
        json!({"Todo":1}),
        vec![edit(1, 2, "Edit", "lone", "new")],
    );
    succeeded(&receipt);
    assert_eq!(backend.count("publish"), 0);
    assert_eq!(backend.stamp("Todo", "lone"), Some(1));
}

/// A record that is both changed and newly added gets one position, at its
/// final stamp, whether the change is an input target or an extra touch and
/// however the membership intents are ordered around it. (Touches and
/// membership intents travel in separate arrays, so an SDK's interleaving of
/// `touch` and `add` calls cannot reach settlement.)
#[test]
fn a_newly_added_changed_record_gets_one_position_at_its_final_stamp() {
    for (label, name, memberships) in [
        ("input target, add", "Edit", vec![add("A", "Todo", "t")]),
        ("extra touch, add", "Settle", vec![add("A", "Todo", "t")]),
        (
            "extra touch, add remove add",
            "Settle",
            vec![
                add("A", "Todo", "t"),
                remove("A", "Todo", "t"),
                add("A", "Todo", "t"),
            ],
        ),
    ] {
        let backend = Backend::new();
        backend.seed("Todo", "t", todo_row("t", "old"), Some(3));
        let changes = if name == "Settle" {
            vec![reference("Todo", "t")]
        } else {
            vec![]
        };
        backend.script(name, effects(changes, memberships));
        let invocation = if name == "Settle" {
            call(1, 1, "Settle", json!({}))
        } else {
            edit(1, 1, "Edit", "t", "new")
        };
        let receipt = push(&backend, 1, json!({"Todo":1}), vec![invocation]);
        succeeded(&receipt);
        assert_eq!(backend.count("advanceStamp"), 1, "{label}");
        assert_eq!(backend.count("ensureStamp"), 0, "{label}");
        assert_eq!(backend.count("setMembership"), 1, "{label}");
        assert_eq!(backend.stamp("Todo", "t"), Some(4), "{label}");
        assert_eq!(
            publishes(&backend),
            [("A".into(), "t".into(), 4)],
            "{label}"
        );
        assert_eq!(backend.members("Todo", "t"), ["A"], "{label}");
        assert_eq!(
            backend.invalidation("A", "Todo", "t"),
            Some((1, 4)),
            "{label}"
        );
    }
}

/// Intents that cancel are compared with the membership at settlement start:
/// a member removed then re-added, and a non-member added then removed, are
/// unchanged. Nothing is written or published and no stamp moves.
#[test]
fn membership_operations_that_cancel_change_nothing_and_publish_nothing() {
    let backend = Backend::new();
    backend.seed("Project", "p", project_row("p"), Some(5));
    backend.enroll("A", "Project", "p", 9);
    backend.script(
        "Settle",
        effects(
            vec![],
            vec![
                remove("A", "Project", "p"),
                add("B", "Project", "p"),
                add("A", "Project", "p"),
                remove("B", "Project", "p"),
                // A record with no metadata: added then removed.
                add("B", "Project", "q"),
                remove("B", "Project", "q"),
            ],
        ),
    );
    let before = backend.tables();
    let receipt = push(
        &backend,
        1,
        json!({}),
        vec![call(1, 1, "Settle", json!({}))],
    );
    succeeded(&receipt);
    assert_eq!(backend.members("Project", "p"), ["A"]);
    assert_eq!(backend.stamp("Project", "p"), Some(5));
    assert_eq!(backend.count("setMembership"), 0);
    assert_eq!(backend.count("publish"), 0);
    assert_eq!(backend.count("advanceStamp"), 0);
    // p keeps a final add (A), so its guard is `ensureStamp`; q's final
    // intent is a removal, so it is only locked, and has no row to lock.
    assert_eq!(
        backend.settlement_log(),
        [
            HostRequest::EnsureStamp {
                model: "Project".into(),
                identity_key: r#"{"id":"p"}"#.into()
            },
            HostRequest::LockRecord {
                model: "Project".into(),
                identity_key: r#"{"id":"q"}"#.into()
            },
            HostRequest::Memberships {
                model: "Project".into(),
                identity_key: r#"{"id":"p"}"#.into()
            },
        ]
    );
    let after = backend.tables();
    assert_eq!(after.stamps, before.stamps, "no stamp row created or moved");
    assert_eq!(after.heads, before.heads);
    assert_eq!(after.memberships, before.memberships);
    assert_eq!(after.invalidations, before.invalidations);
}

/// A removal takes effect only when the record is a member, and a changed
/// record removed from a Channel in the same settlement is not published
/// there: the final relationship wins, even for a deletion.
#[test]
fn a_removal_is_net_and_a_record_removed_while_changed_is_not_published_there() {
    let backend = Backend::new();
    backend.seed("Todo", "t", todo_row("t", "old"), Some(2));
    backend.enroll("A", "Todo", "t", 0);
    backend.enroll("B", "Todo", "t", 0);
    backend.write("Settle", "Todo", "t", None);
    backend.script(
        "Settle",
        effects(
            vec![reference("Todo", "t")],
            vec![remove("A", "Todo", "t"), remove("C", "Todo", "t")],
        ),
    );
    let receipt = push(
        &backend,
        1,
        json!({}),
        vec![call(1, 1, "Settle", json!({}))],
    );
    succeeded(&receipt);
    assert_eq!(backend.row("Todo", "t"), None);
    assert_eq!(backend.stamp("Todo", "t"), Some(3));
    assert_eq!(backend.members("Todo", "t"), ["B"]);
    assert_eq!(
        publishes(&backend),
        [("B".into(), "t".into(), 3)],
        "the deletion reaches B only"
    );
    assert_eq!(
        backend
            .log()
            .into_iter()
            .filter(|request| matches!(request, HostRequest::SetMembership { .. }))
            .collect::<Vec<_>>(),
        [HostRequest::SetMembership {
            channel: "A".into(),
            model: "Todo".into(),
            identity_key: r#"{"id":"t"}"#.into(),
            present: false
        }],
        "removing a non-member (C) writes nothing"
    );
    // A removal-only settlement locks the existing record and moves no stamp.
    backend.clear_log();
    backend.script("Settle", effects(vec![], vec![remove("B", "Todo", "t")]));
    let receipt = push(
        &backend,
        2,
        json!({}),
        vec![call(1, 2, "Settle", json!({}))],
    );
    succeeded(&receipt);
    assert_eq!(backend.count("lockRecord"), 1);
    assert_eq!(backend.count("publish"), 0);
    assert_eq!(backend.stamp("Todo", "t"), Some(3));
    assert!(backend.members("Todo", "t").is_empty());
}

/// An output-only read and an unchanged enrollment leave an existing stamp
/// where it is; enrolling a record without metadata initializes it at 1.
/// Re-adding an existing member publishes nothing.
#[test]
fn output_only_reads_and_unchanged_enrollment_keep_the_existing_stamp() {
    let backend = Backend::new();
    backend.seed("Project", "p", project_row("p"), Some(5));
    backend.script(
        "ReadProject",
        json!({"outputs":{"project":{"id":"p"}},"changes":[],"memberships":[]}),
    );
    let receipt = push(
        &backend,
        1,
        json!({"Project":1}),
        vec![call(1, 1, "ReadProject", json!({}))],
    );
    succeeded(&receipt);
    assert_eq!(
        receipt["completions"][0]["outcome"]["result"],
        json!({"project":{"id":"p","name":"P"}})
    );
    assert_eq!(authority(&receipt), [("Project".into(), "p".into(), 5)]);
    assert_eq!(backend.count("advanceStamp"), 0);
    assert_eq!(backend.stamp("Project", "p"), Some(5));
    // Enrollment of an unchanged record distributes its existing stamp.
    backend.seed("Project", "n", project_row("n"), None);
    backend.clear_log();
    backend.script(
        "Settle",
        effects(
            vec![],
            vec![add("A", "Project", "p"), add("A", "Project", "n")],
        ),
    );
    let receipt = push(
        &backend,
        2,
        json!({}),
        vec![call(1, 2, "Settle", json!({}))],
    );
    succeeded(&receipt);
    assert_eq!(backend.count("advanceStamp"), 0);
    assert_eq!(backend.stamp("Project", "p"), Some(5));
    assert_eq!(backend.stamp("Project", "n"), Some(1), "initialized at 1");
    assert_eq!(
        publishes(&backend),
        [("A".into(), "n".into(), 1), ("A".into(), "p".into(), 5)]
    );
    assert_eq!(
        authority(&receipt),
        [],
        "enrollment is not caller authority"
    );
    // Adding an existing member again is not observable.
    backend.clear_log();
    backend.script("Settle", effects(vec![], vec![add("A", "Project", "p")]));
    let receipt = push(
        &backend,
        3,
        json!({}),
        vec![call(1, 3, "Settle", json!({}))],
    );
    succeeded(&receipt);
    assert_eq!(backend.count("publish"), 0);
    assert_eq!(backend.count("setMembership"), 0);
    assert_eq!(backend.stamp("Project", "p"), Some(5));
}

/// A retried call ID replays its saved outcome: no handler, no stamp, no
/// cursor and no membership change, and the same result bytes.
#[test]
fn a_saved_call_replays_without_restamping_republishing_or_re_enrolling() {
    let backend = Backend::new();
    backend.seed("Todo", "t", todo_row("t", "old"), Some(1));
    backend.seed("Project", "p", project_row("p"), Some(4));
    backend.enroll("B", "Todo", "t", 0);
    backend.script(
        "EditAndRead",
        json!({"outputs":{"todo":{"id":"t"}},"changes":[reference("Project","p")],
               "memberships":[add("A","Todo","t"),add("A","Project","p")]}),
    );
    let first = push(
        &backend,
        1,
        json!({"Todo":1}),
        vec![edit(1, 1, "EditAndRead", "t", "new")],
    );
    succeeded(&first);
    let settled = backend.tables();
    assert_eq!(
        (
            settled.stamps[&record("Todo", "t")],
            settled.stamps[&record("Project", "p")]
        ),
        (2, 5)
    );
    backend.clear_log();
    let replay = push(
        &backend,
        2,
        json!({"Todo":1}),
        vec![edit(1, 1, "EditAndRead", "t", "new")],
    );
    assert_eq!(replay["completions"], first["completions"]);
    assert_eq!(replay["records"], first["records"]);
    assert_eq!(backend.ops(), ["claim", "claimCall", "saveReceipt"]);
    assert_eq!(
        backend.tables(),
        settled,
        "stamps, heads and relationships unchanged"
    );
}

/// Guards are taken in canonical record order before any membership is read;
/// membership writes and publications then follow Channel, then record order.
#[test]
fn settlement_guards_records_in_key_order_then_writes_in_channel_order() {
    let backend = Backend::new();
    for id in ["p1", "p2", "p3"] {
        backend.seed("Project", id, project_row(id), Some(2));
    }
    backend.enroll("A", "Project", "p3", 0);
    backend.script(
        "Settle",
        effects(
            vec![reference("Project", "p2")],
            vec![
                add("B", "Project", "p1"),
                remove("A", "Project", "p3"),
                add("A", "Project", "p2"),
                add("A", "Project", "p1"),
            ],
        ),
    );
    let receipt = push(
        &backend,
        1,
        json!({}),
        vec![call(1, 1, "Settle", json!({}))],
    );
    succeeded(&receipt);
    let key = |id: &str| format!(r#"{{"id":"{id}"}}"#);
    let project = || "Project".to_string();
    let membership = |channel: &str, id: &str, present: bool| HostRequest::SetMembership {
        channel: channel.into(),
        model: project(),
        identity_key: key(id),
        present,
    };
    let publish = |channel: &str, id: &str, stamp: u64| HostRequest::Publish {
        channel: channel.into(),
        model: project(),
        identity: json!({"id":id}),
        identity_key: key(id),
        stamp,
    };
    assert_eq!(
        backend.settlement_log(),
        [
            HostRequest::EnsureStamp {
                model: project(),
                identity_key: key("p1")
            },
            HostRequest::AdvanceStamp {
                model: project(),
                identity_key: key("p2")
            },
            HostRequest::LockRecord {
                model: project(),
                identity_key: key("p3")
            },
            HostRequest::Memberships {
                model: project(),
                identity_key: key("p1")
            },
            HostRequest::Memberships {
                model: project(),
                identity_key: key("p2")
            },
            HostRequest::Memberships {
                model: project(),
                identity_key: key("p3")
            },
            membership("A", "p1", true),
            membership("A", "p2", true),
            membership("A", "p3", false),
            membership("B", "p1", true),
            publish("A", "p1", 2),
            publish("A", "p2", 3),
            publish("B", "p1", 2),
        ]
    );
}

/// The legacy and external paths settle the same effects through the same
/// algorithm as an Action: the same guards, membership writes and positions.
#[test]
fn legacy_and_external_paths_share_the_settlement() {
    let prepare = || {
        let backend = Backend::new();
        backend.seed("Todo", "t", todo_row("t", "old"), Some(1));
        backend.seed("Project", "p", project_row("p"), Some(3));
        backend.enroll("B", "Todo", "t", 0);
        backend
    };
    let memberships = vec![add("A", "Todo", "t"), add("A", "Project", "p")];
    // An Action: the input target plus an extra touch of p.
    let action = prepare();
    action.script(
        "Edit",
        effects(vec![reference("Project", "p")], memberships.clone()),
    );
    succeeded(&push(
        &action,
        1,
        json!({"Todo":1}),
        vec![edit(1, 1, "Edit", "t", "new")],
    ));
    // A legacy slot handler answering the same effects.
    let legacy = prepare();
    legacy.script(
        "edit",
        json!({"changes":[reference("Project","p")],"memberships":memberships.clone()}),
    );
    let receipt = legacy_push(&legacy, 1, json!({"Todo":1}), "t", "new");
    succeeded(&receipt);
    // An external transaction reporting both records changed.
    let external = prepare();
    let answer = run(axton_server::settle_external(
        &config(),
        &json!({"changes":[reference("Todo","t"),reference("Project","p")],"memberships":memberships}),
        &external,
    ))
    .unwrap();
    assert_eq!(
        answer,
        json!([
            {"model":"Project","identity":{"id":"p"},"stamp":4},
            {"model":"Todo","identity":{"id":"t"},"stamp":2}
        ])
    );
    assert_eq!(action.settlement_log(), legacy.settlement_log());
    assert_eq!(action.settlement_log(), external.settlement_log());
    assert_eq!(
        external.count("load"),
        0,
        "an external settlement reads nothing back"
    );
    for backend in [&action, &legacy, &external] {
        assert_eq!(
            publishes(backend),
            [
                ("A".into(), "p".into(), 4),
                ("A".into(), "t".into(), 2),
                ("B".into(), "t".into(), 2)
            ]
        );
    }
    // The legacy receipt carries its input target only, not the extra touch.
    assert_eq!(
        receipt["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|record| (record["model"].clone(), record["stamp"].clone()))
            .collect::<Vec<_>>(),
        [(json!("Todo"), json!(2))]
    );
}

/// A membership naming a blank Channel or an unregistered Model rejects only
/// its own call; the next call in the batch still settles.
#[test]
fn an_invalid_membership_rejects_only_its_call() {
    for (label, intent, code) in [
        (
            "blank channel",
            json!({"channel":" ","model":"Todo","identity":{"id":"t"},"present":true}),
            "publish.invalid",
        ),
        (
            "unknown model",
            json!({"channel":"A","model":"Ghost","identity":{"id":"t"},"present":true}),
            "loader.unregistered",
        ),
    ] {
        let backend = Backend::new();
        backend.seed("Todo", "t", todo_row("t", "old"), Some(1));
        backend.script("Settle", effects(vec![], vec![intent]));
        let receipt = push(
            &backend,
            1,
            json!({"Todo":1}),
            vec![
                call(1, 1, "Settle", json!({})),
                edit(2, 2, "Edit", "t", "new"),
            ],
        );
        assert_eq!(
            receipt["rejections"],
            json!([{"ordinal":1,"code":code}]),
            "{label}"
        );
        assert_eq!(backend.count("setMembership"), 0, "{label}");
        assert_eq!(backend.stamp("Todo", "t"), Some(2), "{label}");
    }
}
