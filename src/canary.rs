use std::{
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use clap::{Args, ValueEnum};
use futures_util::StreamExt;
use reqwest::{Client, Response, Url};
use serde::{Deserialize, Serialize};
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    process::Command,
    time::{Instant, timeout},
};
use uuid::Uuid;

use crate::{
    error::BridgeError,
    scenetrove::{encode_segment, validated_base_url},
    storage::ensure_private_regular,
};

const SNAPSHOT_LIMIT: u64 = 16 * 1024 * 1024;
const MIN_STREAM_LIMIT: u64 = 1024 * 1024;
const MAX_STREAM_LIMIT: u64 = 256 * 1024 * 1024;

#[derive(Args, Debug)]
pub struct CanaryOptions {
    #[arg(long)]
    base_url: String,
    #[arg(long)]
    camera: String,
    #[arg(long, default_value_t = 20)]
    seconds: u64,
    #[arg(long, default_value_t = 64 * 1024 * 1024)]
    max_bytes: u64,
    #[arg(long, value_enum, default_value_t = StreamFormat::Mpegps)]
    stream_format: StreamFormat,
    #[arg(long)]
    skip_snapshot: bool,
    #[arg(long)]
    token_file: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum StreamFormat {
    Mpegps,
    Ts,
}

impl StreamFormat {
    const fn extension(self) -> &'static str {
        match self {
            Self::Mpegps => "mpegps",
            Self::Ts => "ts",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<ProbeStream>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProbeStream {
    codec_name: Option<String>,
    codec_type: Option<String>,
}

#[derive(Serialize)]
struct CanaryResult {
    status: &'static str,
    snapshot_bytes: u64,
    stream_bytes: u64,
    streams: Vec<ProbeStream>,
}

pub async fn run(options: CanaryOptions) -> Result<(), BridgeError> {
    validate_options(&options)?;
    let token = read_token(options.token_file.as_deref())?;
    let base = validated_base_url(&options.base_url)?;
    let client = Client::builder()
        .timeout(Duration::from_secs(options.seconds.saturating_add(45)))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let snapshot_bytes = if options.skip_snapshot {
        0
    } else {
        verify_snapshot(&client, &base, &options.camera, &token).await?
    };
    let media = temporary_media_path(options.stream_format);
    let stream_result = verify_stream(&client, &base, &options, &token, &media).await;
    let _ignored = fs::remove_file(&media).await;
    let (stream_bytes, streams) = stream_result?;
    let result = CanaryResult {
        status: "ok",
        snapshot_bytes,
        stream_bytes,
        streams,
    };
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &result)?;
    stdout.write_all(b"\n")?;
    Ok(())
}

fn validate_options(options: &CanaryOptions) -> Result<(), BridgeError> {
    if !(1..=120).contains(&options.seconds) {
        return Err(BridgeError::Configuration(
            "canary seconds must be between 1 and 120".into(),
        ));
    }
    if !(MIN_STREAM_LIMIT..=MAX_STREAM_LIMIT).contains(&options.max_bytes) {
        return Err(BridgeError::Configuration(
            "canary max bytes must be between 1 MiB and 256 MiB".into(),
        ));
    }
    if options.camera.is_empty()
        || !options
            .camera
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(BridgeError::Configuration(
            "camera alias must be alphanumeric with '-' or '_'".into(),
        ));
    }
    Ok(())
}

fn read_token(path: Option<&Path>) -> Result<String, BridgeError> {
    if let Some(path) = path {
        return ensure_private_regular(path, 32);
    }
    let mut token = String::new();
    io::stdin().lock().read_line(&mut token)?;
    let token = token.trim().to_owned();
    if token.len() < 32 {
        return Err(BridgeError::Configuration(
            "API token is missing on standard input".into(),
        ));
    }
    Ok(token)
}

async fn verify_snapshot(
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

async fn verify_stream(
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

fn temporary_media_path(format: StreamFormat) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ezviz-bridge-canary-{}.{}",
        Uuid::new_v4(),
        format.extension()
    ))
}

#[cfg(test)]
mod tests;
