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
    config::CameraConfig, error::BridgeError, hub_failure::StreamFailure, metrics::Metrics,
    transport::CameraTransport,
};

#[path = "hub_producer.rs"]
mod producer;
use producer::run_producer;

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
    producer_generation: AtomicU64,
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
                producer_generation: AtomicU64::new(0),
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
            let needs_fresh_stream = subscribers.is_empty()
                && lock(&self.inner.producer)
                    .as_ref()
                    .is_some_and(|current| !current.task.is_finished());
            self.ensure_running(needs_fresh_stream);
            subscribers.insert(id, sender);
            subscribers.len()
        };
        self.inner
            .metrics
            .gauge("raw_subscribers", &self.inner.alias, count as f64)
            .await;
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

    fn ensure_running(&self, restart: bool) {
        let mut producer = lock(&self.inner.producer);
        if let Some(current) = producer.as_ref()
            && !current.task.is_finished()
        {
            if !restart {
                return;
            }
            current.cancel.cancel();
        }
        let cancel = CancellationToken::new();
        let generation = self
            .inner
            .producer_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        *lock(&self.inner.last_failure) = None;
        let inner = Arc::clone(&self.inner);
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { run_producer(inner, task_cancel, generation).await });
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

fn lock<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
