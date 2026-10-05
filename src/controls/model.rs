//! Control read model built from the cached account-wide resource list.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{fields, names};

/// `Alarm_DetectHumanCar` types understood by pyezvizapi 1.0.0.7.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionMode {
    HumanShape,
    ImageChange,
    Pir,
}

impl DetectionMode {
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::HumanShape => 1,
            Self::ImageChange => 3,
            Self::Pir => 5,
        }
    }

    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        match code {
            1 => Some(Self::HumanShape),
            3 => Some(Self::ImageChange),
            5 => Some(Self::Pir),
            _ => None,
        }
    }
}

/// Detection sensitivity; `kind` is the vendor algorithm type, not exposed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Sensitivity {
    pub value: i64,
    pub min: i64,
    pub max: i64,
    #[serde(skip)]
    pub kind: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Battery {
    pub percent: Option<i64>,
    pub work_mode: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Firmware {
    pub version: Option<String>,
    pub update_available: Option<bool>,
}

/// `GET /v1/cameras/{camera}/controls` response body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Controls {
    pub online: Option<bool>,
    pub defence_enabled: Option<bool>,
    pub alarm_schedule_enabled: Option<bool>,
    pub detection_mode: Option<DetectionMode>,
    pub sensitivity: Option<Sensitivity>,
    pub switches: BTreeMap<&'static str, bool>,
    pub ptz: bool,
    pub battery: Option<Battery>,
    pub firmware: Option<Firmware>,
}

/// Raw sections of one device. `Debug` is intentionally absent.
#[derive(Clone, Default)]
pub struct CameraData {
    pub device: Value,
    pub status: Value,
    pub switches: Value,
    pub upgrade: Value,
    pub time_plan: Value,
}

/// Configured serials to their raw sections.
pub type Snapshot = BTreeMap<String, CameraData>;

const SECTIONS: [&str; 4] = ["STATUS", "SWITCH", "TIME_PLAN", "UPGRADE"];

/// Adds one resource-list page, keeping only configured serials. The
/// verification-code hash is dropped so the snapshot never retains it.
pub fn merge_page(snapshot: &mut Snapshot, page: &Value, serials: &BTreeSet<String>) {
    let devices = page.get("deviceInfos").and_then(Value::as_array);
    for device in devices.into_iter().flatten() {
        if let Some(serial) = device
            .get("deviceSerial")
            .and_then(Value::as_str)
            .filter(|serial| serials.contains(*serial))
        {
            snapshot.entry(serial.to_owned()).or_default().device = device.clone();
        }
    }
    for section in SECTIONS {
        let entries = page.get(section).and_then(Value::as_object);
        for (serial, value) in entries.into_iter().flatten() {
            if !serials.contains(serial) {
                continue;
            }
            let data = snapshot.entry(serial.clone()).or_default();
            let slot = match section {
                "STATUS" => &mut data.status,
                "SWITCH" => &mut data.switches,
                "TIME_PLAN" => &mut data.time_plan,
                _ => &mut data.upgrade,
            };
            *slot = value.clone();
            if let Some(status) = data.status.as_object_mut() {
                status.remove("encryptPwd");
            }
        }
    }
}

#[must_use]
pub fn page_has_next(page: &Value) -> bool {
    page.get("page")
        .and_then(|value| value.get("hasNext"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Builds the API view of one device.
#[must_use]
pub fn controls_from(data: &CameraData, sensitivity: Option<Sensitivity>) -> Controls {
    Controls {
        online: fields::online(&data.device),
        defence_enabled: fields::defence(&data.device, &data.status),
        alarm_schedule_enabled: fields::alarm_schedule(&data.time_plan),
        detection_mode: fields::detection_mode(&data.status),
        sensitivity,
        switches: switches(data),
        ptz: fields::ptz(&data.device),
        battery: fields::battery(&data.status),
        firmware: fields::firmware(&data.device, &data.upgrade),
    }
}

fn switches(data: &CameraData) -> BTreeMap<&'static str, bool> {
    let mut result = BTreeMap::new();
    for item in data.switches.as_array().into_iter().flatten() {
        let Some(spec) = item
            .get("type")
            .and_then(fields::int)
            .and_then(|kind| u16::try_from(kind).ok())
            .and_then(names::by_kind)
        else {
            continue;
        };
        let supported = spec
            .capability
            .is_none_or(|key| fields::support_ext(&data.device, key).is_some());
        if let Some(enabled) = item
            .get("enable")
            .and_then(fields::flag)
            .filter(|_| supported)
        {
            result.insert(spec.name, enabled);
        }
    }
    result
}

/// `supportExt["61"]` (`SupportSensibilityAdjust`): `"1"` selects algorithm
/// type 0 (0..=6), `"3"` type 3 (0..=100), as Home Assistant core does.
#[must_use]
pub fn sensitivity_kind(device: &Value) -> Option<u8> {
    match fields::support_ext(device, "61")?.as_str() {
        "1" => Some(0),
        "3" => Some(3),
        _ => None,
    }
}

const fn sensitivity_max(kind: u8) -> i64 {
    if kind == 3 { 100 } else { 6 }
}

/// Reads the sensitivity of `kind` for `channel` from `queryAlgorithmConfig`.
#[must_use]
pub fn parse_sensitivity(raw: &Value, kind: u8, channel: u16) -> Option<Sensitivity> {
    if raw.get("resultCode").and_then(fields::int) != Some(0) {
        return None;
    }
    let config = fields::object(raw.get("algorithmConfig")?);
    let max = sensitivity_max(kind);
    config
        .get("algorithmList")?
        .as_array()?
        .iter()
        .filter(|entry| entry.get("type").and_then(fields::int) == Some(i64::from(kind)))
        .find(|entry| {
            entry
                .get("channel")
                .and_then(fields::int)
                .is_none_or(|value| value == i64::from(channel))
        })
        .and_then(|entry| entry.get("value").and_then(fields::int))
        .filter(|value| (0..=max).contains(value))
        .map(|value| Sensitivity {
            value,
            min: 0,
            max,
            kind,
        })
}
