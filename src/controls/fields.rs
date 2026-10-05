//! Lenient readers for EZVIZ resource-list fields.
//!
//! EZVIZ mixes numbers, numeric strings and JSON objects serialized into
//! strings; every reader returns `None` instead of guessing.

use serde_json::Value;

use super::{Battery, DetectionMode, Firmware};

/// Integer from a JSON number or numeric string.
pub(super) fn int(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

/// Boolean from `true`/`false`, `0`/`1` or their string forms.
pub(super) fn flag(value: &Value) -> Option<bool> {
    if let Some(flag) = value.as_bool() {
        return Some(flag);
    }
    match int(value)? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// A JSON object that may arrive serialized into a string.
pub(super) fn object(value: &Value) -> Value {
    value.as_str().map_or_else(
        || value.clone(),
        |text| serde_json::from_str(text).unwrap_or(Value::Null),
    )
}

/// `deviceInfos.supportExt[key]` as a string, if present.
pub(super) fn support_ext(device: &Value, key: &str) -> Option<String> {
    let support = object(device.get("supportExt")?);
    let value = support.get(key)?;
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_i64().map(|number| number.to_string()))
}

/// Decoded `STATUS.optionals`.
pub(super) fn optionals(status: &Value) -> Value {
    status.get("optionals").map_or(Value::Null, object)
}

/// `STATUS.optionals.Alarm_DetectHumanCar.type` mapped to a known mode.
pub(super) fn detection_mode(status: &Value) -> Option<DetectionMode> {
    let setting = object(optionals(status).get("Alarm_DetectHumanCar")?);
    DetectionMode::from_code(int(setting.get("type")?)?)
}

/// `STATUS.globalStatus`; `None` when `supportExt["1"]` (defence) is `"0"`.
pub(super) fn defence(device: &Value, status: &Value) -> Option<bool> {
    if support_ext(device, "1").as_deref() == Some("0") {
        return None;
    }
    // Home Assistant core reads any non-zero value as armed.
    int(status.get("globalStatus")?)
        .filter(|value| *value >= 0)
        .map(|value| value != 0)
}

/// The `TIME_PLAN` entry of type 2 is the alarm schedule.
pub(super) fn alarm_schedule(time_plan: &Value) -> Option<bool> {
    time_plan
        .as_array()?
        .iter()
        .find(|plan| plan.get("type").and_then(int) == Some(2))
        .and_then(|plan| plan.get("enable"))
        .and_then(flag)
}

/// `deviceInfos.status`: 1 is online, any other number offline.
pub(super) fn online(device: &Value) -> Option<bool> {
    int(device.get("status")?).map(|status| status == 1)
}

/// `supportExt["154"]` (`SupportPtz`) must be exactly `"1"`.
pub(super) fn ptz(device: &Value) -> bool {
    support_ext(device, "154").as_deref() == Some("1")
}

/// Battery level and work mode from `STATUS.optionals`.
pub(super) fn battery(status: &Value) -> Option<Battery> {
    let optionals = optionals(status);
    let percent = optionals
        .get("powerRemaining")
        .and_then(int)
        .filter(|percent| (0..=100).contains(percent));
    let work_mode = optionals
        .get("batteryCameraWorkMode")
        .and_then(int)
        .and_then(work_mode);
    (percent.is_some() || work_mode.is_some()).then_some(Battery { percent, work_mode })
}

/// Names of the pyezvizapi 1.0.0.7 `BatteryCameraWorkMode` values.
const fn work_mode(code: i64) -> Option<&'static str> {
    match code {
        0 => Some("power_saving"),
        1 => Some("high_performance"),
        2 => Some("plugged_in"),
        3 => Some("super_power_saving"),
        4 => Some("user_customization"),
        _ => None,
    }
}

/// Installed version and `UPGRADE.isNeedUpgrade == 3` (update available).
pub(super) fn firmware(device: &Value, upgrade: &Value) -> Option<Firmware> {
    let version = device
        .get("version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|version| !version.is_empty() && version.len() <= 64)
        .map(str::to_owned);
    let update_available = upgrade
        .get("isNeedUpgrade")
        .and_then(int)
        .map(|state| state == 3);
    (version.is_some() || update_available.is_some()).then_some(Firmware {
        version,
        update_available,
    })
}
