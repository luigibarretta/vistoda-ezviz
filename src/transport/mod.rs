mod client;
mod client_stream;
mod enroll;
mod http;
mod image;
mod token;
mod video;
mod video_pipeline;

pub use client::EzvizTransport;
pub use enroll::{EnrollmentState, PendingEnrollment, begin, enroll};
pub use token::EzvizToken;

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::{config::CameraConfig, error::BridgeError};

#[async_trait]
pub trait CameraTransport: Send + Sync {
    async fn snapshot_jpeg(&self, camera: &CameraConfig) -> Result<Vec<u8>, BridgeError>;

    async fn stream_mpeg_ps(
        &self,
        camera: &CameraConfig,
        cancel: CancellationToken,
        output: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError>;
}

pub trait ChunkConsumer: Send + Sync {
    fn consume(&self, chunk: Vec<u8>) -> bool;
}

#[must_use]
pub const fn timeout_duration(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}
