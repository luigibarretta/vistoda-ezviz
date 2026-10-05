//! Public errors of the control endpoints. Bodies never carry serials, codes,
//! URLs or vendor messages.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use crate::error::BridgeError;

#[derive(Debug, PartialEq, Eq)]
pub enum ControlError {
    /// 404: the camera alias is not configured.
    CameraNotFound,
    /// 503: the EZVIZ runtime is not connected.
    Unavailable,
    /// 400: malformed body, unknown key or value of the wrong type or range.
    Invalid(&'static str),
    /// 409: the camera does not report this control or capability.
    Unsupported,
    /// 409: the current value differs from `expected_value`.
    Conflict(Value),
    /// 502: the write was not confirmed by the read-back.
    Unconfirmed,
    /// 502: EZVIZ refused or failed the request.
    Upstream,
}

impl ControlError {
    /// Logs only the error category, never the vendor detail.
    #[must_use]
    pub fn upstream(error: &BridgeError) -> Self {
        tracing::warn!(
            error_type = error.diagnostic_code(),
            "EZVIZ control request failed"
        );
        Self::Upstream
    }

    #[must_use]
    pub fn public_response(&self) -> (StatusCode, Value) {
        match self {
            Self::CameraNotFound => (StatusCode::NOT_FOUND, json!({"error": "camera_not_found"})),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error": "upstream_unavailable"}),
            ),
            Self::Invalid(detail) => (
                StatusCode::BAD_REQUEST,
                json!({"error": "invalid_control", "detail": detail}),
            ),
            Self::Unsupported => (
                StatusCode::CONFLICT,
                json!({"error": "unsupported_control"}),
            ),
            Self::Conflict(current) => (
                StatusCode::CONFLICT,
                json!({"error": "conflict", "current": current}),
            ),
            Self::Unconfirmed => (StatusCode::BAD_GATEWAY, json!({"error": "unconfirmed"})),
            Self::Upstream => (StatusCode::BAD_GATEWAY, json!({"error": "upstream_failed"})),
        }
    }
}

impl IntoResponse for ControlError {
    fn into_response(self) -> Response {
        let (status, body) = self.public_response();
        let mut response = (status, Json(body)).into_response();
        crate::api::no_store(response.headers_mut());
        response
    }
}
