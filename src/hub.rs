use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex as StdMutex, MutexGuard, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use tokio::{sync::mpsc, task::JoinHandle, time::sleep};
use tokio_util::sync::CancellationToken;

use crate::{
    config::CameraConfig,
    error::BridgeError,
    hub_failure::StreamFailure,
    metrics::Metrics,
    transport::{CameraTransport, ChunkConsumer},
};

pub struct HubSubscription {
    pub id: u64,
    pub receiver: mpsc::Receiver<Bytes>,
    hub: Weak<RawInner>,
}

impl Drop for HubSubscription {
    fn drop(&mut self) {
        if let Some(inner) = self.hub.upgrade() {
            remove_subscriber(inner, self.id);
        }
    }
}

struct Producer {
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

#[derive(Default)]
struct Failures {
    count: u32,
    next_start: Option<Instant>,
}

pub struct RawStreamHub {
    inner: Arc<RawInner>,
}

struct RawInner {
    alias: String,
    camera: CameraConfig,
    transport: Arc<dyn CameraTransport>,
    metrics: Metrics,
    subscribers: StdMutex<BTreeMap<u64, mpsc::Sender<Bytes>>>,
    producer: StdMutex<Option<Producer>>,
    failures: StdMutex<Failures>,
    producer_started: StdMutex<Option<Instant>>,
    next_id: AtomicU64,
    queue_chunks: usize,
    max_subscribers: usize,
    idle_grace: Duration,
    closed: AtomicBool,
    last_failure: StdMutex<Option<StreamFailure>>,
}

impl RawStreamHub {
    #[must_use]
    pub fn new(
        alias: String,
        camera: CameraConfig,
        transport: Arc<dyn CameraTransport>,
        metrics: Metrics,
        queue_chunks: usize,
        max_subscribers: usize,
        idle_grace: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(RawInner {
                alias,
                camera,
                transport,
                metrics,
                subscribers: StdMutex::new(BTreeMap::new()),
                producer: StdMutex::new(None),
                failures: StdMutex::new(Failures::default()),
                producer_started: StdMutex::new(None),
                next_id: AtomicU64::new(1),
                queue_chunks,
                max_subscribers,
                idle_grace,
                closed: AtomicBool::new(false),
                last_failure: StdMutex::new(None),
            }),
        })
    }

    pub async fn subscribe(self: &Arc<Self>) -> Result<HubSubscription, BridgeError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(BridgeError::Upstream("stream hub is closed".into()));
        }
        let (sender, receiver) = mpsc::channel(self.inner.queue_chunks);
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let count = {
            let mut subscribers = lock(&self.inner.subscribers);
            if subscribers.len() >= self.inner.max_subscribers {
                return Err(BridgeError::Capacity(
                    "maximum stream subscribers reached".into(),
                ));
            }
            subscribers.insert(id, sender);
            subscribers.len()
        };
        self.inner
            .metrics
            .gauge("raw_subscribers", &self.inner.alias, count as f64)
            .await;
        self.ensure_running();
        Ok(HubSubscription {
            id,
            receiver,
            hub: Arc::downgrade(&self.inner),
        })
    }

    pub fn unsubscribe(&self, id: u64) {
        remove_subscriber(Arc::clone(&self.inner), id);
    }

    #[must_use]
    pub fn startup_error(&self) -> BridgeError {
        lock(&self.inner.last_failure)
            .map_or(BridgeError::UpstreamUnavailable, StreamFailure::into_error)
    }

    fn ensure_running(&self) {
        let mut producer = lock(&self.inner.producer);
        if producer
            .as_ref()
            .is_some_and(|current| !current.task.is_finished())
        {
            return;
        }
        let cancel = CancellationToken::new();
        *lock(&self.inner.last_failure) = None;
        let inner = Arc::clone(&self.inner);
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { run_producer(inner, task_cancel).await });
        *producer = Some(Producer { cancel, task });
    }

    pub async fn close(&self) {
        self.inner.closed.store(true, Ordering::Release);
        let producer = lock(&self.inner.producer).take();
        if let Some(producer) = producer {
            producer.cancel.cancel();
            let _result = producer.task.await;
        }
        lock(&self.inner.subscribers).clear();
    }
}

fn remove_subscriber(inner: Arc<RawInner>, id: u64) {
    let count = {
        let mut subscribers = lock(&inner.subscribers);
        subscribers.remove(&id);
        subscribers.len()
    };
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            inner
                .metrics
                .gauge("raw_subscribers", &inner.alias, count as f64)
                .await;
            if count == 0 {
                sleep(inner.idle_grace).await;
                if lock(&inner.subscribers).is_empty()
                    && let Some(producer) = lock(&inner.producer).as_ref()
                {
                    producer.cancel.cancel();
                }
            }
        });
    }
}

struct Distributor {
    hub: Weak<RawInner>,
}

impl ChunkConsumer for Distributor {
    fn consume(&self, chunk: Vec<u8>) -> bool {
        let Some(inner) = self.hub.upgrade() else {
            return false;
        };
        let mut subscribers = lock(&inner.subscribers);
        if subscribers.is_empty() {
            return !inner.closed.load(Ordering::Acquire);
        }
        for slice in chunk.chunks(64 * 1024) {
            let bytes = Bytes::copy_from_slice(slice);
            subscribers.retain(|_, sender| sender.try_send(bytes.clone()).is_ok());
        }
        let producer_started = lock(&inner.producer_started).take();
        if let Some(started) = producer_started {
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
        true
    }
}

async fn run_producer(inner: Arc<RawInner>, cancel: CancellationToken) {
    let delay = lock(&inner.failures)
        .next_start
        .map(|start| start.saturating_duration_since(Instant::now()))
        .unwrap_or_default();
    if !delay.is_zero() {
        tokio::select! { () = sleep(delay) => {}, () = cancel.cancelled() => return }
    }
    let started = Instant::now();
    *lock(&inner.producer_started) = Some(started);
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
    });
    let result = inner
        .transport
        .stream_mpeg_ps(&inner.camera, cancel.clone(), consumer)
        .await;
    if let Err(error) = &result {
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

fn lock<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
