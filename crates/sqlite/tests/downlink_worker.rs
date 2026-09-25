//! Downlink worker transitions: the Rust worker drives a scripted host. Real
//! sockets and HTTP are the SDK suites' job; here every event is a value and
//! every action is asserted. Enqueueing an event only takes it into typed
//! state; pages are applied by the bounded pump (`next`).
mod common;
use axton_client::*;
use axton_sqlite::SqliteStore;
use common::*;
use serde_json::{Value, json};

fn text(page: &PullPage) -> String {
    String::from_utf8(page.encode().unwrap()).unwrap()
}
/// The acknowledgement: every channel at its current head.
fn ack(heads: &[(&str, u64)]) -> String {
    let ack =
        SubscriptionAck::new(heads.iter().map(|(c, h)| (c.to_string(), *h)).collect()).unwrap();
    String::from_utf8(ack.encode().unwrap()).unwrap()
}
/// What an applied page answers: the push lane wakes and the Scopes whose
/// cursors the commit moved.
fn applied(scopes: &[&str]) -> Vec<DownlinkAction> {
    vec![
        DownlinkAction::Wake { lane: "push" },
        DownlinkAction::Changed {
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
        },
    ]
}
fn request(action: &DownlinkAction) -> (u64, PullRequest) {
    match action {
        DownlinkAction::Request { request, body } => {
            (*request, PullRequest::decode(body.as_bytes()).unwrap())
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
fn open(action: &DownlinkAction) -> (u64, SubscribeRequest) {
    match action {
        DownlinkAction::Open { epoch, subscribe } => (
            *epoch,
            SubscribeRequest::decode(subscribe.as_bytes()).unwrap(),
        ),
        other => panic!("expected an open, got {other:?}"),
    }
}
fn wait(action: &DownlinkAction) -> u64 {
    match action {
        DownlinkAction::Wait { millis } => *millis,
        other => panic!("expected a wait, got {other:?}"),
    }
}
fn waiting(action: &DownlinkAction) -> bool {
    matches!(action, DownlinkAction::Wait { .. })
}
fn reports(action: &DownlinkAction) -> &[Report] {
    match action {
        DownlinkAction::Report { reports } => reports,
        other => panic!("expected reports, got {other:?}"),
    }
}
/// The first two actions of an applied page: the push wake and the commit.
fn committed(actions: &[DownlinkAction], scopes: &[&str]) {
    assert_eq!(&actions[..2], &applied(scopes)[..]);
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

struct Lane {
    client: Client<SqliteStore>,
    worker: DownlinkWorker,
    now: u64,
}
impl Lane {
    fn new(dir: &std::path::Path) -> Self {
        let mut client = common::open(&dir.join("db"));
        seed(&mut client, "local");
        Self {
            client,
            worker: DownlinkWorker::default(),
            now: 1_000,
        }
    }
    /// One enqueue: typed state only. It answers with no actions, so nothing
    /// the host does depends on a callback's return.
    fn enqueue(&mut self, event: DownlinkEvent) {
        assert_eq!(
            self.worker
                .handle(&mut self.client, event, self.now, 500)
                .unwrap(),
            vec![],
            "an enqueue decides nothing: the pump answers"
        );
    }
    /// One bounded pump: at most one page application.
    fn pump(&mut self) -> Vec<DownlinkAction> {
        self.worker
            .handle(&mut self.client, DownlinkEvent::Next, self.now, 500)
            .unwrap()
    }
    /// The host loop: pump until the worker waits or has nothing left, in order.
    fn drain(&mut self) -> Vec<DownlinkAction> {
        let mut actions = vec![];
        for _ in 0..=QUEUED_FRAMES + 2 {
            let pumped = self.pump();
            let stop = pumped.is_empty() || pumped.iter().any(waiting);
            actions.extend(pumped);
            if stop {
                return actions;
            }
        }
        panic!("the pump never went idle: {actions:?}")
    }
    /// Enqueue one event and run the host loop over its consequences.
    fn send(&mut self, event: DownlinkEvent) -> Vec<DownlinkAction> {
        self.enqueue(event);
        self.drain()
    }
    fn message(&mut self, epoch: u64, body: String) -> Vec<DownlinkAction> {
        self.send(DownlinkEvent::Message { epoch, body })
    }
    fn frame(&mut self, epoch: u64, page: &PullPage) -> Vec<DownlinkAction> {
        self.message(epoch, text(page))
    }
    fn response(&mut self, request: u64, page: &PullPage) -> Vec<DownlinkAction> {
        self.send(DownlinkEvent::Response {
            request,
            body: text(page),
        })
    }
    /// Every channel a downlink test drives is subscribed and initialized.
    fn cursor(&mut self, channel: &str) -> Option<u64> {
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
    /// Subscribe `channel`, start the lane and acknowledge at `head`: the
    /// epoch of the socket and what the acknowledgement asked for.
    fn streaming(&mut self, channel: &str, head: u64) -> (u64, Vec<DownlinkAction>) {
        self.set(channel, true);
        let actions = self.send(DownlinkEvent::Start);
        let (epoch, subscribe) = open(&actions[0]);
        assert_eq!(subscribe.channels, [channel]);
        let acknowledged = self.message(epoch, ack(&[(channel, head)]));
        (epoch, acknowledged)
    }
}

#[test]
fn an_enqueued_page_commits_nothing_until_the_pump_applies_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let actions = lane.send(DownlinkEvent::Start);
    let (epoch, _) = open(&actions[0]);
    lane.enqueue(DownlinkEvent::Message {
        epoch,
        body: ack(&[("a", 0)]),
    });
    assert_eq!(lane.drain(), vec![], "at the head: no catch-up");
    // The frame only enters the queue: no transaction runs inside the callback.
    lane.enqueue(DownlinkEvent::Message {
        epoch,
        body: text(&page("a", 0, 1, Some("streamed"))),
    });
    assert_eq!(lane.cursor("a"), Some(0), "the cursor waits for the pump");
    assert_eq!(lane.text(), "local", "the page waits for the pump");
    // The pump applies it: content and cursor in the same commit.
    assert_eq!(lane.pump(), applied(&["a"]));
    assert_eq!(
        (lane.cursor("a"), lane.text()),
        (Some(1), json!("streamed"))
    );
    assert_eq!(lane.pump(), vec![], "nothing left to pump");
}

#[test]
fn one_pump_applies_one_page_so_foreground_work_interleaves() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    let (epoch, _) = lane.streaming("a", 0);
    for at in 0..3u64 {
        lane.enqueue(DownlinkEvent::Message {
            epoch,
            body: text(&page("a", at, at + 1, Some("bulk"))),
        });
    }
    for at in 0..3u64 {
        assert_eq!(lane.pump(), applied(&["a"]), "one commit per pump");
        assert_eq!(lane.cursor("a"), Some(at + 1));
    }
    assert_eq!(lane.pump(), vec![]);
}

#[test]
fn the_worker_outlives_the_socket_it_was_streaming_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    let (first, _) = lane.streaming("a", 0);
    assert_eq!(
        lane.frame(first, &page("a", 0, 1, Some("first"))),
        applied(&["a"])
    );
    // The socket is replaced; the worker keeps its durable position and its
    // schedule, and opens the next session itself.
    let closed = lane.send(DownlinkEvent::Closed { epoch: first });
    assert_eq!(
        closed[0],
        DownlinkAction::Close {
            epoch: first,
            reason: None
        }
    );
    lane.now += wait(&closed[1]);
    let actions = lane.drain();
    let (second, subscribe) = open(&actions[0]);
    assert!(second > first, "a new socket, a new epoch");
    assert_eq!(subscribe.channels, ["a"]);
    let actions = lane.message(second, ack(&[("a", 2)]));
    assert_eq!(
        cursors(&request(&actions[0]).1),
        vec![("a", 1)],
        "the worker resumes from the cursor it committed on the old socket"
    );
    assert_eq!(
        lane.response(request(&actions[0]).0, &page("a", 1, 2, Some("second"))),
        applied(&["a"])
    );
    assert_eq!((lane.cursor("a"), lane.text()), (Some(2), json!("second")));
}

#[test]
fn control_work_flows_while_a_gap_page_waits_for_its_repair() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    let (epoch, _) = lane.streaming("a", 0);
    // A gap holds the front of the page queue and one repair request runs.
    let actions = lane.frame(epoch, &page("a", 5, 6, Some("gap")));
    let (id, repair) = request(&actions[0]);
    assert_eq!(cursors(&repair), vec![("a", 0)]);
    // Frames behind the gap queue up; nothing commits while the repair is out.
    for at in 6..9u64 {
        assert_eq!(
            lane.frame(epoch, &page("a", at, at + 1, Some("behind"))),
            vec![]
        );
    }
    assert_eq!(lane.cursor("a"), Some(0));
    // A stale completion belongs to no request in flight and changes nothing.
    assert_eq!(
        lane.response(id + 7, &page("a", 0, 5, Some("stale"))),
        vec![]
    );
    assert_eq!(lane.cursor("a"), Some(0));
    // The repair the gap waited for is processed although the gap page is
    // still at the front of the queue, and the queue then connects.
    let actions = lane.response(id, &page("a", 0, 5, Some("repaired")));
    assert_eq!(&actions[..2], applied(&["a"]));
    assert_eq!(lane.cursor("a"), Some(9));
    assert_eq!(lane.text(), "behind");
}

#[test]
fn a_socket_that_closes_while_a_repair_is_out_ends_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    let (epoch, _) = lane.streaming("a", 0);
    let actions = lane.frame(epoch, &page("a", 5, 6, Some("gap")));
    let (id, _) = request(&actions[0]);
    // The socket ends: the control event is delivered although a page is held
    // and an HTTP request is still out.
    let closed = lane.send(DownlinkEvent::Closed { epoch });
    assert_eq!(
        closed[0],
        DownlinkAction::Close {
            epoch,
            reason: None
        }
    );
    assert!((200..=300).contains(&wait(&closed[1])));
    // Its answer belongs to a session that is gone: nothing but the retry is left.
    let late = lane.response(id, &page("a", 0, 5, Some("late")));
    assert_eq!(late.len(), 1, "{late:?}");
    assert!((200..=300).contains(&wait(&late[0])));
    assert_eq!(lane.cursor("a"), Some(0));
}

#[test]
fn an_http_failure_retries_the_session_and_a_stale_failure_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    // The acknowledged head is beyond the cursor: one catch-up is in flight.
    let (epoch, actions) = lane.streaming("a", 3);
    let (id, _) = request(&actions[0]);
    assert_eq!(
        lane.send(DownlinkEvent::Failed {
            request: id + 1,
            reason: Some("timeout".into())
        }),
        vec![],
        "a failure of another request is not this session's"
    );
    let failed = lane.send(DownlinkEvent::Failed {
        request: id,
        reason: Some("live failed: 503".into()),
    });
    assert_eq!(
        failed[0],
        DownlinkAction::Close {
            epoch,
            reason: None
        },
        "the host already reported the failure; the session ends"
    );
    assert!((200..=300).contains(&wait(&failed[1])));
    assert_eq!(lane.cursor("a"), Some(0));
}

#[test]
fn a_session_subscribes_pulls_only_when_behind_and_then_streams() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.enqueue(DownlinkEvent::Start);
    assert_eq!(
        lane.drain(),
        vec![],
        "no channels: the lane stays idle until a subscribe wakes it"
    );
    lane.set("a", true);
    let actions = lane.send(DownlinkEvent::Wake);
    let (epoch, subscribe) = open(&actions[0]);
    assert_eq!(subscribe.channels, ["a"]);
    assert_eq!(actions.len(), 1);
    // A streamed page before the acknowledgement is a protocol violation.
    let early = lane.frame(epoch, &page("a", 5, 6, Some("early")));
    assert_eq!(
        early[0],
        DownlinkAction::Close {
            epoch,
            reason: Some("live page before acknowledgement".into())
        }
    );
    let backoff = wait(&early[1]);
    assert!((200..=300).contains(&backoff), "{backoff}");
    lane.now += backoff;
    let actions = lane.drain();
    let (epoch, _) = open(&actions[0]);
    assert_eq!(epoch, 2, "each session has its own epoch");
    // The head is beyond the durable cursor: one pull from the cursor.
    let actions = lane.message(epoch, ack(&[("a", 4)]));
    let (id, pull) = request(&actions[0]);
    assert_eq!(cursors(&pull), vec![("a", 0)]);
    assert_eq!(actions.len(), 1);
    // Frames streamed while the pull is in flight wait in the queue.
    assert_eq!(
        lane.frame(epoch, &page("a", 3, 4, Some("streamed"))),
        vec![]
    );
    assert_eq!(lane.frame(epoch, &page("a", 5, 6, Some("beyond"))), vec![]);
    // The pull lands; the queue is re-read: the first frame is covered, the
    // second has a gap and stays while one more pull runs from the cursor.
    let actions = lane.response(id, &page("a", 0, 4, Some("caught up")));
    committed(&actions, &["a"]);
    let (id, again) = request(&actions[2]);
    assert_eq!(
        cursors(&again),
        vec![("a", 4)],
        "the held gap recovers from the durable cursor"
    );
    assert_eq!(actions.len(), 3, "the covered frame did nothing");
    assert_eq!(lane.cursor("a"), Some(4));
    assert_eq!(lane.text(), "caught up");
    // The pull connects the held frame, which is applied right after, in a
    // commit of its own.
    let actions = lane.response(id, &page("a", 4, 5, Some("recovered gap")));
    assert_eq!(actions, [applied(&["a"]), applied(&["a"])].concat());
    assert_eq!(lane.cursor("a"), Some(6));
    assert_eq!(lane.text(), "beyond");
    // Streaming: applied frames wake the push lane, covered frames do nothing.
    assert_eq!(
        lane.frame(epoch, &page("a", 6, 7, Some("live"))),
        applied(&["a"])
    );
    assert_eq!(lane.frame(epoch, &page("a", 6, 7, Some("dup"))), vec![]);
    assert_eq!(lane.text(), "live");
    // A gap holds the frame and pulls from the durable cursor; when the pull
    // covers the frame, the frame is discarded.
    let actions = lane.frame(epoch, &page("a", 9, 10, Some("gap")));
    let (id, recover) = request(&actions[0]);
    assert_eq!(cursors(&recover), vec![("a", 7)]);
    assert_eq!(actions.len(), 1);
    assert_eq!(
        lane.response(id, &page("a", 7, 10, Some("recovered"))),
        applied(&["a"])
    );
    assert_eq!(lane.cursor("a"), Some(10));
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
    let (epoch, subscribe) = open(&lane.send(DownlinkEvent::Start)[0]);
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
        applied(&["a"])
    );
    assert_eq!((lane.cursor("a"), lane.cursor("b")), (Some(4), Some(2)));
}

#[test]
fn one_pull_covers_every_channel_and_continues_while_any_channel_is_full() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("b", true);
    lane.set("a", true);
    let actions = lane.send(DownlinkEvent::Start);
    let (epoch, subscribe) = open(&actions[0]);
    assert_eq!(
        subscribe.channels,
        ["a", "b"],
        "the frame carries the normalized set"
    );
    // `b` is at its head; `a` is behind: still one pull naming both.
    let actions = lane.message(epoch, ack(&[("b", 0), ("a", 51)]));
    let (id, pull) = request(&actions[0]);
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
    let actions = lane.response(id, &first);
    committed(&actions, &["a"]);
    let (id, next) = request(&actions[2]);
    assert_eq!(
        cursors(&next),
        vec![("a", 50), ("b", 0)],
        "a full channel continues"
    );
    assert_eq!(
        lane.response(id, &multi(&[("a", 50, 51, 51), ("b", 0, 0, 0)], vec![])),
        applied(&["a"]),
        "no channel continues: streaming"
    );
}

#[test]
fn overflow_discards_the_queue_and_recovers_every_channel_after_the_request_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    lane.set("b", true);
    let (epoch, _) = open(&lane.send(DownlinkEvent::Start)[0]);
    let actions = lane.message(epoch, ack(&[("a", 1), ("b", 0)]));
    let (id, _) = request(&actions[0]);
    assert_eq!(lane.frame(epoch, &page("a", 1, 2, Some("queued"))), vec![]);
    assert_eq!(
        lane.send(DownlinkEvent::Overflow { epoch }),
        vec![],
        "the request keeps going"
    );
    // The answer lands; the queued frame is gone and another pull follows,
    // because frames may have been lost beyond this answer.
    let actions = lane.response(id, &multi(&[("a", 0, 1, 1), ("b", 0, 0, 0)], vec![]));
    committed(&actions, &["a"]);
    let (id, again) = request(&actions[2]);
    assert_eq!(cursors(&again), vec![("a", 1), ("b", 0)]);
    assert_eq!(actions.len(), 3);
    assert_eq!(
        lane.response(
            id,
            &multi(
                &[("a", 1, 2, 2), ("b", 0, 0, 0)],
                vec![authority(Some("recovered"), 1)]
            )
        ),
        applied(&["a"])
    );
    assert_eq!(lane.text(), "recovered");
    // While streaming, an overflow pulls at once; a second one while pulling
    // asks for one more pull, not two.
    let actions = lane.send(DownlinkEvent::Overflow { epoch });
    let (id, _) = request(&actions[0]);
    assert_eq!(actions.len(), 1);
    assert_eq!(lane.send(DownlinkEvent::Overflow { epoch }), vec![]);
    assert_eq!(lane.send(DownlinkEvent::Overflow { epoch }), vec![]);
    let actions = lane.response(id, &multi(&[("a", 2, 2, 2), ("b", 0, 0, 0)], vec![]));
    let (id, _) = request(&actions[0]);
    assert_eq!(actions.len(), 1);
    assert_eq!(
        lane.response(id, &multi(&[("a", 2, 2, 2), ("b", 0, 0, 0)], vec![])),
        vec![]
    );
}

#[test]
fn the_frame_queue_is_bounded_and_overflows_into_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let (epoch, _) = open(&lane.send(DownlinkEvent::Start)[0]);
    assert_eq!(lane.message(epoch, ack(&[("a", 0)])), vec![]);
    // A gap frame stays queued behind one pull; the frames after it queue up.
    let actions = lane.frame(epoch, &page("a", 5, 6, Some("gap")));
    let (id, _) = request(&actions[0]);
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
    let actions = lane.response(id, &page("a", 0, 5, Some("pulled")));
    committed(&actions, &["a"]);
    let (id, again) = request(&actions[2]);
    assert_eq!(cursors(&again), vec![("a", 5)]);
    assert_eq!(
        actions.len(),
        3,
        "no held frame applied: the queue was discarded"
    );
    assert_eq!(lane.response(id, &empty("a", 5)), vec![]);
}

#[test]
fn reports_reach_the_host_as_actions() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let (epoch, _) = open(&lane.send(DownlinkEvent::Start)[0]);
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
    committed(&actions, &["a"]);
    let reported = reports(&actions[2]);
    assert_eq!(reported.len(), 2);
    assert_eq!(reported[0].kind, ReportKind::ReadFailed);
    assert_eq!(reported[0].code.as_deref(), Some("loader.failed"));
    assert_eq!(reported[1].kind, ReportKind::Skipped);
    assert_eq!(reported[1].identity, json!({"id":"x"}));
    assert_eq!(lane.text(), "A");
    assert_eq!(lane.cursor("a"), Some(3));
    // Reports come back from a pull too.
    let actions = lane.frame(epoch, &page("a", 9, 10, Some("gap")));
    let (id, _) = request(&actions[0]);
    let actions = lane.response(
        id,
        &multi(&[("a", 3, 10, 10)], vec![authority(Some("other"), 1)]),
    );
    committed(&actions, &["a"]);
    assert_eq!(reports(&actions[2])[0].kind, ReportKind::Conflict);
}

#[test]
fn a_subscription_change_ends_the_session_and_the_next_one_uses_the_new_set() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let (first, _) = open(&lane.send(DownlinkEvent::Start)[0]);
    let actions = lane.message(first, ack(&[("a", 3)]));
    let (id, pending) = request(&actions[0]);
    lane.set("b", true);
    // Whatever event comes next observes the committed change: the old session
    // closes without backoff and a new one opens with both channels.
    let actions = lane.send(DownlinkEvent::Wake);
    assert_eq!(
        actions[0],
        DownlinkAction::Close {
            epoch: first,
            reason: None
        }
    );
    let (second, subscribe) = open(&actions[1]);
    assert_eq!(subscribe.channels, ["a", "b"]);
    assert_eq!(actions.len(), 2);
    // The old session's late answers and frames are ignored.
    assert_eq!(
        lane.response(id, &page("a", pending.cursors["a"], 3, Some("late"))),
        vec![]
    );
    assert_eq!(
        lane.frame(first, &page("a", 0, 1, Some("old socket"))),
        vec![]
    );
    assert_eq!(lane.send(DownlinkEvent::Closed { epoch: first }), vec![]);
    assert_eq!(lane.cursor("a"), Some(0));
    // Unsubscribing everything ends the session and leaves the lane idle.
    lane.message(second, ack(&[("a", 0), ("b", 0)]));
    lane.set("a", false);
    lane.set("b", false);
    assert_eq!(
        lane.drain(),
        vec![DownlinkAction::Close {
            epoch: second,
            reason: None
        }]
    );
    assert_eq!(lane.drain(), vec![]);
}

#[test]
fn a_dropped_socket_reconnects_with_backoff_and_resubscribes() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let (first, _) = open(&lane.send(DownlinkEvent::Start)[0]);
    let actions = lane.message(first, ack(&[("a", 3)]));
    let (id, _) = request(&actions[0]);
    lane.response(id, &page("a", 0, 3, Some("before")));
    let actions = lane.send(DownlinkEvent::Closed { epoch: first });
    assert_eq!(
        actions[0],
        DownlinkAction::Close {
            epoch: first,
            reason: None
        },
        "the host closes whatever is left of the session"
    );
    let backoff = wait(&actions[1]);
    assert!((200..=300).contains(&backoff), "{backoff}");
    assert_eq!(lane.drain(), vec![DownlinkAction::Wait { millis: backoff }]);
    lane.now += backoff;
    let actions = lane.drain();
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
    let actions = lane.send(DownlinkEvent::Closed { epoch: second });
    assert!(wait(&actions[1]) > backoff);
}

#[test]
fn protocol_violations_close_with_a_reason_and_retry() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let mut actions = lane.send(DownlinkEvent::Start);
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
            DownlinkAction::Close {
                epoch,
                reason: Some(reason.into())
            }
        );
        lane.now += wait(&closed[1]);
        actions = lane.drain();
    }
    let (epoch, _) = open(&actions[0]);
    let actions = lane.message(epoch, ack(&[("a", 1)]));
    request(&actions[0]);
    let actions = lane.message(epoch, ack(&[("a", 1)]));
    assert_eq!(
        actions[0],
        DownlinkAction::Close {
            epoch,
            reason: Some("invalid live subscription acknowledgement".into())
        },
        "a second acknowledgement is a violation"
    );
    lane.now += wait(&actions[1]);
    // A catch-up answer that does not answer the request ends the session too.
    let (epoch, _) = open(&lane.drain()[0]);
    let actions = lane.message(epoch, ack(&[("a", 1)]));
    let (id, _) = request(&actions[0]);
    let actions = lane.response(id, &empty("other", 0));
    assert_eq!(
        actions[0],
        DownlinkAction::Close {
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
    let (first, _) = open(&lane.send(DownlinkEvent::Start)[0]);
    lane.message(first, ack(&[("a", 0)]));
    assert_eq!(
        lane.send(DownlinkEvent::Pause),
        vec![DownlinkAction::Close {
            epoch: first,
            reason: None
        }]
    );
    assert_eq!(
        lane.send(DownlinkEvent::Wake),
        vec![],
        "paused: a wake schedules nothing"
    );
    assert_eq!(lane.send(DownlinkEvent::Closed { epoch: first }), vec![]);
    let actions = lane.send(DownlinkEvent::Resume);
    let (second, _) = open(&actions[0]);
    assert!(second > first);
    assert_eq!(
        lane.send(DownlinkEvent::Stop),
        vec![DownlinkAction::Close {
            epoch: second,
            reason: None
        }]
    );
    assert_eq!(lane.drain(), vec![]);
    assert_eq!(lane.send(DownlinkEvent::Wake), vec![]);
    assert_eq!(lane.send(DownlinkEvent::Resume), vec![]);
}

#[test]
fn a_page_from_a_previous_subscription_is_stale_not_a_gap() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let (first, _) = open(&lane.send(DownlinkEvent::Start)[0]);
    let actions = lane.message(first, ack(&[("a", 9)]));
    let (id, issued) = request(&actions[0]);
    // Unsubscribe and resubscribe while the request is in flight.
    lane.set("a", false);
    lane.set("a", true);
    let actions = lane.drain();
    assert_eq!(
        actions[0],
        DownlinkAction::Close {
            epoch: first,
            reason: None
        }
    );
    let (second, _) = open(&actions[1]);
    let actions = lane.message(second, ack(&[("a", 9)]));
    let (fresh_id, fresh) = request(&actions[0]);
    assert_eq!(cursors(&fresh), vec![("a", 0)]);
    // The old answer reaches no request in flight; the engine's own gate keeps
    // such a page stale rather than a gap ([downlink.rs] covers it directly).
    assert_eq!(
        lane.response(id, &page("a", issued.cursors["a"], 9, Some("obsolete"))),
        vec![]
    );
    assert_eq!(
        lane.response(fresh_id, &page("a", 0, 9, Some("fresh"))),
        applied(&["a"])
    );
    assert_eq!(lane.text(), "fresh");
}

#[test]
fn every_event_of_a_replaced_socket_is_fenced_by_its_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let (first, _) = open(&lane.send(DownlinkEvent::Start)[0]);
    let acknowledged = lane.message(first, ack(&[("a", 1)]));
    let (stale_id, _) = request(&acknowledged[0]);
    let actions = lane.send(DownlinkEvent::Closed { epoch: first });
    lane.now += wait(&actions[1]);
    let (second, _) = open(&lane.drain()[0]);
    assert!(second > first, "a new socket, a new epoch");
    assert_eq!(lane.message(second, ack(&[("a", 0)])), vec![]);
    // Whatever the abandoned socket and its request still deliver belongs to
    // no session; the one that replaced it keeps streaming.
    assert_eq!(lane.frame(first, &page("a", 0, 1, Some("late"))), vec![]);
    assert_eq!(lane.message(first, ack(&[("a", 1)])), vec![]);
    assert_eq!(lane.send(DownlinkEvent::Overflow { epoch: first }), vec![]);
    assert_eq!(
        lane.response(stale_id, &page("a", 0, 1, Some("late pull"))),
        vec![]
    );
    assert_eq!(lane.send(DownlinkEvent::Closed { epoch: first }), vec![]);
    assert_eq!(lane.text(), "local", "nothing of the old socket applied");
    assert_eq!(
        lane.frame(second, &page("a", 0, 1, Some("second socket"))),
        applied(&["a"])
    );
    assert_eq!(
        (lane.cursor("a"), lane.text()),
        (Some(1), json!("second socket"))
    );
}

#[test]
fn an_applied_page_leaves_the_push_lane_and_its_frozen_request_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    lane.client
        .transaction(|tx| tx.enqueue(mutation("queued")))
        .unwrap();
    // The push lane freezes a batch and keeps it in flight.
    let mut cycle = SyncCycle::default();
    cycle.restart_push_only();
    let push = cycle
        .next(&mut lane.client)
        .unwrap()
        .expect("a frozen batch");
    assert_eq!(push.kind, "push");
    let (epoch, _) = open(&lane.send(DownlinkEvent::Start)[0]);
    let actions = lane.message(epoch, ack(&[("a", 1)]));
    let (id, _) = request(&actions[0]);
    assert_eq!(
        lane.response(id, &page("a", 0, 1, Some("caught up"))),
        applied(&["a"]),
        "the downlink only wakes the push lane"
    );
    assert_eq!(
        lane.frame(epoch, &page("a", 1, 2, Some("streamed"))),
        applied(&["a"])
    );
    assert_eq!(lane.cursor("a"), Some(2));
    // The push lane made its own progress: the same frozen bytes are resent.
    assert_eq!(
        cycle.next(&mut lane.client).unwrap().map(|a| a.body),
        Some(push.body),
        "the downlink never borrows the push cycle"
    );
}

#[test]
fn a_restarted_lane_does_not_close_the_socket_of_the_lane_it_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    let (first, _) = lane.streaming("a", 0);
    // The lane closed without pumping its stop, as a host that closes does.
    lane.enqueue(DownlinkEvent::Stop);
    let actions = lane.send(DownlinkEvent::Start);
    let (second, subscribe) = open(&actions[0]);
    assert!(second > first, "the next lane opens its own socket");
    assert_eq!(subscribe.channels, ["a"]);
    assert_eq!(
        actions.len(),
        1,
        "nothing of the replaced lane's session is announced to this one"
    );
}
