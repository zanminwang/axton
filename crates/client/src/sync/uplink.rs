//! The Store worker protocol. Commands contain admitted facts, never sockets.
use super::DeliveryQueue;
use crate::{ApplyReport, store_file::StoreStatus, wire};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
#[derive(Clone)]
pub struct WorkFence {
    pub context: wire::RequestContext,
    pub epoch: u64,
    pub(crate) current: Arc<AtomicU64>,
}
impl WorkFence {
    pub fn current(&self) -> bool {
        self.current.load(Ordering::SeqCst) == self.epoch
    }
}
pub enum StoreCommand {
    Guarded {
        fence: WorkFence,
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
        fence: WorkFence,
        report: Box<std::result::Result<StoreReport, String>>,
    },
    Obsolete,
    ActivePlans(Vec<String>),
    Snapshot(StoreStatus),
    Needs {
        // Present only when settlement committed terminal Call progress.
        status: Option<StoreStatus>,
    },
    Frozen(Option<wire::MutationRequest>),
    /// The uplink cannot continue from durable state; nothing retries it.
    Halted(String),
    Committed {
        status: StoreStatus,
        report: ApplyReport,
        plan: Option<(String, u64)>,
        active_plans: Option<Vec<String>>,
    },
}
