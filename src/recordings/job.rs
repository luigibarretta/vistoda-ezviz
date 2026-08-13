use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    time::timeout,
};

use super::{RecordingManager, model::utc_now};
use crate::error::BridgeError;

const PACK_START: &[u8] = b"\x00\x00\x01\xba";

impl RecordingManager {
    pub(super) async fn record(&self, id: &str) {
        let result = self.capture(id).await;
        let mut state = self.state.lock().await;
        let Some(manifest) = state.manifests.get_mut(id) else {
            return;
        };
        if let Err(error) = result {
            let code = match error {
                BridgeError::Capacity(_) => "capacity",
                BridgeError::Recording(_) => "invalid_media",
                _ => "upstream_failure",
            };
            manifest.fail(code);
            self.metrics
                .increment("recordings_failed_total", &manifest.camera)
                .await;
        }
        if let Err(error) = self.persist(&state) {
            tracing::error!(
                error_type = "recording_journal",
                "recording journal persist failed"
            );
            let _ignored = error;
        }
    }

    async fn capture(&self, id: &str) -> Result<(), BridgeError> {
        let (camera, duration) = {
            let mut state = self.state.lock().await;
            let (camera, duration) = {
                let manifest = state
                    .manifests
                    .get_mut(id)
                    .ok_or_else(|| BridgeError::Recording("recording disappeared".into()))?;
                manifest.status = "recording".into();
                (manifest.camera.clone(), manifest.requested_duration_seconds)
            };
            self.persist(&state)?;
            (camera, duration)
        };
        let hub = self
            .hubs
            .get(&camera)
            .ok_or_else(|| BridgeError::Recording("camera alias is not configured".into()))?;
        let mut subscription = hub.subscribe().await?;
        let partial = self.directory.join(format!(".{id}.partial"));
        let final_path = self.directory.join(format!("{id}.mpegps"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&partial)
            .await?;
        let work = async {
            let mut digest = Sha256::new();
            let mut written = 0_u64;
            let mut alignment = Vec::new();
            let mut started = None;
            let mut started_at = None;
            while let Some(source) = subscription.receiver.recv().await {
                let chunk = if started.is_none() {
                    alignment.extend_from_slice(&source);
                    let Some(marker) = alignment
                        .windows(PACK_START.len())
                        .position(|window| window == PACK_START)
                    else {
                        if alignment.len() > 1024 * 1024 {
                            return Err(BridgeError::Recording(
                                "MPEG-PS pack header was not found".into(),
                            ));
                        }
                        continue;
                    };
                    started = Some(Instant::now());
                    started_at = Some(utc_now());
                    alignment.split_off(marker)
                } else {
                    source.to_vec()
                };
                let next = written.saturating_add(chunk.len() as u64);
                if next > self.max_bytes {
                    return Err(BridgeError::Capacity(
                        "recording reached the configured byte limit".into(),
                    ));
                }
                file.write_all(&chunk).await?;
                digest.update(&chunk);
                written = next;
                if started.is_some_and(|instant| instant.elapsed() >= Duration::from_secs(duration))
                {
                    break;
                }
            }
            let started = started.ok_or_else(|| {
                BridgeError::Recording("recording contained no aligned MPEG-PS media".into())
            })?;
            file.sync_all().await?;
            drop(file);
            fs::rename(&partial, &final_path).await?;
            std::fs::File::open(&self.directory)?.sync_all()?;
            Ok::<_, BridgeError>((
                started,
                started_at.unwrap_or_else(utc_now),
                written,
                hex::encode(digest.finalize()),
            ))
        };
        let result = timeout(Duration::from_secs(duration + 45), work)
            .await
            .unwrap_or_else(|_| Err(BridgeError::Recording("recording timed out".into())));
        hub.unsubscribe(subscription.id);
        let (started, started_at, written, digest) = match result {
            Ok(value) => value,
            Err(error) => {
                let _ignored = fs::remove_file(&partial).await;
                return Err(error);
            }
        };
        let mut state = self.state.lock().await;
        let manifest = state
            .manifests
            .get_mut(id)
            .ok_or_else(|| BridgeError::Recording("recording disappeared".into()))?;
        manifest.status = "ready".into();
        manifest.started_at = Some(started_at);
        manifest.completed_at = Some(utc_now());
        manifest.actual_duration_seconds =
            Some((started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0);
        manifest.bytes = Some(written);
        manifest.sha256 = Some(digest);
        manifest.error_code = None;
        self.metrics
            .increment("recordings_ready_total", &camera)
            .await;
        Ok(())
    }
}
