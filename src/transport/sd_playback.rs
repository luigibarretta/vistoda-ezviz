//! microSD playback through the cloud VTM relay (ADR-0020, ADR-0024).
//!
//! The relay serves a recorded interval through the same VTDU allocation and
//! media framing as live view, so the clear and encrypted pipelines are
//! reused unchanged; only the request differs (`/playback?`, client type 3,
//! stream-info type 10). The relay closes the session after the interval.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::{
    config::CameraConfig,
    device::candidate_keys,
    error::BridgeError,
    transport::{ChunkConsumer, EzvizTransport, timeout_duration, video::RtpVideo, video_pipeline},
    vtm::VtmSession,
};

impl EzvizTransport {
    pub(super) async fn stream_sd(
        &self,
        camera: &CameraConfig,
        (begin, end): (&str, &str),
        cancel: CancellationToken,
        output: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError> {
        let keys = if self.video_is_encrypted(camera).await {
            let status = self.cached_status(camera).await.ok();
            let keys = candidate_keys(self, camera, status.as_ref()).await;
            if keys.is_empty() {
                return Err(BridgeError::Configuration(
                    "encrypted camera has no usable verification code".into(),
                ));
            }
            Some(keys)
        } else {
            None
        };
        let url = self.cloud_playback_url(camera, begin, end).await?;
        let session =
            VtmSession::connect_playback(url, timeout_duration(self.timeout_seconds)).await?;
        match keys.as_ref().and_then(|keys| keys.first()) {
            Some(key) => {
                video_pipeline::copy_as_mpeg_ts(
                    session,
                    RtpVideo::encrypted(key.code())?,
                    None,
                    true,
                    cancel,
                    output,
                )
                .await
            }
            None => Self::copy_clear(session, cancel, output).await,
        }
    }
}
