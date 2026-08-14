use std::{
    sync::{Arc, Weak, atomic::Ordering},
    time::{Duration, Instant},
};

use bytes::Bytes;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::{Failures, RawInner, lock};
use crate::{hub_failure::StreamFailure, transport::ChunkConsumer};

struct Distributor {
    hub: Weak<RawInner>,
    generation: u64,
}

impl ChunkConsumer for Distributor {
    fn consume(&self, chunk: Vec<u8>) -> bool {
        let Some(inner) = self.hub.upgrade() else {
            return false;
        };
        let mut subscribers = lock(&inner.subscribers);
        if inner.producer_generation.load(Ordering::Acquire) != self.generation {
            return false;
        }
        if subscribers.is_empty() {
            return !inner.closed.load(Ordering::Acquire);
        }
        for slice in chunk.chunks(64 * 1024) {
            let bytes = Bytes::copy_from_slice(slice);
            subscribers.retain(|_, sender| sender.try_send(bytes.clone()).is_ok());
        }
        report_startup(&inner);
        true
    }
}

fn report_startup(inner: &RawInner) {
    let Some(started) = lock(&inner.producer_started).take() else {
        return;
    };
    let metrics = inner.metrics.clone();
    let alias = inner.alias.clone();
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            metrics
                .gauge(
                    "upstream_startup_seconds",
                    &alias,
                    started.elapsed().as_secs_f64(),
                )
                .await;
        });
    }
}

pub(super) async fn run_producer(inner: Arc<RawInner>, cancel: CancellationToken, generation: u64) {
    let delay = lock(&inner.failures)
        .next_start
        .map(|start| start.saturating_duration_since(Instant::now()))
        .unwrap_or_default();
    if !delay.is_zero() {
        tokio::select! { () = sleep(delay) => {}, () = cancel.cancelled() => return }
    }
    *lock(&inner.producer_started) = Some(Instant::now());
    inner
        .metrics
        .increment("upstream_starts_total", &inner.alias)
        .await;
    inner
        .metrics
        .gauge("upstream_active", &inner.alias, 1.0)
        .await;
    let consumer: Arc<dyn ChunkConsumer> = Arc::new(Distributor {
        hub: Arc::downgrade(&inner),
        generation,
    });
    let result = inner
        .transport
        .stream_mpeg_ps(&inner.camera, cancel.clone(), consumer)
        .await;
    if inner.producer_generation.load(Ordering::Acquire) != generation {
        return;
    }
    record_result(&inner, &cancel, &result).await;
}

async fn record_result(
    inner: &RawInner,
    cancel: &CancellationToken,
    result: &Result<(), crate::error::BridgeError>,
) {
    if let Err(error) = result {
        tracing::warn!(
            camera = %inner.alias,
            error_type = error.diagnostic_code(),
            detail = error.diagnostic_detail(),
            "upstream stream producer stopped"
        );
        *lock(&inner.last_failure) = Some(StreamFailure::from_error(error));
    } else {
        *lock(&inner.last_failure) = None;
    }
    if !cancel.is_cancelled() && result.is_err() {
        inner
            .metrics
            .increment("upstream_failures_total", &inner.alias)
            .await;
        let mut failures = lock(&inner.failures);
        failures.count = failures.count.saturating_add(1);
        let backoff = if failures.count >= 3 {
            Duration::from_secs(600)
        } else {
            Duration::from_secs(2_u64.pow(failures.count))
        };
        failures.next_start = Some(Instant::now() + backoff);
    } else {
        *lock(&inner.failures) = Failures::default();
    }
    lock(&inner.producer_started).take();
    inner
        .metrics
        .gauge("upstream_active", &inner.alias, 0.0)
        .await;
    lock(&inner.subscribers).clear();
    inner
        .metrics
        .gauge("raw_subscribers", &inner.alias, 0.0)
        .await;
}
