//! Cached cloud device status and the encrypted-video runtime decision.
//! Errors never carry URLs, serials, codes or session identifiers.

use std::{sync::Arc, time::Duration};

use serde_json::Value;
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;

use crate::{
    config::CameraConfig,
    device::{DeviceStatus, ENCRYPTION_TTL, FAILURE_TTL, candidate_keys, parse_device_status},
    error::BridgeError,
    transport::{
        ChunkConsumer, EzvizTransport, client::StatusEntry, timeout_duration, video_pipeline,
    },
    vtm::VtmSession,
};

const PAGELIST_PATH: &str = "/v3/userdevices/v1/resources/pagelist";
const PAGE_SIZE: usize = 50;
const MAX_PAGES: usize = 40;

/// Longest wait for cloud status before a live start falls back to config.
const LIVE_STATUS_WAIT: Duration = Duration::from_secs(3);

impl EzvizTransport {
    fn status_slot(&self, serial: &str) -> Arc<tokio::sync::Mutex<Option<StatusEntry>>> {
        let mut map = self
            .status_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(map.entry(serial.to_owned()).or_default())
    }

    /// Cached device status: single flight per serial, the shared map lock is
    /// never held across I/O, and failures are cached for `FAILURE_TTL`.
    pub(super) async fn cached_status(
        &self,
        camera: &CameraConfig,
    ) -> Result<DeviceStatus, BridgeError> {
        let slot = self.status_slot(&camera.serial);
        let mut entry = slot.lock().await;
        if let Some(cached) = entry
            .as_ref()
            .filter(|cached| cached.expires > Instant::now())
        {
            return cached
                .status
                .clone()
                .ok_or(BridgeError::UpstreamUnavailable);
        }
        let result = self.fetch_status(&camera.serial).await;
        let ttl = if result.is_ok() {
            ENCRYPTION_TTL
        } else {
            FAILURE_TTL
        };
        *entry = Some(StatusEntry {
            expires: Instant::now() + ttl,
            status: result.as_ref().ok().cloned(),
        });
        result
    }

    /// Records a failed lookup unless another caller is fetching right now.
    fn mark_status_unavailable(&self, serial: &str) {
        if let Ok(mut entry) = self.status_slot(serial).try_lock() {
            *entry = Some(StatusEntry {
                expires: Instant::now() + FAILURE_TTL,
                status: None,
            });
        }
    }

    async fn fetch_status(&self, serial: &str) -> Result<DeviceStatus, BridgeError> {
        let mut offset = 0_usize;
        for _ in 0..MAX_PAGES {
            let page = self
                .message_json(
                    PAGELIST_PATH,
                    &[
                        ("groupId", "-1".into()),
                        ("limit", PAGE_SIZE.to_string()),
                        ("offset", offset.to_string()),
                        ("filter", "STATUS".into()),
                    ],
                )
                .await?;
            if let Some(status) = parse_device_status(&page, serial) {
                return Ok(status);
            }
            let has_next = page
                .get("page")
                .and_then(|value| value.get("hasNext"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !has_next {
                break;
            }
            offset = offset.saturating_add(PAGE_SIZE);
        }
        Err(BridgeError::Upstream(
            "camera status was not found in the device list".into(),
        ))
    }

    /// Runtime path decision: the cloud `isEncrypt` flag wins; the
    /// configuration is only a fallback when status is unavailable within
    /// `LIVE_STATUS_WAIT`, so a clear camera never waits longer than that.
    pub(super) async fn video_is_encrypted(&self, camera: &CameraConfig) -> bool {
        let lookup = timeout(LIVE_STATUS_WAIT, self.cached_status(camera)).await;
        if lookup.is_err() {
            self.mark_status_unavailable(&camera.serial);
        }
        let reported = lookup
            .ok()
            .and_then(Result::ok)
            .and_then(|status| status.video_encrypted);
        encrypted_path(camera.decrypt_video, reported)
    }

    /// Decrypts RTP video, trying each usable code once; a code rejected by
    /// the first parameter set falls through to the next source.
    pub(super) async fn stream_encrypted_video(
        &self,
        camera: &CameraConfig,
        cancel: CancellationToken,
        output: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError> {
        let status = self.cached_status(camera).await.ok();
        let keys = candidate_keys(self, camera, status.as_ref()).await;
        if keys.is_empty() {
            return Err(BridgeError::Configuration(
                "encrypted camera has no usable verification code".into(),
            ));
        }
        let last = keys.len() - 1;
        for (index, key) in keys.iter().enumerate() {
            let url = self.cloud_stream_url(camera).await?;
            let session = VtmSession::connect(url, timeout_duration(self.timeout_seconds)).await?;
            let result = video_pipeline::copy_as_mpeg_ts(
                session,
                key.code(),
                &self.ffmpeg_path,
                self.timeout_seconds,
                cancel.clone(),
                Arc::clone(&output),
            )
            .await;
            match result {
                Err(BridgeError::Authentication) if index < last && !cancel.is_cancelled() => {
                    tracing::warn!("verification code was rejected by the stream; trying next");
                }
                other => return other,
            }
        }
        Err(BridgeError::Authentication)
    }
}

/// Fetched `isEncrypt` overrides the configured flag in both directions.
#[must_use]
pub(super) const fn encrypted_path(configured: bool, reported: Option<bool>) -> bool {
    match reported {
        Some(encrypted) => encrypted,
        None => configured,
    }
}

#[cfg(test)]
mod tests {
    use super::encrypted_path;

    #[test]
    fn reported_encryption_overrides_the_configured_flag() {
        // A verification code on a clear camera keeps the plain path.
        assert!(!encrypted_path(true, Some(false)));
        // An encrypted camera takes the decrypting path with or without a code.
        assert!(encrypted_path(true, Some(true)));
        assert!(encrypted_path(false, Some(true)));
        // Unknown status falls back to the configuration (0.8 behavior).
        assert!(!encrypted_path(false, None));
        assert!(encrypted_path(true, None));
    }
}
