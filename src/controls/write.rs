//! Validation and mapping of control writes.

use serde::Deserialize;
use serde_json::{Value, json};

use super::{ControlError, Controls, DetectionMode, PtzDirection, SwitchSpec, VendorWrite, names};

/// `PUT /v1/cameras/{camera}/controls` request body.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlRequest {
    pub key: String,
    pub value: Value,
    pub expected_value: Value,
}

/// `POST /v1/cameras/{camera}/ptz` request body.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PtzRequest {
    pub direction: PtzDirection,
}

/// A writable control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ControlKey {
    Defence,
    DetectionMode,
    Sensitivity,
    Switch(&'static SwitchSpec),
}

/// A validated control value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Desired {
    Flag(bool),
    Mode(DetectionMode),
    Level(i64),
}

impl Desired {
    pub(super) fn to_json(self) -> Value {
        match self {
            Self::Flag(flag) => json!(flag),
            Self::Mode(mode) => json!(mode),
            Self::Level(level) => json!(level),
        }
    }
}

pub(super) fn parse_key(key: &str) -> Result<ControlKey, ControlError> {
    match key {
        "defence_enabled" => Ok(ControlKey::Defence),
        "detection_mode" => Ok(ControlKey::DetectionMode),
        "sensitivity" => Ok(ControlKey::Sensitivity),
        _ => key
            .strip_prefix("switch.")
            .and_then(names::by_name)
            .map(ControlKey::Switch)
            .ok_or(ControlError::Invalid("unknown control key")),
    }
}

/// Type-checks a value for `key`; ranges are checked against the camera.
pub(super) fn parse_value(key: ControlKey, value: &Value) -> Result<Desired, ControlError> {
    let parsed = match key {
        ControlKey::Defence | ControlKey::Switch(_) => value.as_bool().map(Desired::Flag),
        ControlKey::DetectionMode => serde_json::from_value(value.clone())
            .ok()
            .map(Desired::Mode),
        ControlKey::Sensitivity => value.as_i64().map(Desired::Level),
    };
    parsed.ok_or(ControlError::Invalid(
        "value does not match the control type",
    ))
}

/// Current value of `key`, or `None` when the camera does not report it.
pub(super) fn current(controls: &Controls, key: ControlKey) -> Option<Desired> {
    match key {
        ControlKey::Defence => controls.defence_enabled.map(Desired::Flag),
        ControlKey::DetectionMode => controls.detection_mode.map(Desired::Mode),
        ControlKey::Sensitivity => controls
            .sensitivity
            .map(|level| Desired::Level(level.value)),
        ControlKey::Switch(spec) => controls.switches.get(spec.name).copied().map(Desired::Flag),
    }
}

/// Maps a validated value to its vendor write, checking sensitivity range.
pub(super) fn vendor_write(
    key: ControlKey,
    desired: Desired,
    controls: &Controls,
) -> Result<VendorWrite, ControlError> {
    match (key, desired) {
        (ControlKey::Defence, Desired::Flag(flag)) => Ok(VendorWrite::Defence(flag)),
        (ControlKey::Switch(spec), Desired::Flag(enable)) => Ok(VendorWrite::Switch {
            kind: spec.kind,
            enable,
        }),
        (ControlKey::DetectionMode, Desired::Mode(mode)) => {
            Ok(VendorWrite::DetectionMode(mode.code()))
        }
        (ControlKey::Sensitivity, Desired::Level(value)) => {
            let range = controls.sensitivity.ok_or(ControlError::Unsupported)?;
            if !(range.min..=range.max).contains(&value) {
                return Err(ControlError::Invalid(
                    "sensitivity is outside the camera range",
                ));
            }
            Ok(VendorWrite::Sensitivity {
                kind: range.kind,
                value,
            })
        }
        _ => Err(ControlError::Invalid(
            "value does not match the control type",
        )),
    }
}
