use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::Mutex;

use crate::{
    config::CameraConfig, error::BridgeError, metrics::Metrics, transport::CameraTransport,
};

#[derive(Clone)]
struct CachedSnapshot {
    data: Arc<Vec<u8>>,
    created: Instant,
}

pub struct SnapshotService {
    cameras: BTreeMap<String, CameraConfig>,
    transport: Arc<dyn CameraTransport>,
    metrics: Metrics,
    cache_duration: Duration,
    stale_duration: Duration,
    states: BTreeMap<String, Mutex<Option<CachedSnapshot>>>,
}

impl SnapshotService {
    #[must_use]
    pub fn new(
        cameras: BTreeMap<String, CameraConfig>,
        transport: Arc<dyn CameraTransport>,
        metrics: Metrics,
        cache_duration: Duration,
        stale_duration: Duration,
    ) -> Self {
        let states = cameras
            .keys()
            .map(|alias| (alias.clone(), Mutex::new(None)))
            .collect();
        Self {
            cameras,
            transport,
            metrics,
            cache_duration,
            stale_duration,
            states,
        }
    }

    pub async fn get(&self, alias: &str) -> Result<Arc<Vec<u8>>, BridgeError> {
        let camera = self.cameras.get(alias).ok_or(BridgeError::CameraNotFound)?;
        let state = self.states.get(alias).ok_or(BridgeError::CameraNotFound)?;
        let mut cached = state.lock().await;
        if cached
            .as_ref()
            .is_some_and(|value| value.created.elapsed() <= self.cache_duration)
        {
            self.metrics
                .increment("snapshot_cache_hits_total", alias)
                .await;
            return Ok(Arc::clone(
                &cached
                    .as_ref()
                    .ok_or_else(|| BridgeError::Upstream("snapshot cache disappeared".into()))?
                    .data,
            ));
        }
        match self.transport.snapshot_jpeg(camera).await {
            Ok(image) => {
                let data = Arc::new(image);
                *cached = Some(CachedSnapshot {
                    data: Arc::clone(&data),
                    created: Instant::now(),
                });
                self.metrics.increment("snapshots_total", alias).await;
                Ok(data)
            }
            Err(error) => {
                if let Some(existing) = cached
                    .as_ref()
                    .filter(|value| value.created.elapsed() <= self.stale_duration)
                {
                    self.metrics
                        .increment("snapshot_stale_fallbacks_total", alias)
                        .await;
                    return Ok(Arc::clone(&existing.data));
                }
                Err(error)
            }
        }
    }
}
