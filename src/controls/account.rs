//! Account-level group defence mode (the app's home-page arming control and
//! Home Assistant core's `alarm_control_panel`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::fields;

/// pyezvizapi `DefenseModeType`: 1 home, 2 away, 3 sleep (0 is unset).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefenceMode {
    Home,
    Away,
    Sleep,
}

impl DefenceMode {
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Home => 1,
            Self::Away => 2,
            Self::Sleep => 3,
        }
    }

    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        match code {
            1 => Some(Self::Home),
            2 => Some(Self::Away),
            3 => Some(Self::Sleep),
            _ => None,
        }
    }
}

/// `GET /v1/account/defence` response body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AccountDefence {
    pub mode: Option<DefenceMode>,
}

/// `PUT /v1/account/defence` request body.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefenceUpdate {
    pub mode: DefenceMode,
    /// Required; `null` means the caller saw no mode.
    #[serde(deserialize_with = "required_nullable")]
    pub expected_mode: Option<DefenceMode>,
}

/// A custom deserializer disables serde's implicit `None` for a missing field.
fn required_nullable<'de, D>(deserializer: D) -> Result<Option<DefenceMode>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::deserialize(deserializer)
}

/// Reads `mode` from a `group/defenceMode` response (a numeric string).
#[must_use]
pub fn parse_group_mode(value: &Value) -> Option<DefenceMode> {
    DefenceMode::from_code(value.get("mode").and_then(fields::int)?)
}
