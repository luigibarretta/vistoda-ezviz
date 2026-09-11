use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("configuration error: {0}")]
    Configuration(String),
    #[error("authentication failed")]
    Authentication,
    #[error("vendor credentials were rejected")]
    InvalidCredentials,
    #[error("verification code was rejected")]
    InvalidOtp,
    #[error("another enrollment is active")]
    EnrollmentBusy,
    #[error("enrollment expired or was consumed")]
    EnrollmentExpired,
    #[error("camera was not found")]
    CameraNotFound,
    #[error("camera is offline")]
    CameraOffline,
    #[error("requested media representation is unavailable")]
    UnsupportedMedia,
    #[error("capacity limit reached: {0}")]
    Capacity(String),
    #[error("invalid recording request: {0}")]
    Recording(String),
    #[error("recording is still active")]
    RecordingActive,
    #[error("upstream protocol failed: {0}")]
    Upstream(String),
    #[error("upstream media is unavailable")]
    UpstreamUnavailable,
    #[error("I/O operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP operation failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON operation failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

impl BridgeError {
    #[must_use]
    pub(crate) const fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::Configuration(_) => "configuration",
            Self::Authentication => "authentication",
            Self::InvalidCredentials => "invalid_credentials",
            Self::InvalidOtp => "invalid_otp",
            Self::EnrollmentBusy => "enrollment_busy",
            Self::EnrollmentExpired => "enrollment_expired",
            Self::CameraNotFound => "camera_not_found",
            Self::CameraOffline => "camera_offline",
            Self::UnsupportedMedia => "unsupported_media",
            Self::Capacity(_) => "capacity",
            Self::Recording(_) => "recording",
            Self::RecordingActive => "recording_active",
            Self::Upstream(_) => "upstream",
            Self::UpstreamUnavailable => "upstream_unavailable",
            Self::Io(_) => "io",
            Self::Http(_) => "http",
            Self::Json(_) => "json",
            Self::Task(_) => "task",
        }
    }

    #[must_use]
    pub(crate) fn diagnostic_detail(&self) -> &str {
        match self {
            Self::Configuration(detail)
            | Self::Capacity(detail)
            | Self::Recording(detail)
            | Self::Upstream(detail) => detail,
            Self::Authentication => "authentication failed",
            Self::InvalidCredentials => "vendor credentials were rejected",
            Self::InvalidOtp => "verification code was rejected",
            Self::EnrollmentBusy => "another enrollment is active",
            Self::EnrollmentExpired => "enrollment expired or was consumed",
            Self::CameraNotFound => "camera not found",
            Self::CameraOffline => "camera offline",
            Self::UnsupportedMedia => "requested media representation is unavailable",
            Self::RecordingActive => "recording active",
            Self::UpstreamUnavailable => "upstream media unavailable",
            Self::Io(_) => "I/O failure",
            Self::Http(_) => "HTTP transport failure",
            Self::Json(_) => "JSON decoding failure",
            Self::Task(_) => "task failure",
        }
    }

    #[must_use]
    pub fn public_response(&self) -> (StatusCode, Value) {
        match self {
            Self::Authentication => (StatusCode::UNAUTHORIZED, json!({"error":"unauthorized"})),
            Self::InvalidCredentials => (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"error":"invalid_auth"}),
            ),
            Self::InvalidOtp => (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"error":"invalid_otp"}),
            ),
            Self::EnrollmentBusy => (StatusCode::CONFLICT, json!({"error":"enrollment_busy"})),
            Self::EnrollmentExpired => (StatusCode::GONE, json!({"error":"enrollment_expired"})),
            Self::CameraNotFound => (StatusCode::NOT_FOUND, json!({"error":"camera_not_found"})),
            Self::CameraOffline => (
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error":"camera_offline"}),
            ),
            Self::UnsupportedMedia => (StatusCode::CONFLICT, json!({"error":"unsupported_media"})),
            Self::Capacity(_) => (StatusCode::TOO_MANY_REQUESTS, json!({"error":"capacity"})),
            Self::Recording(detail) => (
                StatusCode::BAD_REQUEST,
                json!({"error":"invalid_recording_request", "detail":detail}),
            ),
            Self::RecordingActive => (StatusCode::CONFLICT, json!({"error":"recording_active"})),
            Self::UpstreamUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error":"upstream_unavailable"}),
            ),
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"error":"internal_error"}),
            ),
        }
    }
}

impl IntoResponse for BridgeError {
    fn into_response(self) -> axum::response::Response {
        if !matches!(self, Self::Authentication | Self::CameraNotFound) {
            tracing::error!(error_type = self.diagnostic_code(), "request failed");
        }
        let (status, value) = self.public_response();
        (status, Json(value)).into_response()
    }
}
