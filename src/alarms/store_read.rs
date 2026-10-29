//! History reads: recent alarms, history pages and the live cursor.

use super::{CameraHistory, Entry};

impl CameraHistory {
    /// The newest `limit` alarms in ascending sequence order.
    #[must_use]
    pub fn recent(&self, limit: usize) -> Vec<Entry> {
        self.page(None, limit).0
    }

    /// The newest `limit` retained alarms strictly below `before` (all of
    /// them when `None`), in ascending sequence order, and the `before`
    /// cursor of the next older page when older alarms remain. History
    /// paging ignores the replay floor: it browses, it does not deliver.
    #[must_use]
    pub fn page(&self, before: Option<u64>, limit: usize) -> (Vec<Entry>, Option<u64>) {
        let below: Vec<&Entry> = self
            .events
            .iter()
            .filter(|entry| before.is_none_or(|before| entry.sequence < before))
            .collect();
        let skip = below.len().saturating_sub(limit);
        let page: Vec<Entry> = below[skip..].iter().map(|entry| (*entry).clone()).collect();
        let older = page
            .first()
            .filter(|_| skip > 0)
            .map(|entry| entry.sequence);
        (page, older)
    }

    /// Live alarms strictly after `after` (never at or below the replay
    /// floor) and the cursor to send next. A cursor ahead of the head (for
    /// example from another generation) is answered with the current head.
    #[must_use]
    pub fn after(&self, after: u64, limit: usize) -> (Vec<Entry>, u64) {
        if after > self.head {
            return (Vec::new(), self.head);
        }
        let cursor = after.max(self.floor);
        let events: Vec<Entry> = self
            .events
            .iter()
            .filter(|entry| entry.sequence > cursor)
            .take(limit)
            .cloned()
            .collect();
        let next = events.last().map_or(cursor, |entry| entry.sequence);
        (events, next)
    }
}
