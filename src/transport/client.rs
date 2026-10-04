use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use reqwest::{Client, Method, StatusCode};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use super::cloud_key::CLOUD_KEY_STATE_FILE;
use crate::{
    config::CameraConfig,
    device::{CloudKeyGuard, DeviceStatus},
    error::BridgeError,
    storage::{atomic_write_json, ensure_private_regular},
    transport::{
        CameraTransport, ChunkConsumer, EzvizToken,
        http::{meta_code, mobile_headers, required_string},
        image, timeout_duration,
    },
    vtm::VtmSession,
};

/// One cached status lookup; `status: None` records a recent failure.
pub(super) struct StatusEntry {
    pub(super) expires: tokio::time::Instant,
    pub(super) status: Option<DeviceStatus>,
}

/// Per-serial single-flight slots; the outer lock is never held across I/O.
pub(super) type StatusCache = std::sync::Mutex<BTreeMap<String, Arc<Mutex<Option<StatusEntry>>>>>;

pub struct EzvizTransport {
    pub(super) client: Client,
    pub(super) token: RwLock<EzvizToken>,
    token_path: PathBuf,
    pub(super) allocation_lock: Mutex<()>,
    pub(super) ffmpeg_path: PathBuf,
    pub(super) timeout_seconds: u64,
    pub(super) status_cache: StatusCache,
    pub(super) cloud_keys: CloudKeyGuard,
}

impl EzvizTransport {
    pub async fn from_token_file(
        path: PathBuf,
        timeout_seconds: u64,
        ffmpeg_path: PathBuf,
    ) -> Result<Self, BridgeError> {
        let raw = ensure_private_regular(&path, 20)?;
        let token: EzvizToken = serde_json::from_str(&raw)
            .map_err(|_| BridgeError::Configuration("EZVIZ token file is invalid".into()))?;
        token.validate()?;
        let transport = Self {
            client: Client::builder()
                .timeout(timeout_duration(timeout_seconds))
                .redirect(reqwest::redirect::Policy::limited(3))
                .build()?,
            token: RwLock::new(token),
            token_path: path.clone(),
            timeout_seconds,
            allocation_lock: Mutex::new(()),
            ffmpeg_path,
            status_cache: std::sync::Mutex::new(BTreeMap::new()),
            // Persisted next to the session token, inside the private /data.
            cloud_keys: CloudKeyGuard::load(path.with_file_name(CLOUD_KEY_STATE_FILE)),
        };
        transport.refresh_session().await?;
        transport.ensure_service_urls().await?;
        Ok(transport)
    }

    pub(super) async fn refresh_session(&self) -> Result<(), BridgeError> {
        let current = self.token.read().await.clone();
        let feature = current
            .feature_code
            .clone()
            .ok_or_else(|| BridgeError::Configuration("EZVIZ token has no feature code".into()))?;
        let url = format!("https://{}/v3/apigateway/login", current.api_url);
        let response = self
            .request(Method::PUT, &url, &current)
            .form(&[
                ("refreshSessionId", current.rf_session_id.as_str()),
                ("featureCode", feature.as_str()),
            ])
            .send()
            .await?;
        let status = response.status();
        let value: Value = response.json().await?;
        if !status.is_success() || meta_code(&value) != Some(200) {
            return Err(BridgeError::Authentication);
        }
        let session = value
            .get("sessionInfo")
            .ok_or_else(|| BridgeError::Upstream("refresh response omitted sessionInfo".into()))?;
        let mut updated = current;
        updated.session_id = required_string(session, "sessionId")?;
        updated.rf_session_id = required_string(session, "refreshSessionId")?;
        self.persist_token(updated).await
    }

    async fn ensure_service_urls(&self) -> Result<(), BridgeError> {
        if self.token.read().await.service_urls.is_some() {
            return Ok(());
        }
        let value = self
            .api_json(Method::GET, "/v3/configurations/system/info", &[])
            .await?;
        let service_urls = value
            .get("systemConfigInfo")
            .cloned()
            .ok_or_else(|| BridgeError::Upstream("system info omitted service URLs".into()))?;
        let mut token = self.token.read().await.clone();
        token.service_urls = Some(service_urls);
        self.persist_token(token).await
    }

    async fn persist_token(&self, token: EzvizToken) -> Result<(), BridgeError> {
        atomic_write_json(&self.token_path, &token)?;
        *self.token.write().await = token;
        Ok(())
    }

    pub(super) async fn api_json(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Value, BridgeError> {
        for attempt in 0..=1 {
            let token = self.token.read().await.clone();
            let url = format!("https://{}{}", token.api_url, path);
            let response = self
                .request(method.clone(), &url, &token)
                .query(query)
                .send()
                .await?;
            if response.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
                self.refresh_session().await?;
                continue;
            }
            let status = response.status();
            let value: Value = response.json().await?;
            if !status.is_success() {
                return Err(BridgeError::Upstream(format!(
                    "EZVIZ API returned HTTP {status}"
                )));
            }
            return Ok(value);
        }
        Err(BridgeError::Authentication)
    }

    pub(super) fn request(
        &self,
        method: Method,
        url: &str,
        token: &EzvizToken,
    ) -> reqwest::RequestBuilder {
        let mut headers = mobile_headers(token.feature_code.as_deref().unwrap_or(""));
        if let Ok(value) = token.session_id.parse() {
            headers.insert("sessionid", value);
        }
        self.client.request(method, url).headers(headers)
    }
}

pub(super) fn success_code(value: Option<&Value>) -> bool {
    value.is_some_and(|value| value.as_i64() == Some(0) || value.as_str() == Some("0"))
}

#[async_trait]
impl CameraTransport for EzvizTransport {
    async fn snapshot_jpeg(&self, camera: &CameraConfig) -> Result<Vec<u8>, BridgeError> {
        let capture = self
            .api_json(
                Method::PUT,
                &format!("/v3/devconfig/v1/{}/1/capture", camera.serial),
                &[],
            )
            .await?;
        let image_url = image::first_image_url(&capture)
            .ok_or_else(|| BridgeError::Upstream("capture response omitted image URL".into()))?;
        let token = self.token.read().await.clone();
        let response = self.request(Method::GET, &image_url, &token).send().await?;
        if !response.status().is_success() {
            return Err(BridgeError::Upstream("snapshot download failed".into()));
        }
        let bytes = response.bytes().await?.to_vec();
        let clear = if bytes.windows(16).any(|part| part == b"hikencodepicture") {
            self.decrypt_snapshot(camera, &bytes).await?
        } else {
            bytes
        };
        image::jpeg_slice(&clear)
    }

    async fn stream_mpeg_ps(
        &self,
        camera: &CameraConfig,
        cancel: CancellationToken,
        output: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError> {
        if self.video_is_encrypted(camera).await {
            return self.stream_encrypted_video(camera, cancel, output).await;
        }
        let url = self.cloud_stream_url(camera).await?;
        let mut session = VtmSession::connect(url, timeout_duration(self.timeout_seconds)).await?;
        session
            .copy_payloads(&cancel, |chunk| output.consume(chunk))
            .await
    }
}
