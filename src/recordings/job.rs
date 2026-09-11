use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    time::timeout,
};

use super::{RecordingManager, model::utc_now};
use crate::error::BridgeError;

const TS_PACKET_BYTES: usize = 188;
const PACK_START: &[u8] = b"\x00\x00\x01\xba";

#[derive(Clone, Copy)]
enum RecordingFormat {
    MpegPs,
    MpegTs,
}

impl RecordingFormat {
    const fn extension(self) -> &'static str {
        match self {
            Self::MpegPs => "mpegps",
            Self::MpegTs => "ts",
        }
    }

    const fn media_type(self) -> &'static str {
        match self {
            Self::MpegPs => "video/mpeg",
            Self::MpegTs => "video/mp2t",
        }
    }
}

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
                error = %error,
                error_type = "recording_journal",
                "recording journal persist failed"
            );
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
            let mut format = None;
            while let Some(source) = subscription.receiver.recv().await {
                let chunk = if started.is_none() {
                    alignment.extend_from_slice(&source);
                    let Some((marker, detected)) = media_alignment(&alignment) else {
                        if alignment.len() > 1024 * 1024 {
                            return Err(BridgeError::Recording(
                                "MPEG-PS or MPEG-TS alignment was not found".into(),
                            ));
                        }
                        continue;
                    };
                    started = Some(Instant::now());
                    started_at = Some(utc_now());
                    format = Some(detected);
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
                BridgeError::Recording("recording contained no aligned media".into())
            })?;
            let format = format.ok_or_else(|| {
                BridgeError::Recording("recording format was not identified".into())
            })?;
            file.sync_all().await?;
            drop(file);
            let final_path = self.directory.join(format!("{id}.{}", format.extension()));
            fs::rename(&partial, &final_path).await?;
            std::fs::File::open(&self.directory)?.sync_all()?;
            Ok::<_, BridgeError>((
                started,
                started_at.unwrap_or_else(utc_now),
                written,
                hex::encode(digest.finalize()),
                format,
            ))
        };
        let result = timeout(Duration::from_secs(duration + 45), work)
            .await
            .unwrap_or_else(|_| Err(BridgeError::Recording("recording timed out".into())));
        hub.unsubscribe(subscription.id);
        let (started, started_at, written, digest, format) = match result {
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
        manifest.media_type = format.media_type().into();
        manifest.error_code = None;
        self.metrics
            .increment("recordings_ready_total", &camera)
            .await;
        Ok(())
    }
}

fn media_alignment(data: &[u8]) -> Option<(usize, RecordingFormat)> {
    if let Some(offset) = data
        .windows(PACK_START.len())
        .position(|window| window == PACK_START)
    {
        return Some((offset, RecordingFormat::MpegPs));
    }
    (0..data.len().min(TS_PACKET_BYTES)).find_map(|offset| {
        (data.get(offset) == Some(&0x47)
            && data.get(offset + TS_PACKET_BYTES) == Some(&0x47)
            && data.get(offset + TS_PACKET_BYTES * 2) == Some(&0x47))
        .then_some((offset, RecordingFormat::MpegTs))
    })
}

#[cfg(test)]
mod tests {
    use super::{PACK_START, RecordingFormat, TS_PACKET_BYTES, media_alignment};

    #[test]
    fn finds_ts_sync_after_an_untrusted_prefix() {
        let mut data = vec![0xaa; 17 + TS_PACKET_BYTES * 3];
        for index in 0..3 {
            data[17 + index * TS_PACKET_BYTES] = 0x47;
        }
        assert!(matches!(
            media_alignment(&data),
            Some((17, RecordingFormat::MpegTs))
        ));
        data[17 + TS_PACKET_BYTES * 2] = 0;
        assert!(media_alignment(&data).is_none());
        let mut ps = vec![0xaa; 8];
        ps.extend_from_slice(PACK_START);
        assert!(matches!(
            media_alignment(&ps),
            Some((8, RecordingFormat::MpegPs))
        ));
    }
}
