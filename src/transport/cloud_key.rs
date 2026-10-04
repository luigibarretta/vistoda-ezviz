//! Guarded access to the cloud verification code.
//!
//! `/api/device/query/encryptkey` can make EZVIZ email or text the account
//! owner a verification code, so it is reached only through
//! `CloudKeyGuard` (at most one request per camera every 24 hours, persisted)
//! and only when an actual decryption has no usable option code.

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::{
    config::CameraConfig,
    device::candidate_keys,
    error::BridgeError,
    transport::{EzvizTransport, client::success_code, http::required_string, image},
};

/// Attempt times file, stored next to the session token under `/data`.
pub(super) const CLOUD_KEY_STATE_FILE: &str = "cloud-key-attempts.json";

impl EzvizTransport {
    /// Cached cloud code or one guarded lookup.
    pub(super) async fn camera_key(&self, serial: &str) -> Result<Zeroizing<String>, BridgeError> {
        self.cloud_keys
            .fetch(serial, Utc::now().timestamp(), || {
                self.request_camera_key(serial)
            })
            .await
    }

    /// Code fetched earlier in this process, if any.
    pub(super) fn cached_camera_key(&self, serial: &str) -> Option<Zeroizing<String>> {
        self.cloud_keys.cached(serial)
    }

    /// The raw request; only `camera_key` may call it.
    async fn request_camera_key(&self, serial: &str) -> Result<String, BridgeError> {
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
            .await
            .map_err(|error| BridgeError::Http(error.without_url()))?;
        let value: Value = response
            .json()
            .await
            .map_err(|error| BridgeError::Http(error.without_url()))?;
        // Any non-zero result, including 120002 (verification code required),
        // is a failure that latches the 24-hour guard.
        if !success_code(value.get("resultCode")) {
            return Err(BridgeError::Upstream(
                "camera verification code request was refused".into(),
            ));
        }
        required_string(&value, "encryptkey")
    }

    /// Decrypts an encrypted snapshot with the option code, else the guarded
    /// cloud code.
    pub(super) async fn decrypt_snapshot(
        &self,
        camera: &CameraConfig,
        bytes: &[u8],
    ) -> Result<Vec<u8>, BridgeError> {
        let status = self.cached_status(camera).await.ok();
        let keys = candidate_keys(self, camera, status.as_ref()).await;
        let key = keys.first().ok_or_else(|| {
            BridgeError::Upstream("encrypted snapshot has no usable verification code".into())
        })?;
        image::decrypt_image(bytes, key.code())
    }
}
