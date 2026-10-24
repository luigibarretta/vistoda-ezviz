//! microSD intervals saved as ordinary recordings (ADR-0024): the same
//! manifest, SHA-256, spool quota, MP4 playback, NFS backup and `SceneTrove`
//! acknowledgement as a live capture, from a VTM playback session instead of
//! the live hub.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use tokio::{sync::mpsc, time::timeout};
use tokio_util::sync::CancellationToken;

use super::{RecordingManager, RecordingManifest, job::Job};
use crate::{error::BridgeError, transport::ChunkConsumer};

/// Chunks buffered between the playback session and the spool writer.
const QUEUE_CHUNKS: usize = 512;
const CHUNK_BYTES: usize = 64 * 1024;
/// Time allowed beyond the interval for waking, transfer and teardown.
const MARGIN: Duration = Duration::from_secs(120);

/// One requested interval: epoch seconds for the manifest, camera-local
/// `yyyyMMddTHHmmssZ` for the relay.
pub struct SdInterval {
    pub start: i64,
    pub end: i64,
    pub begin_local: String,
    pub end_local: String,
}

/// The producer owns the only sender, so the writer sees the end of the
/// interval when the playback session returns.
struct Channel {
    sender: mpsc::Sender<Bytes>,
    overflowed: Arc<AtomicBool>,
}

impl ChunkConsumer for Channel {
    fn consume(&self, chunk: Vec<u8>) -> bool {
        for slice in chunk.chunks(CHUNK_BYTES) {
            if self.sender.try_send(Bytes::copy_from_slice(slice)).is_err() {
                self.overflowed.store(true, Ordering::Release);
                return false;
            }
        }
        true
    }
}

impl RecordingManager {
    pub async fn start_sd(
        self: &Arc<Self>,
        camera: &str,
        interval: SdInterval,
        idempotency_key: &str,
    ) -> Result<RecordingManifest, BridgeError> {
        let hub = self
            .hubs
            .get(camera)
            .ok_or_else(|| BridgeError::Recording("camera alias is not configured".into()))?;
        let length = interval.end.saturating_sub(interval.start);
        if !(1..=i64::try_from(self.max_duration).unwrap_or(i64::MAX)).contains(&length) {
            return Err(BridgeError::Recording(format!(
                "microSD interval must last between 1 and {} seconds",
                self.max_duration
            )));
        }
        let manifest = RecordingManifest::pending_sd(camera, interval.start, interval.end);
        let retry = self
            .state
            .lock()
            .await
            .idempotency
            .contains_key(idempotency_key);
        if !retry && hub.is_live() {
            return Err(BridgeError::Capacity(
                "camera is streaming live; save the clip after live view ends".into(),
            ));
        }
        let job = Job::Sd {
            begin: interval.begin_local,
            end: interval.end_local,
        };
        self.enqueue(idempotency_key, manifest, job, true).await
    }

    pub(super) async fn capture_sd(
        &self,
        id: &str,
        begin: &str,
        end: &str,
    ) -> Result<(), BridgeError> {
        let (camera, duration) = self.begin(id).await?;
        let hub = Arc::clone(
            self.hubs
                .get(&camera)
                .ok_or_else(|| BridgeError::Recording("camera alias is not configured".into()))?,
        );
        let (sender, mut receiver) = mpsc::channel(QUEUE_CHUNKS);
        let overflowed = Arc::new(AtomicBool::new(false));
        let output: Arc<dyn ChunkConsumer> = Arc::new(Channel {
            sender,
            overflowed: Arc::clone(&overflowed),
        });
        let cancel = CancellationToken::new();
        let interval = (begin.to_owned(), end.to_owned());
        let task_cancel = cancel.clone();
        let session = tokio::spawn(async move {
            hub.sd_playback((&interval.0, &interval.1), task_cancel, output)
                .await
        });
        let limit = Duration::from_secs(duration) + MARGIN;
        let written =
            if let Ok(written) = timeout(limit, self.write_media(id, &mut receiver, None)).await {
                written
            } else {
                self.discard_partial(id).await;
                Err(BridgeError::Recording("microSD transfer timed out".into()))
            };
        cancel.cancel();
        let produced = session
            .await
            .unwrap_or_else(|_| Err(BridgeError::Upstream("microSD task failed".into())));
        let overflow = overflowed.load(Ordering::Acquire);
        let written = match (written, produced) {
            // The session's own error explains an empty or partial transfer
            // better than the writer's, and a partial file is never kept.
            (Err(_), Err(error)) | (Err(error), Ok(())) => return Err(error),
            (Ok(written), result) => {
                let refused = match result {
                    Err(error) => Some(error),
                    Ok(()) if overflow => Some(BridgeError::Capacity(
                        "microSD transfer outran the spool".into(),
                    )),
                    Ok(()) => None,
                };
                if let Some(error) = refused {
                    let name = format!("{id}.{}", written.format.extension());
                    let _ignored = tokio::fs::remove_file(self.directory.join(name)).await;
                    return Err(error);
                }
                written
            }
        };
        let length = f64::from(u32::try_from(duration).unwrap_or(u32::MAX));
        self.complete(id, &camera, &written, length).await
    }
}
