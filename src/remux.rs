use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex as StdMutex, MutexGuard, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use tokio::{sync::mpsc, task::JoinHandle, time::sleep};
use tokio_util::sync::CancellationToken;

use crate::{error::BridgeError, hub::RawStreamHub, metrics::Metrics};

#[path = "remux_args.rs"]
mod args;
#[path = "remux_process.rs"]
mod process;
#[path = "remux_warm.rs"]
mod warm;
use warm::TsWarmCache;

pub struct TsSubscription {
    pub id: u64,
    pub receiver: mpsc::Receiver<Bytes>,
    hub: Weak<MpegTsHub>,
}

impl Drop for TsSubscription {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade() {
            hub.remove_subscriber(self.id);
        }
    }
}

struct Producer {
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

pub struct MpegTsHub {
    alias: String,
    raw: Arc<RawStreamHub>,
    metrics: Metrics,
    ffmpeg: PathBuf,
    subscribers: Arc<StdMutex<BTreeMap<u64, mpsc::Sender<Bytes>>>>,
    warm: Arc<StdMutex<TsWarmCache>>,
    producer: StdMutex<Option<Producer>>,
    next_id: AtomicU64,
    queue_chunks: usize,
    max_subscribers: usize,
    idle_grace: Duration,
    closed: AtomicBool,
}

impl MpegTsHub {
    #[must_use]
    pub fn new(
        alias: String,
        raw: Arc<RawStreamHub>,
        metrics: Metrics,
        ffmpeg: PathBuf,
        queue_chunks: usize,
        max_subscribers: usize,
        idle_grace: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            alias,
            raw,
            metrics,
            ffmpeg,
            subscribers: Arc::new(StdMutex::new(BTreeMap::new())),
            warm: Arc::new(StdMutex::new(TsWarmCache::default())),
            producer: StdMutex::new(None),
            next_id: AtomicU64::new(1),
            queue_chunks,
            max_subscribers,
            idle_grace,
            closed: AtomicBool::new(false),
        })
    }

    pub async fn subscribe(self: &Arc<Self>) -> Result<TsSubscription, BridgeError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(BridgeError::Upstream("MPEG-TS hub is closed".into()));
        }
        let (sender, receiver) = mpsc::channel(self.queue_chunks);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let count = {
            let mut subscribers = lock(&self.subscribers);
            if subscribers.len() >= self.max_subscribers {
                return Err(BridgeError::Capacity(
                    "maximum MPEG-TS subscribers reached".into(),
                ));
            }
            let snapshot = { lock(&self.warm).snapshot() };
            if let Some(snapshot) = snapshot {
                sender.try_send(snapshot).map_err(|_| {
                    BridgeError::Upstream("MPEG-TS warm cache delivery failed".into())
                })?;
            }
            subscribers.insert(id, sender);
            subscribers.len()
        };
        self.metrics
            .gauge("ts_subscribers", &self.alias, count as f64)
            .await;
        self.ensure_running();
        Ok(TsSubscription {
            id,
            receiver,
            hub: Arc::downgrade(self),
        })
    }

    pub fn unsubscribe(&self, id: u64) {
        self.remove_subscriber(id);
    }

    fn remove_subscriber(&self, id: u64) {
        let count = {
            let mut subscribers = lock(&self.subscribers);
            subscribers.remove(&id);
            subscribers.len()
        };
        let metrics = self.metrics.clone();
        let alias = self.alias.clone();
        let subscribers = Arc::clone(&self.subscribers);
        let cancel = lock(&self.producer)
            .as_ref()
            .map(|value| value.cancel.clone());
        let grace = self.idle_grace;
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                metrics.gauge("ts_subscribers", &alias, count as f64).await;
                if count != 0 {
                    return;
                }
                sleep(grace).await;
                if lock(&subscribers).is_empty()
                    && let Some(cancel) = cancel
                {
                    cancel.cancel();
                }
            });
        }
    }

    pub async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let producer = lock(&self.producer).take();
        if let Some(producer) = producer {
            producer.cancel.cancel();
            let _result = producer.task.await;
        }
        lock(&self.subscribers).clear();
        lock(&self.warm).clear();
    }
}

fn lock<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
