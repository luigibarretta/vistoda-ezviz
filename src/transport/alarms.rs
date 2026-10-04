//! Read-only EZVIZ unified-message calls for the alarm feed.
//!
//! Only GET endpoints are used; messages are never marked read or deleted.
//! Errors never carry URLs, serials or session identifiers.

use async_trait::async_trait;
use reqwest::{Method, StatusCode};
use serde_json::Value;

use crate::{
    alarms::AlarmSource,
    config::CameraConfig,
    device::candidate_keys,
    error::BridgeError,
    transport::{EzvizTransport, http::meta_code},
};

const SUMMARY_PATH: &str = "/v3/unifiedmsg/summarybydevice/v2";
const LIST_PATH: &str = "/v3/unifiedmsg/list/v2";
/// Every alarm subtype, as used by the official app's alarm tab.
const ALARM_STYPE: &str = "92";
/// Server-side session expiry reported inside an HTTP 200 envelope.
const SESSION_EXPIRED: i64 = 99_997;
const PAGE_LIMIT: &str = "20";

impl EzvizTransport {
    /// GET with session refresh on HTTP 401 or `meta.code` 99997; any other
    /// non-200 `meta.code` is an error. Shared by the device APIs.
    pub(super) async fn message_json(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Value, BridgeError> {
        for attempt in 0..=1 {
            let token = self.token.read().await.clone();
            let url = format!("https://{}{}", token.api_url, path);
            let response = self
                .request(Method::GET, &url, &token)
                .query(query)
                .send()
                .await
                .map_err(|error| BridgeError::Http(error.without_url()))?;
            let status = response.status();
            let value: Value = if status == StatusCode::UNAUTHORIZED {
                Value::Null
            } else {
                response
                    .json()
                    .await
                    .map_err(|error| BridgeError::Http(error.without_url()))?
            };
            let expired =
                status == StatusCode::UNAUTHORIZED || meta_code(&value) == Some(SESSION_EXPIRED);
            if expired && attempt == 0 {
                self.refresh_session().await?;
                continue;
            }
            if expired {
                return Err(BridgeError::Authentication);
            }
            if !status.is_success() {
                return Err(BridgeError::Upstream(format!(
                    "EZVIZ message API returned HTTP {}",
                    status.as_u16()
                )));
            }
            return match meta_code(&value) {
                Some(200) | None => Ok(value),
                Some(code) => Err(BridgeError::Upstream(format!(
                    "EZVIZ message API returned code {code}"
                ))),
            };
        }
        Err(BridgeError::Authentication)
    }
}

#[async_trait]
impl AlarmSource for EzvizTransport {
    async fn alarm_summary(&self) -> Result<Value, BridgeError> {
        self.message_json(SUMMARY_PATH, &[("stype", ALARM_STYPE.into())])
            .await
    }

    async fn alarm_list(
        &self,
        serial: &str,
        date: &str,
        end_time: &str,
    ) -> Result<Value, BridgeError> {
        self.message_json(
            LIST_PATH,
            &[
                ("serials", serial.into()),
                ("stype", ALARM_STYPE.into()),
                ("limit", PAGE_LIMIT.into()),
                ("date", date.into()),
                ("endTime", end_time.into()),
            ],
        )
        .await
    }

    async fn alarm_picture(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, BridgeError> {
        if !url.starts_with("https://") {
            return Err(BridgeError::Upstream(
                "alarm picture URL is not HTTPS".into(),
            ));
        }
        // Pre-signed URL: plain GET without EZVIZ session headers.
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| BridgeError::Http(error.without_url()))?;
        if !response.status().is_success() {
            return Err(BridgeError::Upstream(
                "alarm picture download failed".into(),
            ));
        }
        let too_large = || BridgeError::Upstream("alarm picture exceeds the size limit".into());
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return Err(too_large());
        }
        let mut data = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| BridgeError::Http(error.without_url()))?
        {
            if data.len() + chunk.len() > max_bytes {
                return Err(too_large());
            }
            data.extend_from_slice(&chunk);
        }
        Ok(data)
    }

    async fn picture_keys(&self, camera: &CameraConfig) -> Result<Vec<String>, BridgeError> {
        // Same hash-checked order as video: option, then cloud.
        let status = self.cached_status(camera).await.ok();
        Ok(candidate_keys(self, camera, status.as_ref())
            .await
            .iter()
            .map(|key| key.code().to_owned())
            .collect())
    }
}
