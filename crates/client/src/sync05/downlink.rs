//! Network control, independent of the writer. Inputs are correlated and staged
//! here; only Store reports can advance committed progress or settle Calls.
use super::{DeliveryQueue, StoreCommand, StoreReport};
use crate::{
    Result, invalid,
    runtime::{Diagnostic, EffectOutcome, Event, HttpRoute, Operation, SocketEvent},
    store05::StoreStatus05,
    wire,
};
use std::collections::{BTreeMap, VecDeque};
#[derive(Clone)]
enum Flight {
    Handshake(wire::HandshakeRequest),
    Delta(wire::DeltaRequest),
    Push(wire::MutationRequest),
    Socket,
    Expire,
    Retry,
    Refresh,
}
pub struct Control {
    context: wire::RequestContext,
    status: StoreStatus05,
    queue: DeliveryQueue,
    flights: BTreeMap<String, Flight>,
    issued: u64,
    connected: bool,
    paused: bool,
    live: bool,
    network_state: Option<(bool, bool)>,
    /// Highest handshake, delivery or accepted receipt head.
    delivery_head: u64,
    bootstrap_prefetch: bool,
    events: Vec<Event>,
    jobs: VecDeque<StoreCommand>,
    applying: bool,
    applying_plan: Option<String>,
    freezing: bool,
    wake_after_needs: bool,
    waiting_push: Option<wire::MutationRequest>,
    failures: u32,
    epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
    refresh_auth: bool,
    refreshing: bool,
    refresh_waiters: Vec<Flight>,
}
impl Control {
    pub fn new(status: StoreStatus05) -> Self {
        Self::with_bootstrap_prefetch(status, true)
    }
    /// Override default one-page lookahead. Received positions never become Store progress.
    pub fn with_bootstrap_prefetch(status: StoreStatus05, enabled: bool) -> Self {
        Self {
            context: status.context.clone(),
            delivery_head: status.cursor.unwrap_or(0),
            bootstrap_prefetch: enabled,
            status,
            queue: DeliveryQueue::new(16 * 1024 * 1024, 32),
            flights: BTreeMap::new(),
            issued: 0,
            connected: false,
            paused: false,
            live: false,
            network_state: None,
            events: vec![],
            jobs: VecDeque::new(),
            applying: false,
            applying_plan: None,
            freezing: false,
            wake_after_needs: false,
            waiting_push: None,
            failures: 0,
            epoch: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)),
            refresh_auth: false,
            refreshing: false,
            refresh_waiters: vec![],
        }
    }
    pub fn set_refresh_auth(&mut self, enabled: bool) {
        self.refresh_auth = enabled;
    }
    pub fn connect(&mut self) -> Result<()> {
        self.connected = true;
        self.paused = false;
        self.handshake()
    }
    fn effect(&mut self, flight: Flight, operation: Operation) {
        self.issued += 1;
        let id = format!("control05:{}", self.issued);
        self.flights.insert(id.clone(), flight);
        self.events.push(Event::Effect {
            effect_id: id,
            operation,
        });
    }
    fn http<T: serde::Serialize>(
        &mut self,
        flight: Flight,
        route: HttpRoute,
        request: &T,
    ) -> Result<()> {
        let body = serde_json::to_string(request)?;
        self.effect(flight, Operation::Http { route, body });
        Ok(())
    }
    fn handshake(&mut self) -> Result<()> {
        self.retire_bootstrap_prefetch();
        self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.applying = false;
        self.applying_plan = None;
        self.freezing = false;
        self.wake_after_needs = false;
        self.waiting_push = None;
        let old = self
            .flights
            .iter()
            .filter(|(_, f)| {
                matches!(
                    f,
                    Flight::Socket | Flight::Delta(_) | Flight::Handshake(_) | Flight::Push(_)
                )
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in old {
            self.flights.remove(&id);
            self.events.push(Event::CancelEffect { effect_id: id });
        }
        let request = wire::HandshakeRequest {
            protocol: wire::PROTOCOL,
            store_id: self.context.store_id.clone(),
            stream: self.context.stream.clone(),
        };
        self.http(
            Flight::Handshake(request.clone()),
            HttpRoute::Handshake,
            &request,
        )
    }
    pub fn stop(&mut self) {
        self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.applying = false;
        self.applying_plan = None;
        self.connected = false;
        self.live = false;
        self.network_state = None;
        for id in self.flights.keys() {
            self.events.push(Event::CancelEffect {
                effect_id: id.clone(),
            });
        }
        self.flights.clear();
        self.jobs.clear();
        self.freezing = false;
        self.wake_after_needs = false;
        self.waiting_push = None;
        self.refreshing = false;
        self.refresh_waiters.clear();
    }
    pub fn pause(&mut self) {
        self.stop();
        self.paused = true;
    }
    pub fn wake(&mut self) {
        if self.connected
            && !self.wake_after_needs
            && !self.freezing
            && !self.flights.values().any(|f| matches!(f, Flight::Push(_)))
        {
            self.freezing = true;
            self.jobs.push_back(StoreCommand::Freeze)
        }
    }
    pub fn accepts(&self, id: &str) -> bool {
        id.starts_with("control05:")
    }
    pub fn events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
    pub fn jobs(&mut self) -> Vec<StoreCommand> {
        let fence = super::WorkFence05 {
            context: self.context.clone(),
            epoch: self.epoch.load(std::sync::atomic::Ordering::SeqCst),
            current: self.epoch.clone(),
        };
        self.jobs
            .drain(..)
            .map(|command| StoreCommand::Guarded {
                fence: fence.clone(),
                command: Box::new(command),
            })
            .collect()
    }
    pub fn receive(&mut self, id: &str, outcome: EffectOutcome, now: u64) -> Result<()> {
        let Some(flight) = self.flights.get(id).cloned() else {
            return Ok(());
        };
        if !matches!(flight, Flight::Socket) {
            self.flights.remove(id);
        }
        if !outcome.ok {
            if let Some(error) = &outcome.error
                && let (Some(status), Some(body)) = (error.status, &error.refusal)
            {
                let diagnostic = Diagnostic::Refused {
                    message: error.message.clone(),
                    status,
                    body: serde_json::from_str(body)
                        .unwrap_or_else(|_| serde_json::Value::String(body.clone())),
                };
                self.stop();
                self.events.push(Event::Report { diagnostic });
                return Ok(());
            }
            if outcome
                .error
                .as_ref()
                .is_some_and(|e| e.status == Some(401))
                && self.refresh_auth
                && !matches!(flight, Flight::Refresh)
            {
                if let Some(error) = &outcome.error {
                    self.events.push(Event::Report {
                        diagnostic: Diagnostic::Error {
                            message: error.message.clone(),
                            status: error.status,
                        },
                    });
                }
                self.flights.remove(id);
                self.refresh_waiters.push(flight);
                if !self.refreshing {
                    self.refreshing = true;
                    self.effect(Flight::Refresh, Operation::RefreshAuth);
                }
                return Ok(());
            }
            if matches!(flight, Flight::Refresh) {
                self.refreshing = false;
                self.refresh_waiters.clear();
            }
            self.flights.remove(id);
            let (message, status) = outcome
                .error
                .map_or(("network failed".into(), None), |e| (e.message, e.status));
            self.retry_status(message, status);
            return Ok(());
        }
        if matches!(flight, Flight::Refresh) {
            self.refreshing = false;
            for waiter in std::mem::take(&mut self.refresh_waiters) {
                self.resend(waiter)?;
            }
            return Ok(());
        }
        if matches!(flight, Flight::Expire) {
            self.jobs.push_back(StoreCommand::Cleanup(now));
            return self.schedule(now);
        }
        if matches!(flight, Flight::Retry) {
            self.queue.retry_blocked();
            return self.handshake();
        }
        if matches!(flight, Flight::Socket) {
            match serde_json::from_value::<SocketEvent>(outcome.value.unwrap_or_default())? {
                SocketEvent::Opened => {
                    self.live = true;
                    self.schedule(now)?;
                }
                SocketEvent::Closed | SocketEvent::Overflow => {
                    self.flights.remove(id);
                    self.live = false;
                    // Missing socket fragments cannot be fetched from a server
                    // cache. Retire incomplete ordinary responses immediately;
                    // repair reconstructs current state from committed B/C.
                    self.queue.plans.retain(|_, plan| {
                        (plan.next..plan.header.units.len() as u64)
                            .all(|index| plan.complete(index))
                    });
                    self.schedule(now)?;
                    self.retry("socket closed or overflow".into());
                }
                SocketEvent::Message { body } => {
                    let raw: serde_json::Value = serde_json::from_str(&body)?;
                    if raw.get("head").is_some() && raw.get("header").is_none() {
                        let ack = wire::decode::<wire::HandshakeResponse>(body.as_bytes())?;
                        ack.admit(&wire::HandshakeRequest {
                            protocol: wire::PROTOCOL,
                            store_id: self.context.store_id.clone(),
                            stream: self.context.stream.clone(),
                        })?;
                        self.delivery_head = self.delivery_head.max(ack.head);
                        // The socket handshake reports the generation too; one
                        // Store path adopts it like the HTTP handshake's.
                        self.jobs.push_back(StoreCommand::Initialize(ack));
                        self.schedule(now)?;
                        return Ok(());
                    }
                    let response: wire::DeliveryResponse = serde_json::from_str(&body)?;
                    // Socket offers are finite ranges. Authentication fixed the
                    // Stream; local Store/context and plan still need admission.
                    if response.header.bootstrap {
                        return Err(invalid("invalid socket delivery purpose"));
                    }
                    response.header.context.admit_store(&self.context)?;
                    if response.header.context.materialization != self.context.materialization {
                        wire::Validate::validate(&response.header)?;
                        self.delivery_head = self.delivery_head.max(response.header.observed_head);
                        self.schedule(now)?;
                        return Ok(());
                    }
                    // An idle receiver can apply a live offer immediately. While
                    // ordinary delivery is queued, coalesce new
                    // offers into the latest head instead of buffering a response
                    // per commit. Fresh HTTP repair starts at committed B/C.
                    // An HTTP wait alone must not block an eligible live offer.
                    // Fragments of an already admitted response still complete it.
                    let cursor = self.status.cursor.unwrap_or(0);
                    let advances_prefix =
                        response.header.after <= cursor && response.header.through > cursor;
                    if !self.queue.plans.contains_key(&response.header.plan_id)
                        && (!self.queue.plans.is_empty()
                            || (!advances_prefix
                                && self.flights.values().any(|f| matches!(f, Flight::Delta(_)))))
                    {
                        DeliveryQueue::validate_offer(
                            &response.header,
                            &response.parts,
                            &self.context,
                            now,
                        )?;
                        let _span = tracing::info_span!(target: "axton::client", "delivery.coalesce", observedHead = response.header.observed_head, committedCursor = self.status.cursor.unwrap_or(0)).entered();
                        self.delivery_head = self.delivery_head.max(response.header.observed_head);
                        self.schedule(now)?;
                        return Ok(());
                    }
                    self.queue
                        .receive(&response.header, &response.parts, &self.context, now)?;
                    self.delivery_head = self.delivery_head.max(response.header.observed_head);
                    self.schedule(now)?;
                }
            }
            return Ok(());
        }
        let body = match outcome.value {
            Some(serde_json::Value::String(body)) => body,
            Some(serde_json::Value::Object(v)) => v
                .get("body")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid("HTTP body required"))?
                .into(),
            _ => return Err(invalid("HTTP body required")),
        };
        match flight {
            Flight::Handshake(request) => {
                let response = wire::decode::<wire::HandshakeResponse>(body.as_bytes())?;
                response.admit(&request)?;
                self.delivery_head = self.delivery_head.max(response.head);
                self.failures = 0;
                self.jobs.push_back(StoreCommand::Initialize(response));
                // Socket reception is live immediately, before Store SQL runs.
                self.effect(
                    Flight::Socket,
                    Operation::Socket {
                        subscribe: serde_json::to_string(&request)?,
                    },
                );
            }
            Flight::Delta(request) => {
                let response: wire::DeliveryResponse = serde_json::from_str(&body)?;
                response.admit(&request, &self.context)?;
                response.require_complete()?;
                self.delivery_head = self.delivery_head.max(response.header.observed_head);
                let speculative =
                    request.bootstrap && request.after > self.status.bootstrap_cursor.unwrap_or(0);
                if speculative {
                    if !self.queue.receive_prefetched(
                        &response.header,
                        &response.parts,
                        &self.context,
                        now,
                    )? {
                        // Retry from committed B after the current page finishes.
                        // Do not repeatedly fetch a successor that cannot fit.
                        self.bootstrap_prefetch = false;
                    }
                } else {
                    self.queue
                        .receive(&response.header, &response.parts, &self.context, now)?;
                }
                self.schedule(now)?;
            }
            Flight::Push(request) => {
                let receipt = wire::decode::<wire::BatchAcknowledgement>(body.as_bytes())?;
                if receipt.context != request.context
                    || receipt.batch_id != request.batch_id
                    || receipt.digest != request.digest
                {
                    return Err(invalid("Batch acknowledgement correlation mismatch"));
                }
                // Settlement evidence arrives only through the Stream. Raising
                // the head lets an idle socket's Store pull (C, syncCursor].
                for result in &receipt.results {
                    if let wire::MutationOutcome::Accepted { sync_cursor, .. } = result.outcome {
                        self.delivery_head = self.delivery_head.max(sync_cursor);
                    }
                }
                self.jobs.push_back(StoreCommand::Acknowledge(receipt));
            }
            _ => {}
        }
        Ok(())
    }
    fn resend(&mut self, flight: Flight) -> Result<()> {
        match flight {
            Flight::Handshake(request) => self.http(
                Flight::Handshake(request.clone()),
                HttpRoute::Handshake,
                &request,
            ),
            Flight::Delta(request) => {
                self.http(Flight::Delta(request.clone()), HttpRoute::Pull, &request)
            }
            Flight::Push(request) => {
                self.http(Flight::Push(request.clone()), HttpRoute::Push, &request)
            }
            Flight::Socket => self.handshake(),
            _ => Ok(()),
        }
    }
    fn retry(&mut self, message: String) {
        self.retry_status(message, None)
    }
    fn retry_status(&mut self, message: String, status: Option<u16>) {
        self.events.push(Event::Report {
            diagnostic: Diagnostic::Error { message, status },
        });
        if self.connected && !self.flights.values().any(|f| matches!(f, Flight::Retry)) {
            self.failures = self.failures.saturating_add(1);
            let millis = (250u64.saturating_mul(1u64 << self.failures.min(7))).min(30000);
            self.effect(Flight::Retry, Operation::Timer { millis });
        }
    }
    pub fn network_error(&mut self, message: String) {
        self.retry(message)
    }
    pub fn failed(&mut self, message: String) {
        self.retire_bootstrap_prefetch();
        if let Some(id) = self.applying_plan.take()
            && let Some(p) = self.queue.plans.get_mut(&id)
        {
            p.blocked = true;
        }
        self.applying = false;
        self.freezing = false;
        self.retry(message)
    }
    pub fn report(&mut self, report: StoreReport, now: u64) -> Result<()> {
        match report {
            StoreReport::Obsolete => return Ok(()),
            StoreReport::Guarded { fence, report } => {
                if !fence.current() || fence.context != self.context {
                    return Ok(());
                }
                return match *report {
                    Ok(report) => self.report(report, now),
                    Err(message) => {
                        self.failed(message);
                        Ok(())
                    }
                };
            }
            StoreReport::Snapshot(status) => {
                if status.context != self.context {
                    // A reset or an upgrade put the Store in a new context.
                    // Staging of the old one is void.
                    self.queue = DeliveryQueue::new(16 * 1024 * 1024, 32);
                    self.applying = false;
                    self.applying_plan = None;
                    if status.context.store_id == self.context.store_id {
                        // An upgrade: the same Store in a new materialization.
                        // Pulls and applies of the old context are dropped and
                        // a Freeze in flight returns obsolete; a push in flight
                        // still acknowledges. Scheduling continues from the
                        // committed positions with no second handshake.
                        let pulls = self
                            .flights
                            .iter()
                            .filter(|(_, f)| matches!(f, Flight::Delta(r) if r.context != status.context))
                            .map(|(id, _)| id.clone())
                            .collect::<Vec<_>>();
                        for id in pulls {
                            self.flights.remove(&id);
                            self.events.push(Event::CancelEffect { effect_id: id });
                        }
                        self.jobs
                            .retain(|job| !matches!(job, StoreCommand::Apply { .. }));
                        // A pull parked for an auth refresh would be resent in
                        // the old context and fail admission.
                        self.refresh_waiters.retain(
                            |flight| !matches!(flight, Flight::Delta(r) if r.context != status.context),
                        );
                        self.freezing = false;
                        self.waiting_push = None;
                    } else {
                        let reconnect = self.connected;
                        self.stop();
                        self.delivery_head = status.cursor.unwrap_or(0);
                        self.context = status.context.clone();
                        if reconnect {
                            self.connect()?;
                        }
                    }
                }
                self.context = status.context.clone();
                self.status = status;
                self.wake_after_needs = true;
                self.jobs.push_back(StoreCommand::Needs);
            }
            StoreReport::Needs { status } => {
                if let Some(status) = status {
                    self.status = status;
                    // Receipt acknowledgement can freeze before settlement makes
                    // lifecycle dependents ready. Wake only after durable progress.
                    self.wake_after_needs = true;
                }
                let wake = std::mem::take(&mut self.wake_after_needs);
                if let Some(request) = self.waiting_push.take() {
                    self.http(Flight::Push(request.clone()), HttpRoute::Push, &request)?;
                }
                if wake {
                    self.wake();
                }
            }
            StoreReport::Frozen(request) => {
                self.freezing = false;
                if let Some(request) = request {
                    if self.wake_after_needs {
                        self.waiting_push = Some(request);
                    } else {
                        self.http(Flight::Push(request.clone()), HttpRoute::Push, &request)?;
                    }
                }
            }
            StoreReport::Halted(message) => {
                // Report once and stop: a retry would hit the same state.
                self.stop();
                self.events.push(Event::Report {
                    diagnostic: Diagnostic::Error {
                        message,
                        status: None,
                    },
                });
                return Ok(());
            }
            StoreReport::ActivePlans(active) => {
                self.queue
                    .plans
                    .retain(|id, p| p.next == 0 || active.contains(id));
            }
            StoreReport::Committed {
                status,
                plan,
                active_plans,
                ..
            } => {
                if let Some(active) = active_plans {
                    self.queue
                        .plans
                        .retain(|id, p| p.next == 0 || active.contains(id));
                }
                self.status = status;
                self.wake_after_needs = true;
                self.jobs.push_back(StoreCommand::Needs);
                if let Some((id, next)) = plan {
                    self.applying = false;
                    self.applying_plan = None;
                    if let Some(p) = self.queue.plans.get_mut(&id) {
                        p.next = next;
                        p.parts.retain(|(unit, _), _| *unit >= next);
                        if next >= p.header.units.len() as u64 {
                            self.queue.plans.remove(&id);
                        }
                    }
                }
            }
        }
        self.schedule(now)
    }
    fn retire_bootstrap_prefetch(&mut self) {
        let committed = self.status.bootstrap_cursor.unwrap_or(0);
        let future = self
            .flights
            .iter()
            .filter_map(|(id, flight)| match flight {
                Flight::Delta(r) if r.bootstrap && r.after > committed => Some(id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        for id in future {
            self.flights.remove(&id);
            self.events.push(Event::CancelEffect { effect_id: id });
        }
        self.refresh_waiters.retain(
            |flight| !matches!(flight, Flight::Delta(r) if r.bootstrap && r.after > committed),
        );
        self.queue
            .plans
            .retain(|_, p| !p.header.bootstrap || p.header.after <= committed);
    }
    fn bootstrap_prefetch_after(&self, committed: u64, through: u64) -> Option<u64> {
        if !self.bootstrap_prefetch {
            return None;
        }
        let mut pages = self.queue.plans.values().filter(|p| {
            p.header.bootstrap && p.header.context == self.context && p.header.through > committed
        });
        let current = pages.next()?;
        if pages.next().is_some()
            || current.blocked
            || current.header.after > committed
            || (current.next..current.header.units.len() as u64)
                .any(|index| !current.complete(index))
        {
            return None;
        }
        let end = current.header.through;
        (end > committed && end < through).then_some(end)
    }
    fn schedule(&mut self, now: u64) -> Result<()> {
        if !self.connected || self.paused {
            return Ok(());
        }
        let before = self.queue.len();
        self.queue.expire(now);
        if before != self.queue.len() {
            self.jobs.push_back(StoreCommand::Cleanup(now));
        }
        if let Some(expires) = self.queue.plans.values().map(|p| p.header.expires_at).min()
            && !self.flights.values().any(|f| matches!(f, Flight::Expire))
        {
            self.effect(
                Flight::Expire,
                Operation::Timer {
                    millis: expires.saturating_sub(now).max(1),
                },
            );
        }
        let state = (
            self.live,
            self.status.cursor.is_none_or(|c| c < self.delivery_head),
        );
        if self.network_state != Some(state) {
            self.network_state = Some(state);
            self.jobs.push_back(StoreCommand::NetworkState {
                live: state.0,
                catching_up: state.1,
            });
        }
        if !self.applying {
            for (id, p) in &self.queue.plans {
                if p.blocked
                    || self.queue.plans.values().any(|blocked| {
                        blocked.blocked && blocked.header.bootstrap == p.header.bootstrap
                    })
                {
                    continue;
                }
                let position = if p.header.bootstrap {
                    self.status.bootstrap_cursor.unwrap_or(0)
                } else {
                    self.status.cursor.unwrap_or(0)
                };
                if p.header.after > position {
                    continue;
                }
                if p.unit(p.next)?.is_some() {
                    let mut queue = DeliveryQueue::new(16 * 1024 * 1024, 32);
                    queue.plans.insert(id.clone(), p.worker_snapshot());
                    self.jobs.push_back(StoreCommand::Apply {
                        plan_id: id.clone(),
                        queue,
                        now,
                    });
                    self.applying = true;
                    self.applying_plan = Some(id.clone());
                    break;
                }
            }
        }
        if self
            .flights
            .values()
            .chain(self.refresh_waiters.iter())
            .any(|f| matches!(f, Flight::Delta(_)))
        {
            return Ok(());
        }
        if self.status.start_cursor.is_none() {
            return Ok(());
        }
        let bootstrap = self.status.bootstrap_cursor != self.status.start_cursor;
        let after = if bootstrap {
            self.status.bootstrap_cursor.unwrap_or(0)
        } else {
            self.status.cursor.unwrap_or(0)
        };
        let through = if bootstrap {
            self.status.start_cursor.unwrap()
        } else {
            self.delivery_head
        };
        if self.queue.plans.values().any(|p| {
            p.header.bootstrap == bootstrap && p.header.after <= after && p.header.through >= after
        }) {
            if bootstrap && let Some(after) = self.bootstrap_prefetch_after(after, through) {
                let request = wire::DeltaRequest {
                    context: self.context.clone(),
                    after,
                    through,
                    bootstrap: true,
                };
                self.http(Flight::Delta(request.clone()), HttpRoute::Pull, &request)?;
            }
            return Ok(());
        }
        if bootstrap || after < through {
            let request = wire::DeltaRequest {
                context: self.context.clone(),
                after,
                through,
                bootstrap,
            };
            self.http(Flight::Delta(request.clone()), HttpRoute::Pull, &request)?;
        }
        Ok(())
    }
}
