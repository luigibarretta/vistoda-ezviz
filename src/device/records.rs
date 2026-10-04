//! Read-only SD-card record index (`/v3/streaming/{v2,common}/records`).
//!
//! Both responses carry a base64 zlib payload, as decoded by the 7.6.1 app in
//! `RecordDataRemoteEzviz`. V2 holds a JSON list of `{B, E, Type}` with
//! camera-local `yyyy-MM-ddTHH:mm:ss` times; common holds 8-byte entries
//! `[start h, m, s, stop h, m, s, recordType, pad]` relative to `baseDay`.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{DeviceStatus, RecordSearchVersion, integer};
use crate::error::BridgeError;

/// Largest number of records returned for one day.
pub const MAX_RECORDS: usize = 500;
/// V2 page size used by the official app.
pub const V2_PAGE_SIZE: usize = 100;
/// Bounded V2 continuation pages (5 x 100 = `MAX_RECORDS`).
pub const MAX_V2_PAGES: usize = MAX_RECORDS / V2_PAGE_SIZE;
/// Upper bound for encoded and inflated payloads.
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
const LOCAL_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

/// One recorded interval in epoch seconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdRecord {
    pub start: i64,
    pub end: i64,
    #[serde(rename = "type")]
    pub kind: String,
}

/// One upstream search request; times are camera-local wall clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordQuery {
    pub version: RecordSearchVersion,
    pub start: String,
    pub stop: String,
    pub size: usize,
}

/// Parsed V2 page plus its continuation cursor.
pub struct V2Page {
    pub records: Vec<SdRecord>,
    pub finished: bool,
    pub last_end: Option<String>,
}

/// Camera-local query window covering one calendar day.
#[must_use]
pub fn day_window(day: NaiveDate) -> (String, String) {
    let date = day.format("%Y-%m-%d");
    (format!("{date}T00:00:00"), format!("{date}T23:59:59"))
}

fn ensure_meta(value: &Value) -> Result<(), BridgeError> {
    match value
        .get("meta")
        .and_then(|meta| meta.get("code"))
        .and_then(integer)
    {
        Some(200) | None => Ok(()),
        Some(code) => Err(BridgeError::Upstream(format!(
            "EZVIZ record search returned code {code}"
        ))),
    }
}

/// Decodes the app's base64 + zlib payload within `MAX_PAYLOAD_BYTES`.
pub fn decode_payload(encoded: &str) -> Result<Vec<u8>, BridgeError> {
    let invalid = || BridgeError::Upstream("record payload is invalid".into());
    if encoded.len() > MAX_PAYLOAD_BYTES {
        return Err(invalid());
    }
    let compact: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
    let compressed = STANDARD.decode(compact).map_err(|_| invalid())?;
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&compressed, MAX_PAYLOAD_BYTES)
        .map_err(|_| invalid())
}

fn v2_kind(value: Option<&Value>) -> &'static str {
    if value.and_then(integer) == Some(1) {
        "event"
    } else {
        "continuous"
    }
}

/// Parses one V2 response page.
pub fn parse_v2_page(value: &Value, status: &DeviceStatus) -> Result<V2Page, BridgeError> {
    ensure_meta(value)?;
    let finished = value.get("isFinished").and_then(integer).unwrap_or(1) != 0;
    let items = match value.get("records") {
        Some(Value::String(text)) if !text.trim().is_empty() => {
            let raw = decode_payload(text)?;
            let text = String::from_utf8(raw)
                .map_err(|_| BridgeError::Upstream("record payload is not UTF-8".into()))?;
            if text.trim().is_empty() {
                Vec::new()
            } else {
                serde_json::from_str::<Vec<Value>>(text.trim())
                    .map_err(|_| BridgeError::Upstream("record list is not JSON".into()))?
            }
        }
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let mut records = Vec::new();
    let mut last_end = None;
    for item in items.iter().take(MAX_RECORDS) {
        let (Some(begin), Some(end)) = (
            item.get("B").and_then(Value::as_str),
            item.get("E").and_then(Value::as_str),
        ) else {
            continue;
        };
        let (Ok(begin_local), Ok(end_local)) = (
            NaiveDateTime::parse_from_str(begin.trim(), LOCAL_FORMAT),
            NaiveDateTime::parse_from_str(end.trim(), LOCAL_FORMAT),
        ) else {
            continue;
        };
        last_end = Some(end.trim().to_owned());
        if begin_local >= end_local {
            continue;
        }
        records.push(SdRecord {
            start: status.epoch(begin_local),
            end: status.epoch(end_local),
            kind: v2_kind(item.get("Type")).into(),
        });
    }
    Ok(V2Page {
        records,
        finished,
        last_end,
    })
}

const fn common_kind(code: u8) -> &'static str {
    match code {
        1 => "event",
        8..=10 => "other",
        _ => "continuous",
    }
}

/// Parses a common (packed) response for one camera-local day.
pub fn parse_common(value: &Value, status: &DeviceStatus) -> Result<Vec<SdRecord>, BridgeError> {
    ensure_meta(value)?;
    let base_day = value.get("baseDay").and_then(Value::as_str).unwrap_or("");
    let data = value.get("data").and_then(Value::as_str).unwrap_or("");
    let count = value.get("searchCount").and_then(integer).unwrap_or(0);
    if base_day.trim().is_empty() || data.trim().is_empty() || count <= 0 {
        return Ok(Vec::new());
    }
    let day = NaiveDate::parse_from_str(base_day.trim(), "%Y-%m-%d")
        .map_err(|_| BridgeError::Upstream("record base day is invalid".into()))?;
    let limit = usize::try_from(count)
        .unwrap_or(MAX_RECORDS)
        .min(MAX_RECORDS);
    let packed = decode_payload(data)?;
    let mut records = Vec::new();
    for entry in packed.chunks(8).filter(|entry| entry.len() >= 7) {
        let start = NaiveTime::from_hms_opt(entry[0].into(), entry[1].into(), entry[2].into());
        let stop = NaiveTime::from_hms_opt(entry[3].into(), entry[4].into(), entry[5].into());
        let (Some(start), Some(stop)) = (start, stop) else {
            continue;
        };
        if start >= stop {
            continue;
        }
        records.push(SdRecord {
            start: status.epoch(day.and_time(start)),
            end: status.epoch(day.and_time(stop)),
            kind: common_kind(entry[6]).into(),
        });
        if records.len() >= limit {
            break;
        }
    }
    Ok(records)
}

/// Sorts, removes duplicate page-boundary entries and applies `MAX_RECORDS`.
#[must_use]
pub fn normalize(mut records: Vec<SdRecord>) -> Vec<SdRecord> {
    records.sort_by_key(|record| (record.start, record.end));
    records.dedup_by(|next, previous| next.start == previous.start && next.end == previous.end);
    records.truncate(MAX_RECORDS);
    records
}
