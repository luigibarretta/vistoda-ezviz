use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("configuration error: {0}")]
    Configuration(String),
    #[error("authentication failed")]
    Authentication,
    #[error("camera was not found")]
    CameraNotFound,
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
    pub fn public_response(&self) -> (StatusCode, Value) {
        match self {
            Self::Authentication => (StatusCode::UNAUTHORIZED, json!({"error":"unauthorized"})),
            Self::CameraNotFound => (StatusCode::NOT_FOUND, json!({"error":"camera_not_found"})),
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
            tracing::error!(error_type = error_kind(&self), "request failed");
        }
        let (status, value) = self.public_response();
        (status, Json(value)).into_response()
    }
}

const fn error_kind(error: &BridgeError) -> &'static str {
    match error {
        BridgeError::Configuration(_) => "configuration",
        BridgeError::Authentication => "authentication",
        BridgeError::CameraNotFound => "camera_not_found",
        BridgeError::Capacity(_) => "capacity",
        BridgeError::Recording(_) => "recording",
        BridgeError::RecordingActive => "recording_active",
        BridgeError::Upstream(_) => "upstream",
        BridgeError::UpstreamUnavailable => "upstream_unavailable",
        BridgeError::Io(_) => "io",
        BridgeError::Http(_) => "http",
        BridgeError::Json(_) => "json",
        BridgeError::Task(_) => "task",
    }
}
