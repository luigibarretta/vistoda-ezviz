//! Read-only EZVIZ unified-message calls for the alarm feed.
//!
//! Only GET endpoints are used; messages are never marked read or deleted.
//! Errors never carry URLs, serials or session identifiers.

use async_trait::async_trait;
use reqwest::Method;
use serde_json::Value;

use crate::{
    alarms::AlarmSource,
    config::CameraConfig,
    device::candidate_keys,
    error::BridgeError,
    transport::{EzvizTransport, http::meta_code, media::download_presigned},
};

const SUMMARY_PATH: &str = "/v3/unifiedmsg/summarybydevice/v2";
const LIST_PATH: &str = "/v3/unifiedmsg/list/v2";
/// Every alarm subtype, as used by the official app's alarm tab.
const ALARM_STYPE: &str = "92";
const PAGE_LIMIT: &str = "20";

impl EzvizTransport {
    /// Session-bearing GET; any non-200 `meta.code` is an error. Shared by
    /// the alarm feed, the device APIs and the control reads.
    pub(super) async fn message_json(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Value, BridgeError> {
        let reply = self
            .authed(Method::GET, path, |request, _| request.query(query))
            .await?;
        if !reply.status.is_success() {
            return Err(BridgeError::Upstream(format!(
                "EZVIZ message API returned HTTP {}",
                reply.status.as_u16()
            )));
        }
        match meta_code(&reply.value) {
            Some(200) | None => Ok(reply.value),
            Some(code) => Err(BridgeError::Upstream(format!(
                "EZVIZ message API returned code {code}"
            ))),
        }
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
        // Pre-signed URL: plain GET without EZVIZ session headers.
        download_presigned(&self.media, url, max_bytes).await
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
