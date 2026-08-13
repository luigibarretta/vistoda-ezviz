use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex, MutexGuard, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::mpsc,
    task::JoinHandle,
    time::sleep,
};
use tokio_util::sync::CancellationToken;

use crate::{error::BridgeError, hub::RawStreamHub, metrics::Metrics};

const ARGUMENTS: &[&str] = &[
    "-hide_banner",
    "-loglevel",
    "error",
    "-analyzeduration",
    "500000",
    "-probesize",
    "1000000",
    "-fflags",
    "+genpts+nobuffer",
    "-f",
    "mpeg",
    "-i",
    "pipe:0",
    "-map",
    "0:v:0?",
    "-map",
    "0:a:0?",
    "-c",
    "copy",
    "-muxdelay",
    "0",
    "-flush_packets",
    "1",
    "-f",
    "mpegts",
    "pipe:1",
];

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

    fn ensure_running(self: &Arc<Self>) {
        let mut producer = lock(&self.producer);
        if producer
            .as_ref()
            .is_some_and(|current| !current.task.is_finished())
        {
            return;
        }
        let cancel = CancellationToken::new();
        let hub = Arc::clone(self);
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { hub.run(task_cancel).await });
        *producer = Some(Producer { cancel, task });
    }

    async fn run(self: Arc<Self>, cancel: CancellationToken) {
        let started = Instant::now();
        let result = self.run_process(&cancel, started).await;
        if result.is_err() && !cancel.is_cancelled() {
            self.metrics
                .increment("remux_failures_total", &self.alias)
                .await;
        }
        self.metrics.gauge("remux_active", &self.alias, 0.0).await;
        lock(&self.subscribers).clear();
        self.metrics.gauge("ts_subscribers", &self.alias, 0.0).await;
    }

    async fn run_process(
        &self,
        cancel: &CancellationToken,
        started: Instant,
    ) -> Result<(), BridgeError> {
        let mut child = Command::new(&self.ffmpeg)
            .args(ARGUMENTS)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        self.metrics
            .increment("remux_starts_total", &self.alias)
            .await;
        self.metrics.gauge("remux_active", &self.alias, 1.0).await;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| BridgeError::Upstream("FFmpeg stdin is unavailable".into()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| BridgeError::Upstream("FFmpeg stdout is unavailable".into()))?;
        let mut raw = self.raw.subscribe().await?;
        let feed_cancel = cancel.clone();
        let feed = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = feed_cancel.cancelled() => break,
                    chunk = raw.receiver.recv() => match chunk {
                        Some(chunk) => stdin.write_all(&chunk).await?,
                        None => break,
                    }
                }
            }
            let _result = stdin.shutdown().await;
            Ok::<u64, std::io::Error>(raw.id)
        });
        let mut buffer = vec![0_u8; 64 * 1024];
        let mut first_chunk = true;
        let read_result = loop {
            let read = tokio::select! {
                () = cancel.cancelled() => break Ok(()),
                read = stdout.read(&mut buffer) => read,
            }?;
            if read == 0 {
                break Ok(());
            }
            if first_chunk {
                self.metrics
                    .gauge(
                        "remux_startup_seconds",
                        &self.alias,
                        started.elapsed().as_secs_f64(),
                    )
                    .await;
                first_chunk = false;
            }
            publish(&self.subscribers, &Bytes::copy_from_slice(&buffer[..read]));
        };
        cancel.cancel();
        let raw_id = feed.await??;
        self.raw.unsubscribe(raw_id);
        let _result = child.kill().await;
        let _status = child.wait().await?;
        read_result
    }

    pub async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let producer = lock(&self.producer).take();
        if let Some(producer) = producer {
            producer.cancel.cancel();
            let _result = producer.task.await;
        }
        lock(&self.subscribers).clear();
    }
}

fn publish(subscribers: &StdMutex<BTreeMap<u64, mpsc::Sender<Bytes>>>, chunk: &Bytes) {
    lock(subscribers).retain(|_, sender| sender.try_send(chunk.clone()).is_ok());
}

fn lock<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
