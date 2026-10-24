use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RecordingManifest {
    pub schema_version: u8,
    pub recording_id: String,
    pub camera: String,
    pub status: String,
    pub requested_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub requested_duration_seconds: u64,
    pub actual_duration_seconds: Option<f64>,
    pub media_type: String,
    pub bytes: Option<u64>,
    pub sha256: Option<String>,
    pub error_code: Option<String>,
    /// `sd` for a microSD interval; absent for live captures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// microSD interval in epoch seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd_start: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd_end: Option<i64>,
}

impl RecordingManifest {
    #[must_use]
    pub fn pending(camera: &str, duration: u64) -> Self {
        Self {
            schema_version: 1,
            recording_id: Uuid::new_v4().to_string(),
            camera: camera.to_owned(),
            status: "pending".into(),
            requested_at: utc_now(),
            started_at: None,
            completed_at: None,
            requested_duration_seconds: duration,
            actual_duration_seconds: None,
            media_type: "application/octet-stream".into(),
            bytes: None,
            sha256: None,
            error_code: None,
            source: None,
            sd_start: None,
            sd_end: None,
        }
    }

    /// A queued microSD interval `[start, end)` in epoch seconds.
    #[must_use]
    pub fn pending_sd(camera: &str, start: i64, end: i64) -> Self {
        let duration = u64::try_from(end.saturating_sub(start)).unwrap_or(0);
        Self {
            source: Some("sd".into()),
            sd_start: Some(start),
            sd_end: Some(end),
            ..Self::pending(camera, duration)
        }
    }

    /// Same camera and same requested media: an idempotent retry.
    #[must_use]
    pub fn same_request(&self, other: &Self) -> bool {
        self.camera == other.camera
            && self.requested_duration_seconds == other.requested_duration_seconds
            && self.source == other.source
            && self.sd_start == other.sd_start
            && self.sd_end == other.sd_end
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self.status.as_str(), "pending" | "recording")
    }

    pub fn fail(&mut self, code: &str) {
        self.status = "failed".into();
        self.completed_at = Some(utc_now());
        self.error_code = Some(code.into());
    }
}

#[must_use]
pub fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}
