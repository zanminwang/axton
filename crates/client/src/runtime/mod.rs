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
//! # Module layout
//!
//! - [`protocol`]: the envelopes shared with every SDK.
//! - `tasks`: the task table, the queues, request correlation and close.
//! - `transactions`: the active transaction, its capability tokens, savepoint
//!   stack, failure accounting and the callback effect.
//! - `commands`: the command set: the local reads, writes, Scope, status and
//!   sync commands executed against the client. While the checkpoints of #134
//!   land, the lane and direct-call commands of the former `RuntimeHost`
//!   remain here as tasks; the runtime replaces them with owned lifecycles
//!   and effects in its later checkpoints.
pub mod protocol;

pub use protocol::*;
