//! Cloud device-status parsing shared by encryption, storage and record APIs.
//!
//! The parsed values come from the `STATUS` and `deviceInfos` sections of the
//! consumer resource page list. The encryption hash is the app's
//! `MD5(MD5(verification code))` digest; it is never serialized or logged.

use chrono::{Local, NaiveDateTime, Utc};
use serde_json::Value;

/// Device capability index of the app's "record search protocol" switch.
const RECORD_SEARCH_CAPABILITY: &str = "256";
/// Largest accepted UTC offset.
const MAX_OFFSET_SECONDS: i32 = 14 * 3600;

/// SD-card record index endpoint selected by the device capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordSearchVersion {
    /// `GET /v3/streaming/v2/records`: zlib/base64 JSON list (capability 1).
    V2,
    /// `GET /v3/streaming/common/records`: zlib/base64 packed day (capability 2).
    Common,
}

/// Read-only cloud view of one camera. `Debug` is intentionally absent.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct DeviceStatus {
    /// `STATUS.isEncrypt`; `None` when the cloud omitted it.
    pub video_encrypted: Option<bool>,
    /// Lower-case `STATUS.encryptPwd` (twice-MD5 of the verification code).
    pub encrypt_pwd_hash: Option<String>,
    /// Camera UTC offset from `STATUS.optionals.timeZone`.
    pub utc_offset_seconds: Option<i32>,
    /// Preferred SD record index protocol from `supportExt["256"]`.
    pub record_search: Option<RecordSearchVersion>,
}

impl DeviceStatus {
    /// Converts a camera-local wall-clock time to epoch seconds, falling back
    /// to the bridge's local zone when the camera offset is unknown.
    #[must_use]
    pub fn epoch(&self, local: NaiveDateTime) -> i64 {
        let offset = self
            .utc_offset_seconds
            .unwrap_or_else(|| Utc::now().with_timezone(&Local).offset().local_minus_utc());
        local.and_utc().timestamp() - i64::from(offset)
    }
}

/// Extracts one serial's status from a page-list response, if present.
#[must_use]
pub fn parse_device_status(page: &Value, serial: &str) -> Option<DeviceStatus> {
    let status = page.get("STATUS")?.get(serial)?;
    if !status.is_object() {
        return None;
    }
    let video_encrypted = status
        .get("isEncrypt")
        .and_then(integer)
        .and_then(|value| match value {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        });
    let encrypt_pwd_hash = status
        .get("encryptPwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|hash| hash.len() == 32 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase);
    let utc_offset_seconds = status
        .get("optionals")
        .and_then(|optionals| optionals.get("timeZone"))
        .and_then(Value::as_str)
        .and_then(parse_utc_offset);
    Some(DeviceStatus {
        video_encrypted,
        encrypt_pwd_hash,
        utc_offset_seconds,
        record_search: record_search(page, serial),
    })
}

fn record_search(page: &Value, serial: &str) -> Option<RecordSearchVersion> {
    let device = page
        .get("deviceInfos")?
        .as_array()?
        .iter()
        .find(|item| item.get("deviceSerial").and_then(Value::as_str) == Some(serial))?;
    let raw = device.get("supportExt")?;
    // The app stores capabilities as a JSON object serialized into a string.
    let support: Value = raw
        .as_str()
        .map_or_else(|| Some(raw.clone()), |text| serde_json::from_str(text).ok())?;
    match support.get(RECORD_SEARCH_CAPABILITY).and_then(integer)? {
        1 => Some(RecordSearchVersion::V2),
        2 => Some(RecordSearchVersion::Common),
        _ => None,
    }
}

/// Parses `UTC+02:00`, `GMT-5`, `+0530` or `UTC` into signed offset seconds.
/// Zone names such as `Europe/Rome` are not resolved and return `None`.
#[must_use]
pub fn parse_utc_offset(value: &str) -> Option<i32> {
    let upper = value.trim().to_ascii_uppercase();
    let rest = upper
        .strip_prefix("UTC")
        .or_else(|| upper.strip_prefix("GMT"))
        .unwrap_or(&upper)
        .trim();
    if rest.is_empty() {
        return (upper == "UTC" || upper == "GMT").then_some(0);
    }
    // Byte slicing below is only valid for ASCII input.
    if !rest.is_ascii() {
        return None;
    }
    let (sign, digits) = match rest.as_bytes()[0] {
        b'+' => (1, &rest[1..]),
        b'-' => (-1, &rest[1..]),
        _ => (1, rest),
    };
    let (hours, minutes) = match digits.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None if digits.len() == 4 => digits.split_at(2),
        None => (digits, "0"),
    };
    if hours.is_empty()
        || hours.len() > 2
        || minutes.len() > 2
        || !hours
            .bytes()
            .chain(minutes.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let hours: i32 = hours.parse().ok()?;
    let minutes: i32 = minutes.parse().ok()?;
    if minutes >= 60 {
        return None;
    }
    let offset = sign * (hours * 3600 + minutes * 60);
    (offset.abs() <= MAX_OFFSET_SECONDS).then_some(offset)
}

/// Integer from a JSON number or numeric string.
pub fn integer(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const HASH: &str = "0123456789ABCDEF0123456789ABCDEF";

    #[test]
    fn status_fields_are_scoped_to_the_exact_serial() {
        let page = json!({
            "STATUS": {
                "CAM1": {"isEncrypt": 1, "encryptPwd": HASH,
                         "optionals": {"timeZone": "UTC+02:00"}},
                "CAM2": {"isEncrypt": "0"}
            },
            "deviceInfos": [
                {"deviceSerial": "CAM2", "supportExt": "{\"256\":\"1\"}"},
                {"deviceSerial": "CAM1", "supportExt": "{\"256\":\"2\"}"}
            ]
        });
        let first = parse_device_status(&page, "CAM1").unwrap_or_else(|| panic!("CAM1"));
        assert_eq!(first.video_encrypted, Some(true));
        assert_eq!(
            first.encrypt_pwd_hash.as_deref(),
            Some(&*HASH.to_ascii_lowercase())
        );
        assert_eq!(first.utc_offset_seconds, Some(7200));
        assert_eq!(first.record_search, Some(RecordSearchVersion::Common));
        let second = parse_device_status(&page, "CAM2").unwrap_or_else(|| panic!("CAM2"));
        assert_eq!(second.video_encrypted, Some(false));
        assert_eq!(second.encrypt_pwd_hash, None);
        assert_eq!(second.record_search, Some(RecordSearchVersion::V2));
        assert!(parse_device_status(&page, "CAM3").is_none());
    }

    #[test]
    fn malformed_status_values_degrade_to_unknown() {
        let page = json!({"STATUS": {"CAM1": {"isEncrypt": 7, "encryptPwd": "short",
            "optionals": {"timeZone": "Europe/Rome"}}},
            "deviceInfos": [{"deviceSerial": "CAM1", "supportExt": "not json"}]});
        let status = parse_device_status(&page, "CAM1").unwrap_or_else(|| panic!("CAM1"));
        assert!(status == DeviceStatus::default());
    }

    #[test]
    fn utc_offsets_cover_vendor_spellings_and_bounds() {
        for (input, expected) in [
            ("UTC+02:00", Some(7200)),
            ("GMT-5", Some(-18_000)),
            ("+0530", Some(19_800)),
            ("utc", Some(0)),
            ("UTC-03:30", Some(-12_600)),
            ("UTC+15:00", None),
            ("UTC+02:75", None),
            ("Europe/Rome", None),
            ("1\u{e9}1", None),
            ("UTC+1\u{e9}1", None),
            ("+\u{e9}", None),
            ("", None),
        ] {
            assert_eq!(parse_utc_offset(input), expected, "{input}");
        }
    }

    #[test]
    fn epoch_uses_the_camera_offset() {
        let status = DeviceStatus {
            utc_offset_seconds: Some(7200),
            ..DeviceStatus::default()
        };
        let local = NaiveDateTime::parse_from_str("2026-10-04T12:00:00", "%Y-%m-%dT%H:%M:%S")
            .unwrap_or_else(|error| panic!("{error}"));
        // 12:00 at UTC+02:00 is 10:00 UTC.
        assert_eq!(status.epoch(local), 1_791_108_000);
    }
}
