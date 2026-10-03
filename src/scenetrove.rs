use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use chrono::DateTime;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use tokio::{fs, time::sleep};

use crate::{
    error::BridgeError,
    media_format::MediaFormat,
    scenetrove_receipt::{receipt_path, recover_receipt, remove_receipt},
    storage::{atomic_write_json, ensure_private_regular},
};

#[path = "scenetrove_transfer.rs"]
mod transfer;
use transfer::{acknowledge, bounded_json, write_media};

#[derive(Clone, Debug)]
pub struct PullRequest {
    pub base_url: String,
    pub camera: String,
    pub duration_seconds: u64,
    pub idempotency_key: String,
    pub token_file: PathBuf,
    pub destination: PathBuf,
    pub timeout_seconds: u64,
    pub max_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PullResult {
    pub recording_id: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Deserialize)]
struct Manifest {
    recording_id: String,
    status: String,
    completed_at: Option<String>,
    bytes: Option<u64>,
    sha256: Option<String>,
    error_code: Option<String>,
    media_type: String,
}

pub async fn pull(request: PullRequest) -> Result<PullResult, BridgeError> {
    let base = validated_base_url(&request.base_url)?;
    let token = ensure_private_regular(&request.token_file, 32)?;
    fs::create_dir_all(&request.destination).await?;
    let receipt = receipt_path(&request.destination, &request.idempotency_key);
    let client = Client::builder()
        .timeout(Duration::from_secs(request.timeout_seconds))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    if let Some(result) = recover_receipt(&receipt, &request.destination)? {
        acknowledge(&client, &base, &token, &result.recording_id).await?;
        remove_receipt(&receipt, &request.destination).await?;
        return Ok(result);
    }
    let create_url = base
        .join(&format!(
            "v1/cameras/{}/recordings",
            encode_segment(&request.camera)
        ))
        .map_err(|_| BridgeError::Configuration("recording URL is invalid".into()))?;
    let response = client
        .post(create_url)
        .bearer_auth(&token)
        .header("idempotency-key", &request.idempotency_key)
        .json(&serde_json::json!({"duration_seconds":request.duration_seconds}))
        .send()
        .await?;
    let mut manifest: Manifest = bounded_json(response).await?;
    let deadline = Instant::now() + Duration::from_secs(request.timeout_seconds);
    while !matches!(manifest.status.as_str(), "ready" | "failed") {
        if Instant::now() >= deadline {
            return Err(BridgeError::Upstream(
                "recording did not finish before deadline".into(),
            ));
        }
        sleep(Duration::from_millis(500)).await;
        let url = base
            .join(&format!(
                "v1/recordings/{}",
                encode_segment(&manifest.recording_id)
            ))
            .map_err(|_| BridgeError::Configuration("status URL is invalid".into()))?;
        manifest = bounded_json(client.get(url).bearer_auth(&token).send().await?).await?;
    }
    if manifest.status != "ready" {
        return Err(BridgeError::Upstream(format!(
            "bridge recording failed: {}",
            manifest.error_code.as_deref().unwrap_or("unknown")
        )));
    }
    let expected_bytes = manifest
        .bytes
        .filter(|value| *value > 0 && *value <= request.max_bytes)
        .ok_or_else(|| BridgeError::Upstream("recording byte bound is invalid".into()))?;
    let expected_digest = manifest
        .sha256
        .as_deref()
        .filter(|value| value.len() == 64)
        .ok_or_else(|| BridgeError::Upstream("recording digest is invalid".into()))?;
    let completed = manifest
        .completed_at
        .as_deref()
        .ok_or_else(|| BridgeError::Upstream("recording completion time is missing".into()))?;
    let stamp = DateTime::parse_from_rfc3339(completed)
        .map_err(|_| BridgeError::Upstream("recording completion time is invalid".into()))?
        .format("%Y%m%dT%H%M%SZ");
    let media_format = MediaFormat::from_media_type(&manifest.media_type)?;
    let filename = format!(
        "hiv{stamp}-{}.{}",
        manifest.recording_id,
        media_format.extension()
    );
    let final_path = request.destination.join(&filename);
    let partial = request
        .destination
        .join(format!(".{filename}.{}.partial", uuid::Uuid::new_v4()));
    let media_url = base
        .join(&format!(
            "v1/recordings/{}/media",
            encode_segment(&manifest.recording_id)
        ))
        .map_err(|_| BridgeError::Configuration("media URL is invalid".into()))?;
    let response = client.get(media_url).bearer_auth(&token).send().await?;
    if response.status() != StatusCode::OK {
        return Err(BridgeError::Upstream("media download failed".into()));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .unwrap_or_default();
    if content_type != manifest.media_type {
        return Err(BridgeError::Upstream(
            "recording response media type does not match its manifest".into(),
        ));
    }
    let media_result = write_media(response, &partial, request.max_bytes, media_format).await;
    let (actual_bytes, actual_digest) = match media_result {
        Ok(result) => result,
        Err(error) => {
            let _ignored = fs::remove_file(&partial).await;
            return Err(error);
        }
    };
    if actual_bytes != expected_bytes || actual_digest != expected_digest {
        let _ignored = fs::remove_file(&partial).await;
        return Err(BridgeError::Upstream(
            "recording media does not match its manifest".into(),
        ));
    }
    fs::rename(&partial, &final_path).await?;
    std::fs::File::open(&request.destination)?.sync_all()?;
    let result = PullResult {
        recording_id: manifest.recording_id,
        path: final_path,
        bytes: actual_bytes,
        sha256: actual_digest,
    };
    atomic_write_json(&receipt, &result)?;
    acknowledge(&client, &base, &token, &result.recording_id).await?;
    remove_receipt(&receipt, &request.destination).await?;
    Ok(result)
}

pub(crate) fn validated_base_url(value: &str) -> Result<Url, BridgeError> {
    let mut url = Url::parse(value)
        .map_err(|_| BridgeError::Configuration("base URL must be absolute HTTP(S)".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(BridgeError::Configuration(
            "base URL cannot contain credentials, query or fragment".into(),
        ));
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}
pub(crate) fn encode_segment(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
#[cfg(test)]
#[path = "scenetrove_tests.rs"]
mod tests;
