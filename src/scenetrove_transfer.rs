//! Bounded bridge HTTP exchanges for the recording pull: JSON reads, media download and ACK.

use std::path::Path;

use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{fs::OpenOptions, io::AsyncWriteExt};

use super::encode_segment;
use crate::{
    error::BridgeError,
    media_format::{MediaFormat, PROBE_BYTES},
};

const JSON_LIMIT: usize = 1024 * 1024;

pub(super) async fn bounded_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, BridgeError> {
    if !response.status().is_success() {
        return Err(BridgeError::Upstream(format!(
            "bridge returned HTTP {}",
            response.status()
        )));
    }
    let bytes = response.bytes().await?;
    if bytes.len() > JSON_LIMIT {
        return Err(BridgeError::Upstream(
            "bridge JSON exceeded its bound".into(),
        ));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

pub(super) async fn write_media(
    response: reqwest::Response,
    path: &Path,
    max_bytes: u64,
    media_format: MediaFormat,
) -> Result<(u64, String), BridgeError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .await?;
    let mut stream = response.bytes_stream();
    let mut digest = Sha256::new();
    let mut written = 0_u64;
    let mut prefix = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        written = written.saturating_add(chunk.len() as u64);
        if written > max_bytes {
            return Err(BridgeError::Capacity(
                "recording exceeded download byte bound".into(),
            ));
        }
        if prefix.len() < PROBE_BYTES {
            prefix.extend_from_slice(&chunk[..chunk.len().min(PROBE_BYTES - prefix.len())]);
        }
        digest.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.sync_all().await?;
    if !media_format.matches_prefix(&prefix) {
        return Err(BridgeError::Upstream(
            "recording does not match its declared media format".into(),
        ));
    }
    Ok((written, hex::encode(digest.finalize())))
}

pub(super) async fn acknowledge(
    client: &Client,
    base: &Url,
    token: &str,
    id: &str,
) -> Result<(), BridgeError> {
    let url = base
        .join(&format!("v1/recordings/{}", encode_segment(id)))
        .map_err(|_| BridgeError::Configuration("ACK URL is invalid".into()))?;
    let status = client.delete(url).bearer_auth(token).send().await?.status();
    if status != StatusCode::NO_CONTENT {
        return Err(BridgeError::Upstream(format!(
            "recording ACK returned HTTP {status}"
        )));
    }
    Ok(())
}
