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
        }
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
