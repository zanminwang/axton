//! The Store worker protocol. Commands contain admitted facts, never sockets.
use super::DeliveryQueue;
use crate::{ApplyReport, store05::StoreStatus05, wire};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
#[derive(Clone)]
pub struct WorkFence05 {
    pub context: wire::RequestContext,
    pub epoch: u64,
    pub(crate) current: Arc<AtomicU64>,
}
impl WorkFence05 {
    pub fn current(&self) -> bool {
        self.current.load(Ordering::SeqCst) == self.epoch
    }
}
pub enum StoreCommand {
    Guarded {
        fence: WorkFence05,
        command: Box<StoreCommand>,
    },
    /// An admitted handshake: its head and the backend's projection generation.
    Initialize(wire::HandshakeResponse),
    NetworkState {
        live: bool,
        catching_up: bool,
    },
    Freeze,
    Acknowledge(wire::BatchAcknowledgement),
    Apply {
        plan_id: String,
        queue: DeliveryQueue,
        now: u64,
    },
    Cleanup(u64),
    Snapshot,
    Needs,
}
pub enum StoreReport {
    Guarded {
        fence: WorkFence05,
        report: Box<std::result::Result<StoreReport, String>>,
    },
    Obsolete,
    ActivePlans(Vec<String>),
    Snapshot(StoreStatus05),
    Needs {
        // Present only when settlement committed terminal Call progress.
        status: Option<StoreStatus05>,
    },
    Frozen(Option<wire::MutationRequest>),
    /// The uplink cannot continue from durable state; nothing retries it.
    Halted(String),
    Committed {
        status: StoreStatus05,
        report: ApplyReport,
        plan: Option<(String, u64)>,
        active_plans: Option<Vec<String>>,
    },
}
