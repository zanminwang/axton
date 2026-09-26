//! Whole-Scope bootstrap on the simulation
//! ([#151](https://github.com/zanminwang/axton/issues/151)): a client that
//! subscribes from now reaches everything published before its origin through
//! the bounded historical interval, while other clients keep publishing.
//!
//! The simulated backend answers both pull modes through the one public
//! `process_pull` entry point, so the bounded page is the same contract the
//! HTTP adapter serves. The client side is driven the way the Downlink worker
//! drives it - schedule a page from the ledger, apply it with its progress in
//! one transaction, settle a fixed barrier after delivery commits - rather than
//! by a second copy of that worker.
use axton_client::BootstrapPhase;
use axton_sim::{Action, MutationSpec, Sim, schema::entry_key};

fn create(sim: &mut Sim, client: usize, id: &str, text: &str) {
    sim.apply(Action::Enqueue {
        client,
        mutation: MutationSpec::CreateEntry {
            id: id.into(),
            text: text.into(),
        },
    })
    .unwrap();
}
fn edit(sim: &mut Sim, client: usize, id: &str, text: &str) {
    sim.apply(Action::Enqueue {
        client,
        mutation: MutationSpec::Edit {
            id: id.into(),
            text: text.into(),
        },
    })
    .unwrap();
}
/// Ask for one historical page and deliver everything the network holds.
fn load(sim: &mut Sim, client: usize) {
    sim.apply(Action::LoadPull { client }).unwrap();
    sim.drain();
}
/// The committed historical progress of one registration.
fn progress(sim: &mut Sim, client: usize, channel: &str) -> u64 {
    let id = sim
        .client(client)
        .subscription_state(channel)
        .unwrap()
        .unwrap()
        .subscription_id;
    sim.client(client)
        .bootstrap_state(channel, id)
        .unwrap()
        .cursor
}

/// A client that subscribes from now loads nothing older by subscribing, and
/// Bootstrap covers exactly that history: the interval terminates although two
/// other clients keep publishing, and once writes stop every client holds the
/// same authority.
#[test]
fn a_from_now_client_bootstraps_its_history_while_others_keep_publishing() {
    let mut sim = Sim::new(7, 3);
    for client in [0, 1] {
        sim.apply(Action::Subscribe {
            client,
            channel: "a".into(),
        })
        .unwrap();
    }
    // The Scope's history: sixty records, so the interval needs more than one
    // bounded page.
    for i in 0..60 {
        create(&mut sim, i % 2, &format!("e{i}"), &format!("text {i}"));
    }
    sim.settle();
    let origin = sim.host.head("a");
    assert!(origin >= 60, "the Scope has a history: {origin}");

    // The newcomer registers from now: its origin is the head, and delivery
    // starts after it (D9).
    sim.apply(Action::SubscribeAtHead {
        client: 2,
        channel: "a".into(),
    })
    .unwrap();
    assert_eq!(sim.client(2).cursor("a").unwrap(), Some(origin));
    sim.settle();
    assert_eq!(
        sim.read_text(2, &entry_key("e0")),
        None,
        "subscribing delivered nothing published before the origin"
    );

    sim.apply(Action::Bootstrap {
        client: 2,
        channel: "a".into(),
    })
    .unwrap();
    assert_eq!(sim.bootstrap_phase(2, "a"), BootstrapPhase::Requested);

    // One page at a time, with the other clients publishing beside it. The
    // upper bound is the origin, so the interval finishes although the head
    // does not stop moving.
    let mut pages = 0;
    for round in 0..16 {
        if !matches!(
            sim.bootstrap_phase(2, "a"),
            BootstrapPhase::Requested | BootstrapPhase::Loading
        ) {
            break;
        }
        load(&mut sim, 2);
        pages += 1;
        create(
            &mut sim,
            round % 2,
            &format!("later-{round}"),
            "published later",
        );
        edit(&mut sim, round % 2, "e0", &format!("edited {round}"));
        sim.apply(Action::Freeze { client: round % 2 }).unwrap();
        sim.drain();
    }
    assert!(
        (2..=4).contains(&pages),
        "a bounded interval takes a bounded number of pages: {pages}"
    );
    assert_eq!(
        progress(&mut sim, 2, "a"),
        origin,
        "the historical interval finished at the origin it started with"
    );
    assert!(
        matches!(
            sim.bootstrap_phase(2, "a"),
            BootstrapPhase::CatchingUp | BootstrapPhase::Complete
        ),
        "the interval terminated: {:?}",
        sim.bootstrap_phase(2, "a")
    );

    // Writes stop. Delivery reaches the fixed barrier, the run completes, and
    // every client holds the same authority.
    sim.settle();
    assert_eq!(
        sim.bootstrap_phase(2, "a"),
        BootstrapPhase::Complete,
        "delivery reached the barrier the final page fixed"
    );
    assert_eq!(sim.client(2).cursor("a").unwrap(), Some(sim.host.head("a")));
    for i in 0..60 {
        let key = entry_key(&format!("e{i}"));
        assert_eq!(
            sim.read_text(2, &key),
            sim.read_text(0, &key),
            "e{i} converged on the bootstrapped client"
        );
        assert!(
            sim.read_text(2, &key).is_some(),
            "e{i} was loaded by the historical interval"
        );
    }
    sim.check().unwrap();
    assert_eq!(sim.conflicts, 0);
    assert!(
        sim.reports.is_empty(),
        "nothing failed to apply: {:?}",
        sim.reports
    );
}

/// The historical interval never chases the head: every page of a run carries
/// the origin the subscription committed, and a record republished above it
/// leaves the interval for the subscription's own delivery (spec section 3).
#[test]
fn a_record_republished_above_the_origin_leaves_the_historical_interval() {
    let mut sim = Sim::new(3, 2);
    sim.apply(Action::Subscribe {
        client: 0,
        channel: "a".into(),
    })
    .unwrap();
    create(&mut sim, 0, "moving", "the record that moves");
    for i in 0..4 {
        create(&mut sim, 0, &format!("e{i}"), &format!("text {i}"));
    }
    sim.settle();
    let origin = sim.host.head("a");

    sim.apply(Action::SubscribeAtHead {
        client: 1,
        channel: "a".into(),
    })
    .unwrap();
    sim.apply(Action::Bootstrap {
        client: 1,
        channel: "a".into(),
    })
    .unwrap();
    // Before the scan runs, the record is published again: its one retained
    // position moves above the origin.
    sim.host.ensure_publish(&entry_key("moving"), "a");
    assert!(sim.host.head("a") > origin);

    // The interval is still the one the subscription committed, and it covers
    // only what stayed inside it.
    load(&mut sim, 1);
    assert_eq!(progress(&mut sim, 1, "a"), origin);
    assert_eq!(
        sim.read_text(1, &entry_key("e0")),
        Some("text 0".into()),
        "the historical interval delivered what stayed in it"
    );
    assert_eq!(
        sim.read_text(1, &entry_key("moving")),
        None,
        "the record whose position moved above the origin is not the scan's"
    );

    // What left the interval is the subscription's to deliver, and it does.
    sim.settle();
    assert_eq!(
        sim.read_text(1, &entry_key("moving")),
        Some("the record that moves".into())
    );
    assert_eq!(sim.bootstrap_phase(1, "a"), BootstrapPhase::Complete);
    sim.check().unwrap();
}
