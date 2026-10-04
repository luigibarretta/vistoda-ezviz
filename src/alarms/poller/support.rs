//! Small helpers of the alarm poller, kept apart for the LOC budget.

use std::collections::BTreeSet;

use super::SerialState;
use crate::alarms::AlarmFeed;

/// IDs already seen for this serial plus every alias history, and the newest
/// stored alarm time used to decide whether the previous day must be read.
pub(super) async fn known_ids(
    feed: &AlarmFeed,
    aliases: &[String],
    state: &SerialState,
) -> (BTreeSet<String>, Option<i64>) {
    let mut known: BTreeSet<String> = state.seen.iter().cloned().collect();
    let mut newest = None;
    for alias in aliases {
        if let Some(slot) = feed.cameras.get(alias) {
            let history = slot.history.lock().await;
            newest = newest.max(history.newest_occurred_at());
            known.extend(
                history
                    .recent(crate::alarms::MAX_EVENTS)
                    .into_iter()
                    .map(|e| e.record.id),
            );
        }
    }
    (known, newest)
}

pub(super) async fn record_outcome(
    feed: &AlarmFeed,
    alias: &str,
    outcome: &crate::alarms::store::InsertOutcome,
    live: bool,
) {
    for (name, count) in [
        ("alarm_pictures_stored_total", outcome.pictures_stored),
        ("alarm_pictures_failed_total", outcome.pictures_failed),
        (
            "alarms_received_total",
            if live { outcome.inserted } else { 0 },
        ),
    ] {
        if count > 0 {
            feed.metrics.add(name, alias, count as u64).await;
        }
    }
    if live && outcome.inserted > 0 {
        tracing::info!(camera = %alias, count = outcome.inserted, "alarm events received");
    }
}
