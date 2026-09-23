//! Per-record stamps and channel subscriptions. Stamps outlive deletion and
//! unsubscription: they are the evidence that keeps stale content out.
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
    pub fn cursor(&mut self, channel: &str) -> Result<Option<u64>> {
        self.scalar(
            "SELECT cursor FROM axton_subscription WHERE channel=?",
            &[json!(channel)],
        )?
        .map(|v| as_u64(&v))
        .transpose()
    }
    pub fn set_cursor(&mut self, channel: &str, cursor: u64) -> Result<()> {
        self.exec("axton_subscription", "INSERT INTO axton_subscription (channel, cursor) VALUES (?,?) ON CONFLICT(channel) DO UPDATE SET cursor=excluded.cursor", &[json!(channel), json!(cursor)])?;
        Ok(())
    }
    pub fn delete_subscription(&mut self, channel: &str) -> Result<()> {
        self.exec(
            "axton_subscription",
            "DELETE FROM axton_subscription WHERE channel=?",
            &[json!(channel)],
        )?;
        Ok(())
    }
    pub fn subscriptions(&mut self) -> Result<Vec<(String, u64)>> {
        let rows = self.rows(
            "SELECT channel, cursor FROM axton_subscription ORDER BY channel",
            &[],
        )?;
        rows.rows
            .into_iter()
            .map(|r| Ok((r[0].as_str().unwrap_or("").to_string(), as_u64(&r[1])?)))
            .collect()
    }
}
