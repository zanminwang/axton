//! The live session: subscribe over WebSocket, compare the acknowledged heads
//! with the durable cursors, catch up over HTTP only when behind, then consume
//! the stream through a gated in-memory frame queue. Rust decides; the host
//! owns sockets, HTTP, timers and credential refresh, and reports what
//! happened as events ([Live session](../../../docs/engineering/architecture/client/connection/controller/live-session.md)).
use crate::*;
use std::collections::VecDeque;

/// What the host tells the session. `now` and `entropy` travel beside the
/// event ([`LiveSession::handle`]); `epoch` names the session an I/O event
/// belongs to, so whatever an abandoned socket or request still delivers is
/// ignored.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum LiveEvent {
    /// The lane starts; the first session begins on the next `next`.
    Start,
    /// The lane stops for good: the session ends and nothing is scheduled.
    Stop,
    /// The session ends without backoff; nothing runs until `resume`.
    Pause,
    Resume,
    /// Something changed that may need a session (a subscription committed).
    Wake,
    /// The host's timer fired, or it wants the next decision.
    Next,
    /// A frame arrived on the socket of this epoch: the acknowledgement or a page.
    Message {
        epoch: u64,
        body: String,
    },
    /// The response to a `request` action of this epoch.
    CatchUp {
        epoch: u64,
        body: String,
    },
    /// The host's own frame buffer for this epoch overflowed and frames were
    /// dropped before they reached the session.
    Overflow {
        epoch: u64,
    },
    /// The socket of this epoch closed, or a request of it failed. The host has
    /// already reported the error and refreshed credentials if it chose to.
    Closed {
        epoch: u64,
    },
}

/// What the host does next, in order.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LiveAction {
    /// Open the socket and send `subscribe` once it is open. Frames it delivers
    /// are `message` events of this epoch; its end is `closed`.
    Open { epoch: u64, subscribe: String },
    /// `POST /sync/pull` with `body`, one request for every subscribed channel;
    /// the response is a `catchUp` event of this epoch, a failure is `closed`.
    Request { epoch: u64, body: String },
    /// Close the socket of this epoch and abandon its request, if any. A
    /// `reason` is a protocol violation the host reports as an error.
    Close { epoch: u64, reason: Option<String> },
    /// A page applied and may have settled a batch: wake the push lane.
    Wake { lane: &'static str },
    /// What the last page could not apply; the host hands it to the application.
    Report { reports: Vec<Report> },
    /// Nothing to do for `millis`; then report `next`.
    Wait { millis: u64 },
}

struct Session {
    epoch: u64,
    /// The subscription generation the channels were snapshotted under.
    generation: u64,
    subscribe: SubscribeRequest,
    acknowledged: bool,
    /// The one HTTP request in flight.
    active: Option<PullRequest>,
    /// Another pull is needed once the one in flight ends (an overflow while
    /// pulling: the lost frames may lie beyond the response).
    again: bool,
    /// Streamed pages not yet applied, in arrival order. The front is applied
    /// when every channel it names connects to its cursor; a page with a gap
    /// stays until a pull connects it or covers it.
    queue: VecDeque<PullPage>,
}

/// Frames held in the queue. Beyond this the queue is discarded whole and
/// every channel recovers from the durable cursor: the server log is the
/// durable queue, the cursor the pointer into it.
pub const QUEUED_FRAMES: usize = 64;

/// One live lane: [`ConnectionDriver`] scheduling around one session at a time.
#[derive(Default)]
pub struct LiveSession {
    driver: ConnectionDriver,
    epoch: u64,
    session: Option<Session>,
}

/// Collect one page's outcome into the actions: a wake when it applied,
/// its reports when it has any.
fn settle(progress: &DownlinkProgress, actions: &mut Vec<LiveAction>) {
    if progress.disposition == "applied" {
        actions.push(LiveAction::Wake { lane: "push" });
    }
    if !progress.report.reports.is_empty() {
        actions.push(LiveAction::Report {
            reports: progress.report.reports.clone(),
        });
    }
}

impl LiveSession {
    fn current(&self, epoch: u64) -> bool {
        self.session.as_ref().is_some_and(|s| s.epoch == epoch)
    }
    fn session(&mut self) -> &mut Session {
        self.session.as_mut().expect("current session")
    }

    /// End the session; the host closes its socket and abandons its request.
    fn end(&mut self, reason: Option<String>, actions: &mut Vec<LiveAction>) {
        if let Some(session) = self.session.take() {
            actions.push(LiveAction::Close {
                epoch: session.epoch,
                reason,
            });
        }
    }

    /// A protocol violation or a transport failure: the session ends and the
    /// lane retries with backoff.
    fn fail(
        &mut self,
        reason: Option<String>,
        now: u64,
        entropy: u64,
        actions: &mut Vec<LiveAction>,
    ) {
        self.end(reason, actions);
        self.driver.complete(false, now, entropy);
    }

    pub fn handle<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        event: LiveEvent,
        now: u64,
        entropy: u64,
    ) -> Result<Vec<LiveAction>> {
        let mut actions = vec![];
        // A committed subscribe or unsubscribe invalidates the session: the
        // lane starts over with the new channel set, without backoff.
        if self
            .session
            .as_ref()
            .is_some_and(|s| s.generation != client.subscription_generation())
        {
            self.end(None, &mut actions);
            self.driver.complete(true, now, 0);
            self.driver.wake();
        }
        match event {
            LiveEvent::Start => self.driver.start(now),
            LiveEvent::Stop => {
                self.end(None, &mut actions);
                self.driver.stop();
            }
            LiveEvent::Pause => {
                if self.session.is_some() {
                    self.end(None, &mut actions);
                    self.driver.complete(true, now, 0);
                }
                self.driver.pause();
            }
            LiveEvent::Resume => self.driver.resume(now),
            LiveEvent::Wake => self.driver.wake(),
            LiveEvent::Next => {}
            LiveEvent::Message { epoch, body } => {
                if self.current(epoch) {
                    self.message(client, &body, now, entropy, &mut actions)?;
                }
            }
            LiveEvent::CatchUp { epoch, body } => {
                if self.current(epoch) {
                    self.catch_up(client, &body, now, entropy, &mut actions)?;
                }
            }
            LiveEvent::Overflow { epoch } => {
                if self.current(epoch) {
                    self.overflow(client, &mut actions)?;
                }
            }
            LiveEvent::Closed { epoch } => {
                if self.current(epoch) {
                    self.fail(None, now, entropy, &mut actions);
                }
            }
        }
        if self.session.is_none() {
            match self.driver.next(now) {
                ConnectionAction::Sync => self.begin(client, now, &mut actions)?,
                ConnectionAction::Wait { millis } => actions.push(LiveAction::Wait { millis }),
                ConnectionAction::Idle => {}
            }
        }
        Ok(actions)
    }

    /// Snapshot the channels and the generation; with no channels the session
    /// ends successfully and the lane stays idle until a subscribe wakes it.
    fn begin<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        now: u64,
        actions: &mut Vec<LiveAction>,
    ) -> Result<()> {
        let channels: Vec<String> = client.desired_channels()?.into_iter().collect();
        if channels.is_empty() {
            self.driver.complete(true, now, 0);
            return Ok(());
        }
        let subscribe = SubscribeRequest::new(channels, client.declared_models())?;
        let frame = String::from_utf8(subscribe.encode()?).map_err(|_| invalid("utf8"))?;
        self.epoch += 1;
        self.session = Some(Session {
            epoch: self.epoch,
            generation: client.subscription_generation(),
            subscribe,
            acknowledged: false,
            active: None,
            again: false,
            queue: VecDeque::new(),
        });
        actions.push(LiveAction::Open {
            epoch: self.epoch,
            subscribe: frame,
        });
        Ok(())
    }

    fn message<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        body: &str,
        now: u64,
        entropy: u64,
        actions: &mut Vec<LiveAction>,
    ) -> Result<()> {
        let decoded = match LiveMessage::decode(body.as_bytes()) {
            Ok(decoded) => decoded,
            Err(e) => {
                self.fail(Some(e.to_string()), now, entropy, actions);
                return Ok(());
            }
        };
        let session = self.session();
        match decoded {
            LiveMessage::Acknowledged(ack) => {
                if session.acknowledged || !ack.confirms(&session.subscribe) {
                    self.fail(
                        Some("invalid live subscription acknowledgement".into()),
                        now,
                        entropy,
                        actions,
                    );
                    return Ok(());
                }
                session.acknowledged = true;
                // Every channel at its head: the stream is the truth from here.
                // Any channel behind: one pull from the durable cursors first.
                let mut behind = false;
                for (channel, head) in &ack.cursors {
                    // An uninitialized subscription has no cursor to be behind.
                    if client.cursor(channel)?.is_some_and(|cursor| *head > cursor) {
                        behind = true;
                    }
                }
                if behind {
                    self.pull(client, actions)?;
                }
                Ok(())
            }
            LiveMessage::Page(page) => {
                if !session.acknowledged {
                    self.fail(
                        Some("live page before acknowledgement".into()),
                        now,
                        entropy,
                        actions,
                    );
                    return Ok(());
                }
                if session.queue.len() >= QUEUED_FRAMES {
                    return self.overflow(client, actions);
                }
                session.queue.push_back(page);
                self.drain(client, actions)
            }
        }
    }

    /// Apply queued frames from the front while no pull is in flight. A frame
    /// that applied or is covered leaves the queue; a frame with a gap stays
    /// and one pull from the durable cursors runs.
    fn drain<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        actions: &mut Vec<LiveAction>,
    ) -> Result<()> {
        loop {
            let session = self.session();
            if session.active.is_some() {
                return Ok(());
            }
            let Some(front) = session.queue.front() else {
                return Ok(());
            };
            let progress = client.receive_downlink(front.clone(), None)?;
            settle(&progress, actions);
            if progress.disposition == "recover" {
                return self.pull(client, actions);
            }
            self.session().queue.pop_front();
        }
    }

    /// Frames were lost between the socket and the session; which channels
    /// they belonged to is unknown, so the queue is discarded and every
    /// channel recovers from its durable cursor. A pull in flight keeps its
    /// progress and another follows it.
    fn overflow<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        actions: &mut Vec<LiveAction>,
    ) -> Result<()> {
        let session = self.session();
        if !session.acknowledged {
            return Ok(());
        }
        session.queue.clear();
        self.pull(client, actions)
    }

    /// Issue one pull for every subscribed channel when none is in flight;
    /// otherwise remember that another is needed.
    fn pull<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        actions: &mut Vec<LiveAction>,
    ) -> Result<()> {
        let session = self.session();
        if session.active.is_some() {
            session.again = true;
            return Ok(());
        }
        let Some(body) = client.downlink_request()? else {
            return Ok(());
        };
        session.active = Some(PullRequest::decode(body.as_bytes())?);
        actions.push(LiveAction::Request {
            epoch: session.epoch,
            body,
        });
        Ok(())
    }

    fn catch_up<S: ClientStore>(
        &mut self,
        client: &mut Client<S>,
        body: &str,
        now: u64,
        entropy: u64,
        actions: &mut Vec<LiveAction>,
    ) -> Result<()> {
        let session = self.session();
        let Some(request) = session.active.take() else {
            return Err(invalid("catch-up response without a request"));
        };
        let page = match PullPage::decode(body.as_bytes()) {
            Ok(page) => page,
            Err(e) => {
                self.fail(
                    Some(format!("invalid pull response: {e}")),
                    now,
                    entropy,
                    actions,
                );
                return Ok(());
            }
        };
        let progress = match client.receive_downlink(page, Some(request)) {
            Ok(progress) => progress,
            Err(e) if e.to_string() == "response does not match pull request" => {
                self.fail(Some(e.to_string()), now, entropy, actions);
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        settle(&progress, actions);
        let session = self.session();
        if !progress.continues.is_empty() || std::mem::take(&mut session.again) {
            // The round goes on from the durable cursors until no channel continues.
            return self.pull(client, actions);
        }
        self.drain(client, actions)
    }
}
