//! Apply a pull page: every channel it names is gated by its cursor, every
//! change lands by its stamp, and the cursors move only after the whole page
//! did ([Distribution](../../../docs/engineering/architecture/client/engine/distribution.md)).
use crate::store::ClientStore;
use crate::{ApplyReport, Client};
use axton_core::{PullPage, Result, invalid};
use std::collections::BTreeMap;

impl<S: ClientStore> Client<S> {
    /// Apply one page in one transaction: all changes, then all cursors. A
    /// channel the client no longer subscribes to, or whose cursor already
    /// covers the range, contributes nothing; a channel whose `from` is beyond
    /// the cursor is a gap and the page is not applied at all. A change that
    /// cannot be applied is reported and leaves nothing behind.
    pub fn apply_page(&mut self, page: PullPage) -> Result<ApplyReport> {
        page.validate()?;
        // A page answering a pull issued before a channel was unsubscribed and
        // subscribed again was built against a cursor this subscription no longer
        // has; it is stale, not a gap, and the next pull from the reset cursor
        // delivers everything.
        if self.stale_subscription_page(&page) {
            return Ok(ApplyReport {
                stale: true,
                ..Default::default()
            });
        }
        self.apply_current_page(page)
    }
    /// The channels of the page that move this client's cursors: each with its
    /// new cursor. `Err` names a gap. An empty map means the page is covered.
    pub(crate) fn moving_channels(&mut self, page: &PullPage) -> Result<BTreeMap<String, u64>> {
        let mut moving = BTreeMap::new();
        for (channel, range) in &page.cursors {
            // Only an initialized subscription has a position a page can move.
            // A channel this client unsubscribed, or one still waiting for its
            // first boundary, contributes nothing: that part of the page - a
            // pull still in flight when the unsubscribe committed - is ignored.
            let Some(current) = self.view(|e| e.cursor(channel))? else {
                continue;
            };
            if range.to <= current {
                continue;
            }
            if range.from > current {
                return Err(invalid("pull cursor gap"));
            }
            moving.insert(channel.clone(), range.to);
        }
        Ok(moving)
    }
    /// `apply_page` after the subscription-epoch check; the check consumes the
    /// matching request, so each incoming page runs it exactly once.
    pub(crate) fn apply_current_page(&mut self, page: PullPage) -> Result<ApplyReport> {
        let moving = self.moving_channels(&page)?;
        if moving.is_empty() {
            return Ok(ApplyReport {
                stale: true,
                ..Default::default()
            });
        }
        self.write(|e| {
            for (channel, range) in &page.cursors {
                if let Some(to) = moving.get(channel) {
                    let current = e
                        .cursor(channel)?
                        .ok_or_else(|| invalid("cursor moved during page application"))?;
                    if current != range.from && current >= *to {
                        return Err(invalid("cursor moved during page application"));
                    }
                }
            }
            // Content first, by stamp alone: a record shared by two channels is
            // in the page once and lands once.
            let mut report = e.apply_records(&page.changes)?;
            for (channel, to) in &moving {
                e.advance_cursor(channel, *to)?;
            }
            report.cursors = moving.clone();
            Ok(report)
        })
    }
}
