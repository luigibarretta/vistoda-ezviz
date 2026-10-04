//! Read-only EZVIZ alarm event feed: polling, bounded history and pictures.

mod crypto;
mod fetch;
mod model;
mod pictures;
mod poller;
pub(crate) mod store;
#[cfg(test)]
mod tests;

pub use crypto::{PictureError, decode_picture, decode_with_keys, decrypt_checksum_picture};
pub use model::{
    AlarmRecord, DeviceSummary, ParsedMessage, category, is_token, parse_list, parse_summary,
};
pub use store::MAX_EVENTS;

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{config::CameraConfig, error::BridgeError, metrics::Metrics};
use store::CameraHistory;

/// Largest batch returned by one alarm request.
pub const MAX_BATCH: usize = 50;
/// Longest accepted long-poll wait.
pub const MAX_WAIT_SECONDS: u64 = 25;
/// Picture bytes shared by all cameras.
pub const TOTAL_PICTURE_BYTES: u64 = 256 * 1024 * 1024;

/// Read-only EZVIZ unified-message operations used by the poller.
#[async_trait]
pub trait AlarmSource: Send + Sync {
    /// `GET /v3/unifiedmsg/summarybydevice/v2?stype=92`.
    async fn alarm_summary(&self) -> Result<Value, BridgeError>;
    /// `GET /v3/unifiedmsg/list/v2` for one serial, day and `endTime` cursor.
    async fn alarm_list(
        &self,
        serial: &str,
        date: &str,
        end_time: &str,
    ) -> Result<Value, BridgeError>;
    /// Plain bounded GET of a pre-signed picture URL.
    async fn alarm_picture(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, BridgeError>;
    /// Ordered candidate device keys for `picCrypt=1` pictures; each is tried
    /// until the picture header hash accepts one.
    async fn picture_keys(&self, camera: &CameraConfig) -> Result<Vec<String>, BridgeError>;
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AlarmEvent {
    pub sequence: u64,
    pub id: String,
    pub occurred_at: i64,
    pub alarm_type: i64,
    pub category: String,
    pub title: String,
    pub has_picture: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AlarmBatch {
    pub camera: String,
    pub generation: Uuid,
    pub next_sequence: u64,
    pub events: Vec<AlarmEvent>,
}

pub(crate) struct CameraSlot {
    pub(crate) config: CameraConfig,
    pub(crate) history: Mutex<CameraHistory>,
    head: watch::Sender<u64>,
}

pub struct AlarmFeed {
    generation: Uuid,
    pub(crate) cameras: BTreeMap<String, CameraSlot>,
    pub(crate) metrics: Metrics,
    shutdown: CancellationToken,
    task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl AlarmFeed {
    /// Loads every configured camera's persisted history under `directory`.
    #[must_use]
    pub fn load(
        directory: &std::path::Path,
        cameras: &BTreeMap<String, CameraConfig>,
        metrics: Metrics,
    ) -> Arc<Self> {
        let budget = TOTAL_PICTURE_BYTES / u64::try_from(cameras.len().max(1)).unwrap_or(1);
        let cameras = cameras
            .iter()
            .map(|(alias, config)| {
                let history = CameraHistory::load(&directory.join(alias), config, budget);
                let (head, _receiver) = watch::channel(history.head());
                let slot = CameraSlot {
                    config: config.clone(),
                    history: Mutex::new(history),
                    head,
                };
                (alias.clone(), slot)
            })
            .collect();
        Arc::new(Self {
            generation: Uuid::new_v4(),
            cameras,
            metrics,
            shutdown: CancellationToken::new(),
            task: std::sync::Mutex::new(None),
        })
    }

    #[must_use]
    pub const fn generation(&self) -> Uuid {
        self.generation
    }

    /// Starts the background poller; a zero interval leaves polling disabled.
    pub fn spawn(self: &Arc<Self>, source: Arc<dyn AlarmSource>, interval: Duration) {
        if interval.is_zero() {
            return;
        }
        let task = tokio::spawn(poller::run(
            Arc::clone(self),
            source,
            interval,
            self.shutdown.clone(),
        ));
        if let Ok(mut slot) = self.task.lock()
            && let Some(previous) = slot.replace(task)
        {
            previous.abort();
        }
    }

    /// Stops polling and releases pending long-polls.
    pub fn shutdown(&self) {
        self.shutdown.cancel();
    }

    pub async fn close(&self) {
        self.shutdown();
        let task = self.task.lock().ok().and_then(|mut slot| slot.take());
        if let Some(task) = task {
            let _ignored = task.await;
        }
    }

    /// Appends a batch to one camera and wakes its long-polls.
    pub(crate) async fn publish(
        &self,
        alias: &str,
        batch: Vec<(AlarmRecord, Option<Vec<u8>>)>,
        live: bool,
    ) -> Result<store::InsertOutcome, BridgeError> {
        let slot = self.cameras.get(alias).ok_or(BridgeError::CameraNotFound)?;
        let mut history = slot.history.lock().await;
        let outcome = history.insert(batch, live)?;
        slot.head.send_replace(history.head());
        Ok(outcome)
    }

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
            return Ok(self.envelope(camera, history.head(), history.recent(MAX_BATCH)));
        };
        let deadline =
            tokio::time::Instant::now() + wait.min(Duration::from_secs(MAX_WAIT_SECONDS));
        let mut receiver = slot.head.subscribe();
        loop {
            let (events, next) = slot.history.lock().await.after(after, MAX_BATCH);
            let stale = next < after;
            if !events.is_empty() || stale || tokio::time::Instant::now() >= deadline {
                return Ok(self.envelope(camera, next, events));
            }
            tokio::select! {
                () = self.shutdown.cancelled() => return Ok(self.envelope(camera, next, events)),
                changed = tokio::time::timeout_at(deadline, receiver.changed()) => {
                    if !matches!(changed, Ok(Ok(()))) {
                        return Ok(self.envelope(camera, next, events));
                    }
                }
            }
        }
    }

    fn envelope(&self, camera: &str, next: u64, events: Vec<store::Entry>) -> AlarmBatch {
        AlarmBatch {
            camera: camera.to_owned(),
            generation: self.generation,
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

    /// Returns a stored JPEG for a retained alarm.
    pub async fn picture(&self, camera: &str, id: &str) -> Result<Option<Vec<u8>>, BridgeError> {
        let slot = self
            .cameras
            .get(camera)
            .ok_or(BridgeError::CameraNotFound)?;
        if !is_token(id) {
            return Ok(None);
        }
        let history = slot.history.lock().await;
        if history.picture_bytes(id).is_none() {
            return Ok(None);
        }
        Ok(history.pictures.read(id).await)
    }
}

/// Directory for persisted alarm state under the app data directory.
#[must_use]
pub fn data_directory(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("alarms")
}
