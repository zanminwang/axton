//! The SQL of the bootstrap ledger: the columns of `axton_subscription` that
//! carry a Scope's historical load, how one row decodes into the #150
//! subscription state and the load beside it, and the fenced statement that
//! writes the load back. The phases, the bounds and the transitions are in
//! [`bootstrap`](crate::bootstrap); this module only reads and writes rows
//! ([#151](https://github.com/zanminwang/axton/issues/151)).
use crate::bootstrap::{BootstrapPhase, BootstrapState};
use crate::engine::{Engine, as_u64};
use crate::store::ClientStore;
use crate::{BOOTSTRAP_MARK, BootstrapError, SubscriptionState};
use axton_core::{Result, invalid};
use serde_json::{Value, json};

/// The whole subscription row: the #150 identity and boundary, and the load
/// beside it. Both halves are read at once so one transaction decides on S, L
/// and B together.
pub(crate) struct Loaded {
    pub subscription: SubscriptionState,
    pub state: BootstrapState,
}

const COLUMNS: &str = "channel, subscription_id, starting_cursor, cursor, \
     bootstrap_state, bootstrap_run, bootstrap_cursor, bootstrap_barrier, bootstrap_error";

fn optional(value: &Value) -> Result<Option<u64>> {
    if value.is_null() {
        return Ok(None);
    }
    as_u64(value).map(Some)
}
fn decode(row: &[Value]) -> Result<Loaded> {
    let scope = row[0]
        .as_str()
        .ok_or_else(|| invalid("stored Scope name is not text"))?
        .to_string();
    let subscription_id = as_u64(&row[1])?;
    let state = BootstrapState {
        scope: scope.clone(),
        subscription_id,
        state: BootstrapPhase::parse(
            row[4]
                .as_str()
                .ok_or_else(|| invalid("stored bootstrap state is not text"))?,
        )?,
        run: as_u64(&row[5])?,
        cursor: as_u64(&row[6])?,
        barrier: optional(&row[7])?,
        error: match row[8].as_str() {
            Some(text) => Some(serde_json::from_str(text)?),
            None => None,
        },
    };
    state.coherent()?;
    Ok(Loaded {
        subscription: SubscriptionState {
            scope,
            subscription_id,
            starting_cursor: optional(&row[2])?,
            cursor: optional(&row[3])?,
        },
        state,
    })
}
/// The error every call that names a registration this client no longer holds
/// is refused with. The SDK maps it to `subscription.closed`.
fn closed(scope: &str, subscription_id: u64) -> axton_core::Error {
    invalid(format!(
        "subscription {subscription_id} for {scope} is closed; it has no bootstrap state"
    ))
}

impl<S: ClientStore> Engine<'_, S> {
    /// Record that this transaction changed `channel`'s load. The mark is
    /// stripped before the changed set reaches watchers and hosts, and it
    /// bumps no subscription generation: a load changes no membership, so it
    /// must not make the open live session stale or a pull in flight
    /// ([`BOOTSTRAP_MARK`]).
    pub(crate) fn mark_bootstrap(&mut self, channel: &str) {
        self.changed.insert(format!("{BOOTSTRAP_MARK}{channel}"));
    }
    /// The whole row for `channel`, or `None` when it is not subscribed.
    pub(crate) fn bootstrap_row(&mut self, channel: &str) -> Result<Option<Loaded>> {
        let rows = self.rows(
            &format!("SELECT {COLUMNS} FROM axton_subscription WHERE channel=?"),
            &[json!(channel)],
        )?;
        rows.rows.first().map(|r| decode(r)).transpose()
    }
    /// The row `subscription_id` names, or the closed error: a registration
    /// that is gone, or one another registration replaced, has no load state
    /// and never adopts another one's.
    pub(crate) fn bootstrap_of(&mut self, channel: &str, subscription_id: u64) -> Result<Loaded> {
        match self.bootstrap_row(channel)? {
            Some(row) if row.subscription.subscription_id == subscription_id => Ok(row),
            _ => Err(closed(channel, subscription_id)),
        }
    }
    /// Write every bootstrap field of one row, fenced by the identity and by
    /// the run it was read at. `false` means the row moved underneath and
    /// nothing was written.
    pub(crate) fn set_bootstrap(&mut self, state: &BootstrapState, from_run: u64) -> Result<bool> {
        state.coherent()?;
        // Bounded here, at the one place a failure is stored: the fields are
        // public, so a caller could hand over a message or a list this row must
        // not carry.
        let error = match &state.error {
            Some(error) => json!(serde_json::to_string(&BootstrapError::new(
                error.code.clone(),
                error.message.clone(),
                error.records.clone(),
            ))?),
            None => Value::Null,
        };
        let affected = self.exec(
            "axton_subscription",
            "UPDATE axton_subscription SET bootstrap_state=?, bootstrap_run=?, bootstrap_cursor=?, \
             bootstrap_barrier=?, bootstrap_error=? \
             WHERE channel=? AND subscription_id=? AND bootstrap_run=?",
            &[
                json!(state.state.as_str()),
                json!(state.run),
                json!(state.cursor),
                json!(state.barrier),
                error,
                json!(state.scope),
                json!(state.subscription_id),
                json!(from_run),
            ],
        )?;
        Ok(affected == 1)
    }
    /// The initialized runs that still have work, in Scope order. A run whose
    /// subscription has no origin yet has no interval to scan: it is registered
    /// work, not schedulable work.
    pub(crate) fn bootstrap_tasks(&mut self) -> Result<Vec<BootstrapState>> {
        Ok(self
            .bootstrap_task_rows()?
            .into_iter()
            .map(|row| row.state)
            .collect())
    }
    /// The same runs with the subscription half beside them: the scheduler
    /// needs S, which lives in the #150 columns, to bound the next page.
    pub(crate) fn bootstrap_task_rows(&mut self) -> Result<Vec<Loaded>> {
        let rows = self.rows(
            &format!(
                "SELECT {COLUMNS} FROM axton_subscription \
                 WHERE starting_cursor IS NOT NULL AND bootstrap_state IN ('requested','loading','catching_up') \
                 ORDER BY channel"
            ),
            &[],
        )?;
        rows.rows.iter().map(|r| decode(r)).collect()
    }
    /// The named Scopes whose fixed barrier ordinary delivery has reached: the
    /// runs a settlement would actually complete. Read on the committed reader
    /// so a caller can tell there is nothing to do without opening a write.
    pub(crate) fn settleable_scopes(&mut self, scopes: &[String]) -> Result<Vec<String>> {
        if scopes.is_empty() {
            return Ok(vec![]);
        }
        let named = vec!["?"; scopes.len()].join(",");
        let rows = self.rows(
            &format!(
                "SELECT channel FROM axton_subscription \
                 WHERE bootstrap_state='catching_up' AND bootstrap_barrier IS NOT NULL \
                   AND cursor IS NOT NULL AND cursor >= bootstrap_barrier \
                   AND channel IN ({named}) ORDER BY channel"
            ),
            &scopes.iter().map(|s| json!(s)).collect::<Vec<_>>(),
        )?;
        rows.rows
            .iter()
            .map(|r| {
                r[0].as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("stored Scope name is not text"))
            })
            .collect()
    }
    /// Mark a run complete when the barrier it fixed has been reached, and
    /// answer with the state it committed. Reading L and writing the phase in
    /// one transaction is what makes the completion evidence exact.
    pub(crate) fn settle_barrier(&mut self, channel: &str) -> Result<Option<BootstrapState>> {
        let Some(row) = self.bootstrap_row(channel)? else {
            return Ok(None);
        };
        let (Some(barrier), BootstrapPhase::CatchingUp) = (row.state.barrier, row.state.state)
        else {
            return Ok(None);
        };
        // A subscription without a committed position has not reached anything:
        // it is never read as position zero (D9).
        if row
            .subscription
            .cursor
            .is_none_or(|delivered| delivered < barrier)
        {
            return Ok(None);
        }
        let mut state = row.state;
        state.state = BootstrapPhase::Complete;
        if self.set_bootstrap(&state, state.run)? {
            self.mark_bootstrap(channel);
            return Ok(Some(state));
        }
        Ok(None)
    }
}
