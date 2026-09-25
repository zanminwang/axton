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
    /// The request this id names failed; `reason` is what the host reported.
    Failed {
        request: u64,
        #[serde(default)]
        reason: Option<String>,
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
    /// `POST /sync/pull` with `body`, one request for every subscribed channel;
    /// its answer is a `response` event of this id, a failure is `failed`.
    Request { request: u64, body: String },
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
    /// The one HTTP request in flight.
    active: Option<Pending>,
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
            }
            DownlinkEvent::Stop => {
                self.end(None);
                self.driver.stop();
            }
            DownlinkEvent::Pause => {
                if self.session.open() {
                    self.end(None);
                    self.driver.complete(true, now, 0);
                }
                self.driver.pause();
            }
            DownlinkEvent::Resume => self.driver.resume(now),
            DownlinkEvent::Wake => self.driver.wake(),
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
                if self.answers(request) {
                    self.control.push_back(Control::Response(body));
                }
            }
            DownlinkEvent::Failed { request, .. } => {
                // The host has reported the failure already; the session ends
                // and the lane retries with backoff.
                if self.answers(request) {
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
    /// attempt. The SDKs abandon the socket as soon as `subscribe` is called, so
    /// the dead socket is often reported before the commit's wake arrives.
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
        self.process(client, now, entropy, &mut actions)?;
        self.flush(&mut actions);
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
    fn process<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        now: u64,
        entropy: u64,
        actions: &mut Vec<DownlinkAction>,
    ) -> Result<()> {
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
                return Ok(());
            }
        }
        // Pages wait while a pull is in flight: its answer moves the cursors
        // they are measured against.
        while self.session.open() && self.active.is_none() {
            let Some(front) = self.pages.front().cloned() else {
                break;
            };
            let progress = client.receive_downlink(front, None)?;
            settle(&progress, actions);
            if progress.disposition == "recover" {
                // The gap stays at the front until a pull connects or covers it.
                return self.pull(client, actions);
            }
            self.pages.pop_front();
            if progress.disposition == "applied" {
                return Ok(());
            }
        }
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
