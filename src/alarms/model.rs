//! EZVIZ unified-message parsing and the bounded public alarm event model.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Longest public title, in characters.
pub const MAX_TITLE_CHARS: usize = 120;
/// Longest accepted alarm identifier; longer upstream IDs are hashed.
pub const MAX_ID_CHARS: usize = 96;

/// One alarm as persisted in the per-camera history.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AlarmRecord {
    pub id: String,
    pub occurred_at: i64,
    pub alarm_type: i64,
    pub category: String,
    pub title: String,
    pub has_picture: bool,
    #[serde(default)]
    pub picture_bytes: u64,
}

/// A parsed upstream message plus the transient picture descriptor, which is
/// never persisted or logged because `pic` is a pre-signed URL.
#[derive(Clone, Debug)]
pub struct ParsedMessage {
    pub record: AlarmRecord,
    pub channel: Option<u16>,
    pub time_ms: Option<i64>,
    pub picture: Option<PictureSource>,
}

#[derive(Clone)]
pub struct PictureSource {
    pub url: String,
    pub crypt: i64,
    pub checksum: Option<String>,
}

impl std::fmt::Debug for PictureSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PictureSource")
            .field("crypt", &self.crypt)
            .finish_non_exhaustive()
    }
}

/// One device entry from `summarybydevice/v2`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceSummary {
    pub serial: String,
    pub total: u64,
    pub top_id: Option<String>,
    /// Camera-local UTC offset inferred from the top message, in seconds.
    pub utc_offset: Option<i32>,
}

/// Maps an EZVIZ `ext.alarmType` (and message `subType`) to a public category.
#[must_use]
pub const fn category(alarm_type: i64, sub_type: i64) -> &'static str {
    match alarm_type {
        10_000 | 10_002 | 10_013 | 10_014 | 10_140 => "motion",
        10_010 | 10_015 | 10_079 | 10_120 | 10_121 | 10_131 | 15_500 => "person",
        10_130 => "vehicle",
        2_701 | 10_016 => "doorbell",
        10_003 | 10_022 | 15_008 => "sound",
        15_003 => "pet",
        10_032 | 10_071 | 30_010 => "offline",
        10_011 | 12_021 | 12_027 => "tamper",
        _ => match sub_type {
            2_701 => "doorbell",
            2_402 => "motion",
            2_403 => "person",
            2_404 => "vehicle",
            2_405 => "sound",
            _ => "other",
        },
    }
}

/// Parses the per-device summaries, ignoring malformed entries.
#[must_use]
pub fn parse_summary(value: &Value) -> Vec<DeviceSummary> {
    let Some(entries) = value.get("summaries").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let serial = entry.get("deviceSerial")?.as_str()?.to_owned();
            let top = entry.get("topMessage");
            Some(DeviceSummary {
                serial,
                total: entry
                    .get("total")
                    .and_then(integer)
                    .and_then(|total| u64::try_from(total).ok())
                    .unwrap_or(0),
                top_id: top.and_then(message_id),
                utc_offset: top.and_then(utc_offset),
            })
        })
        .collect()
}

/// Parses one `list/v2` page into messages and the `hasNext` flag.
#[must_use]
pub fn parse_list(value: &Value) -> (Vec<ParsedMessage>, bool) {
    let has_next = value
        .get("hasNext")
        .is_some_and(|flag| flag.as_bool() == Some(true) || integer(flag) == Some(1));
    let messages = value
        .get("message")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_message).collect())
        .unwrap_or_default();
    (messages, has_next)
}

#[must_use]
pub fn parse_message(item: &Value) -> Option<ParsedMessage> {
    let ext = item.get("ext");
    let time_ms = item.get("time").and_then(integer);
    let sub_type = item.get("subType").and_then(integer).unwrap_or(0);
    let alarm_type = ext
        .and_then(|ext| ext.get("alarmType"))
        .and_then(integer)
        .unwrap_or(0);
    let id = message_id(item).or_else(|| fallback_id(item, alarm_type))?;
    let title = ["title", "detail"]
        .iter()
        .find_map(|key| item.get(*key).and_then(Value::as_str))
        .or_else(|| {
            ext.and_then(|ext| ext.get("alarmName"))
                .and_then(Value::as_str)
        })
        .map(sanitize_title)
        .unwrap_or_default();
    let picture = item
        .get("pic")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        .map(|url| PictureSource {
            url: url.to_owned(),
            crypt: item.get("picCrypt").and_then(integer).unwrap_or(0),
            checksum: item
                .get("picChecksum")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    Some(ParsedMessage {
        record: AlarmRecord {
            id,
            occurred_at: time_ms.map_or_else(|| chrono::Utc::now().timestamp(), |ms| ms / 1000),
            alarm_type,
            category: category(alarm_type, sub_type).to_owned(),
            title,
            has_picture: false,
            picture_bytes: 0,
        },
        channel: item
            .get("channel")
            .and_then(integer)
            .and_then(|channel| u16::try_from(channel).ok()),
        time_ms,
        picture,
    })
}

/// Returns a bounded upstream message identifier usable as a path token.
fn message_id(item: &Value) -> Option<String> {
    let raw = item.get("msgId").and_then(|value| {
        value
            .as_str()
            .map(str::to_owned)
            .or_else(|| value.as_i64().map(|number| number.to_string()))
    })?;
    if raw.is_empty() {
        return None;
    }
    if is_token(&raw) {
        return Some(raw);
    }
    Some(hashed_id(&raw))
}

/// Deterministic identifier for messages without a usable `msgId`.
fn fallback_id(item: &Value, alarm_type: i64) -> Option<String> {
    let serial = item.get("deviceSerial").and_then(Value::as_str)?;
    let channel = item.get("channel").and_then(integer).unwrap_or(0);
    let time = item.get("timeStr").and_then(Value::as_str)?;
    Some(hashed_id(&format!(
        "{serial}|{channel}|{alarm_type}|{time}"
    )))
}

fn hashed_id(raw: &str) -> String {
    format!("h{}", &hex::encode(Sha256::digest(raw.as_bytes()))[..40])
}

/// Accepts the bounded identifier alphabet used for alarm IDs and file names.
#[must_use]
pub fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_CHARS
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Keeps printable characters only, collapses whitespace and bounds length.
#[must_use]
pub fn sanitize_title(raw: &str) -> String {
    let collapsed = raw
        .split(|character: char| character.is_whitespace() || character.is_control())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    collapsed.chars().take(MAX_TITLE_CHARS).collect()
}

/// Infers the camera-local UTC offset from `timeStr` (local) versus `time`
/// (epoch milliseconds), rounded to 15 minutes and bounded to ±14 hours.
#[must_use]
pub fn utc_offset(item: &Value) -> Option<i32> {
    let epoch = item.get("time").and_then(integer)? / 1000;
    let text = item.get("timeStr").and_then(Value::as_str)?;
    let local = chrono::NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M:%S").ok()?;
    let raw = local.and_utc().timestamp() - epoch;
    let rounded = (raw + 450).div_euclid(900) * 900;
    i32::try_from(rounded)
        .ok()
        .filter(|offset| offset.abs() <= 14 * 3600)
}

fn integer(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}
