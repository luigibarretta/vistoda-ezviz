//! Remux producer: spawns `ffmpeg`, feeds it raw media and fans out MPEG-TS chunks.

use std::{
    collections::BTreeMap,
    process::Stdio,
    sync::{Arc, Mutex as StdMutex},
    time::Instant,
};

use bytes::Bytes;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

use super::{MpegTsHub, Producer, args::ARGUMENTS, lock, warm::TsWarmCache};
use crate::error::BridgeError;

impl MpegTsHub {
    pub(super) fn ensure_running(self: &Arc<Self>) {
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
        lock(&self.warm).clear();
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
            publish(
                &self.subscribers,
                &self.warm,
                &Bytes::copy_from_slice(&buffer[..read]),
            );
        };
        cancel.cancel();
        let raw_id = feed.await??;
        self.raw.unsubscribe(raw_id);
        let _result = child.kill().await;
        let _status = child.wait().await?;
        read_result
    }
}

fn publish(
    subscribers: &StdMutex<BTreeMap<u64, mpsc::Sender<Bytes>>>,
    warm: &StdMutex<TsWarmCache>,
    chunk: &Bytes,
) {
    let mut subscribers = lock(subscribers);
    lock(warm).ingest(chunk);
    subscribers.retain(|_, sender| sender.try_send(chunk.clone()).is_ok());
}
