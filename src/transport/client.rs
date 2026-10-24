use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use reqwest::{Client, Method};
use serde_json::Value;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::cloud_key::CLOUD_KEY_STATE_FILE;
use crate::{
    config::CameraConfig,
    device::{CloudKeyGuard, DeviceStatus},
    error::BridgeError,
    session_health::SessionHealth,
    transport::{
        CameraTransport, ChunkConsumer, EzvizToken,
        http::mobile_headers,
        image,
        media::{download_presigned, media_client},
        session::SessionStore,
        timeout_duration,
    },
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
    /// Pre-signed media downloads; never carries session headers.
    pub(super) media: Client,
    pub(super) session: Arc<SessionStore>,
    /// `https` in production; plain HTTP only for the in-process fake server.
    pub(super) scheme: &'static str,
    pub(super) allocation_lock: Mutex<()>,
    pub(super) timeout_seconds: u64,
    pub(super) status_cache: StatusCache,
    pub(super) cloud_keys: CloudKeyGuard,
}

/// First retry delay of the startup session check, doubled up to the maximum.
const START_RETRY: Duration = Duration::from_secs(15);
const START_RETRY_MAX: Duration = Duration::from_secs(10 * 60);
/// Upper bound for one camera snapshot download.
const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

impl EzvizTransport {
    /// Loads the stored session without contacting EZVIZ; see [`Self::start`].
    pub fn load(
        path: PathBuf,
        timeout_seconds: u64,
        health: SessionHealth,
    ) -> Result<Self, BridgeError> {
        let timeout = timeout_duration(timeout_seconds);
        Ok(Self {
            client: Client::builder()
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::limited(3))
                .build()?,
            media: media_client(timeout)?,
            // Persisted next to the session token, inside the private /data.
            cloud_keys: CloudKeyGuard::load(path.with_file_name(CLOUD_KEY_STATE_FILE)),
            session: Arc::new(SessionStore::load(path, health)?),
            scheme: "https",
            timeout_seconds,
            allocation_lock: Mutex::new(()),
            status_cache: std::sync::Mutex::new(BTreeMap::new()),
        })
    }

    /// Refreshes the session and discovers service URLs in the background,
    /// retrying transient failures with backoff. A refused session stops the
    /// loop and is reported as `reauth_required`; the API stays up either way.
    pub fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let transport = Arc::clone(self);
        tokio::spawn(async move {
            let mut delay = START_RETRY;
            loop {
                let (_, generation) = transport.session.current().await;
                let result = match transport.refresh_session(generation).await {
                    Ok(()) => transport.ensure_service_urls().await,
                    Err(error) => Err(error),
                };
                match result {
                    Ok(()) => return tracing::info!("EZVIZ session ready"),
                    Err(BridgeError::ReauthRequired) => {
                        return tracing::error!("EZVIZ refused the stored session; enroll again");
                    }
                    Err(error) => tracing::warn!(
                        error_type = error.diagnostic_code(),
                        retry_seconds = delay.as_secs(),
                        "EZVIZ session start failed; serving degraded"
                    ),
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(START_RETRY_MAX);
            }
        })
    }

    /// Takes over a token file written by a runtime re-enrollment.
    pub async fn adopt_enrollment(&self, staged: &Path) -> Result<(), BridgeError> {
        self.session.adopt(staged).await?;
        self.ensure_service_urls().await
    }

    /// Single-flight refresh of the session seen at `generation`.
    pub(super) async fn refresh_session(&self, generation: u64) -> Result<(), BridgeError> {
        self.session
            .refresh(&self.client, self.scheme, generation)
            .await
    }

    pub(super) async fn token(&self) -> EzvizToken {
        self.session.token().await
    }

    async fn ensure_service_urls(&self) -> Result<(), BridgeError> {
        if self.token().await.service_urls.is_some() {
            return Ok(());
        }
        let value = self
            .api_json(Method::GET, "/v3/configurations/system/info", &[])
            .await?;
        let service_urls = value
            .get("systemConfigInfo")
            .cloned()
            .ok_or_else(|| BridgeError::Upstream("system info omitted service URLs".into()))?;
        self.session
            .update(|token| token.service_urls = Some(service_urls))
            .await
    }

    pub(super) async fn api_json(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Value, BridgeError> {
        let reply = self
            .authed(method, path, |request, _| request.query(query))
            .await?;
        if !reply.status.is_success() {
            return Err(BridgeError::Upstream(format!(
                "EZVIZ API returned HTTP {}",
                reply.status.as_u16()
            )));
        }
        Ok(reply.value)
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

    #[cfg(test)]
    pub(super) const fn over_plain_http(mut self) -> Self {
        self.scheme = "http";
        self
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
        // Pre-signed URL: HTTPS (or HTTP on EZVIZ hosts), no session headers,
        // bounded body; see `media.rs`.
        let bytes = download_presigned(&self.media, &image_url, MAX_SNAPSHOT_BYTES).await?;
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
        self.stream_clear(camera, cancel, output).await
    }

    async fn stream_sd_playback(
        &self,
        camera: &CameraConfig,
        interval: (&str, &str),
        cancel: CancellationToken,
        output: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError> {
        self.stream_sd(camera, interval, cancel, output).await
    }
}
