use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use bytes::Bytes;
use ezviz_vtm_bridge::{
    config::CameraConfig,
    error::BridgeError,
    hub::RawStreamHub,
    metrics::Metrics,
    transport::{CameraTransport, ChunkConsumer},
};
use tokio_util::sync::CancellationToken;

struct Fake;

struct Offline;

#[async_trait]
impl CameraTransport for Fake {
    async fn snapshot_jpeg(&self, _: &CameraConfig) -> Result<Vec<u8>, BridgeError> {
        Ok(Vec::new())
    }

    async fn stream_mpeg_ps(
        &self,
        _: &CameraConfig,
        cancel: CancellationToken,
        output: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError> {
        output.consume(vec![1, 2, 3]);
        cancel.cancelled().await;
        Ok(())
    }
}

#[async_trait]
impl CameraTransport for Offline {
    async fn snapshot_jpeg(&self, _: &CameraConfig) -> Result<Vec<u8>, BridgeError> {
        Err(BridgeError::CameraOffline)
    }

    async fn stream_mpeg_ps(
        &self,
        _: &CameraConfig,
        _: CancellationToken,
        _: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError> {
        Err(BridgeError::CameraOffline)
    }
}

#[tokio::test]
async fn subscribers_share_one_bounded_hub() {
    let camera = CameraConfig {
        serial: "hidden".into(),
        decrypt_video: false,
        media_key_file: None,
    };
    let hub = RawStreamHub::new(
        "front".into(),
        camera,
        Arc::new(Fake),
        Metrics::default(),
        2,
        2,
        Duration::ZERO,
    );
    let mut first = hub
        .subscribe()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        first.receiver.recv().await,
        Some(Bytes::from_static(&[1, 2, 3]))
    );
    hub.unsubscribe(first.id);
    hub.close().await;
}

#[tokio::test]
async fn startup_preserves_camera_offline_failure() {
    let camera = CameraConfig {
        serial: "hidden".into(),
        decrypt_video: false,
        media_key_file: None,
    };
    let hub = RawStreamHub::new(
        "front".into(),
        camera,
        Arc::new(Offline),
        Metrics::default(),
        2,
        2,
        Duration::ZERO,
    );
    let mut subscription = hub
        .subscribe()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(subscription.receiver.recv().await, None);
    assert!(matches!(hub.startup_error(), BridgeError::CameraOffline));
    hub.close().await;
}
