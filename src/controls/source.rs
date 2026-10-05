//! Vendor calls behind the native controls.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::{config::CameraConfig, error::BridgeError};

/// One vendor write. Each variant maps to exactly one EZVIZ request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VendorWrite {
    /// `PUT /v3/devices/{serial}/0/{enable}/{type}/switchStatus`.
    Switch { kind: u16, enable: bool },
    /// `PUT /v3/devices/{serial}/{channel}/changeDefenceStatusReq`.
    Defence(bool),
    /// `PUT /v3/devconfig/v1/keyValue/{serial}/1/op`, key `Alarm_DetectHumanCar`.
    DetectionMode(u8),
    /// `POST /api/device/configAlgorithm` with the capability's `type`.
    Sensitivity { kind: u8, value: i64 },
}

/// PTZ direction accepted by the API and sent upper-case to EZVIZ.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PtzDirection {
    Up,
    Down,
    Left,
    Right,
}

impl PtzDirection {
    #[must_use]
    pub const fn command(self) -> &'static str {
        match self {
            Self::Up => "UP",
            Self::Down => "DOWN",
            Self::Left => "LEFT",
            Self::Right => "RIGHT",
        }
    }
}

/// PTZ phase: one step is a `Start` followed by a `Stop`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PtzAction {
    Start,
    Stop,
}

impl PtzAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "START",
            Self::Stop => "STOP",
        }
    }
}

/// EZVIZ calls used by the controls. There is deliberately no method that
/// could request the cloud verification code.
#[async_trait]
pub trait ControlSource: Send + Sync {
    /// One resource-list page with the `STATUS`, `SWITCH`, `TIME_PLAN` and
    /// `UPGRADE` sections plus `deviceInfos`.
    async fn control_page(&self, offset: usize, limit: usize) -> Result<Value, BridgeError>;
    /// Raw `/api/device/queryAlgorithmConfig` response for one camera.
    async fn algorithm_config(&self, camera: &CameraConfig) -> Result<Value, BridgeError>;
    /// Raw `/v3/userdevices/v1/group/defenceMode` response.
    async fn group_defence_mode(&self) -> Result<Value, BridgeError>;
    /// Sends one write; `Ok` means EZVIZ accepted it, not that it applied.
    async fn write(&self, camera: &CameraConfig, write: VendorWrite) -> Result<(), BridgeError>;
    /// `POST /v3/userdevices/v1/group/switchDefenceMode` with code 1, 2 or 3.
    async fn set_group_defence_mode(&self, mode: u8) -> Result<(), BridgeError>;
    /// One PTZ command at the default speed.
    async fn ptz(
        &self,
        camera: &CameraConfig,
        direction: PtzDirection,
        action: PtzAction,
    ) -> Result<(), BridgeError>;
}
