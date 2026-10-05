//! EZVIZ calls behind the native controls.
//!
//! Every endpoint is used by pyezvizapi 1.0.0.7 (and so by Home Assistant
//! core) and declared by the 7.6.1 app without a `validateCode`, SMS or risk
//! parameter: `DeviceApi.switchStatus`, `changeDefenceStatus`, `ptzControl`,
//! `getDefenceMode`, `switchDefenceMode`, `PlayComponentApi.configDeviceKeyValue`
//! and `VideoGoNetSDK` `/api/device/queryAlgorithmConfig` and
//! `/api/device/configAlgorithm`. This module must never call the cloud
//! verification-code lookup; a unit test below enforces that.

use async_trait::async_trait;
use reqwest::Method;
use serde_json::Value;

use crate::{
    config::CameraConfig,
    controls::{ControlSource, PtzAction, PtzDirection, VendorWrite},
    error::BridgeError,
    transport::{
        EzvizTransport,
        control_http::{Body, key_value_query, require_meta_ok, require_result_ok},
    },
};

const PAGELIST_PATH: &str = "/v3/userdevices/v1/resources/pagelist";
const CONTROL_FILTER: &str = "STATUS,SWITCH,TIME_PLAN,UPGRADE";
const GROUP_MODE_PATH: &str = "/v3/userdevices/v1/group/defenceMode";
const SWITCH_MODE_PATH: &str = "/v3/userdevices/v1/group/switchDefenceMode";
const ALGORITHM_GET_PATH: &str = "/api/device/queryAlgorithmConfig";
const ALGORITHM_SET_PATH: &str = "/api/device/configAlgorithm";
/// pyezvizapi's default PTZ speed.
const PTZ_SPEED: &str = "5";
/// `meta.code` 504: the camera did not answer (often a sleeping battery
/// camera). Retried once; pyezvizapi's own retry is unbounded.
const CAMERA_TIMEOUT: i64 = 504;

#[async_trait]
impl ControlSource for EzvizTransport {
    async fn control_page(&self, offset: usize, limit: usize) -> Result<Value, BridgeError> {
        self.message_json(
            PAGELIST_PATH,
            &[
                ("groupId", "-1".into()),
                ("limit", limit.to_string()),
                ("offset", offset.to_string()),
                ("filter", CONTROL_FILTER.into()),
            ],
        )
        .await
    }

    async fn algorithm_config(&self, camera: &CameraConfig) -> Result<Value, BridgeError> {
        let fields = [("subSerial", camera.serial.clone())];
        let value = self
            .control_json(
                Method::POST,
                ALGORITHM_GET_PATH,
                None,
                &Body::Legacy(&fields),
            )
            .await?;
        require_result_ok(&value)?;
        Ok(value)
    }

    async fn group_defence_mode(&self) -> Result<Value, BridgeError> {
        self.message_json(GROUP_MODE_PATH, &[("groupId", "-1".into())])
            .await
    }

    async fn write(&self, camera: &CameraConfig, write: VendorWrite) -> Result<(), BridgeError> {
        match write {
            VendorWrite::Switch { kind, enable } => {
                // Device-level switch on channel 0, as Home Assistant core sends it.
                let path = format!(
                    "/v3/devices/{}/0/{}/{kind}/switchStatus",
                    camera.serial,
                    u8::from(enable)
                );
                let value = self
                    .control_json(Method::PUT, &path, None, &Body::Empty)
                    .await?;
                require_meta_ok(&value)
            }
            VendorWrite::Defence(enable) => self.set_defence(camera, enable).await,
            VendorWrite::DetectionMode(code) => {
                let path = format!("/v3/devconfig/v1/keyValue/{}/1/op", camera.serial);
                let query =
                    key_value_query("Alarm_DetectHumanCar", &format!("{{\"type\":{code}}}"));
                let value = self
                    .control_json(Method::PUT, &path, Some(&query), &Body::Empty)
                    .await?;
                require_meta_ok(&value)
            }
            VendorWrite::Sensitivity { kind, value } => {
                let fields = [
                    ("subSerial", camera.serial.clone()),
                    ("type", kind.to_string()),
                    ("channelNo", camera.channel.to_string()),
                    ("value", value.to_string()),
                ];
                let value = self
                    .control_json(
                        Method::POST,
                        ALGORITHM_SET_PATH,
                        None,
                        &Body::Legacy(&fields),
                    )
                    .await?;
                require_result_ok(&value)
            }
        }
    }

    async fn set_group_defence_mode(&self, mode: u8) -> Result<(), BridgeError> {
        let fields = [("groupId", "-1".to_owned()), ("mode", mode.to_string())];
        let value = self
            .control_json(Method::POST, SWITCH_MODE_PATH, None, &Body::Form(&fields))
            .await?;
        require_meta_ok(&value)
    }

    async fn ptz(
        &self,
        camera: &CameraConfig,
        direction: PtzDirection,
        action: PtzAction,
    ) -> Result<(), BridgeError> {
        let path = format!("/v3/devices/{}/ptzControl", camera.serial);
        let fields = [
            ("command", direction.command().to_owned()),
            ("action", action.as_str().to_owned()),
            ("channelNo", camera.channel.to_string()),
            ("speed", PTZ_SPEED.to_owned()),
            ("uuid", uuid::Uuid::new_v4().to_string()),
            ("serial", camera.serial.clone()),
        ];
        let value = self
            .control_json(Method::PUT, &path, None, &Body::Form(&fields))
            .await?;
        require_meta_ok(&value)
    }
}

impl EzvizTransport {
    /// Per-camera arming (`type=Global`, `actor=V`), at most two attempts.
    async fn set_defence(&self, camera: &CameraConfig, enable: bool) -> Result<(), BridgeError> {
        let path = format!(
            "/v3/devices/{}/{}/changeDefenceStatusReq",
            camera.serial, camera.channel
        );
        let fields = [
            ("type", "Global".to_owned()),
            ("status", u8::from(enable).to_string()),
            ("actor", "V".to_owned()),
        ];
        let mut value = Value::Null;
        for _ in 0..2 {
            value = self
                .control_json(Method::PUT, &path, None, &Body::Form(&fields))
                .await?;
            if crate::transport::http::meta_code(&value) != Some(CAMERA_TIMEOUT) {
                break;
            }
        }
        require_meta_ok(&value)
    }
}

#[cfg(test)]
mod tests {
    /// The control transport must never reach the cloud verification code:
    /// that request can make EZVIZ email or text the owner.
    #[test]
    fn control_transport_never_touches_the_cloud_key() {
        for source in [include_str!("controls.rs"), include_str!("control_http.rs")] {
            let code = source.split("#[cfg(test)]").next().unwrap_or_default();
            for forbidden in [
                "encryptkey",
                "camera_key",
                "cloud_keys",
                "cloud_verification_code",
            ] {
                assert!(!code.contains(forbidden), "{forbidden}");
            }
        }
    }
}
