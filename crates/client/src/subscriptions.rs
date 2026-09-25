//! Durable subscriptions: the intent to follow a Scope, the identity allocated
//! to that registration, and the delivery boundary it committed. A row means
//! subscribed; NULL cursors mean the intent is durable but its first boundary
//! is not committed yet, and zero is an initialized position, never a stand-in
//! for uninitialized
//! ([#150](https://github.com/zanminwang/axton/issues/150)).
use crate::engine::{Engine, as_u64};
use crate::store::ClientStore;
use crate::{Client, SUBSCRIPTION_MARK};
use axton_core::{MAX_SAFE_INTEGER, Result, invalid};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One stored subscription. `subscription_id` is client-local and never
/// recycled: it fences a handle, an acknowledgement or a request against the
/// registration it was made for, including a recreation at the same Scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionState {
    pub scope: String,
    pub subscription_id: u64,
    /// The boundary the first initialization committed; `None` until then.
    pub starting_cursor: Option<u64>,
    /// How far delivery has committed, at or above `starting_cursor`.
    pub cursor: Option<u64>,
}

const COLUMNS: &str = "channel, subscription_id, starting_cursor, cursor";

fn optional(value: &Value) -> Result<Option<u64>> {
    if value.is_null() {
        return Ok(None);
    }
    as_u64(value).map(Some)
}
fn decode(row: &[Value]) -> Result<SubscriptionState> {
    Ok(SubscriptionState {
        scope: row[0]
            .as_str()
            .ok_or_else(|| invalid("stored Scope name is not text"))?
            .to_string(),
        subscription_id: as_u64(&row[1])?,
        starting_cursor: optional(&row[2])?,
        cursor: optional(&row[3])?,
    })
}

impl<S: ClientStore> Engine<'_, S> {
    /// Record that this transaction changed which Scopes are subscribed. The
    /// mark is stripped before the changed set reaches watchers; it bumps the
    /// subscription generation and makes pulls in flight stale.
    pub(crate) fn mark_subscription(&mut self, channel: &str) {
        self.changed.insert(format!("{SUBSCRIPTION_MARK}{channel}"));
    }
    pub fn subscription(&mut self, channel: &str) -> Result<Option<SubscriptionState>> {
        let rows = self.rows(
            &format!("SELECT {COLUMNS} FROM axton_subscription WHERE channel=?"),
            &[json!(channel)],
        )?;
        rows.rows.first().map(|r| decode(r)).transpose()
    }
    /// Every subscription, subscribed order, initialized or not.
    pub fn subscription_states(&mut self) -> Result<Vec<SubscriptionState>> {
        let rows = self.rows(
            &format!("SELECT {COLUMNS} FROM axton_subscription ORDER BY channel"),
            &[],
        )?;
        rows.rows.iter().map(|r| decode(r)).collect()
    }
    /// The initialized subscriptions with their cursors. An uninitialized one
    /// has no delivery position and is left out: it belongs to the desired set
    /// ([`Engine::subscription_states`]), not to a pull.
    pub fn subscriptions(&mut self) -> Result<Vec<(String, u64)>> {
        Ok(self
            .subscription_states()?
            .into_iter()
            .filter_map(|s| s.cursor.map(|c| (s.scope, c)))
            .collect())
    }
    /// How far `channel` committed delivery: `None` when it has no
    /// subscription, and `None` while its subscription is uninitialized.
    pub fn cursor(&mut self, channel: &str) -> Result<Option<u64>> {
        Ok(self.subscription(channel)?.and_then(|s| s.cursor))
    }
    /// The subscription for `channel`, registering it when absent, and whether
    /// this call created it. Insert-if-absent, never an upsert: a repeated
    /// registration reads the stored identity and cursors untouched.
    pub fn ensure_subscription(&mut self, channel: &str) -> Result<(SubscriptionState, bool)> {
        if let Some(state) = self.subscription(channel)? {
            return Ok((state, false));
        }
        let subscription_id = self.bump("next_subscription")?;
        let state = SubscriptionState {
            scope: channel.to_string(),
            subscription_id,
            starting_cursor: None,
            cursor: None,
        };
        self.exec(
            "axton_subscription",
            &format!("INSERT INTO axton_subscription ({COLUMNS}) VALUES (?,?,NULL,NULL)"),
            &[json!(channel), json!(subscription_id)],
        )?;
        Ok((state, true))
    }
    /// Commit the first delivery boundary of an uninitialized subscription:
    /// both cursors at `cursor`, for that identity only. `false` when the row
    /// is gone, holds another identity, or is already initialized - all of
    /// which mean this initialization no longer applies.
    pub fn initialize_subscription(
        &mut self,
        channel: &str,
        subscription_id: u64,
        cursor: u64,
    ) -> Result<bool> {
        let affected = self.exec(
            "axton_subscription",
            "UPDATE axton_subscription SET starting_cursor=?, cursor=? WHERE channel=? AND subscription_id=? AND starting_cursor IS NULL",
            &[json!(cursor), json!(cursor), json!(channel), json!(subscription_id)],
        )?;
        Ok(affected == 1)
    }
    /// Move the cursor of the initialized subscription `subscription_id`
    /// names. An update, never an insert: it cannot resurrect a Scope this
    /// client unsubscribed, cannot initialize one whose first boundary is still
    /// pending, and cannot move a subscription that replaced the one the caller
    /// read. A caller reads the row before it moves it, so no row to update is
    /// a fault, not a no-op.
    pub fn advance_cursor(
        &mut self,
        channel: &str,
        subscription_id: u64,
        cursor: u64,
    ) -> Result<()> {
        let affected = self.exec(
            "axton_subscription",
            "UPDATE axton_subscription SET cursor=? WHERE channel=? AND subscription_id=? AND cursor IS NOT NULL",
            &[json!(cursor), json!(channel), json!(subscription_id)],
        )?;
        if affected != 1 {
            return Err(invalid(format!(
                "no initialized subscription {subscription_id} for {channel}; its cursor cannot advance"
            )));
        }
        Ok(())
    }
    /// Unsubscribe `channel`, and whether a row went. `subscription_id` fences
    /// the removal to one registration: an old handle cannot delete the
    /// subscription that replaced it. `None` removes whichever is stored.
    pub fn remove_subscription(
        &mut self,
        channel: &str,
        subscription_id: Option<u64>,
    ) -> Result<bool> {
        let affected = match subscription_id {
            Some(id) => self.exec(
                "axton_subscription",
                "DELETE FROM axton_subscription WHERE channel=? AND subscription_id=?",
                &[json!(channel), json!(id)],
            )?,
            None => self.exec(
                "axton_subscription",
                "DELETE FROM axton_subscription WHERE channel=?",
                &[json!(channel)],
            )?,
        };
        Ok(affected > 0)
    }
    /// Raise the allocator to `next` so a rebuilt replica cannot reissue an
    /// identity the replica it replaced handed out. Lowering it is refused by
    /// the statement itself.
    pub fn carry_subscription_allocator(&mut self, next: u64) -> Result<()> {
        let next = next.min(MAX_SAFE_INTEGER);
        self.exec(
            "axton_client",
            "UPDATE axton_client SET next_subscription=? WHERE next_subscription<?",
            &[json!(next), json!(next)],
        )?;
        Ok(())
    }
}

impl<S: ClientStore> Client<S> {
    /// Register durable intent to follow `scope` and answer with its stored
    /// state. Repeating it returns the same identity and the same cursors; a
    /// new registration starts uninitialized, with no delivery position until
    /// its first boundary is committed.
    pub fn ensure_subscription(&mut self, scope: &str) -> Result<SubscriptionState> {
        self.write(|e| {
            let (state, created) = e.ensure_subscription(scope)?;
            if created {
                e.mark_subscription(scope);
            }
            Ok(state)
        })
    }
    pub fn subscription_state(&mut self, scope: &str) -> Result<Option<SubscriptionState>> {
        self.view(|e| e.subscription(scope))
    }
    /// Unsubscribe the registration `subscription_id` names. A Scope whose
    /// current subscription is another one is left alone: an old handle cannot
    /// remove the subscription that replaced it.
    pub fn remove_subscription(&mut self, scope: &str, subscription_id: u64) -> Result<()> {
        self.write(|e| {
            if e.remove_subscription(scope, Some(subscription_id))? {
                e.mark_subscription(scope);
            }
            Ok(())
        })
    }
    /// How far `channel` committed delivery; see [`Engine::cursor`].
    pub fn cursor(&mut self, channel: &str) -> Result<Option<u64>> {
        self.view(|e| e.cursor(channel))
    }
    /// The initialized subscriptions with their cursors; see
    /// [`Engine::subscriptions`].
    pub fn subscriptions(&mut self) -> Result<Vec<(String, u64)>> {
        self.view(|e| e.subscriptions())
    }
    /// Every subscribed Scope, whether or not its first boundary is committed:
    /// the set a live session asks for.
    pub fn desired_channels(&mut self) -> Result<std::collections::BTreeSet<String>> {
        Ok(self
            .view(|e| e.subscription_states())?
            .into_iter()
            .map(|s| s.scope)
            .collect())
    }
}
