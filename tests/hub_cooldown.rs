//! After a camera goes offline the hub waits before waking it again, and a
//! subscriber that arrives meanwhile is told why.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use ezviz_vtm_bridge::{
    config::CameraConfig,
    error::BridgeError,
    hub::RawStreamHub,
    metrics::Metrics,
    transport::{CameraTransport, ChunkConsumer},
};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

struct Counting {
    starts: AtomicU64,
    error: fn() -> BridgeError,
}

#[async_trait]
impl CameraTransport for Counting {
    async fn snapshot_jpeg(&self, _: &CameraConfig) -> Result<Vec<u8>, BridgeError> {
        Ok(Vec::new())
    }

    async fn stream_mpeg_ps(
        &self,
        _: &CameraConfig,
        _: CancellationToken,
        _: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError> {
        self.starts.fetch_add(1, Ordering::AcqRel);
        Err((self.error)())
    }
}

fn hub(transport: Arc<Counting>) -> Arc<RawStreamHub> {
    let camera = CameraConfig {
        serial: "hidden".into(),
        channel: 1,
        substream: false,
        decrypt_video: false,
        media_key_file: None,
    };
    RawStreamHub::new(
        "front".into(),
        camera,
        transport,
        Metrics::default(),
        2,
        2,
        Duration::ZERO,
    )
}

async fn fail_once(hub: &Arc<RawStreamHub>) {
    let mut subscription = hub
        .subscribe()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(subscription.receiver.recv().await, None);
    hub.unsubscribe(subscription.id);
}

#[tokio::test]
async fn an_offline_camera_is_not_woken_again_by_an_immediate_reconnect() {
    let transport = Arc::new(Counting {
        starts: AtomicU64::new(0),
        error: || BridgeError::CameraOffline,
    });
    let hub = hub(Arc::clone(&transport));
    fail_once(&hub).await;
    let _reconnect = hub
        .subscribe()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    sleep(Duration::from_millis(2_500)).await;
    assert_eq!(
        transport.starts.load(Ordering::Acquire),
        1,
        "cooldown holds"
    );
    assert!(matches!(hub.startup_error(), BridgeError::CameraOffline));
    hub.close().await;
}

#[tokio::test]
async fn other_failures_keep_the_short_first_backoff() {
    let transport = Arc::new(Counting {
        starts: AtomicU64::new(0),
        error: || BridgeError::Upstream("relay hiccup".into()),
    });
    let hub = hub(Arc::clone(&transport));
    fail_once(&hub).await;
    let _reconnect = hub
        .subscribe()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    sleep(Duration::from_millis(2_500)).await;
    assert_eq!(
        transport.starts.load(Ordering::Acquire),
        2,
        "2 s backoff only"
    );
    hub.close().await;
}
