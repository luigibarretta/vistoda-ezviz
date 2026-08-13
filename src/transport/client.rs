use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use chrono::Utc;
use reqwest::{Client, Method, StatusCode};
use serde_json::Value;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::{
    config::CameraConfig,
    error::BridgeError,
    storage::{atomic_write_json, ensure_private_regular},
    transport::{
        CameraTransport, ChunkConsumer, EzvizToken,
        http::{find_resource, jwt_sign, meta_code, mobile_headers, required_string, value_u16},
        image, timeout_duration,
    },
    vtm::{VtmSession, build_vtm_url},
};

pub struct EzvizTransport {
    client: Client,
    token: RwLock<EzvizToken>,
    token_path: PathBuf,
    timeout_seconds: u64,
}

impl EzvizTransport {
    pub async fn from_token_file(path: PathBuf, timeout_seconds: u64) -> Result<Self, BridgeError> {
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
            token_path: path,
            timeout_seconds,
        };
        transport.refresh_session().await?;
        transport.ensure_service_urls().await?;
        Ok(transport)
    }

    async fn refresh_session(&self) -> Result<(), BridgeError> {
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

    async fn api_json(
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

    fn request(&self, method: Method, url: &str, token: &EzvizToken) -> reqwest::RequestBuilder {
        let mut headers = mobile_headers(token.feature_code.as_deref().unwrap_or(""));
        if let Ok(value) = token.session_id.parse() {
            headers.insert("sessionid", value);
        }
        self.client.request(method, url).headers(headers)
    }

    async fn cloud_stream_url(&self, serial: &str) -> Result<String, BridgeError> {
        let page = self
            .api_json(
                Method::GET,
                "/v3/userdevices/v1/resources/pagelist",
                &[
                    ("groupId", "-1".into()),
                    ("limit", "50".into()),
                    ("offset", "0".into()),
                    ("filter", "VTM".into()),
                ],
            )
            .await?;
        let resource = find_resource(&page, serial)?;
        let resource_id = required_string(resource, "resourceId")?;
        let biz_url = resource
            .get("streamBizUrl")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let server = self
            .api_json(Method::GET, &format!("/v3/streaming/vtm/{serial}/1"), &[])
            .await?;
        let server = server
            .get("streamServerConfig")
            .or_else(|| page.get("VTM").and_then(|value| value.get(&resource_id)))
            .ok_or_else(|| BridgeError::Upstream("VTM server metadata is missing".into()))?;
        let host = ["externalIp", "domain", "internalIp"]
            .iter()
            .find_map(|key| server.get(*key).and_then(Value::as_str))
            .ok_or_else(|| BridgeError::Upstream("VTM server host is missing".into()))?;
        let port = value_u16(server.get("port"))?;
        let vtdu = self.vtdu_token().await?;
        Ok(build_vtm_url(
            host,
            port,
            serial,
            biz_url,
            &vtdu,
            Utc::now().timestamp_millis(),
        ))
    }

    async fn vtdu_token(&self) -> Result<String, BridgeError> {
        let token = self.token.read().await.clone();
        let sign = jwt_sign(&token.session_id)?;
        let url = format!("{}/vtdutoken2", token.auth_address()?);
        let response = self
            .request(Method::GET, &url, &token)
            .query(&[("ssid", &token.session_id), ("sign", &sign)])
            .send()
            .await?;
        let status = response.status();
        let value: Value = response.json().await?;
        if !status.is_success() || !success_code(value.get("retcode")) {
            return Err(BridgeError::Upstream("VTDU token request failed".into()));
        }
        value
            .get("tokens")
            .and_then(Value::as_array)
            .and_then(|tokens| tokens.first())
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| BridgeError::Upstream("VTDU response omitted its token".into()))
    }

    async fn camera_key(&self, serial: &str) -> Result<String, BridgeError> {
        let token = self.token.read().await.clone();
        let feature = token.feature_code.as_deref().unwrap_or_default();
        let url = format!("https://{}/api/device/query/encryptkey", token.api_url);
        let response = self
            .request(Method::POST, &url, &token)
            .form(&[
                ("checkcode", ""),
                ("serial", serial),
                ("clientNo", "web_site"),
                ("clientType", "3"),
                ("netType", "WIFI"),
                ("featureCode", feature),
                ("sessionId", &token.session_id),
            ])
            .send()
            .await?;
        let value: Value = response.json().await?;
        if !success_code(value.get("resultCode")) {
            return Err(BridgeError::Upstream(
                "camera image key request failed".into(),
            ));
        }
        required_string(&value, "encryptkey")
    }
}

fn success_code(value: Option<&Value>) -> bool {
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
            image::decrypt_image(&bytes, &self.camera_key(&camera.serial).await?)?
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
        if camera.decrypt_video {
            return Err(BridgeError::Upstream(
                "continuous encrypted video is not supported".into(),
            ));
        }
        let url = self.cloud_stream_url(&camera.serial).await?;
        let mut session = VtmSession::connect(url, timeout_duration(self.timeout_seconds)).await?;
        session
            .copy_payloads(&cancel, |chunk| output.consume(chunk))
            .await
    }
}
