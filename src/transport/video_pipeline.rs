use std::{path::Path, process::Stdio, sync::Arc};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use crate::{
    error::BridgeError,
    transport::{ChunkConsumer, timeout_duration, video::EncryptedRtpVideo},
    vtm::VtmSession,
};

pub(super) async fn copy_as_mpeg_ts(
    mut session: VtmSession,
    verification_code: &str,
    ffmpeg_path: &Path,
    timeout_seconds: u64,
    cancel: CancellationToken,
    output: Arc<dyn ChunkConsumer>,
) -> Result<(), BridgeError> {
    let mut decryptor = EncryptedRtpVideo::new(verification_code)?;
    let mut child = None;
    let mut stdin = None;
    let mut reader = None;
    let pipeline_cancel = CancellationToken::new();

    loop {
        let packet = tokio::select! {
            () = cancel.cancelled() => None,
            () = pipeline_cancel.cancelled() => None,
            result = session.next_payload(&pipeline_cancel, true) => result?,
        };
        let Some(packet) = packet else { break };
        let clear = decryptor.feed(&packet)?;
        if clear.is_empty() {
            continue;
        }
        if child.is_none() {
            let codec = decryptor.codec().ok_or_else(|| {
                BridgeError::Upstream("encrypted video codec is unavailable".into())
            })?;
            let mut process = Command::new(ffmpeg_path)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-fflags",
                    "+genpts+nobuffer",
                    "-f",
                    codec.ffmpeg_name(),
                    "-i",
                    "pipe:0",
                    "-map",
                    "0:v:0",
                    "-an",
                    "-c:v",
                    "copy",
                    "-muxdelay",
                    "0",
                    "-mpegts_flags",
                    "+resend_headers",
                    "-f",
                    "mpegts",
                    "pipe:1",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()?;
            stdin = process.stdin.take();
            reader = Some(spawn_reader(
                process.stdout.take().ok_or_else(|| {
                    BridgeError::Upstream("encrypted-video FFmpeg stdout is unavailable".into())
                })?,
                Arc::clone(&output),
                pipeline_cancel.clone(),
            ));
            child = Some(process);
        }
        let writer = stdin.as_mut().ok_or_else(|| {
            BridgeError::Upstream("encrypted-video FFmpeg stdin is unavailable".into())
        })?;
        timeout(timeout_duration(timeout_seconds), writer.write_all(&clear))
            .await
            .map_err(|_| BridgeError::Upstream("video decrypt pipeline timed out".into()))??;
    }

    if let Some(mut writer) = stdin {
        let _ignored = writer.shutdown().await;
    }
    let Some(mut process) = child else {
        return if cancel.is_cancelled() {
            Ok(())
        } else {
            Err(BridgeError::Upstream(
                "encrypted stream ended before a supported codec was found".into(),
            ))
        };
    };
    let status = timeout(timeout_duration(5), process.wait()).await;
    pipeline_cancel.cancel();
    if let Some(reader) = reader {
        reader.await??;
    }
    if cancel.is_cancelled() {
        return Ok(());
    }
    match status {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(_)) => Err(BridgeError::Upstream(
            "encrypted-video FFmpeg remux failed".into(),
        )),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => {
            let _ignored = process.kill().await;
            Err(BridgeError::Upstream(
                "encrypted-video FFmpeg shutdown timed out".into(),
            ))
        }
    }
}

fn spawn_reader(
    mut stdout: tokio::process::ChildStdout,
    consumer: Arc<dyn ChunkConsumer>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<Result<(), BridgeError>> {
    tokio::spawn(async move {
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                result = stdout.read(&mut buffer) => result?,
            };
            if read == 0 || !consumer.consume(buffer[..read].to_vec()) {
                cancel.cancel();
                return Ok(());
            }
        }
    })
}
