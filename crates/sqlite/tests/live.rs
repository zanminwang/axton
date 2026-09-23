//! Live session transitions: the Rust controller drives a scripted host.
//! Real sockets and HTTP are the SDK suites' job; here every event is a
//! value and every action is asserted.
mod common;
use axton_client::*;
use axton_sqlite::SqliteStore;
use common::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const PUSH_WAKE: LiveAction = LiveAction::Wake { lane: "push" };

fn text(page: &PullPage) -> String {
    String::from_utf8(page.encode().unwrap()).unwrap()
}
/// The acknowledgement: every channel at its current head.
fn ack(heads: &[(&str, u64)]) -> String {
    let ack =
        SubscriptionAck::new(heads.iter().map(|(c, h)| (c.to_string(), *h)).collect()).unwrap();
    String::from_utf8(ack.encode().unwrap()).unwrap()
}
fn empty(channel: &str, at: u64) -> PullPage {
    multi(&[(channel, at, at, at)], vec![])
}
/// A full page of `channel` from `from`: fifty records, the channel continues.
fn full(channel: &str, from: u64) -> PullPage {
    let to = from + limits::PULL_CHANGES as u64;
    multi(
        &[(channel, from, to, to + 1)],
        (1..=limits::PULL_CHANGES as u64)
            .map(|i| AuthorityRecord {
                model: "Entry".into(),
                identity: json!({"id":format!("{i}")}),
                stamp: from + i,
                state: json!({"text":"bulk","note":null}),
                error: None,
            })
            .collect(),
    )
}
fn request(action: &LiveAction) -> (u64, PullRequest) {
    match action {
        LiveAction::Request { epoch, body } => {
            (*epoch, PullRequest::decode(body.as_bytes()).unwrap())
        }
        other => panic!("expected a request, got {other:?}"),
    }
}
fn cursors(request: &PullRequest) -> Vec<(&str, u64)> {
    request
        .cursors
        .iter()
        .map(|(c, n)| (c.as_str(), *n))
        .collect()
}
fn open(action: &LiveAction) -> (u64, SubscribeRequest) {
    match action {
        LiveAction::Open { epoch, subscribe } => (
            *epoch,
            SubscribeRequest::decode(subscribe.as_bytes()).unwrap(),
        ),
        other => panic!("expected an open, got {other:?}"),
    }
}
fn wait(action: &LiveAction) -> u64 {
    match action {
        LiveAction::Wait { millis } => *millis,
        other => panic!("expected a wait, got {other:?}"),
    }
}
fn reports(action: &LiveAction) -> &[Report] {
    match action {
        LiveAction::Report { reports } => reports,
        other => panic!("expected reports, got {other:?}"),
    }
}

struct Lane {
    client: Client<SqliteStore>,
    live: LiveSession,
    now: u64,
}
impl Lane {
    fn new(dir: &std::path::Path) -> Self {
        let mut client = common::open(&dir.join("db"));
        seed(&mut client, "local");
        Self {
            client,
            live: LiveSession::default(),
            now: 1_000,
        }
    }
    fn send(&mut self, event: LiveEvent) -> Vec<LiveAction> {
        self.live
            .handle(&mut self.client, event, self.now, 500)
            .unwrap()
    }
    fn message(&mut self, epoch: u64, body: String) -> Vec<LiveAction> {
        self.send(LiveEvent::Message { epoch, body })
    }
    fn frame(&mut self, epoch: u64, page: &PullPage) -> Vec<LiveAction> {
        self.message(epoch, text(page))
    }
    fn catch_up(&mut self, epoch: u64, page: &PullPage) -> Vec<LiveAction> {
        self.send(LiveEvent::CatchUp {
            epoch,
            body: text(page),
        })
    }
    fn cursor(&mut self, channel: &str) -> u64 {
        self.client.cursor(channel).unwrap()
    }
    fn set(&mut self, channel: &str, subscribed: bool) {
        self.client
            .transaction(|tx| tx.set_channel(channel.into(), subscribed))
            .unwrap();
    }
    fn text(&mut self) -> Value {
        self.client.read(&key()).unwrap().unwrap()["text"].clone()
    }
}

#[test]
fn a_session_subscribes_pulls_only_when_behind_and_then_streams() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.send(LiveEvent::Start);
    assert_eq!(
        lane.send(LiveEvent::Next),
        vec![],
        "no channels: the lane stays idle until a subscribe wakes it"
    );
    lane.set("a", true);
    let actions = lane.send(LiveEvent::Wake);
    let (epoch, subscribe) = open(&actions[0]);
    assert_eq!(subscribe.channels, ["a"]);
    assert_eq!(actions.len(), 1);
    // A streamed page before the acknowledgement is a protocol violation.
    let early = lane.frame(epoch, &page("a", 5, 6, Some("early")));
    assert_eq!(
        early[0],
        LiveAction::Close {
            epoch,
            reason: Some("live page before acknowledgement".into())
        }
    );
    let backoff = wait(&early[1]);
    assert!((200..=300).contains(&backoff), "{backoff}");
    lane.now += backoff;
    let actions = lane.send(LiveEvent::Next);
    let (epoch, _) = open(&actions[0]);
    assert_eq!(epoch, 2, "each session has its own epoch");
    // The head is beyond the durable cursor: one pull from the cursor.
    let actions = lane.message(epoch, ack(&[("a", 4)]));
    let (e, pull) = request(&actions[0]);
    assert_eq!((e, cursors(&pull)), (epoch, vec![("a", 0)]));
    assert_eq!(actions.len(), 1);
    // Frames streamed while the pull is in flight wait in the queue.
    assert_eq!(
        lane.frame(epoch, &page("a", 3, 4, Some("streamed"))),
        vec![]
    );
    assert_eq!(lane.frame(epoch, &page("a", 5, 6, Some("beyond"))), vec![]);
    // The pull lands; the queue is re-read: the first frame is covered, the
    // second has a gap and stays while one more pull runs from the cursor.
    let actions = lane.catch_up(epoch, &page("a", 0, 4, Some("caught up")));
    assert_eq!(actions[0], PUSH_WAKE, "an applied page wakes the push lane");
    let (_, again) = request(&actions[1]);
    assert_eq!(
        cursors(&again),
        vec![("a", 4)],
        "the held gap recovers from the durable cursor"
    );
    assert_eq!(actions.len(), 2, "the covered frame did nothing");
    assert_eq!(lane.cursor("a"), 4);
    assert_eq!(lane.text(), "caught up");
    // The pull connects the held frame, which is applied right after.
    let actions = lane.catch_up(epoch, &page("a", 4, 5, Some("recovered gap")));
    assert_eq!(actions, vec![PUSH_WAKE, PUSH_WAKE]);
    assert_eq!(lane.cursor("a"), 6);
    assert_eq!(lane.text(), "beyond");
    // Streaming: applied frames wake the push lane, covered frames do nothing.
    assert_eq!(
        lane.frame(epoch, &page("a", 6, 7, Some("live"))),
        vec![PUSH_WAKE]
    );
    assert_eq!(lane.frame(epoch, &page("a", 6, 7, Some("dup"))), vec![]);
    assert_eq!(lane.text(), "live");
    // A gap holds the frame and pulls from the durable cursor; when the pull
    // covers the frame, the frame is discarded.
    let actions = lane.frame(epoch, &page("a", 9, 10, Some("gap")));
    let (_, recover) = request(&actions[0]);
    assert_eq!(cursors(&recover), vec![("a", 7)]);
    assert_eq!(actions.len(), 1);
    assert_eq!(
        lane.catch_up(epoch, &page("a", 7, 10, Some("recovered"))),
        vec![PUSH_WAKE]
    );
    assert_eq!(lane.cursor("a"), 10);
    assert_eq!(lane.text(), "recovered");
}

#[test]
fn heads_equal_to_the_cursors_mean_no_catch_up_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    lane.set("b", true);
    lane.client
        .apply_page(multi(&[("a", 0, 3, 3), ("b", 0, 2, 2)], vec![]))
        .unwrap();
    let (epoch, subscribe) = open(&lane.send(LiveEvent::Start)[0]);
    assert_eq!(subscribe.channels, ["a", "b"]);
    assert_eq!(
        lane.message(epoch, ack(&[("a", 3), ("b", 2)])),
        vec![],
        "every channel is at its head: the stream is the truth"
    );
    assert_eq!(
        lane.frame(
            epoch,
            &multi(&[("a", 3, 4, 4)], vec![authority(Some("A"), 2)])
        ),
        vec![PUSH_WAKE]
    );
    assert_eq!((lane.cursor("a"), lane.cursor("b")), (4, 2));
}

#[test]
fn one_pull_covers_every_channel_and_continues_while_any_channel_is_full() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("b", true);
    lane.set("a", true);
    let actions = lane.send(LiveEvent::Start);
    let (epoch, subscribe) = open(&actions[0]);
    assert_eq!(
        subscribe.channels,
        ["a", "b"],
        "the frame carries the normalized set"
    );
    // `b` is at its head; `a` is behind: still one pull naming both.
    let actions = lane.message(epoch, ack(&[("b", 0), ("a", 51)]));
    let (_, pull) = request(&actions[0]);
    assert_eq!(cursors(&pull), vec![("a", 0), ("b", 0)]);
    assert_eq!(actions.len(), 1, "one request in flight at a time");
    let mut first = full("a", 0);
    first.cursors.insert(
        "b".into(),
        CursorRange {
            from: 0,
            to: 0,
            head: 0,
        },
    );
    let actions = lane.catch_up(epoch, &first);
    assert_eq!(actions[0], PUSH_WAKE);
    let (_, next) = request(&actions[1]);
    assert_eq!(
        cursors(&next),
        vec![("a", 50), ("b", 0)],
        "a full channel continues"
    );
    assert_eq!(
        lane.catch_up(epoch, &multi(&[("a", 50, 51, 51), ("b", 0, 0, 0)], vec![])),
        vec![PUSH_WAKE],
        "no channel continues: streaming"
    );
}

#[test]
fn overflow_discards_the_queue_and_recovers_every_channel_after_the_request_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    lane.set("b", true);
    let (epoch, _) = open(&lane.send(LiveEvent::Start)[0]);
    let actions = lane.message(epoch, ack(&[("a", 1), ("b", 0)]));
    request(&actions[0]);
    assert_eq!(lane.frame(epoch, &page("a", 1, 2, Some("queued"))), vec![]);
    assert_eq!(
        lane.send(LiveEvent::Overflow { epoch }),
        vec![],
        "the request keeps going"
    );
    // The response lands; the queued frame is gone and another pull follows,
    // because frames may have been lost beyond this response.
    let actions = lane.catch_up(epoch, &multi(&[("a", 0, 1, 1), ("b", 0, 0, 0)], vec![]));
    assert_eq!(actions[0], PUSH_WAKE, "the cursor moved");
    let (_, again) = request(&actions[1]);
    assert_eq!(cursors(&again), vec![("a", 1), ("b", 0)]);
    assert_eq!(actions.len(), 2);
    assert_eq!(
        lane.catch_up(
            epoch,
            &multi(
                &[("a", 1, 2, 2), ("b", 0, 0, 0)],
                vec![authority(Some("recovered"), 1)]
            )
        ),
        vec![PUSH_WAKE]
    );
    assert_eq!(lane.text(), "recovered");
    // While streaming, an overflow pulls at once; a second one while pulling
    // asks for one more pull, not two.
    let actions = lane.send(LiveEvent::Overflow { epoch });
    request(&actions[0]);
    assert_eq!(actions.len(), 1);
    assert_eq!(lane.send(LiveEvent::Overflow { epoch }), vec![]);
    assert_eq!(lane.send(LiveEvent::Overflow { epoch }), vec![]);
    let actions = lane.catch_up(epoch, &multi(&[("a", 2, 2, 2), ("b", 0, 0, 0)], vec![]));
    request(&actions[0]);
    assert_eq!(actions.len(), 1);
    assert_eq!(
        lane.catch_up(epoch, &multi(&[("a", 2, 2, 2), ("b", 0, 0, 0)], vec![])),
        vec![]
    );
}

#[test]
fn the_frame_queue_is_bounded_and_overflows_into_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let (epoch, _) = open(&lane.send(LiveEvent::Start)[0]);
    assert_eq!(lane.message(epoch, ack(&[("a", 0)])), vec![]);
    // A gap frame stays queued behind one pull; the frames after it queue up.
    let actions = lane.frame(epoch, &page("a", 5, 6, Some("gap")));
    request(&actions[0]);
    for i in 0..QUEUED_FRAMES as u64 - 1 {
        assert_eq!(
            lane.frame(epoch, &page("a", 6 + i, 7 + i, Some("more"))),
            vec![]
        );
    }
    // One more frame than the bound: the queue is discarded; the pull in
    // flight keeps going and another follows it.
    assert_eq!(
        lane.frame(epoch, &page("a", 100, 101, Some("overflow"))),
        vec![]
    );
    let actions = lane.catch_up(epoch, &page("a", 0, 5, Some("pulled")));
    assert_eq!(actions[0], PUSH_WAKE);
    let (_, again) = request(&actions[1]);
    assert_eq!(cursors(&again), vec![("a", 5)]);
    assert_eq!(
        actions.len(),
        2,
        "no held frame applied: the queue was discarded"
    );
    assert_eq!(lane.catch_up(epoch, &empty("a", 5)), vec![]);
}

#[test]
fn reports_reach_the_host_as_actions() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let (epoch, _) = open(&lane.send(LiveEvent::Start)[0]);
    lane.message(epoch, ack(&[("a", 0)]));
    lane.frame(epoch, &page("a", 0, 1, Some("A")));
    let failed = AuthorityRecord {
        model: "Entry".into(),
        identity: json!({"id":"e"}),
        stamp: 7,
        state: Value::Null,
        error: Some("loader.failed".into()),
    };
    let mut bad = authority_of("x", Some("bad"), 2);
    bad.state = json!({"text":1});
    let actions = lane.frame(epoch, &multi(&[("a", 1, 3, 3)], vec![failed, bad]));
    assert_eq!(actions[0], PUSH_WAKE);
    let reported = reports(&actions[1]);
    assert_eq!(reported.len(), 2);
    assert_eq!(reported[0].kind, ReportKind::ReadFailed);
    assert_eq!(reported[0].code.as_deref(), Some("loader.failed"));
    assert_eq!(reported[1].kind, ReportKind::Skipped);
    assert_eq!(reported[1].identity, json!({"id":"x"}));
    assert_eq!(lane.text(), "A");
    assert_eq!(lane.cursor("a"), 3);
    // Reports come back from a pull too.
    let actions = lane.frame(epoch, &page("a", 9, 10, Some("gap")));
    request(&actions[0]);
    let actions = lane.catch_up(
        epoch,
        &multi(&[("a", 3, 10, 10)], vec![authority(Some("other"), 1)]),
    );
    assert_eq!(actions[0], PUSH_WAKE);
    assert_eq!(reports(&actions[1])[0].kind, ReportKind::Conflict);
}

#[test]
fn a_subscription_change_ends_the_session_and_the_next_one_uses_the_new_set() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let (first, _) = open(&lane.send(LiveEvent::Start)[0]);
    let actions = lane.message(first, ack(&[("a", 3)]));
    let (_, pending) = request(&actions[0]);
    lane.set("b", true);
    // Whatever event comes next observes the committed change: the old session
    // closes without backoff and a new one opens with both channels.
    let actions = lane.send(LiveEvent::Wake);
    assert_eq!(
        actions[0],
        LiveAction::Close {
            epoch: first,
            reason: None
        }
    );
    let (second, subscribe) = open(&actions[1]);
    assert_eq!(subscribe.channels, ["a", "b"]);
    assert_eq!(actions.len(), 2);
    // The old session's late responses and frames are ignored.
    assert_eq!(
        lane.catch_up(first, &page("a", pending.cursors["a"], 3, Some("late"))),
        vec![]
    );
    assert_eq!(
        lane.frame(first, &page("a", 0, 1, Some("old socket"))),
        vec![]
    );
    assert_eq!(lane.send(LiveEvent::Closed { epoch: first }), vec![]);
    assert_eq!(lane.cursor("a"), 0);
    // Unsubscribing everything ends the session and leaves the lane idle.
    lane.message(second, ack(&[("a", 0), ("b", 0)]));
    lane.set("a", false);
    lane.set("b", false);
    assert_eq!(
        lane.send(LiveEvent::Next),
        vec![LiveAction::Close {
            epoch: second,
            reason: None
        }]
    );
    assert_eq!(lane.send(LiveEvent::Next), vec![]);
}

#[test]
fn a_dropped_socket_reconnects_with_backoff_and_resubscribes() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let (first, _) = open(&lane.send(LiveEvent::Start)[0]);
    lane.message(first, ack(&[("a", 3)]));
    lane.catch_up(first, &page("a", 0, 3, Some("before")));
    let actions = lane.send(LiveEvent::Closed { epoch: first });
    assert_eq!(
        actions[0],
        LiveAction::Close {
            epoch: first,
            reason: None
        },
        "the host closes whatever is left of the session"
    );
    let backoff = wait(&actions[1]);
    assert!((200..=300).contains(&backoff), "{backoff}");
    assert_eq!(
        lane.send(LiveEvent::Next),
        vec![LiveAction::Wait { millis: backoff }]
    );
    lane.now += backoff;
    let actions = lane.send(LiveEvent::Next);
    let (second, subscribe) = open(&actions[0]);
    assert_eq!(
        subscribe.channels,
        ["a"],
        "resubscribes without an application event"
    );
    let actions = lane.message(second, ack(&[("a", 5)]));
    assert_eq!(
        cursors(&request(&actions[0]).1),
        vec![("a", 3)],
        "catch-up resumes from the durable cursor"
    );
    // A second failure doubles the wait; a success resets it.
    let actions = lane.send(LiveEvent::Closed { epoch: second });
    assert!(wait(&actions[1]) > backoff);
}

#[test]
fn protocol_violations_close_with_a_reason_and_retry() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let mut actions = lane.send(LiveEvent::Start);
    for (frame, reason) in [
        (
            ack(&[("a", 0), ("b", 0)]),
            "invalid live subscription acknowledgement",
        ),
        (
            r#"{"type":"subscribed","cursors":{"a":-1}}"#.into(),
            "invalid live subscription acknowledgement",
        ),
        (
            r#"{"cursors":{"a":{"from":2,"to":1,"head":1}},"changes":[]}"#.into(),
            "invalid live page: page moves backwards",
        ),
    ] {
        let (epoch, _) = open(&actions[0]);
        let closed = lane.message(epoch, frame);
        assert_eq!(
            closed[0],
            LiveAction::Close {
                epoch,
                reason: Some(reason.into())
            }
        );
        lane.now += wait(&closed[1]);
        actions = lane.send(LiveEvent::Next);
    }
    let (epoch, _) = open(&actions[0]);
    let actions = lane.message(epoch, ack(&[("a", 1)]));
    request(&actions[0]);
    let actions = lane.message(epoch, ack(&[("a", 1)]));
    assert_eq!(
        actions[0],
        LiveAction::Close {
            epoch,
            reason: Some("invalid live subscription acknowledgement".into())
        },
        "a second acknowledgement is a violation"
    );
    lane.now += wait(&actions[1]);
    // A catch-up response that does not answer the request ends the session too.
    let (epoch, _) = open(&lane.send(LiveEvent::Next)[0]);
    lane.message(epoch, ack(&[("a", 1)]));
    let actions = lane.catch_up(epoch, &empty("other", 0));
    assert_eq!(
        actions[0],
        LiveAction::Close {
            epoch,
            reason: Some("response does not match pull request".into())
        }
    );
}

#[test]
fn pause_ends_the_session_without_backoff_resume_reopens_and_stop_is_final() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let (first, _) = open(&lane.send(LiveEvent::Start)[0]);
    lane.message(first, ack(&[("a", 0)]));
    assert_eq!(
        lane.send(LiveEvent::Pause),
        vec![LiveAction::Close {
            epoch: first,
            reason: None
        }]
    );
    assert_eq!(
        lane.send(LiveEvent::Wake),
        vec![],
        "paused: a wake schedules nothing"
    );
    assert_eq!(lane.send(LiveEvent::Closed { epoch: first }), vec![]);
    let actions = lane.send(LiveEvent::Resume);
    let (second, _) = open(&actions[0]);
    assert!(second > first);
    assert_eq!(
        lane.send(LiveEvent::Stop),
        vec![LiveAction::Close {
            epoch: second,
            reason: None
        }]
    );
    assert_eq!(lane.send(LiveEvent::Next), vec![]);
    assert_eq!(lane.send(LiveEvent::Wake), vec![]);
    assert_eq!(lane.send(LiveEvent::Resume), vec![]);
}

#[test]
fn a_page_from_a_previous_subscription_is_stale_not_a_gap_through_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let (first, _) = open(&lane.send(LiveEvent::Start)[0]);
    let actions = lane.message(first, ack(&[("a", 9)]));
    let (_, issued) = request(&actions[0]);
    // Unsubscribe and resubscribe while the request is in flight.
    lane.set("a", false);
    lane.set("a", true);
    let actions = lane.send(LiveEvent::Next);
    assert_eq!(
        actions[0],
        LiveAction::Close {
            epoch: first,
            reason: None
        }
    );
    let (second, _) = open(&actions[1]);
    let actions = lane.message(second, ack(&[("a", 9)]));
    let (_, fresh) = request(&actions[0]);
    assert_eq!(cursors(&fresh), vec![("a", 0)]);
    // The old answer cannot reach the new session (epoch), and even through
    // the engine's own gate it is stale rather than a gap.
    assert_eq!(
        lane.catch_up(first, &page("a", issued.cursors["a"], 9, Some("obsolete"))),
        vec![]
    );
    assert_eq!(
        lane.catch_up(second, &page("a", 0, 9, Some("fresh"))),
        vec![PUSH_WAKE]
    );
    assert_eq!(lane.text(), "fresh");
    let _ = BTreeMap::<String, u64>::new();
}
