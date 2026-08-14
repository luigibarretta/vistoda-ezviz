use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

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

struct Generational {
    starts: AtomicU64,
}

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

#[async_trait]
impl CameraTransport for Generational {
    async fn snapshot_jpeg(&self, _: &CameraConfig) -> Result<Vec<u8>, BridgeError> {
        Ok(Vec::new())
    }

    async fn stream_mpeg_ps(
        &self,
        _: &CameraConfig,
        cancel: CancellationToken,
        output: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError> {
        let generation = self.starts.fetch_add(1, Ordering::AcqRel) + 1;
        output.consume(vec![u8::try_from(generation).unwrap_or(u8::MAX)]);
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

#[tokio::test]
async fn first_subscriber_after_idle_gets_a_fresh_upstream_generation() {
    let camera = CameraConfig {
        serial: "hidden".into(),
        decrypt_video: false,
        media_key_file: None,
    };
    let transport = Arc::new(Generational {
        starts: AtomicU64::new(0),
    });
    let hub = RawStreamHub::new(
        "front".into(),
        camera,
        transport,
        Metrics::default(),
        2,
        2,
        Duration::from_secs(30),
    );

    let mut first = hub
        .subscribe()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(first.receiver.recv().await, Some(Bytes::from_static(&[1])));
    hub.unsubscribe(first.id);

    let mut resumed = hub
        .subscribe()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        resumed.receiver.recv().await,
        Some(Bytes::from_static(&[2]))
    );
    hub.unsubscribe(resumed.id);
    hub.close().await;
}
