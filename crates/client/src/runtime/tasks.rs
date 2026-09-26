//! The task table: request correlation, the ordinary FIFO, scheduling,
//! rebuild fencing and close
//! ([#134](https://github.com/zanminwang/axton/issues/134)).
use super::effects::Ready;
use super::transactions::Continuation;
use super::*;
use crate::ClientStore;
use std::collections::{BTreeSet, VecDeque};

/// Every request id still routed - queued, running, parked behind a callback
/// or waiting on the continuation lane - and the ordinary FIFO.
#[derive(Default)]
pub(super) struct Tasks {
    routed: BTreeSet<String>,
    queue: VecDeque<Queued>,
}
pub(super) struct Queued {
    pub(super) request_id: String,
    pub(super) command: Value,
}
impl Tasks {
    /// Route `request_id`; false when it is already routed, which the SDK's
    /// never-reused counter rules out unless it violates the contract.
    fn admit(&mut self, request_id: &str) -> bool {
        self.routed.insert(request_id.to_string())
    }
    pub(super) fn release(&mut self, request_id: &str) {
        self.routed.remove(request_id);
    }
}

impl<S: ClientStore + 'static> ClientRuntime<S> {
    /// Admit one input: queue, correlate or record it. Runs no database work,
    /// so control and callback answers are serviceable while a callback holds
    /// the transaction.
    pub fn receive(
        &mut self,
        input: Input,
        now: u64,
        entropy: u64,
    ) -> std::result::Result<(), BridgeError> {
        if self.lifecycle != Lifecycle::Open {
            return Err(BridgeError::Closed);
        }
        match input {
            Input::Task {
                request_id,
                command,
            } => {
                if self.admit(&request_id) {
                    self.tasks.queue.push_back(Queued {
                        request_id,
                        command,
                    });
                }
            }
            Input::TransactionCommand {
                request_id,
                transaction_id,
                scope,
                command,
            } => {
                if self.admit(&request_id) {
                    self.continue_transaction(Continuation {
                        request_id,
                        transaction_id,
                        scope,
                        command,
                    });
                }
            }
            Input::CallbackResult {
                effect_id,
                transaction_id,
                ok,
                error,
            } => self.callback_result(&effect_id, &transaction_id, ok, error),
            Input::EffectResult { effect_id, outcome } => {
                self.effect_result(effect_id, outcome, now, entropy)
            }
            Input::Close => self.lifecycle = Lifecycle::Closing,
        }
        Ok(())
    }
    /// Run at most one local unit and say whether anything happened. Close
    /// goes first; an open transaction's lane and result come before
    /// anything else, which waits while a callback owns the writer. Otherwise
    /// ordinary tasks and lane units take turns, so neither starves.
    pub fn step(&mut self, now: u64, entropy: u64) -> bool {
        match self.lifecycle {
            Lifecycle::Closed => return false,
            Lifecycle::Closing => {
                self.close();
                return true;
            }
            Lifecycle::Open => {}
        }
        if self.transaction.is_some() {
            return self.step_transaction();
        }
        let ordinary = !self.tasks.queue.is_empty();
        let lane = self.lane_ready();
        let run_lane = match (ordinary, lane) {
            (false, false) => return false,
            (true, false) => false,
            (false, true) => true,
            (true, true) => self.lane_turn,
        };
        self.lane_turn = !run_lane;
        if run_lane {
            self.lane_unit(now, entropy);
        } else {
            self.ordinary_unit(now);
        }
        true
    }
    fn lane_ready(&self) -> bool {
        !self.ready.is_empty()
            || self
                .connection
                .as_ref()
                .is_some_and(|c| c.downlink.dirty || c.push.dirty)
    }
    /// One lane unit: a ready continuation, else a Downlink pump, else a
    /// push-lane turn. A continuation that committed wakes both lanes.
    fn lane_unit(&mut self, now: u64, entropy: u64) {
        if let Some(ready) = self.ready.pop_front() {
            let generation = self.client.generation();
            match ready {
                Ready::PushReceipt { body } => self.push_receipt(body, now, entropy),
                Ready::ApplyDirect {
                    request_id,
                    response,
                } => self.apply_direct(request_id, response),
                Ready::PrerequisiteOutcome { key, error } => self.prerequisite_outcome(key, error),
                Ready::PrerequisiteNext => self.next_prerequisite(),
            }
            if self.client.generation() != generation {
                self.wake_lanes();
            }
            return;
        }
        let Some(connection) = &self.connection else {
            return;
        };
        if connection.downlink.dirty {
            self.downlink_turn(now, entropy);
        } else if connection.push.dirty {
            self.push_turn(now, entropy);
        }
    }
    /// One ordinary task. The runtime-owned lifecycles - the connection, direct
    /// calls, prerequisites and rebuild - are decided here; everything else is
    /// a command against the client. A task that committed wakes both lanes.
    fn ordinary_unit(&mut self, now: u64) {
        let Some(Queued {
            request_id,
            command,
        }) = self.tasks.queue.pop_front()
        else {
            return;
        };
        let kind = command["kind"].as_str().unwrap_or_default();
        if kind == "transaction" {
            self.open_transaction(request_id);
            return;
        }
        let connected = self.connection.is_some();
        let generation = self.client.generation();
        let outcome = match kind {
            "connect" => Some(self.connect(&command, now)),
            "connection" if connected => Some(self.control(&command, now)),
            "downlink" | "startSync" | "next" | "complete" if connected => {
                Some(Err(lanes::ALREADY_ACTIVE.into()))
            }
            "invoke" => self.invoke(&request_id, &command),
            "runPrerequisites" => self.run_prerequisites(&request_id, &command),
            "rebuild" => Some(self.rebuild(&command, now)),
            _ => Some(
                commands::execute(&mut self.client, &mut self.lanes, &command)
                    .map_err(|e| e.to_string()),
            ),
        };
        if kind != "rebuild" {
            self.changed_since(generation);
        }
        if self.client.generation() != generation {
            self.wake_lanes();
        }
        if let Some(outcome) = outcome {
            self.complete(request_id, outcome);
        }
    }
    /// `rebuild {discardPending?}` as the command answers it, plus the fence:
    /// everything in flight belongs to the replaced replica. Lane effects are
    /// cancelled and the lanes start over in the same intent, direct calls
    /// fail with an unknown execution, the prerequisite loop moves on, and
    /// every abandoned durable call is completed. A refused rebuild changes
    /// nothing.
    fn rebuild(&mut self, command: &Value, now: u64) -> std::result::Result<Value, String> {
        let report = commands::execute(&mut self.client, &mut self.lanes, command)
            .map_err(|e| e.to_string())?;
        self.fail_directs(direct::EXECUTION_UNKNOWN);
        self.rebuilt_prerequisites();
        self.rebuilt_lanes(now);
        self.events.push(Event::Changed {
            tables: self.client.last_changed().iter().cloned().collect(),
        });
        for abandoned in report["abandonedCalls"].as_array().into_iter().flatten() {
            let frozen = abandoned["frozen"].as_bool().unwrap_or(false);
            self.events.push(Event::CallCompleted {
                call_id: abandoned["callId"].as_str().unwrap_or_default().to_string(),
                outcome: json!({
                    "status": "failed",
                    "code": "abandoned",
                    "execution": if frozen { "unknown" } else { "rejected" },
                }),
            });
        }
        Ok(report)
    }
    fn admit(&mut self, request_id: &str) -> bool {
        if self.tasks.admit(request_id) {
            return true;
        }
        self.report(Diagnostic::Protocol {
            message: format!("duplicate request id {request_id}"),
        });
        false
    }
    /// Priority close: roll back the open session, cancel every outstanding
    /// effect, fail the parked parent, its queued commands and every queued
    /// task with `client_closed`, fail direct calls as unavailable and the
    /// prerequisite loop as closed, end the lanes, then announce the end.
    fn close(&mut self) {
        let transaction = self.transaction.take();
        if transaction.is_some()
            && let Err(e) = self.client.rollback_session()
        {
            self.error(format!("rollback at close failed: {e}"));
        }
        for effect_id in std::mem::take(&mut self.effects).into_keys() {
            self.events.push(Event::CancelEffect { effect_id });
        }
        if let Some(transaction) = transaction {
            self.complete(transaction.request_id, Err("client_closed".into()));
            for command in transaction.lane {
                self.complete(command.request_id, Err("client_closed".into()));
            }
        }
        for task in std::mem::take(&mut self.tasks.queue) {
            self.complete(task.request_id, Err("client_closed".into()));
        }
        self.fail_directs(direct::UNAVAILABLE);
        self.finish_prerequisites(Err("client_closed".into()));
        self.ready.clear();
        self.inbox.clear();
        self.close_lanes();
        self.lifecycle = Lifecycle::Closed;
        self.events.push(Event::RuntimeClosed);
    }
}
