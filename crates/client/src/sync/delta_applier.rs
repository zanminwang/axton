use super::DeliveryQueue;
use crate::{ApplyReport, Client, ClientStore, Result, authority::Held, engine::Engine, invalid};
use serde_json::{Value, json};
const DDL: &str = "CREATE TABLE IF NOT EXISTS axton_delivery_progress(plan_id TEXT PRIMARY KEY,digest TEXT NOT NULL,header TEXT NOT NULL,next_unit INTEGER NOT NULL,covered INTEGER)";
const KEY_DDL: &str = "CREATE TABLE IF NOT EXISTS axton_delivery_key(plan_id TEXT NOT NULL,identity TEXT NOT NULL,PRIMARY KEY(plan_id,identity))";
impl<S: ClientStore> Engine<'_, S> {
    fn delivery_staging(&mut self) -> Result<()> {
        self.exec("axton_delivery_progress", DDL, &[])?;
        self.exec("axton_delivery_key", KEY_DDL, &[])?;
        Ok(())
    }
    /// Transient progress of network plans; it proves no coverage.
    pub(crate) fn clear_delivery_staging(&mut self) -> Result<()> {
        self.delivery_staging()?;
        self.exec("axton_delivery_key", "DELETE FROM axton_delivery_key", &[])?;
        self.exec(
            "axton_delivery_progress",
            "DELETE FROM axton_delivery_progress",
            &[],
        )?;
        Ok(())
    }
}
impl<S: ClientStore> Client<S> {
    pub fn cleanup_delivery(&mut self, now: u64) -> Result<Vec<String>> {
        self.write(|e| {
            e.delivery_staging()?;
            e.exec("axton_delivery_key", "DELETE FROM axton_delivery_key WHERE plan_id IN (SELECT plan_id FROM axton_delivery_progress WHERE json_extract(header,'$.expiresAt')<=?)", &[json!(now)])?;
            e.exec("axton_delivery_progress", "DELETE FROM axton_delivery_progress WHERE json_extract(header,'$.expiresAt')<=?", &[json!(now)])?;
            Ok(e.rows("SELECT plan_id FROM axton_delivery_progress", &[])?.rows.into_iter().filter_map(|r| r[0].as_str().map(str::to_owned)).collect())
        })
    }
    pub fn active_delivery_plans(&mut self) -> Result<Vec<String>> {
        self.view(|e| {
            Ok(e.rows("SELECT plan_id FROM axton_delivery_progress", &[])?
                .rows
                .into_iter()
                .filter_map(|r| r[0].as_str().map(str::to_owned))
                .collect())
        })
    }
    /// Commit a handshake: adopt the backend's projection generation, then
    /// its head. A changed generation upgrades the Store in the same commit;
    /// the head becomes `S` and `C` only for a Store that has no `C` yet.
    pub fn initialize_stream(&mut self, start: u64, projection_generation: &str) -> Result<()> {
        self.write(|e| {
            e.adopt_generation(projection_generation)?;
            e.initialize_stream(start)
        })
    }
    pub fn apply_next_delivery(
        &mut self,
        q: &mut DeliveryQueue,
        now: u64,
    ) -> Result<Option<ApplyReport>> {
        q.expire(now);
        let context = self.request_context()?;
        let status = self.store_status()?;
        // Surviving contiguous coverage is the receipt for completed ordinary
        // transfers. Replays must not restore cache state after a later read.
        q.plans.retain(|_, p| {
            let position = if p.header.bootstrap {
                status.bootstrap_cursor
            } else {
                status.cursor
            };
            !(p.header.context == context && position.is_some_and(|c| c >= p.header.through))
        });
        let mut selected = None;
        for (id, p) in &q.plans {
            if p.blocked
                || q.plans.values().any(|blocked| {
                    blocked.blocked && blocked.header.bootstrap == p.header.bootstrap
                })
            {
                continue;
            }
            p.header.context.admit_store(&context)?;
            if p.header.context != context {
                continue;
            }
            let position = if p.header.bootstrap {
                status.bootstrap_cursor.unwrap_or(0)
            } else {
                status.cursor.ok_or_else(|| invalid("handshake required"))?
            };
            if p.header.after > position {
                continue;
            }
            if let Some(unit) = p.unit(p.next)? {
                selected = Some((id.clone(), p.header.clone(), unit));
                break;
            }
        }
        let Some((id, header, unit)) = selected else {
            self.cleanup_delivery(now)?;
            return Ok(None);
        };
        let _span = tracing::info_span!(target: "axton::client", "delivery.apply", planId = %id, unitIndex = unit.index).entered();
        let applied=self.write(|e|{
            e.delivery_staging()?;
            e.exec("axton_delivery_key", "DELETE FROM axton_delivery_key WHERE plan_id IN (SELECT plan_id FROM axton_delivery_progress WHERE json_extract(header,'$.expiresAt')<=?)", &[json!(now)])?;
            e.exec("axton_delivery_progress", "DELETE FROM axton_delivery_progress WHERE json_extract(header,'$.expiresAt')<=?", &[json!(now)])?;
            let saved=e.rows("SELECT digest,next_unit FROM axton_delivery_progress WHERE plan_id=?",&[json!(id)])?.rows;
            let next=if let Some(row)=saved.first(){
                if row[0]!=header.digest {return Err(invalid("persisted plan digest mismatch"))}
                row[1].as_u64().ok_or_else(||invalid("invalid delivery progress"))?
            }else{0};
            if next>unit.index {return Ok((None,next))}
            if next!=unit.index {return Err(invalid("delivery unit prefix gap"))}
            let active=e.context()?;
            header.context.admit(&active)?;
            let mut held=Held::new();
            let changes=&unit.changes;
            let keys_span=tracing::info_span!(target: "axton::client", "delivery.apply.keys").entered();
            for change in changes {e.exec("axton_delivery_key","INSERT INTO axton_delivery_key(plan_id,identity) VALUES(?,?)",&[json!(id),json!(change.key().encoded()?)])?;}
            drop(keys_span);
            let authority_span=tracing::info_span!(target: "axton::client", "delivery.apply.authority").entered();
            let applied=e.stage_authority(&header.context,changes,&mut held)?;
            drop(authority_span);
            let final_unit=unit.index+1==header.units.len() as u64;
            if header.bootstrap {
                let start=e.scalar("SELECT start_cursor FROM axton_store",&[])?.ok_or_else(||invalid("handshake required"))?;
                if start.as_u64().is_none_or(|start| header.through>start){return Err(invalid("bootstrap start mismatch"))}
                if let Some(through)=unit.through {
                    let current=e.scalar("SELECT bootstrap_cursor FROM axton_store",&[])?.and_then(|v|v.as_u64()).unwrap_or(0);
                    if header.after>current {return Err(invalid("bootstrap prefix gap"))}
                    if through>current || final_unit {e.exec("axton_store","UPDATE axton_store SET bootstrap_cursor=?",&[json!(through.max(current))])?;}
                }
            }else{
                let current=e.scalar("SELECT cursor FROM axton_store",&[])?.and_then(|v|v.as_u64()).ok_or_else(||invalid("handshake required"))?;
                if header.after>current {return Err(invalid("sync prefix gap"))}
                if let Some(through)=unit.through.filter(|c|*c>current) {e.commit_sync_coverage(current,through)?;}
            }
            let completions=tracing::info_span!(target: "axton::client", "delivery.apply.reconcile").in_scope(|| e.reconcile_ready(&mut held))?;
            let reports=tracing::info_span!(target: "axton::client", "delivery.apply.rebuild").in_scope(|| e.rebuild_held(&held))?;
            let next=unit.index+1;
            if final_unit {
                e.exec("axton_delivery_key","DELETE FROM axton_delivery_key WHERE plan_id=?",&[json!(id)])?;
                e.exec("axton_delivery_progress","DELETE FROM axton_delivery_progress WHERE plan_id=?",&[json!(id)])?;
            } else if saved.is_empty() {
                e.exec("axton_delivery_progress","INSERT INTO axton_delivery_progress(plan_id,digest,header,next_unit,covered) VALUES(?,?,?,?,?)",&[json!(id),json!(header.digest),json!(serde_json::to_string(header.as_ref())?),json!(next),unit.through.map_or(Value::Null,Value::from)])?;
            }else{
                e.exec("axton_delivery_progress","UPDATE axton_delivery_progress SET next_unit=?,covered=COALESCE(?,covered) WHERE plan_id=?",&[json!(next),unit.through.map_or(Value::Null,Value::from),json!(id)])?;
            }
            // Each ordinary coverage lane has one durable active transfer.
            // Replacing its partial plan preserves authority/coverage; control
            // drops the retired staging and repairs from the durable prefix.
            e.exec("axton_delivery_key","DELETE FROM axton_delivery_key WHERE plan_id IN (SELECT plan_id FROM axton_delivery_progress WHERE plan_id<>? AND json_extract(header,'$.bootstrap')=?)",&[json!(id),json!(header.bootstrap)])?;
            e.exec("axton_delivery_progress","DELETE FROM axton_delivery_progress WHERE plan_id<>? AND json_extract(header,'$.bootstrap')=?",&[json!(id),json!(header.bootstrap)])?;
            Ok((Some(ApplyReport{applied,reports,completions,..Default::default()}),next))
        });
        match applied {
            Ok((report, next)) => {
                let active = self.active_delivery_plans()?;
                q.plans
                    .retain(|other, p| other == &id || p.next == 0 || active.contains(other));
                let plan = q.plans.get_mut(&id).unwrap();
                plan.next = next;
                plan.parts.retain(|(unit, _), _| *unit >= next);
                if next >= header.units.len() as u64 {
                    q.plans.remove(&id);
                }
                Ok(report)
            }
            Err(error) => {
                q.plans.get_mut(&id).unwrap().blocked = true;
                Err(error)
            }
        }
    }
}
impl<S: ClientStore> Client<S> {
    pub fn subscription_state(
        &mut self,
        stream: &str,
    ) -> Result<Option<crate::subscriptions::SubscriptionState>> {
        let status = self.store_status()?;
        if status.context.stream != stream {
            return Ok(None);
        }
        Ok(Some(crate::subscriptions::SubscriptionState {
            stream: stream.into(),
            subscription_id: 1,
            starting_cursor: status.start_cursor,
            cursor: status.cursor,
        }))
    }
    pub fn bootstrap_state(
        &mut self,
        stream: &str,
        id: u64,
    ) -> Result<crate::bootstrap::BootstrapState> {
        let status = self.store_status()?;
        if status.context.stream != stream || id != 1 {
            return Err(invalid("subscription.closed: foreign Stream registration"));
        }
        Ok(crate::bootstrap::BootstrapState {
            stream: stream.into(),
            subscription_id: 1,
            run: 1,
            cursor: status.bootstrap_cursor.unwrap_or(0),
            barrier: status.start_cursor,
            state: if status.start_cursor.is_some()
                && status.start_cursor == status.bootstrap_cursor
            {
                crate::bootstrap::BootstrapPhase::Complete
            } else {
                crate::bootstrap::BootstrapPhase::Requested
            },
            error: None,
        })
    }
}
