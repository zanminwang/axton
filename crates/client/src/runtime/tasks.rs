//! The task table: request correlation, the ordinary FIFO, scheduling and
//! close ([#134](https://github.com/zanminwang/axton/issues/134)).
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
        _now: u64,
        _entropy: u64,
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
            // Checkpoint 1 issues only callback effects, which are answered by
            // `callbackResult`: any effect result is fenced here, whether the
            // id is outstanding or was never issued.
            Input::EffectResult { .. } => {}
            Input::Close => self.lifecycle = Lifecycle::Closing,
        }
        Ok(())
    }
    /// Run at most one local unit and say whether anything happened. Close
    /// goes first; an open transaction's lane and result come before the
    /// ordinary FIFO, which waits while a callback owns the writer.
    pub fn step(&mut self, _now: u64, _entropy: u64) -> bool {
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
        let Some(Queued {
            request_id,
            command,
        }) = self.tasks.queue.pop_front()
        else {
            return false;
        };
        if command["kind"] == "transaction" {
            self.open_transaction(request_id);
            return true;
        }
        let generation = self.client.generation();
        let outcome = commands::execute(&mut self.client, &mut self.lanes, &command)
            .map_err(|e| e.to_string());
        self.changed_since(generation);
        self.complete(request_id, outcome);
        true
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
    /// task with `client_closed`, then announce the end.
    fn close(&mut self) {
        let transaction = self.transaction.take();
        if transaction.is_some()
            && let Err(e) = self.client.rollback_session()
        {
            self.report(Diagnostic::Error {
                message: format!("rollback at close failed: {e}"),
            });
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
        self.lifecycle = Lifecycle::Closed;
        self.events.push(Event::RuntimeClosed);
    }
}
