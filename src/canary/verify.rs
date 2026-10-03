//! Live HTTP probes of the bridge: bounded snapshot fetch, live capture and ffprobe check.

use std::{path::Path, time::Duration};

use futures_util::StreamExt;
use reqwest::{Client, Response, Url};
use serde::{Deserialize, Serialize};
use tokio::{
    fs::OpenOptions,
    io::AsyncWriteExt,
    process::Command,
    time::{Instant, timeout},
};

use super::CanaryOptions;
use crate::{error::BridgeError, scenetrove::encode_segment};

const SNAPSHOT_LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<ProbeStream>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct ProbeStream {
    codec_name: Option<String>,
    codec_type: Option<String>,
}

pub(super) async fn verify_snapshot(
    client: &Client,
    base: &Url,
    camera: &str,
    token: &str,
) -> Result<u64, BridgeError> {
    let url = base
        .join(&format!(
            "v1/cameras/{}/snapshot.jpg",
            encode_segment(camera)
        ))
        .map_err(|_| BridgeError::Configuration("snapshot URL is invalid".into()))?;
    let image = bounded_body(
        client.get(url).bearer_auth(token).send().await?,
        SNAPSHOT_LIMIT,
    )
    .await?;
    if image.len() < 4 || !image.starts_with(b"\xff\xd8\xff") || !image.ends_with(b"\xff\xd9") {
        return Err(BridgeError::Upstream(
            "snapshot is not a complete JPEG".into(),
        ));
    }
    Ok(image.len() as u64)
}

async fn bounded_body(response: Response, limit: u64) -> Result<Vec<u8>, BridgeError> {
    require_success(&response)?;
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len() as u64 + chunk.len() as u64 > limit {
            return Err(BridgeError::Capacity(
                "canary response exceeded its byte bound".into(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(super) async fn verify_stream(
    client: &Client,
    base: &Url,
    options: &CanaryOptions,
    token: &str,
    media: &Path,
) -> Result<(u64, Vec<ProbeStream>), BridgeError> {
    let url = base
        .join(&format!(
            "v1/cameras/{}/live.{}",
            encode_segment(&options.camera),
            options.stream_format.extension()
        ))
        .map_err(|_| BridgeError::Configuration("live URL is invalid".into()))?;
    let response = client.get(url).bearer_auth(token).send().await?;
    require_success(&response)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(media)
        .await?;
    let mut stream = response.bytes_stream();
    let deadline = Instant::now() + Duration::from_secs(options.seconds);
    let mut written = 0_u64;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Some(chunk) = timeout(remaining, stream.next()).await.ok().flatten() else {
            break;
        };
        let chunk = chunk?;
        written = written.saturating_add(chunk.len() as u64);
        if written > options.max_bytes {
            return Err(BridgeError::Capacity(
                "live canary exceeded its byte bound".into(),
            ));
        }
        file.write_all(&chunk).await?;
    }
    file.sync_all().await?;
    drop(file);
    if written == 0 {
        return Err(BridgeError::Upstream(
            "live canary returned no media".into(),
        ));
    }
    Ok((written, probe_media(media).await?))
}

fn require_success(response: &Response) -> Result<(), BridgeError> {
    if response.status().is_success() {
        Ok(())
    } else {
        Err(BridgeError::Upstream(format!(
            "bridge returned HTTP {}",
            response.status()
        )))
    }
}

async fn probe_media(media: &Path) -> Result<Vec<ProbeStream>, BridgeError> {
    let output = timeout(
        Duration::from_secs(30),
        Command::new("/usr/bin/ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_name,codec_type",
                "-of",
                "json",
            ])
            .arg(media)
            .output(),
    )
    .await
    .map_err(|_| BridgeError::Upstream("ffprobe timed out".into()))??;
    if !output.status.success() {
        return Err(BridgeError::Upstream("ffprobe rejected live media".into()));
    }
    let probe: ProbeOutput = serde_json::from_slice(&output.stdout)?;
    if !probe
        .streams
        .iter()
        .any(|stream| stream.codec_type.as_deref() == Some("video"))
    {
        return Err(BridgeError::Upstream(
            "ffprobe found no video stream".into(),
        ));
    }
    Ok(probe.streams)
}
