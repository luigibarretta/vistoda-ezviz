//! Alarm feed reads: live cursor, long-poll and history pages.

use std::time::Duration;

use super::{AlarmBatch, AlarmEvent, AlarmFeed, MAX_BATCH, MAX_WAIT_SECONDS, store};
use crate::error::BridgeError;

impl AlarmFeed {
    /// Cursor read: `after = None` returns recent history without waiting;
    /// otherwise waits up to `wait` for alarms strictly after the cursor.
    pub async fn batch(
        &self,
        camera: &str,
        after: Option<u64>,
        wait: Duration,
    ) -> Result<AlarmBatch, BridgeError> {
        let slot = self
            .cameras
            .get(camera)
            .ok_or(BridgeError::CameraNotFound)?;
        let Some(after) = after else {
            let history = slot.history.lock().await;
            let (events, older) = history.page(None, MAX_BATCH);
            return Ok(self.envelope(camera, history.head(), events, older));
        };
        let deadline =
            tokio::time::Instant::now() + wait.min(Duration::from_secs(MAX_WAIT_SECONDS));
        let mut receiver = slot.head.subscribe();
        loop {
            let (events, next) = slot.history.lock().await.after(after, MAX_BATCH);
            let stale = next < after;
            if !events.is_empty() || stale || tokio::time::Instant::now() >= deadline {
                return Ok(self.envelope(camera, next, events, None));
            }
            tokio::select! {
                () = self.shutdown.cancelled() => return Ok(self.envelope(camera, next, events, None)),
                changed = tokio::time::timeout_at(deadline, receiver.changed()) => {
                    if !matches!(changed, Ok(Ok(()))) {
                        return Ok(self.envelope(camera, next, events, None));
                    }
                }
            }
        }
    }

    /// History read: up to 50 retained alarms strictly below `before`,
    /// without waiting; `next_sequence` is the current head.
    pub async fn history(&self, camera: &str, before: u64) -> Result<AlarmBatch, BridgeError> {
        let slot = self
            .cameras
            .get(camera)
            .ok_or(BridgeError::CameraNotFound)?;
        let history = slot.history.lock().await;
        let (events, older) = history.page(Some(before), MAX_BATCH);
        Ok(self.envelope(camera, history.head(), events, older))
    }

    fn envelope(
        &self,
        camera: &str,
        next: u64,
        events: Vec<store::Entry>,
        older: Option<u64>,
    ) -> AlarmBatch {
        AlarmBatch {
            camera: camera.to_owned(),
            generation: self.generation,
            older,
            next_sequence: next,
            events: events
                .into_iter()
                .map(|entry| AlarmEvent {
                    sequence: entry.sequence,
                    id: entry.record.id,
                    occurred_at: entry.record.occurred_at,
                    alarm_type: entry.record.alarm_type,
                    category: entry.record.category,
                    title: entry.record.title,
                    has_picture: entry.record.has_picture,
                })
                .collect(),
        }
    }
}
