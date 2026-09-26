//! Bootstrap scheduling in the Downlink worker: the one historical request in
//! flight, its rotation across Scopes, the fixed completion barrier and what a
//! reopen resumes. The ledger's own transitions are in
//! [bootstrap.rs](bootstrap.rs); here the scripted host drives the worker that
//! issues the pages ([#151](https://github.com/zanminwang/axton/issues/151)).
mod common;
use axton_client::*;
use common::*;
use serde_json::json;

/// One historical page: `(from, to]` of `scope`, bounded by origin `until`,
/// observed at channel head `head`.
fn historical(
    scope: &str,
    from: u64,
    to: u64,
    until: u64,
    head: u64,
    records: Vec<AuthorityRecord>,
) -> String {
    let page = BootstrapPage {
        channel: scope.to_string(),
        from,
        to,
        until,
        head,
        records,
    };
    String::from_utf8(page.encode().unwrap()).unwrap()
}
/// The bootstrap request an action carries: its id and what it asks for. The
/// flag is the host's only instruction - the durable load's page is fetched
/// from the same route, but it belongs to no socket.
fn asking(action: &DownlinkAction) -> (u64, BootstrapRequest) {
    match action {
        DownlinkAction::Request {
            request,
            body,
            bootstrap: true,
        } => (*request, BootstrapRequest::decode(body.as_bytes()).unwrap()),
        other => panic!("expected a bootstrap request, got {other:?}"),
    }
}
/// The committed run status an action announces.
fn announced(action: &DownlinkAction) -> &BootstrapState {
    match action {
        DownlinkAction::Bootstrap(state) => state,
        other => panic!("expected a bootstrap status, got {other:?}"),
    }
}
/// Every bootstrap request in these actions, in order.
fn requests(actions: &[DownlinkAction]) -> Vec<(u64, BootstrapRequest)> {
    actions
        .iter()
        .filter(|a| {
            matches!(
                a,
                DownlinkAction::Request {
                    bootstrap: true,
                    ..
                }
            )
        })
        .map(asking)
        .collect()
}
/// Every committed run status in these actions, in order.
fn statuses(actions: &[DownlinkAction]) -> Vec<&BootstrapState> {
    actions
        .iter()
        .filter(|a| matches!(a, DownlinkAction::Bootstrap(_)))
        .map(announced)
        .collect()
}
/// The one bootstrap request these actions carry.
fn only(actions: &[DownlinkAction]) -> (u64, BootstrapRequest) {
    let asked = requests(actions);
    assert_eq!(asked.len(), 1, "one request, not {asked:?}: {actions:?}");
    asked.into_iter().next().unwrap()
}

/// An answer for a request the ledger no longer knows, built from its id.
type Stale = fn(u64) -> DownlinkEvent;

/// The epoch of the session these actions opened, if any.
fn session(actions: &[DownlinkAction]) -> Option<u64> {
    actions.iter().find_map(|action| match action {
        DownlinkAction::Open { epoch, .. } => Some(*epoch),
        _ => None,
    })
}

trait Bootstrapping {
    /// Register a durable load of `scope`, as the native command does, without
    /// telling the lane about it.
    fn intend(&mut self, scope: &str);
    /// Register a durable load of `scope` and wake the lane, as a committed
    /// registration does.
    fn register(&mut self, scope: &str) -> Vec<DownlinkAction>;
    fn load(&mut self, scope: &str) -> BootstrapState;
    /// Answer the request `id` names with `body`.
    fn answer(&mut self, id: u64, body: String) -> Vec<DownlinkAction>;
}
impl Bootstrapping for Lane {
    fn intend(&mut self, scope: &str) {
        let id = self.load(scope).subscription_id;
        self.client.request_bootstrap(scope, id).unwrap();
    }
    fn register(&mut self, scope: &str) -> Vec<DownlinkAction> {
        self.intend(scope);
        self.send(DownlinkEvent::Wake)
    }
    fn load(&mut self, scope: &str) -> BootstrapState {
        let id = self
            .client
            .subscription_state(scope)
            .unwrap()
            .expect("a registered subscription")
            .subscription_id;
        self.client.bootstrap_state(scope, id).unwrap()
    }
    fn answer(&mut self, id: u64, body: String) -> Vec<DownlinkAction> {
        self.send(DownlinkEvent::Response { request: id, body })
    }
}

/// A load registered before #150 commits an origin has no interval to scan:
/// the session opens and the scheduler asks for nothing at all. The
/// acknowledgement that commits the origin is the bound the interval was
/// missing, so the same host loop asks for the first page; a run registered
/// offline must not wait for some unrelated commit to wake it.
#[test]
fn no_page_is_asked_for_before_the_origin_is_committed() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.set("a", true);
    let id = lane
        .client
        .subscription_state("a")
        .unwrap()
        .unwrap()
        .subscription_id;
    lane.client.request_bootstrap("a", id).unwrap();
    let actions = lane.send(DownlinkEvent::Start);
    assert_eq!(
        requests(&actions),
        vec![],
        "no origin, no interval: {actions:?}"
    );
    assert_eq!(statuses(&actions), Vec::<&BootstrapState>::new());
    assert_eq!(lane.load("a").state, BootstrapPhase::Requested);
    // The acknowledgement commits the origin, and the host loop that ran it
    // asks for the first page of the interval it has just bounded.
    let acknowledged = lane.message(1, ack(&[("a", 100)]));
    let (_, request) = only(&acknowledged);
    assert_eq!((request.after, request.until), (0, 100));
    assert_eq!(lane.load("a").state, BootstrapPhase::Requested);
    // And nothing asks twice: a later wake finds the request already in flight.
    assert_eq!(requests(&lane.send(DownlinkEvent::Wake)), vec![]);
}

/// The first page of a registered load: one request for the interval `(0, S]`,
/// carrying the client's read contracts, and the ordinary catch-up untouched.
#[test]
fn a_registered_load_asks_for_its_interval_and_leaves_delivery_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    let (epoch, _) = lane.streaming("a", 100);
    let actions = lane.register("a");
    let (id, request) = only(&actions);
    assert_eq!(
        (request.channel.as_str(), request.after, request.until),
        ("a", 0, 100),
        "the historical interval is bounded by the committed origin"
    );
    assert_eq!(request.models, lane.client.declared_models());
    assert_eq!(
        actions.len(),
        1,
        "nothing else: no ordinary pull, no status: {actions:?}"
    );
    // The page's authority commits with its progress; delivery does not move.
    let applied = lane.answer(
        id,
        historical("a", 0, 40, 100, 130, vec![authority(Some("history"), 7)]),
    );
    let state = announced(&applied[0]);
    assert_eq!(
        (state.state, state.cursor, state.barrier),
        (BootstrapPhase::Loading, 40, None)
    );
    assert_eq!(lane.cursor("a"), Some(100), "delivery is untouched");
    // The rotation comes back to the only Scope with work left, at once.
    let (_, next) = only(&applied);
    assert_eq!((next.after, next.until), (40, 100));
    assert!(epoch > 0);
}

/// The two work classes are independent: a page streamed while the historical
/// request is outstanding applies, moves the cursor and wakes the push lane.
#[test]
fn live_delivery_continues_while_a_historical_page_is_outstanding() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    let (epoch, _) = lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    let delivered = lane.frame(epoch, &page("a", 100, 120, Some("live")));
    committed(&delivered, &["a"]);
    assert_eq!((lane.cursor("a"), lane.text()), (Some(120), json!("live")));
    assert_eq!(
        requests(&delivered),
        vec![],
        "the outstanding page is not asked for twice"
    );
    // The held answer still applies afterwards.
    let applied = lane.answer(
        id,
        historical(
            "a",
            0,
            100,
            100,
            130,
            vec![authority_of("h", Some("old"), 3)],
        ),
    );
    let state = announced(&applied[0]);
    assert_eq!(
        (state.state, state.cursor, state.barrier),
        (BootstrapPhase::CatchingUp, 100, Some(130))
    );
}

/// One request in flight across Scopes, and each page moves the turn on: A, B,
/// then A again.
#[test]
fn two_scopes_take_turns_one_page_each() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.saved("a", 100);
    lane.saved("b", 200);
    lane.send(DownlinkEvent::Start);
    lane.intend("a");
    lane.intend("b");
    let first = lane.send(DownlinkEvent::Wake);
    let (id, request) = only(&first);
    assert_eq!(request.channel, "a", "Scope order opens the rotation");
    let second = lane.answer(id, historical("a", 0, 40, 100, 130, vec![]));
    let (id, request) = only(&second);
    assert_eq!(request.channel, "b", "the turn moved on");
    assert_eq!((request.after, request.until), (0, 200));
    let third = lane.answer(id, historical("b", 0, 60, 200, 240, vec![]));
    let (id, request) = only(&third);
    assert_eq!(request.channel, "a", "and back again");
    assert_eq!(
        (request.after, request.until),
        (40, 100),
        "from the progress its own page committed"
    );
    let fourth = lane.answer(id, historical("a", 40, 100, 100, 130, vec![]));
    let (_, request) = only(&fourth);
    assert_eq!(
        request.channel, "b",
        "a Scope that finished its interval leaves the rotation"
    );
}

/// A response that answers a run the ledger has replaced writes nothing and
/// announces nothing: not after an explicit retry, not after an unsubscribe,
/// not after the lane stopped.
#[test]
fn a_stale_historical_answer_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    // The run is failed and retried while its page is in flight.
    let state = lane.load("a");
    assert!(
        lane.client
            .fail_bootstrap(
                "a",
                state.subscription_id,
                state.run,
                BootstrapError::new("test.failed", "a failure of its own", vec![])
            )
            .unwrap()
    );
    lane.client
        .request_bootstrap("a", state.subscription_id)
        .unwrap();
    assert_eq!(lane.load("a").run, state.run + 1);
    let answered = lane.answer(id, historical("a", 0, 40, 100, 130, vec![]));
    assert_eq!(
        statuses(&answered),
        Vec::<&BootstrapState>::new(),
        "the old run's page announced nothing: {answered:?}"
    );
    assert_eq!(lane.load("a").cursor, 0, "and committed nothing");
    // The retried run asks again from the retained progress.
    let (id, request) = only(&answered);
    assert_eq!(request.after, 0);
    // An unsubscribe removes the task; its answer finds nothing.
    lane.set("a", false);
    let answered = lane.answer(id, historical("a", 0, 40, 100, 130, vec![]));
    assert_eq!(statuses(&answered), Vec::<&BootstrapState>::new());
    assert!(lane.client.subscription_state("a").unwrap().is_none());
}

/// The same for an answer that would fail the run rather than advance it: a
/// refusal the server decided, and a body that decodes to nothing, for a
/// registration that is gone. Naming a closed registration is how the ledger
/// reports one, with an error - and an error here would leave the pump, discard
/// the actions it had gathered and end the live session, which a stale answer
/// must never do.
#[test]
fn a_stale_refusal_writes_nothing_and_errors_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.streaming("a", 100);
    let mut head = 100;
    let mut epoch;
    let refusals: [(&str, Stale); 2] = [
        ("b", |request| DownlinkEvent::Failed {
            request,
            reason: Some("request.invalid".into()),
            status: Some(400),
        }),
        ("c", |request| DownlinkEvent::Response {
            request,
            body: "not a bootstrap page".into(),
        }),
    ];
    for (scope, stale) in refusals {
        // Another Scope, registered and initialized, with its page in flight.
        lane.saved(scope, 200);
        epoch = session(&lane.drain()).expect("the membership change replaced the socket");
        lane.message(epoch, ack(&[("a", head), (scope, 200)]));
        lane.intend(scope);
        let (id, request) = only(&lane.send(DownlinkEvent::Wake));
        assert_eq!(request.channel, scope);
        // The registration goes while its page is in flight.
        lane.set(scope, false);
        lane.enqueue(stale(id));
        let pumped = lane
            .worker
            .handle(&mut lane.client, DownlinkEvent::Next, lane.now, 500)
            .expect("a stale answer must not error the pump");
        assert_eq!(
            statuses(&pumped),
            Vec::<&BootstrapState>::new(),
            "nothing was written and nothing announced: {pumped:?}"
        );
        assert!(
            !pumped.iter().any(|action| matches!(
                action,
                DownlinkAction::Close {
                    reason: Some(_),
                    ..
                }
            )),
            "no protocol violation was reported: {pumped:?}"
        );
        // The membership change alone replaced the session, and delivery on the
        // one the lane opens next goes on as before.
        epoch = session(&pumped)
            .or_else(|| session(&lane.drain()))
            .expect("the lane opened its next session");
        lane.message(epoch, ack(&[("a", head)]));
        committed(
            &lane.frame(epoch, &page("a", head, head + 20, Some("live"))),
            &["a"],
        );
        head += 20;
        assert_eq!(lane.cursor("a"), Some(head));
    }
}

/// The slot is cleared when the lane stops, so whatever its request still
/// delivers after a reopen belongs to nobody.
#[test]
fn an_answer_that_arrives_after_a_stop_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    lane.send(DownlinkEvent::Stop);
    let answered = lane.answer(id, historical("a", 0, 40, 100, 130, vec![]));
    assert_eq!(answered, vec![], "a stopped lane answers nothing");
    assert_eq!(lane.load("a").cursor, 0);
    // The persisted task resumes on the next start without another call.
    let started = lane.send(DownlinkEvent::Start);
    let (again, request) = only(&started);
    assert_eq!((request.after, request.until), (0, 100));
    assert_ne!(again, id, "a new request, correlated by a new id");
}

/// A transport failure is not a failed run: the same page is asked for again
/// after a bounded wait, with no overall timeout on the task.
#[test]
fn a_transport_failure_waits_and_asks_for_the_same_page_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    let failed = lane.send(DownlinkEvent::Failed {
        request: id,
        reason: Some("connection reset".into()),
        status: None,
    });
    assert_eq!(statuses(&failed), Vec::<&BootstrapState>::new());
    assert_eq!(requests(&failed), vec![], "not at once");
    let millis = wait(failed.last().expect("a wait"));
    assert!(
        (200..=30_000).contains(&millis),
        "bounded backoff: {millis}"
    );
    assert_eq!(
        lane.load("a").state,
        BootstrapPhase::Requested,
        "the run is untouched"
    );
    lane.now += millis;
    let (mut outstanding, request) = only(&lane.drain());
    assert_eq!(
        (request.after, request.until),
        (0, 100),
        "the same page, from the same progress"
    );
    assert_ne!(outstanding, id);
    // Authentication, a timeout, rate limiting and a server error are transport
    // conditions too: the host refreshes credentials, and the page is asked for
    // again however long that takes.
    for status in [401, 408, 429, 503] {
        let failed = lane.send(DownlinkEvent::Failed {
            request: outstanding,
            reason: None,
            status: Some(status),
        });
        assert_eq!(
            statuses(&failed),
            Vec::<&BootstrapState>::new(),
            "HTTP {status} is not a refusal: {failed:?}"
        );
        assert_eq!(lane.load("a").state, BootstrapPhase::Requested);
        lane.now += wait(failed.last().expect("a wait"));
        (outstanding, _) = only(&lane.drain());
    }
}

/// A refusal the server answered with is terminal until an explicit retry, and
/// the retry keeps the progress the run had committed.
#[test]
fn a_refused_request_fails_the_run_until_an_explicit_retry() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    let applied = lane.answer(id, historical("a", 0, 40, 100, 130, vec![]));
    let (id, _) = only(&applied[1..]);
    let refused = lane.send(DownlinkEvent::Failed {
        request: id,
        reason: Some("request.invalid".into()),
        status: Some(400),
    });
    let state = announced(&refused[0]);
    assert_eq!(state.state, BootstrapPhase::Failed);
    let error = state.error.as_ref().expect("a stored failure");
    assert_eq!(error.code, "bootstrap.request_rejected");
    assert!(error.message.contains("400"), "{}", error.message);
    assert_eq!(state.cursor, 40, "the committed progress is retained");
    assert_eq!(
        requests(&lane.drain()),
        vec![],
        "a failed run is not retried by the scheduler"
    );
    // An explicit retry is what resumes it.
    let retried = lane
        .client
        .request_bootstrap("a", state.subscription_id)
        .unwrap();
    assert_eq!(retried.state, BootstrapPhase::Requested);
    let (_, request) = only(&lane.send(DownlinkEvent::Wake));
    assert_eq!(request.after, 40);
}

/// A page the protocol refuses commits no authority and fails the run visibly.
#[test]
fn a_protocol_invalid_answer_fails_the_run_and_writes_no_authority() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    let broken = lane.answer(id, "{\"mode\":\"bootstrap\"}".into());
    let state = announced(&broken[0]);
    assert_eq!(state.state, BootstrapPhase::Failed);
    assert_eq!(
        state.error.as_ref().map(|e| e.code.as_str()),
        Some("bootstrap.protocol_invalid")
    );
    assert_eq!(state.cursor, 0);
    // A page that answers another interval is refused the same way.
    lane.client
        .request_bootstrap("a", state.subscription_id)
        .unwrap();
    let (id, _) = only(&lane.send(DownlinkEvent::Wake));
    let mismatched = lane.answer(id, historical("a", 40, 60, 100, 130, vec![]));
    assert_eq!(
        announced(&mismatched[0])
            .error
            .as_ref()
            .map(|e| e.code.as_str()),
        Some("bootstrap.protocol_invalid")
    );
}

/// A lane that was never started has no network configuration: registration is
/// durable intent and nothing is asked for.
#[test]
fn a_lane_that_never_started_asks_for_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.saved("a", 100);
    let actions = lane.register("a");
    assert_eq!(actions, vec![], "the intent waits for a lane: {actions:?}");
    assert_eq!(lane.load("a").state, BootstrapPhase::Requested);
}

/// Lost frames recover delivery from the durable cursors; the historical answer
/// they were waiting for is not lost with them.
#[test]
fn an_overflowing_live_queue_keeps_the_historical_answer() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    let (epoch, _) = lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    for i in 0..=QUEUED_FRAMES as u64 {
        lane.enqueue(DownlinkEvent::Message {
            epoch,
            body: text(&page("a", 500 + i, 501 + i, Some("gap"))),
        });
    }
    // The lost frames are recovered from the durable cursor.
    let recovered = lane.drain();
    let (pull, pulled) = recovered
        .iter()
        .filter(|a| {
            matches!(
                a,
                DownlinkAction::Request {
                    bootstrap: false,
                    ..
                }
            )
        })
        .map(request)
        .next()
        .expect("a recovery pull");
    assert_eq!(cursors(&pulled), vec![("a", 100)]);
    // The historical answer was not lost with them.
    let applied = lane.answer(
        id,
        historical(
            "a",
            0,
            100,
            100,
            130,
            vec![authority_of("h", Some("old"), 3)],
        ),
    );
    let state = announced(
        applied
            .iter()
            .find(|a| matches!(a, DownlinkAction::Bootstrap(_)))
            .expect("the answer still applied"),
    );
    assert_eq!(
        (state.state, state.cursor, state.barrier),
        (BootstrapPhase::CatchingUp, 100, Some(130))
    );
    // Nor is the completion: the recovery answer takes delivery to the barrier.
    let complete = lane.response(pull, &page("a", 100, 130, Some("recovered")));
    committed(&complete, &["a"]);
    let settled = statuses(&complete);
    assert_eq!(settled.len(), 1, "the completion landed too: {complete:?}");
    assert_eq!(settled[0].state, BootstrapPhase::Complete);
    assert_eq!(lane.cursor("a"), Some(130));
}

/// A load belongs to the client, not to a socket: replacing the socket keeps
/// the request in flight and its answer still applies.
#[test]
fn socket_replacement_keeps_the_request_and_applies_its_answer() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    let (epoch, _) = lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    let closed = lane.send(DownlinkEvent::Closed { epoch });
    assert!(closed.iter().any(waiting), "the lane retries: {closed:?}");
    lane.now += 5_000;
    let reopened = lane.drain();
    let (second, _) = opened(&reopened[0]);
    assert_ne!(second, epoch, "a new socket");
    let applied = lane.answer(id, historical("a", 0, 100, 100, 130, vec![]));
    let state = announced(&applied[0]);
    assert_eq!(
        (state.state, state.barrier),
        (BootstrapPhase::CatchingUp, Some(130))
    );
}

/// The handoff: the interval finishes at S=100 with a final head of 130 while
/// delivery stands at 120. The run waits for delivery, completes when it
/// reaches 130, and publication past it moves the barrier no further.
#[test]
fn the_fixed_barrier_completes_only_when_delivery_reaches_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    let (epoch, _) = lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    lane.frame(epoch, &page("a", 100, 120, Some("live")));
    assert_eq!(lane.cursor("a"), Some(120));
    let catching = lane.answer(id, historical("a", 0, 100, 100, 130, vec![]));
    let state = announced(&catching[0]);
    assert_eq!(
        (state.state, state.cursor, state.barrier),
        (BootstrapPhase::CatchingUp, 100, Some(130))
    );
    assert_eq!(
        requests(&lane.drain()),
        vec![],
        "a finished interval asks for nothing more"
    );
    // Ordinary delivery reaches the barrier: the same pump completes the run.
    let complete = lane.frame(epoch, &page("a", 120, 130, Some("barrier")));
    committed(&complete, &["a"]);
    let state = announced(
        complete
            .iter()
            .find(|a| matches!(a, DownlinkAction::Bootstrap(_)))
            .expect("the completion was announced"),
    );
    assert_eq!(
        (state.state, state.barrier),
        (BootstrapPhase::Complete, Some(130))
    );
    // Later publication is ordinary synchronization: the evidence is unchanged.
    let later = lane.frame(epoch, &page("a", 130, 150, Some("later")));
    assert_eq!(
        statuses(&later),
        Vec::<&BootstrapState>::new(),
        "nothing settles twice: {later:?}"
    );
    let stored = lane.load("a");
    assert_eq!(
        (stored.state, stored.barrier, stored.cursor),
        (BootstrapPhase::Complete, Some(130), 100)
    );
    assert_eq!(lane.cursor("a"), Some(150));
}

/// A barrier that delivery reached while the lane was gone is settled on the
/// next start, before the session it would otherwise wait for opens.
#[test]
fn a_reopen_settles_a_reached_barrier_before_any_request() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    lane.answer(id, historical("a", 0, 100, 100, 130, vec![]));
    assert_eq!(lane.load("a").state, BootstrapPhase::CatchingUp);
    lane.send(DownlinkEvent::Stop);
    // Delivery reached the barrier outside this lane: a page applied by the
    // client itself, as another process's session would have.
    lane.client
        .apply_page(page("a", 100, 130, Some("caught up")))
        .unwrap();
    assert_eq!(lane.cursor("a"), Some(130));
    assert_eq!(
        lane.load("a").state,
        BootstrapPhase::CatchingUp,
        "nothing settled it yet"
    );
    let started = lane.send(DownlinkEvent::Start);
    let settled = statuses(&started);
    assert_eq!(settled.len(), 1, "the barrier settled: {started:?}");
    assert_eq!(settled[0].state, BootstrapPhase::Complete);
    let announced_at = started
        .iter()
        .position(|a| matches!(a, DownlinkAction::Bootstrap(_)))
        .unwrap();
    let opened_at = started
        .iter()
        .position(|a| matches!(a, DownlinkAction::Open { .. }))
        .expect("the session opened too");
    assert!(
        announced_at < opened_at,
        "settled before any I/O: {started:?}"
    );
}

/// One commit per pump: a queued live page and a queued historical answer are
/// applied by two pumps, in that order.
#[test]
fn one_pump_commits_once_when_delivery_and_a_load_are_both_queued() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    let (epoch, _) = lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    lane.enqueue(DownlinkEvent::Message {
        epoch,
        body: text(&page("a", 100, 120, Some("live"))),
    });
    lane.enqueue(DownlinkEvent::Response {
        request: id,
        body: historical("a", 0, 100, 100, 130, vec![]),
    });
    let first = lane.pump();
    committed(&first, &["a"]);
    assert_eq!(
        statuses(&first),
        Vec::<&BootstrapState>::new(),
        "the load waited for the next pump: {first:?}"
    );
    assert_eq!(lane.load("a").cursor, 0);
    let second = lane.pump();
    assert_eq!(announced(&second[0]).cursor, 100);
    assert_eq!(lane.cursor("a"), Some(120), "delivery is where it was");
}

/// A deferred load never holds the host back: the pump that applies a queued
/// page asks for no sleep, and the deferral is announced only once there is
/// nothing else to pump for.
#[test]
fn a_deferred_load_does_not_hold_back_queued_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    let (epoch, _) = lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    lane.enqueue(DownlinkEvent::Failed {
        request: id,
        reason: Some("offline".into()),
        status: None,
    });
    for (from, to) in [(100, 110), (110, 120)] {
        lane.enqueue(DownlinkEvent::Message {
            epoch,
            body: text(&page("a", from, to, Some("live"))),
        });
    }
    // Delivery goes first, one page per pump, and no pump that commits one asks
    // the host to sleep on a load that is waiting.
    for _ in 0..2 {
        let pumped = lane.pump();
        committed(&pumped, &["a"]);
        assert!(
            !pumped.iter().any(waiting),
            "a deferral must not delay queued delivery: {pumped:?}"
        );
    }
    assert_eq!(lane.cursor("a"), Some(120));
    // With the queue drained the deferral is announced, and the page is asked
    // for again once it has passed.
    let deferred = lane.pump();
    assert_eq!(
        statuses(&deferred),
        Vec::<&BootstrapState>::new(),
        "the run is untouched by a transport failure: {deferred:?}"
    );
    let millis = wait(deferred.last().expect("a wait"));
    assert!((200..=30_000).contains(&millis));
    lane.now += millis;
    let (_, request) = only(&lane.drain());
    assert_eq!((request.after, request.until), (0, 100));
}

/// A reopen resumes a run mid-interval from the progress it committed, with no
/// further call from the frontend and no status of its own to announce.
#[test]
fn a_reopen_resumes_a_loading_run_from_its_committed_progress() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    lane.answer(id, historical("a", 0, 40, 100, 130, vec![]));
    assert_eq!(
        (lane.load("a").state, lane.load("a").cursor),
        (BootstrapPhase::Loading, 40)
    );
    lane.send(DownlinkEvent::Stop);
    let started = lane.send(DownlinkEvent::Start);
    assert_eq!(
        statuses(&started),
        Vec::<&BootstrapState>::new(),
        "resuming commits nothing of its own: {started:?}"
    );
    let (_, request) = only(&started);
    assert_eq!(
        (request.after, request.until),
        (40, 100),
        "from the persisted progress, bounded by the same origin"
    );
}

/// A page whose records could not all be applied hands the reports to the
/// application first, then announces the run the ledger failed: the authority
/// that did apply stays, the interval does not advance, and the retry revisits
/// the same page.
#[test]
fn a_page_with_a_failed_record_reports_it_and_then_fails_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    // One record the schema refuses beside one it accepts.
    let mut broken = authority_of("x", Some("x"), 5);
    broken.state = json!({"text": 22});
    let answered = lane.answer(
        id,
        historical(
            "a",
            0,
            40,
            100,
            130,
            vec![authority_of("h", Some("kept"), 4), broken],
        ),
    );
    assert_eq!(
        reports(&answered[0]).len(),
        1,
        "the reports reach the application first: {answered:?}"
    );
    let state = announced(&answered[1]);
    assert_eq!(state.state, BootstrapPhase::Failed);
    let error = state.error.as_ref().expect("a stored failure");
    assert_eq!(error.code, "bootstrap.records_failed");
    assert_eq!(error.records.len(), 1);
    assert_eq!(state.cursor, 0, "the interval did not advance");
    assert_eq!(
        requests(&lane.drain()),
        vec![],
        "a failed run is not retried by the scheduler"
    );
    // The retry revisits the same page, and what applied is still there.
    lane.client
        .request_bootstrap("a", state.subscription_id)
        .unwrap();
    let (_, request) = only(&lane.send(DownlinkEvent::Wake));
    assert_eq!(request.after, 0);
    assert_eq!(
        lane.client
            .read(&schema().record_key("Entry", &json!({"id":"h"})).unwrap())
            .unwrap()
            .expect("the record that applied is committed")["text"],
        json!("kept")
    );
}

/// A pause stops the schedule without touching the load: nothing is asked for,
/// the page the host already fetched is still applied - no I/O, and stamps make
/// it idempotent - and the resume clears the deferral a failure left behind.
#[test]
fn a_pause_keeps_the_slot_and_a_resume_clears_the_deferral() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    lane.streaming("a", 100);
    let (id, _) = only(&lane.register("a"));
    // A transport failure defers the next page.
    let failed = lane.send(DownlinkEvent::Failed {
        request: id,
        reason: Some("offline".into()),
        status: None,
    });
    assert!((200..=30_000).contains(&wait(failed.last().expect("a wait"))));
    // The pause silences the schedule entirely: no request, and no sleep to
    // wake from - the resume is what starts work again.
    let paused = lane.send(DownlinkEvent::Pause);
    assert_eq!(requests(&paused), vec![], "a paused lane asks for nothing");
    assert!(
        !paused.iter().any(waiting),
        "a paused lane sleeps on nothing: {paused:?}"
    );
    // Resume clears the deferral: the page goes out at once, on the same clock
    // the backoff was measured from.
    let (second, request) = only(&lane.send(DownlinkEvent::Resume));
    assert_eq!((request.after, request.until), (0, 100));
    // Pausing again keeps that page's slot: the answer the host had already
    // fetched is applied while paused, and nothing is asked for until resume.
    lane.send(DownlinkEvent::Pause);
    let applied = lane.answer(
        second,
        historical("a", 0, 40, 100, 130, vec![authority(Some("history"), 7)]),
    );
    let state = announced(&applied[0]);
    assert_eq!(
        (state.state, state.cursor),
        (BootstrapPhase::Loading, 40),
        "the held answer committed while paused: {applied:?}"
    );
    assert_eq!(lane.text(), json!("history"), "its authority landed");
    assert_eq!(requests(&applied), vec![], "and still nothing is asked for");
    let (_, next) = only(&lane.send(DownlinkEvent::Resume));
    assert_eq!(next.after, 40, "the next page follows the resume");
}

/// A load is scheduled independently of the live socket: with the socket deep
/// in its reconnect backoff, HTTP still works, so page follows page on the
/// load's own clock and the lane's one sleep is the earlier of the two
/// schedules - never the socket's when the load wants a pump sooner.
#[test]
fn the_socket_backoff_never_paces_the_historical_pages() {
    let dir = tempfile::tempdir().unwrap();
    let mut lane = Lane::new(dir.path());
    // A boundary committed by an earlier session, so the interval is bounded
    // without this lane ever getting a handshake.
    lane.saved("a", 100);
    // Every socket is refused, so the driver climbs to its 30 s cap.
    let mut actions = lane.send(DownlinkEvent::Start);
    let mut socket = 0;
    for _ in 0..8 {
        let (epoch, _) = opened(&actions[0]);
        let closed = lane.send(DownlinkEvent::Closed { epoch });
        socket = wait(closed.last().expect("the lane retries its socket"));
        if socket >= 20_000 {
            break;
        }
        lane.now += socket;
        actions = lane.drain();
    }
    assert!(
        socket >= 20_000,
        "the socket is far into its backoff: {socket}"
    );
    // The first page goes out while the socket is waiting: a load rides on the
    // lane, not on the session.
    let asked = lane.register("a");
    let (first, request) = only(&asked);
    assert_eq!((request.after, request.until), (0, 100));
    // The page commits, and the next one is asked for in the same host loop:
    // no sleep comes between the commit and the request that follows it.
    let applied = lane.answer(first, historical("a", 0, 40, 100, 130, vec![]));
    let state = announced(&applied[0]);
    assert_eq!(
        (state.state, state.cursor),
        (BootstrapPhase::Loading, 40),
        "the first page committed: {applied:?}"
    );
    let (second, next) = only(&applied);
    let issued = applied
        .iter()
        .position(|action| matches!(action, DownlinkAction::Request { .. }))
        .expect("the next page");
    assert_eq!((next.after, next.until), (40, 100));
    assert!(
        applied.iter().take(issued).all(|action| !waiting(action)),
        "the socket's backoff must not delay the next page: {applied:?}"
    );
    // And a load the transport deferred sleeps on its own 250 ms backoff, not
    // on the socket's: one sleep for the lane, and it is the earlier one.
    let deferred = lane.send(DownlinkEvent::Failed {
        request: second,
        reason: Some("offline".into()),
        status: None,
    });
    let sleeps: Vec<u64> = deferred.iter().filter(|a| waiting(a)).map(wait).collect();
    assert_eq!(
        sleeps.len(),
        1,
        "one sleep for the whole lane: {deferred:?}"
    );
    assert!(
        (200..=250).contains(&sleeps[0]) && sleeps[0] < socket,
        "the lane sleeps on the load's due, not the socket's {socket}: {deferred:?}"
    );
    lane.now += sleeps[0];
    let (_, again) = only(&lane.drain());
    assert_eq!(
        (again.after, again.until),
        (40, 100),
        "the deferred page went out on its own schedule, with the socket still waiting"
    );
}
