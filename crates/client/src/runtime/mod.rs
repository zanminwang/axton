//! The Rust-owned client runtime: one per open client, generic over the
//! store, driven by an actor that owns no policy
//! ([#134](https://github.com/zanminwang/axton/issues/134)).
//!
//! The runtime owns the client, its local execution queue, the task
//! continuations, the one active application transaction, the connection
//! lanes and the observers. The host owns sockets, HTTP, timers, credentials,
//! language objects and application callback bodies; it learns what to do
//! from [`Event::Effect`]s and reports back with [`Input::EffectResult`]s.
//!
//! Three calls drive it, all non-blocking and all on the one thread that owns
//! the store:
//!
//! - [`ClientRuntime::receive`] admits one [`Input`]. It queues and correlates;
//!   it runs no database work, so an effect result or a close can be admitted
//!   while an application callback holds the transaction.
//! - [`ClientRuntime::step`] runs at most one local unit of work - one task,
//!   one transaction command, one commit or rollback, one pump of a lane - and
//!   says whether it did anything. The actor calls it until it answers `false`
//!   and blocks on its mailbox; the runtime never sleeps and never waits on
//!   the network inside a step, so a host timer is an effect like any other.
//! - [`ClientRuntime::take_events`] hands over what the last steps produced,
//!   in order. A task's completion is queued only after its transaction
//!   committed or rolled back, so an SDK that observes a success and reads the
//!   database sees the committed rows.
//!
//! `now` (milliseconds) and `entropy` are facts the actor supplies on every
//! call; deterministic tests pass their own.
//!
//! A unit that committed queues [`Event::Changed`] before its own
//! [`Event::TaskCompleted`], so an SDK that resolves the task and re-queries at
//! once has already heard about the change it is about to read.
//!
//! # Scheduling and transaction ownership
//!
//! Ordinary tasks form one FIFO. The first `transaction` task to run opens the
//! session (`begin_session`), allocates a `transactionId` and a callback
//! effect, and parks: from then on every ordinary task waits, whether it reads
//! or writes, because it must not see or join the open session. The callback's
//! own commands arrive as [`Input::TransactionCommand`]s bearing that id and
//! run on a continuation lane ahead of the parked queue, each completing its
//! own request. Nested savepoints keep a stack of runtime-issued `scope`
//! tokens; a command names the innermost open scope or fails without joining,
//! and a `release` / `rollbackSavepoint` pops only the top. Failures are
//! tracked as the SDK transaction objects track them today: a failed command
//! poisons the unit unless the savepoint it ran in rolls back, a
//! wrong-scope command is a structural failure, and the [`Input::CallbackResult`]
//! then commits (`ok` with nothing outstanding, no unreleased savepoint and no
//! recorded failure) or rolls back and fails the parent with the first failure.
//! A transaction command that arrives after the callback result, or that names
//! a transaction that is not open, fails with `transaction_closed`.
//!
//! [`Input::Close`] is priority control: it rolls back an open session, fails
//! its parent task and every queued task with `client_closed`, cancels every
//! outstanding effect, ends the observers and queues [`Event::RuntimeClosed`]
//! last. After it, [`ClientRuntime::receive`] answers [`BridgeError::Closed`].
//!
//! # Connection lanes and effects
//!
//! A `connect` task records the connection intent and starts both lanes: the
//! push lane (`ConnectionDriver` + `SyncCycle`) and the Downlink worker. From
//! then on the runtime decides every request, retry, refresh, cancellation and
//! report, and the host only executes effects: an HTTP post, a socket stream,
//! a timer, a credential refresh, a prerequisite handler. An effect result is
//! a fact; [`ClientRuntime::receive`] correlates it by `effectId` (a result for
//! an id that is not outstanding is ignored: that is the fence for cancelled,
//! duplicate and stale answers) and turns it into a *ready continuation* or a
//! Downlink worker event without touching the database. [`ClientRuntime::step`]
//! then runs one unit: the application transaction's own lane first, then it
//! alternates between one ordinary task and one *lane unit* - a ready
//! continuation (a receipt), else a
//! Downlink pump, else a push-lane turn - so neither starves. Each unit holds
//! at most one local transaction and none is held across an effect: prepare,
//! effect and apply are three units. Every ordinary task or continuation that
//! committed wakes both lanes, as the SDKs' `work`/`channels` events did.
//!
//! # Module layout
//!
//! - [`protocol`]: the envelopes shared with every SDK.
//! - `tasks`: the task table, the queues, request correlation, scheduling and
//!   close.
//! - `transactions`: the active transaction, its capability tokens, savepoint
//!   stack, failure accounting and the callback effect.
//! - `effects`: the effect table, result correlation, ready continuations and
//!   the one credential refresh the lanes share.
//! - `lanes`: the connection intent, its controls, the push lane and the
//!   Downlink worker as runtime work.
//! - `commands`: the command set: the local reads, writes, Scope, status and
//!   sync commands executed against the client. The former host-driven lane
//!   commands (`connection` lifecycle events, `downlink`, `startSync`,
//!   `next`, `complete`, …) still execute for one more checkpoint of #134 and
//!   are refused while a runtime-owned connection is active.
mod commands;
mod effects;
mod lanes;
pub mod protocol;
mod tasks;
mod transactions;

pub use protocol::*;

use crate::{Client, ClientStore, DownlinkEvent, Result, Schema, StoreFactory};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

/// One open client and everything the runtime decided about it. See the
/// module documentation for the contract of the three driving calls.
pub struct ClientRuntime<S: ClientStore> {
    client: Client<S>,
    lanes: lanes::Lanes,
    /// The runtime-owned connection: its intent, lane effects and credential
    /// refresh. `None` until `connect` and after `stop`.
    connection: Option<lanes::Connection>,
    tasks: tasks::Tasks,
    transaction: Option<transactions::Transaction>,
    /// Every effect the host may still answer, by id, and what it was issued
    /// for. A result for an id that is not here is ignored.
    effects: BTreeMap<String, effects::EffectKind>,
    /// Effect results turned into local work, one unit each, in arrival order.
    ready: VecDeque<effects::Ready>,
    /// Downlink worker events admitted since the last pump; fed to the worker
    /// right before it pumps. The worker's own page queue is the bound.
    inbox: VecDeque<DownlinkEvent>,
    /// Whether the next step prefers a lane unit over an ordinary task.
    lane_turn: bool,
    /// The one counter behind `transactionId`, `scope` and `effectId`: every
    /// identity the runtime issues is fresh for its lifetime.
    issued: u64,
    events: Vec<Event>,
    lifecycle: Lifecycle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Open,
    /// Close was admitted; the next step performs it ahead of any other work.
    Closing,
    Closed,
}

impl<S: ClientStore + 'static> ClientRuntime<S> {
    /// [`Client::open_at`] under the runtime. Errors are the open errors.
    pub fn open_at(
        path: impl AsRef<Path>,
        schema: Schema,
        factory: StoreFactory<S>,
        discard_pending: bool,
    ) -> Result<Self> {
        Ok(Self::new(Client::open_at(
            path,
            schema,
            factory,
            discard_pending,
        )?))
    }
    /// Wrap an already opened client.
    pub fn new(client: Client<S>) -> Self {
        Self {
            client,
            lanes: lanes::Lanes::default(),
            connection: None,
            tasks: tasks::Tasks::default(),
            transaction: None,
            effects: BTreeMap::new(),
            ready: VecDeque::new(),
            inbox: VecDeque::new(),
            lane_turn: false,
            issued: 0,
            events: vec![],
            lifecycle: Lifecycle::Open,
        }
    }
    /// What a successful open answers: the client id and the schema check's
    /// outcome, as the SDKs report it in `status()`.
    pub fn opened(&self) -> Value {
        json!({
            "clientId": self.client.client_id(),
            "schema": commands::schema_json(self.client.schema_state()),
        })
    }
    /// The events queued since the last call, in order.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
    /// Whether [`Event::RuntimeClosed`] has been queued.
    pub fn closed(&self) -> bool {
        self.lifecycle == Lifecycle::Closed
    }
    /// Test seam: the client, for inspecting committed state in Rust tests.
    pub fn client(&mut self) -> &mut Client<S> {
        &mut self.client
    }
}

impl<S: ClientStore> ClientRuntime<S> {
    /// A fresh number for an identity the runtime issues. Never reused; an
    /// exhausted counter refuses rather than wraps.
    fn issue(&mut self) -> std::result::Result<u64, String> {
        self.issued = self
            .issued
            .checked_add(1)
            .ok_or_else(|| "runtime identifiers exhausted".to_string())?;
        Ok(self.issued)
    }
    /// Settle one routed request: the only place a [`Event::TaskCompleted`]
    /// is queued, so a request is completed at most once.
    fn complete(&mut self, request_id: String, outcome: std::result::Result<Value, String>) {
        self.tasks.release(&request_id);
        self.events.push(match outcome {
            Ok(value) => Event::TaskCompleted {
                request_id,
                ok: true,
                value,
                error: None,
            },
            Err(error) => Event::TaskCompleted {
                request_id,
                ok: false,
                value: Value::Null,
                error: Some(error),
            },
        });
    }
    /// Queue [`Event::Changed`] when a unit committed since `generation`.
    fn changed_since(&mut self, generation: u64) {
        if self.client.generation() != generation {
            self.events.push(Event::Changed {
                tables: self.client.last_changed().iter().cloned().collect(),
            });
        }
    }
    fn report(&mut self, diagnostic: Diagnostic) {
        self.events.push(Event::Report { diagnostic });
    }
    fn error(&mut self, message: impl Into<String>) {
        self.report(Diagnostic::Error {
            message: message.into(),
        });
    }
    fn signal(&mut self, signal: Value) {
        self.events.push(Event::LaneSignal { signal });
    }
}
