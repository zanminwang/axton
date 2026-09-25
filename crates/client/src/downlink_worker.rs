//! The Downlink worker: the long-lived owner of inbound delivery. It holds the
//! bounded inbound queue, the durable-cursor policy, the catch-up requests and
//! the lane's schedule; the host only carries sockets, HTTP, timers and
//! credential refresh. A callback enqueues what arrived and wakes the loop; the
//! loop pumps, and only a pump commits
//! ([Downlink worker](../../../docs/engineering/architecture/client/connection/controller/downlink-worker.md)).
use crate::*;
use std::collections::VecDeque;

/// What the host tells the worker. `now` and `entropy` travel beside the event
/// ([`DownlinkWorker::handle`]). `epoch` names the socket session an event
/// belongs to and `request` the catch-up it answers, so whatever an abandoned
/// socket or request still delivers is ignored.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum DownlinkEvent {
    /// The lane starts; the first session begins on the next `next`.
    Start,
    /// The lane stops for good: the session ends and nothing is scheduled.
    Stop,
    /// The session ends without backoff; nothing runs until `resume`.
    Pause,
    Resume,
    /// Something changed that may need work: a subscription or local commit.
    Wake,
    /// Pump: consume what is queued, commit at most one page, and answer with
    /// what to do next. The only event that touches the database.
    Next,
    /// A frame arrived on the socket of this epoch: the acknowledgement or a page.
    Message {
        epoch: u64,
        body: String,
    },
    /// The socket of this epoch closed. The host has already reported the error
    /// and refreshed credentials if it chose to.
    Closed {
        epoch: u64,
    },
    /// The host's own frame buffer for this epoch overflowed and frames were
    /// dropped before they reached the worker.
    Overflow {
        epoch: u64,
    },
    /// The answer to the `request` action this id names.
    Response {
        request: u64,
        body: String,
    },
    /// The request this id names failed; `reason` is what the host reported and
    /// `status` the HTTP status it carried, when it had one. A status in the
    /// 4xx range is a refusal the server decided, not a transport failure.
    Failed {
        request: u64,
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        status: Option<u16>,
    },
}

/// What the host does next, in order.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DownlinkAction {
    /// Open the socket and send `subscribe` once it is open. Frames it delivers
    /// are `message` events of this epoch; its end is `closed`.
    Open { epoch: u64, subscribe: String },
    /// Close the socket of this epoch and abandon its request, if any. A
    /// `reason` is a protocol violation the host reports as an error.
    Close { epoch: u64, reason: Option<String> },
    /// `POST /sync/pull` with `body`; its answer is a `response` event of this
    /// id, a failure is `failed`. An ordinary catch-up carries every subscribed
    /// channel and belongs to the open session, so the session's cancellation
    /// abandons it and its failure ends the session. A `bootstrap` request is
    /// one Scope's historical page on the same route: it belongs to the lane,
    /// not to a socket, so it outlives the session, its failure ends none, and
    /// only `pause` and `close` abandon it.
    Request {
        request: u64,
        body: String,
        bootstrap: bool,
    },
    /// A bootstrap run changed and the change is committed: the phase, the
    /// historical progress, the fixed barrier and the stored failure of one
    /// registration. The SDKs publish it as the subscription's load status; no
    /// delivery decision depends on it
    /// ([#151](https://github.com/zanminwang/axton/issues/151)).
    Bootstrap(BootstrapState),
    /// A page applied and may have settled a batch: wake the push lane.
    Wake { lane: &'static str },
    /// What the last page could not apply; the host hands it to the application.
    Report { reports: Vec<Report> },
    /// A commit landed for these Scopes: their cursors moved.
    Changed { scopes: Vec<String> },
    /// The handshake of the open session covered these Scopes: delivery for
    /// them is established, whether or not anything was committed for them.
    /// The SDKs turn it into the `live` connection status; no sync decision
    /// depends on it.
    Acknowledged { scopes: Vec<String> },
    /// Nothing to do for `millis`; then pump again.
    Wait { millis: u64 },
}

/// Inbound work that a full page queue may never drop: the handshake, an
/// overflow to recover from, and the answer to the request in flight. Each one
/// needs the database, so the pump consumes it, never the enqueue.
enum Control {
    /// The acknowledgement, already confirmed against the subscribe frame.
    Acknowledged(SubscriptionAck),
    /// Frames were lost: recover every channel from its durable cursor.
    Overflow,
    /// The body the request in flight answered with.
    Response(String),
}

/// One catch-up in flight: the id the host correlates its answer by and the
/// request that answer must match.
struct Pending {
    id: u64,
    request: PullRequest,
}

/// The one historical page request in flight, across every Scope: the id the
/// host correlates its answer by, the registration and run it belongs to, and
/// the request that answer must be a page of. It names no socket epoch - a load
/// belongs to the client, not to a session - so replacing the socket neither
/// cancels nor restarts it, and the answer is validated against what is
/// committed when it arrives.
struct PendingBootstrap {
    id: u64,
    subscription_id: u64,
    run: u64,
    request: BootstrapRequest,
}

/// What the host reported about the historical request in flight, waiting for
/// the pump that may commit it. It is held apart from the session's control
/// queue, which an ended session discards: an answer must survive the socket.
enum Loaded {
    /// The body the request answered with.
    Page(String),
    /// The request did not answer. `status` is the HTTP status the host had.
    Failed {
        status: Option<u16>,
        reason: Option<String>,
    },
}

/// The historical schedule: whose turn it is and when the next page may be
/// asked for. It is the lane's own retry policy applied to one work class, so
/// no page is ever in flight twice and no failure is retried in a tight loop.
#[derive(Default)]
struct Loading {
    /// Whether anything may have become schedulable since the last enumeration:
    /// the lane started, a wake arrived - every commit wakes it - or a page
    /// committed. Without it every pump would query the ledger for nothing.
    dirty: bool,
    /// The Scope whose page was asked for last: the rotation's position.
    rotation: Option<String>,
    /// Transport failures in a row, and the time before which none is retried.
    /// There is no overall timeout: waiting for connectivity is not a failure.
    attempt: u32,
    due: u64,
}
impl Loading {
    /// A commit may have made work schedulable.
    fn wake(&mut self) {
        self.dirty = true;
    }
    /// The lane started or resumed: nothing is deferred any more.
    fn restart(&mut self, now: u64) {
        *self = Self {
            dirty: true,
            rotation: self.rotation.take(),
            attempt: 0,
            due: now,
        };
    }
    /// A transport failure: hold the next attempt back, keeping the run.
    fn defer(&mut self, now: u64, entropy: u64) -> u64 {
        let delay = ConnectionDriver::backoff(self.attempt, entropy);
        self.attempt = self.attempt.saturating_add(1);
        self.due = now.saturating_add(delay);
        self.dirty = true;
        delay
    }
    /// The request answered: the transport works, and the ledger may have more.
    fn answered(&mut self, now: u64) {
        self.attempt = 0;
        self.due = now;
        self.dirty = true;
    }
}

/// Streamed page frames held in the queue. Beyond this the queue is discarded
/// whole and every channel recovers from the durable cursor: the server log is
/// the durable queue, the cursor the pointer into it. Control work is queued
/// apart from it and is never dropped for this bound.
pub const QUEUED_FRAMES: usize = 64;

/// The Downlink worker: one lane, one socket session at a time.
#[derive(Default)]
pub struct DownlinkWorker {
    /// The lane's schedule: when to open a session, when to retry, pause, stop.
    driver: ConnectionDriver,
    /// The socket session it directs; it owns no queue and no client.
    session: LiveSession,
    /// Control work in arrival order, never dropped for the page bound.
    control: VecDeque<Control>,
    /// Streamed pages not yet applied, in arrival order. The front is applied
    /// when every channel it names connects to its cursor; a page with a gap
    /// stays until a pull connects it or covers it.
    pages: VecDeque<PullPage>,
    /// The one ordinary catch-up in flight.
    active: Option<Pending>,
    /// The one historical page request in flight, across every Scope. It is a
    /// second slot, not a second queue: the ordinary cursor path never sees it.
    bootstrap: Option<PendingBootstrap>,
    /// What the host reported about that request, for the next pump to apply.
    loaded: Option<Loaded>,
    /// Whose turn the next historical page is, and when it may be asked for.
    loading: Loading,
    /// The lane started and has not re-evaluated the persisted barriers yet.
    reopened: bool,
    /// Another pull is needed once the one in flight ends (an overflow while
    /// pulling: the lost frames may lie beyond the answer).
    again: bool,
    /// Allocates the ids the host correlates catch-up answers by.
    requests: u64,
    /// A session an enqueued event ended; the next pump tells the host to close
    /// its socket.
    closing: Option<(u64, Option<String>)>,
    /// The subscription identity of every Scope the open session subscribed,
    /// snapshotted with the generation when it began. It fences the boundaries
    /// its acknowledgement establishes: a Scope that has been unsubscribed, or
    /// recreated, since is another subscription and takes nothing from it.
    expected: BTreeMap<String, u64>,
}

/// Whether an HTTP status is a refusal the server decided, which no retry can
/// clear: a deterministic request or read-contract failure. Authentication
/// (401), a timeout (408) and rate limiting (429) are transport conditions the
/// lane retries instead, as is every server error.
fn refused(status: u16) -> bool {
    (400..500).contains(&status) && !matches!(status, 401 | 408 | 429)
}

/// Collect one page's outcome into the actions: the push lane wakes and the
/// Scopes it moved are announced when it applied, its reports when it has any.
fn settle(progress: &DownlinkProgress, actions: &mut Vec<DownlinkAction>) {
    if progress.disposition == "applied" {
        actions.push(DownlinkAction::Wake { lane: "push" });
        actions.push(DownlinkAction::Changed {
            scopes: progress.report.cursors.keys().cloned().collect(),
        });
    }
    if !progress.report.reports.is_empty() {
        actions.push(DownlinkAction::Report {
            reports: progress.report.reports.clone(),
        });
    }
}

impl DownlinkWorker {
    /// One event. Everything but [`DownlinkEvent::Next`] is enqueued into typed
    /// state and answers with no actions; `next` is the pump.
    pub fn handle<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        event: DownlinkEvent,
        now: u64,
        entropy: u64,
    ) -> Result<Vec<DownlinkAction>> {
        match event {
            DownlinkEvent::Next => self.pump(client, now, entropy),
            other => {
                self.enqueue(client, other, now, entropy);
                Ok(vec![])
            }
        }
    }

    /// Take one event: lane controls reach the schedule at once, I/O reaches the
    /// queues. No database work, no page application, and nothing the pump must
    /// see is dropped; the client is read only for the in-memory subscription
    /// generation, which decides whether a failure deserves backoff.
    fn enqueue<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        event: DownlinkEvent,
        now: u64,
        entropy: u64,
    ) {
        match event {
            // Handled by `handle`; a pump is not queued.
            DownlinkEvent::Next => {}
            DownlinkEvent::Start => {
                // A lane that replaced a closed one starts clean: the old
                // host abandoned its socket already, so nothing of a leftover
                // session is queued or announced to this one.
                self.end(None);
                self.closing = None;
                self.driver.start(now);
                // Persisted loads resume without another call from the
                // frontend, and every barrier is re-evaluated before any I/O.
                self.loading.restart(now);
                self.reopened = true;
            }
            DownlinkEvent::Stop => {
                self.end(None);
                self.driver.stop();
                // The lane is gone for good: its request is abandoned and its
                // answer belongs to nobody. The durable task is untouched and
                // the next start picks it up.
                self.bootstrap = None;
                self.loaded = None;
                self.loading = Loading::default();
                self.reopened = false;
            }
            DownlinkEvent::Pause => {
                if self.session.open() {
                    self.end(None);
                    self.driver.complete(true, now, 0);
                }
                self.driver.pause();
            }
            DownlinkEvent::Resume => {
                self.driver.resume(now);
                self.loading.restart(now);
            }
            DownlinkEvent::Wake => {
                self.driver.wake();
                self.loading.wake();
            }
            DownlinkEvent::Message { epoch, body } => {
                if self.session.current(epoch) {
                    self.frame(client, &body, now, entropy);
                }
            }
            DownlinkEvent::Closed { epoch } => {
                if self.session.current(epoch) {
                    self.fail(client, None, now, entropy);
                }
            }
            DownlinkEvent::Overflow { epoch } => {
                // Before the handshake there is no stream to have lost frames of.
                if self.session.current(epoch) && self.session.acknowledged() {
                    self.overflowed();
                }
            }
            DownlinkEvent::Response { request, body } => {
                if self.loads(request) {
                    self.loaded = Some(Loaded::Page(body));
                } else if self.answers(request) {
                    self.control.push_back(Control::Response(body));
                }
            }
            DownlinkEvent::Failed {
                request,
                reason,
                status,
            } => {
                // A historical page that failed ends no session: it is retried
                // on its own schedule, or refused if the server decided so.
                if self.loads(request) {
                    self.loaded = Some(Loaded::Failed { status, reason });
                } else if self.answers(request) {
                    // The host has reported the failure already; the session
                    // ends and the lane retries with backoff.
                    self.fail(client, None, now, entropy);
                }
            }
        }
    }

    /// Whether `request` is the catch-up in flight. An answer or failure of any
    /// other belongs to a session that is gone.
    fn answers(&self, request: u64) -> bool {
        self.active.as_ref().is_some_and(|p| p.id == request)
    }
    /// Whether `request` is the historical page in flight. The two slots share
    /// one id space, so one test decides which work an answer belongs to.
    fn loads(&self, request: u64) -> bool {
        self.bootstrap.as_ref().is_some_and(|p| p.id == request)
    }

    /// One frame of the current socket: the handshake is validated for order
    /// here and queued for the pump; a page joins the bounded queue.
    fn frame<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        body: &str,
        now: u64,
        entropy: u64,
    ) {
        let decoded = match LiveMessage::decode(body.as_bytes()) {
            Ok(decoded) => decoded,
            Err(e) => return self.fail(client, Some(e.to_string()), now, entropy),
        };
        match decoded {
            LiveMessage::Acknowledged(ack) => {
                if let Err(e) = self.session.acknowledge(&ack) {
                    return self.fail(client, Some(e.to_string()), now, entropy);
                }
                self.control.push_back(Control::Acknowledged(ack));
            }
            LiveMessage::Page(page) => {
                if let Err(e) = self.session.streamed() {
                    return self.fail(client, Some(e.to_string()), now, entropy);
                }
                if self.pages.len() >= QUEUED_FRAMES {
                    return self.overflowed();
                }
                self.pages.push_back(page);
            }
        }
    }

    /// Frames were lost - the host's buffer or this queue overflowed - and which
    /// channels they belonged to is unknown: the queue is discarded and every
    /// channel recovers from its durable cursor. Redundant overflows coalesce
    /// into the one recovery still to run.
    fn overflowed(&mut self) {
        self.pages.clear();
        if !self.control.iter().any(|c| matches!(c, Control::Overflow)) {
            self.control.push_back(Control::Overflow);
        }
    }

    /// End the session: the host closes its socket and abandons its request, and
    /// whatever it queued is dropped, since the next session recovers from the
    /// durable cursors.
    fn end(&mut self, reason: Option<String>) {
        if let Some(epoch) = self.session.close() {
            self.closing = Some((epoch, reason));
        }
        self.control.clear();
        self.pages.clear();
        self.active = None;
        self.again = false;
        self.expected.clear();
    }

    /// A protocol violation or a transport failure: the session ends and the
    /// lane retries with backoff - unless a committed subscription change had
    /// already invalidated it, in which case the change, not the transport,
    /// ended it: the lane opens the next session at once and counts no failed
    /// attempt. The server or the network can report the socket closed before
    /// the wake of the commit that invalidated it is pumped, so the generation,
    /// not the arrival order, decides: a session whose subscribed set a commit
    /// has replaced reconnects without backoff however its socket ended.
    fn fail<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        reason: Option<String>,
        now: u64,
        entropy: u64,
    ) {
        if self.stale(client) {
            return self.invalidate(now);
        }
        self.end(reason);
        self.driver.complete(false, now, entropy);
    }

    /// Whether the open session subscribed under a subscription set that a
    /// commit has since replaced.
    fn stale<S: ClientStore>(&self, client: &mut Client<S>) -> bool {
        self.session
            .generation()
            .is_some_and(|generation| generation != client.subscription_generation())
    }

    /// The subscribed set the session negotiated is gone: end it and open the
    /// next one without backoff.
    fn invalidate(&mut self, now: u64) {
        self.end(None);
        self.driver.complete(true, now, 0);
        self.driver.wake();
    }

    /// Tell the host to close a session the worker ended, in order.
    fn flush(&mut self, actions: &mut Vec<DownlinkAction>) {
        if let Some((epoch, reason)) = self.closing.take() {
            actions.push(DownlinkAction::Close { epoch, reason });
        }
    }

    /// One bounded pump: control work first, then at most one page application,
    /// then the lane's next decision. One commit per call, so foreground work
    /// interleaves; the host pumps again while actions come back.
    fn pump<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        now: u64,
        entropy: u64,
    ) -> Result<Vec<DownlinkAction>> {
        let mut actions = vec![];
        self.flush(&mut actions);
        // A committed subscribe or unsubscribe invalidates the session: the
        // lane starts over with the new channel set, without backoff.
        if self.stale(client) {
            self.invalidate(now);
            self.flush(&mut actions);
        }
        // A lane that just started re-evaluates every persisted barrier before
        // it issues anything: a run whose delivery reached its barrier while
        // the client was closed completes without another request.
        if std::mem::take(&mut self.reopened) && self.resume(client, &mut actions)? {
            return Ok(actions);
        }
        let committed = self.process(client, now, entropy, &mut actions)?;
        self.flush(&mut actions);
        self.barriers(client, &mut actions)?;
        self.historical(client, now, entropy, committed, &mut actions)?;
        if !self.session.open() {
            match self.driver.next(now) {
                ConnectionAction::Sync => self.begin(client, now, &mut actions)?,
                ConnectionAction::Wait { millis } => actions.push(DownlinkAction::Wait { millis }),
                ConnectionAction::Idle => {}
            }
        }
        Ok(actions)
    }

    /// Consume the queues: every control event, which a page that cannot apply
    /// yet never holds back, then streamed pages from the front until one
    /// commits. A page leaves the queue only by being applied or covered.
    /// `true` when a commit landed, so this pump holds no other.
    fn process<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        now: u64,
        entropy: u64,
        actions: &mut Vec<DownlinkAction>,
    ) -> Result<bool> {
        while self.session.open() {
            let Some(control) = self.control.pop_front() else {
                break;
            };
            let committed = match control {
                Control::Acknowledged(ack) => {
                    self.acknowledged(client, &ack, now, entropy, actions)?
                }
                Control::Overflow => {
                    self.recover(client, actions)?;
                    false
                }
                Control::Response(body) => self.response(client, &body, now, entropy, actions)?,
            };
            if committed {
                return Ok(true);
            }
        }
        // Pages wait while a pull is in flight: its answer moves the cursors
        // they are measured against. A historical page is in neither queue and
        // holds nothing back.
        while self.session.open() && self.active.is_none() {
            let Some(front) = self.pages.front().cloned() else {
                break;
            };
            let progress = client.receive_downlink(front, None)?;
            settle(&progress, actions);
            if progress.disposition == "recover" {
                // The gap stays at the front until a pull connects or covers it.
                self.pull(client, actions)?;
                return Ok(false);
            }
            self.pages.pop_front();
            if progress.disposition == "applied" {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The lane started: complete every persisted run whose fixed barrier
    /// delivery has already reached. `true` when it committed, so the pump
    /// holds that one commit and the host pumps again for the session.
    fn resume<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        actions: &mut Vec<DownlinkAction>,
    ) -> Result<bool> {
        let waiting = client.bootstrap_barriers()?;
        let settled = client.settle_bootstrap_barriers(&waiting)?;
        let committed = !settled.is_empty();
        for state in settled {
            actions.push(DownlinkAction::Bootstrap(state));
        }
        Ok(committed)
    }

    /// Committed delivery progress may have reached a fixed barrier: complete
    /// every run of a Scope this pump moved. The Scopes are the ones the commit
    /// announced, so a barrier is settled by the transaction that reached it
    /// and by nothing else; a run still short of its barrier writes nothing
    /// ([`Client::settle_bootstrap_barriers`]).
    fn barriers<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        actions: &mut Vec<DownlinkAction>,
    ) -> Result<()> {
        let moved: Vec<String> = actions
            .iter()
            .filter_map(|action| match action {
                DownlinkAction::Changed { scopes } => Some(scopes.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        if moved.is_empty() {
            return Ok(());
        }
        for state in client.settle_bootstrap_barriers(&moved)? {
            actions.push(DownlinkAction::Bootstrap(state));
        }
        Ok(())
    }

    /// The historical work class: apply what the request in flight answered,
    /// then ask for the next page. Neither step touches the socket, the live
    /// cursors or the ordinary catch-up slot, and neither runs when this pump
    /// has already committed - one commit per pump, so foreground work and
    /// delivery interleave with a load.
    fn historical<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        now: u64,
        entropy: u64,
        committed: bool,
        actions: &mut Vec<DownlinkAction>,
    ) -> Result<()> {
        if committed || self.answered(client, now, entropy, actions)? {
            return Ok(());
        }
        self.schedule(client, now, actions)
    }

    /// Apply what the host reported about the historical request in flight;
    /// `true` when it committed. A page is refused unless it answers the
    /// request that asked for it, and every test the ledger makes is against
    /// what is committed now, so an answer that outlived its run writes
    /// nothing. A transport failure keeps the run and defers the same page; a
    /// refusal the server decided fails the run until an explicit retry.
    fn answered<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        now: u64,
        entropy: u64,
        actions: &mut Vec<DownlinkAction>,
    ) -> Result<bool> {
        // The slot is emptied only by an answer to the request it holds: taking
        // both at once would clear it on every pump.
        let Some(loaded) = self.loaded.take() else {
            return Ok(false);
        };
        let Some(pending) = self.bootstrap.take() else {
            return Ok(false);
        };
        let scope = pending.request.channel.clone();
        let body = match loaded {
            Loaded::Failed { status, reason } => {
                if !status.is_some_and(refused) {
                    // Offline or interrupted: the run is untouched and the same
                    // page is asked for again once the backoff has passed.
                    let millis = self.loading.defer(now, entropy);
                    self.sleep(millis, actions);
                    return Ok(false);
                }
                self.loading.answered(now);
                let detail = reason.map(|r| format!(": {r}")).unwrap_or_default();
                return self.refuse(
                    client,
                    &pending,
                    BootstrapError::new(
                        REQUEST_REJECTED,
                        format!(
                            "the bootstrap request for {scope} was refused with HTTP {}{detail}",
                            status.unwrap_or_default()
                        ),
                        vec![],
                    ),
                    actions,
                );
            }
            Loaded::Page(body) => body,
        };
        self.loading.answered(now);
        let page = match BootstrapPage::decode(body.as_bytes()) {
            Ok(page) if page.answers(&pending.request) => page,
            Ok(page) => {
                return self.refuse(
                    client,
                    &pending,
                    BootstrapError::new(
                        PROTOCOL_INVALID,
                        format!(
                            "a bootstrap page ({}, {}] of {} does not answer the request ({}, {}] of {scope}",
                            page.from, page.to, page.channel, pending.request.after,
                            pending.request.until
                        ),
                        vec![],
                    ),
                    actions,
                );
            }
            Err(e) => {
                return self.refuse(
                    client,
                    &pending,
                    BootstrapError::new(
                        PROTOCOL_INVALID,
                        format!("invalid bootstrap page for {scope}: {e}"),
                        vec![],
                    ),
                    actions,
                );
            }
        };
        let applied = client.apply_bootstrap_page(
            &scope,
            pending.subscription_id,
            pending.run,
            pending.request.after,
            &page,
        )?;
        // What the page could not apply is the application's, whether or not
        // the run survived it.
        if let Some(report) = applied.report().filter(|r| !r.reports.is_empty()) {
            actions.push(DownlinkAction::Report {
                reports: report.reports.clone(),
            });
        }
        let Some(state) = applied.state() else {
            return Ok(false);
        };
        actions.push(DownlinkAction::Bootstrap(state.clone()));
        Ok(true)
    }

    /// Store a failure the ledger cannot see for itself and announce it;
    /// `false` when the run it named is no longer the active one, in which case
    /// nothing was written.
    fn refuse<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        pending: &PendingBootstrap,
        error: BootstrapError,
        actions: &mut Vec<DownlinkAction>,
    ) -> Result<bool> {
        let scope = &pending.request.channel;
        if !client.fail_bootstrap(scope, pending.subscription_id, pending.run, error)? {
            return Ok(false);
        }
        let state = client.bootstrap_state(scope, pending.subscription_id)?;
        actions.push(DownlinkAction::Bootstrap(state));
        Ok(true)
    }

    /// Ask the host to sleep until the next page is due - but only when this
    /// pump gave it nothing else to do, so a deferred load never holds back
    /// work the host would otherwise pump for at once. A paused lane sleeps on
    /// nothing: `resume` clears the deferral.
    fn sleep(&self, millis: u64, actions: &mut Vec<DownlinkAction>) {
        if self.driver.active() && actions.is_empty() {
            actions.push(DownlinkAction::Wait { millis });
        }
    }

    /// Ask for one historical page when none is in flight: the next run in the
    /// rotation, from the progress it committed, bounded by its own origin. The
    /// read that picks it is closed before the action leaves, so no transaction
    /// and no pump waits on the network.
    fn schedule<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        now: u64,
        actions: &mut Vec<DownlinkAction>,
    ) -> Result<()> {
        // With no network configuration - a lane that never started, or a
        // paused one - there is nothing to issue a request to.
        if self.bootstrap.is_some() || !self.driver.active() || !self.loading.dirty {
            return Ok(());
        }
        if self.loading.due > now {
            self.sleep(self.loading.due - now, actions);
            return Ok(());
        }
        let Some(task) = client.bootstrap_schedule(self.loading.rotation.as_deref())? else {
            // Nothing is schedulable: a wake says when to look again.
            self.loading.dirty = false;
            return Ok(());
        };
        let request = task.request(client.declared_models());
        let body = String::from_utf8(request.encode()?)
            .map_err(|_| invalid("a bootstrap request must be UTF-8"))?;
        self.requests += 1;
        self.loading.rotation = Some(task.state.scope.clone());
        self.bootstrap = Some(PendingBootstrap {
            id: self.requests,
            subscription_id: task.state.subscription_id,
            run: task.state.run,
            request,
        });
        actions.push(DownlinkAction::Request {
            request: self.requests,
            body,
            bootstrap: true,
        });
        Ok(())
    }

    /// Snapshot the desired Scopes, their subscription identities and the
    /// generation; with no Scope the session ends successfully and the lane
    /// stays idle until a subscribe wakes it. A Scope still waiting for its
    /// first boundary belongs to the set the socket asks for.
    fn begin<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        now: u64,
        actions: &mut Vec<DownlinkAction>,
    ) -> Result<()> {
        let desired = client.subscription_states()?;
        if desired.is_empty() {
            self.driver.complete(true, now, 0);
            return Ok(());
        }
        self.expected = desired
            .iter()
            .map(|s| (s.scope.clone(), s.subscription_id))
            .collect();
        let (epoch, subscribe) = self.session.begin(
            self.expected.keys().cloned().collect(),
            client.declared_models(),
            client.subscription_generation(),
        )?;
        actions.push(DownlinkAction::Open { epoch, subscribe });
        Ok(())
    }

    /// The handshake landed: one transaction commits the first delivery
    /// boundary of every subscription still waiting for one and leaves every
    /// initialized cursor where it is
    /// ([`Client::initialize_subscriptions`]). Status follows that commit and
    /// no queued page is applied before it, so the answer is `true` whenever a
    /// boundary landed and one pump still holds one commit. A channel behind
    /// its acknowledged head catches up over HTTP first; a head below a
    /// committed cursor is a server-state fault that ends the session.
    fn acknowledged<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        ack: &SubscriptionAck,
        now: u64,
        entropy: u64,
        actions: &mut Vec<DownlinkAction>,
    ) -> Result<bool> {
        let initialization = client.initialize_subscriptions(&self.expected, &ack.cursors)?;
        if let Some(reason) = initialization.fault {
            self.fail(client, Some(reason), now, entropy);
            return Ok(false);
        }
        let committed = !initialization.initialized.is_empty();
        if committed {
            actions.push(DownlinkAction::Changed {
                scopes: initialization.initialized,
            });
        }
        if !initialization.catch_up.is_empty() {
            self.pull(client, actions)?;
        }
        // Delivery is established for the acknowledged set, whether or not a
        // boundary was committed for any of it: the SDKs read it as `live`.
        actions.push(DownlinkAction::Acknowledged {
            scopes: ack.cursors.keys().cloned().collect(),
        });
        Ok(committed)
    }

    /// Recover every channel from its durable cursor after lost frames. A pull
    /// in flight keeps its progress and another follows it.
    fn recover<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        actions: &mut Vec<DownlinkAction>,
    ) -> Result<()> {
        self.pages.clear();
        self.pull(client, actions)
    }

    /// Issue one pull for every initialized subscription when none is in
    /// flight; otherwise remember that another is needed.
    fn pull<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        actions: &mut Vec<DownlinkAction>,
    ) -> Result<()> {
        if self.active.is_some() {
            self.again = true;
            return Ok(());
        }
        let Some(body) = client.downlink_request()? else {
            return Ok(());
        };
        self.requests += 1;
        self.active = Some(Pending {
            id: self.requests,
            request: PullRequest::decode(body.as_bytes())?,
        });
        actions.push(DownlinkAction::Request {
            request: self.requests,
            body,
            bootstrap: false,
        });
        Ok(())
    }

    /// Apply the answer to the request in flight; `true` when it committed. The
    /// round goes on from the durable cursors while any channel continues.
    fn response<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        body: &str,
        now: u64,
        entropy: u64,
        actions: &mut Vec<DownlinkAction>,
    ) -> Result<bool> {
        // The id was matched when the answer was enqueued; a second answer to
        // the same request finds nothing in flight and is ignored.
        let Some(pending) = self.active.take() else {
            return Ok(false);
        };
        let page = match PullPage::decode(body.as_bytes()) {
            Ok(page) => page,
            Err(e) => {
                self.fail(
                    client,
                    Some(format!("invalid pull response: {e}")),
                    now,
                    entropy,
                );
                return Ok(false);
            }
        };
        let progress = match client.receive_downlink(page, Some(pending.request)) {
            Ok(progress) => progress,
            Err(e) if e.to_string() == "response does not match pull request" => {
                self.fail(client, Some(e.to_string()), now, entropy);
                return Ok(false);
            }
            Err(e) => return Err(e),
        };
        settle(&progress, actions);
        if !progress.continues.is_empty() || std::mem::take(&mut self.again) {
            self.pull(client, actions)?;
        }
        Ok(progress.disposition == "applied")
    }
}
