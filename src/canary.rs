use std::{
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use clap::{Args, ValueEnum};
use reqwest::Client;
use serde::Serialize;
use tokio::fs;
use uuid::Uuid;

use crate::{error::BridgeError, scenetrove::validated_base_url, storage::ensure_private_regular};

mod verify;

use verify::{ProbeStream, verify_snapshot, verify_stream};

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

fn temporary_media_path(format: StreamFormat) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ezviz-bridge-canary-{}.{}",
        Uuid::new_v4(),
        format.extension()
    ))
}

#[cfg(test)]
mod tests;
