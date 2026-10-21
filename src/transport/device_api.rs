//! Read-only EZVIZ calls behind the device APIs.
//!
//! Endpoints were confirmed in the 7.6.1 app: `VideoGoNetSDK` posts
//! `subSerial` to `/api/device/queryStorageStatus`, and `PlayDeviceApi`
//! declares `GET v3/streaming/v2/records` and `GET v3/streaming/common/records`.
//! Errors never carry URLs, serials, codes or session identifiers.

use async_trait::async_trait;
use reqwest::Method;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::{
    config::CameraConfig,
    device::{DeviceSource, DeviceStatus, RecordQuery, RecordSearchVersion},
    error::BridgeError,
    transport::EzvizTransport,
};

const STORAGE_PATH: &str = "/api/device/queryStorageStatus";
const V2_RECORDS_PATH: &str = "/v3/streaming/v2/records";
const COMMON_RECORDS_PATH: &str = "/v3/streaming/common/records";

#[async_trait]
impl DeviceSource for EzvizTransport {
    async fn device_status(&self, camera: &CameraConfig) -> Result<DeviceStatus, BridgeError> {
        self.cached_status(camera).await
    }

    async fn cloud_verification_code(
        &self,
        camera: &CameraConfig,
    ) -> Result<Zeroizing<String>, BridgeError> {
        self.camera_key(&camera.serial).await
    }

    fn cached_cloud_code(&self, camera: &CameraConfig) -> Option<Zeroizing<String>> {
        self.cached_camera_key(&camera.serial)
    }

    async fn storage_status(&self, camera: &CameraConfig) -> Result<Value, BridgeError> {
        let reply = self
            .authed(Method::POST, STORAGE_PATH, |request, token| {
                request.form(&[
                    ("subSerial", camera.serial.as_str()),
                    ("clientType", "3"),
                    ("netType", "WIFI"),
                    (
                        "featureCode",
                        token.feature_code.as_deref().unwrap_or_default(),
                    ),
                    ("sessionId", token.session_id.as_str()),
                ])
            })
            .await?;
        if !reply.status.is_success() {
            return Err(BridgeError::Upstream(format!(
                "EZVIZ storage API returned HTTP {}",
                reply.status.as_u16()
            )));
        }
        Ok(reply.value)
    }

    async fn record_page(
        &self,
        camera: &CameraConfig,
        query: &RecordQuery,
    ) -> Result<Value, BridgeError> {
        let mut params = vec![
            ("deviceSerial", camera.serial.clone()),
            ("channelNo", camera.channel.to_string()),
            ("startTime", query.start.clone()),
            ("stopTime", query.stop.clone()),
            ("size", query.size.to_string()),
        ];
        let path = match query.version {
            RecordSearchVersion::V2 => {
                params.extend([("sortBy", "0".into()), ("requireLabel", "0".into())]);
                V2_RECORDS_PATH
            }
            RecordSearchVersion::Common => {
                // App order: channelSerial, recordType=-1 (all), version=1.
                params.extend([
                    ("channelSerial", camera.serial.clone()),
                    ("recordType", "-1".into()),
                    ("version", "1".into()),
                ]);
                COMMON_RECORDS_PATH
            }
        };
        self.message_json(path, &params).await
    }
}
