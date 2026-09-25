//! Per-record stamps: the last authoritative version applied per record.
//! Stamps outlive deletion and unsubscription, they are the evidence that keeps
//! stale content out. Subscriptions themselves live in
//! [`subscriptions`](crate::subscriptions).
use crate::engine::{Engine, as_u64};
use crate::store::ClientStore;
use axton_core::{RecordKey, Result};
use serde_json::json;

impl<S: ClientStore> Engine<'_, S> {
    pub fn record_stamp(&mut self, key: &RecordKey) -> Result<u64> {
        match self.scalar(
            "SELECT stamp FROM axton_record WHERE model=? AND identity=?",
            &[json!(key.model), json!(key.encoded_identity()?)],
        )? {
            Some(v) => as_u64(&v),
            None => Ok(0),
        }
    }
    pub fn set_record_stamp(&mut self, key: &RecordKey, stamp: u64) -> Result<()> {
        self.exec("axton_record", "INSERT INTO axton_record (model, identity, stamp) VALUES (?,?,?) ON CONFLICT(model, identity) DO UPDATE SET stamp=excluded.stamp", &[json!(key.model), json!(key.encoded_identity()?), json!(stamp)])?;
        Ok(())
    }
}
