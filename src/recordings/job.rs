use std::time::Duration;

use tokio::time::timeout;

use super::{RecordingManager, model::utc_now, writer::Written};
use crate::error::BridgeError;

/// What a queued recording captures.
pub(super) enum Job {
    /// The shared live hub for `requested_duration_seconds`.
    Live,
    /// One microSD interval; camera-local `yyyyMMddTHHmmssZ` bounds.
    Sd { begin: String, end: String },
}

impl RecordingManager {
    pub(super) async fn record(&self, id: &str, job: Job) {
        let result = match job {
            Job::Live => self.capture(id).await,
            Job::Sd { begin, end } => self.capture_sd(id, &begin, &end).await,
        };
        let mut state = self.state.lock().await;
        let Some(manifest) = state.manifests.get_mut(id) else {
            return;
        };
        if let Err(error) = result {
            tracing::warn!(
                camera = %manifest.camera,
                error_type = error.diagnostic_code(),
                detail = %error.logged_detail(),
                "recording failed"
            );
            let code = match error {
                BridgeError::Capacity(_) => "capacity",
                BridgeError::Recording(_) => "invalid_media",
                BridgeError::CameraOffline => "camera_offline",
                BridgeError::SdNoRecording => "sd_no_recording",
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

    /// Marks a queued recording active; returns its camera and duration.
    pub(super) async fn begin(&self, id: &str) -> Result<(String, u64), BridgeError> {
        let mut state = self.state.lock().await;
        let manifest = state
            .manifests
            .get_mut(id)
            .ok_or_else(|| BridgeError::Recording("recording disappeared".into()))?;
        manifest.status = "recording".into();
        let value = (manifest.camera.clone(), manifest.requested_duration_seconds);
        self.persist(&state)?;
        Ok(value)
    }

    async fn capture(&self, id: &str) -> Result<(), BridgeError> {
        let (camera, duration) = self.begin(id).await?;
        let hub = self
            .hubs
            .get(&camera)
            .ok_or_else(|| BridgeError::Recording("camera alias is not configured".into()))?;
        let mut subscription = hub.subscribe().await?;
        let limit = Duration::from_secs(duration);
        let result = if let Ok(result) = timeout(
            limit + Duration::from_secs(45),
            self.write_media(id, &mut subscription.receiver, Some(limit)),
        )
        .await
        {
            result
        } else {
            self.discard_partial(id).await;
            Err(BridgeError::Recording("recording timed out".into()))
        };
        hub.unsubscribe(subscription.id);
        let written = result?;
        let measured = (written.started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0;
        self.complete(id, &camera, &written, measured).await
    }

    /// Publishes a written file in the manifest.
    pub(super) async fn complete(
        &self,
        id: &str,
        camera: &str,
        written: &Written,
        duration_seconds: f64,
    ) -> Result<(), BridgeError> {
        let mut state = self.state.lock().await;
        let manifest = state
            .manifests
            .get_mut(id)
            .ok_or_else(|| BridgeError::Recording("recording disappeared".into()))?;
        manifest.status = "ready".into();
        manifest.started_at = Some(written.started_at.clone());
        manifest.completed_at = Some(utc_now());
        manifest.actual_duration_seconds = Some(duration_seconds);
        manifest.bytes = Some(written.bytes);
        manifest.sha256 = Some(written.sha256.clone());
        manifest.media_type = written.format.media_type().into();
        manifest.error_code = None;
        self.metrics
            .increment("recordings_ready_total", camera)
            .await;
        Ok(())
    }
}
