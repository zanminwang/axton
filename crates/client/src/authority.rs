//! Rebuild held projections after canonical base or queue ownership changes.
use crate::{ClientStore, Report, ReportKind, engine::Engine};
use axton_core::{RecordKey, Result};
use serde_json::Value;
use std::collections::BTreeMap;
pub type Held = BTreeMap<String, RecordKey>;
impl<S: ClientStore> Engine<'_, S> {
    pub fn rebuild_held(&mut self, held: &Held) -> Result<Vec<Report>> {
        let mut reports = vec![];
        for key in held.values() {
            if let Some(ordinal) = self.rebuild(key)? {
                let mut report = Report::new(ReportKind::Diverged, &key.model, &key.identity);
                report.ordinal = Some(ordinal);
                reports.push(report);
            }
        }
        self.refresh_pending()?;
        Ok(reports)
    }
    pub(crate) fn stage_one(
        &mut self,
        key: &RecordKey,
        value: Option<&Value>,
        held: &mut Held,
    ) -> Result<()> {
        if self.dirty(key)? {
            self.before_set(key, value)?;
            self.delete_local_writes(key)?;
            held.insert(key.encoded()?, key.clone());
        } else {
            self.main_set(key, value)?;
        }
        Ok(())
    }
}
