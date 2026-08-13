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
