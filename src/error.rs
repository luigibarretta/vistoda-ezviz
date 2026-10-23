use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::{Value, json};
use thiserror::Error;

/// Upper bound, in bytes, of an error detail written to the log.
pub(crate) const MAX_LOGGED_DETAIL: usize = 160;

#[must_use]
pub(crate) fn redact_detail(detail: &str) -> String {
    let mut text = detail
        .split_whitespace()
        .map(|word| if word.contains("://") { "[url]" } else { word })
        .collect::<Vec<_>>()
        .join(" ");
    if text.len() > MAX_LOGGED_DETAIL {
        let mut end = MAX_LOGGED_DETAIL;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("...");
    }
    text
}

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("configuration error: {0}")]
    Configuration(String),
    /// The camera rejected every verification code (never a session error).
    #[error("verification code was rejected by the camera")]
    KeyRejected,
    /// EZVIZ refused the session and its refresh: the account must be enrolled again.
    #[error("EZVIZ session must be renewed")]
    ReauthRequired,
    /// One endpoint still refused a session that was just refreshed; the
    /// request fails, but the account is not declared revoked.
    #[error("EZVIZ refused a freshly refreshed session")]
    SessionExpired,
    #[error("upstream deadline exceeded")]
    Timeout,
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
            Self::KeyRejected => "key_rejected",
            Self::ReauthRequired => "reauth_required",
            Self::SessionExpired => "session_expired",
            Self::Timeout => "timeout",
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
            Self::KeyRejected => "verification code rejected",
            Self::ReauthRequired => "EZVIZ session must be renewed",
            Self::SessionExpired => "EZVIZ refused a freshly refreshed session",
            Self::Timeout => "upstream deadline exceeded",
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
            Self::Http(error) if error.is_timeout() => "HTTP timeout",
            Self::Http(error) if error.is_connect() => "HTTP connection failure",
            Self::Http(error) if error.is_redirect() => "HTTP redirect failure",
            Self::Http(error) if error.is_body() || error.is_decode() => "HTTP body failure",
            Self::Http(_) => "HTTP transport failure",
            Self::Json(_) => "JSON decoding failure",
            Self::Task(_) => "task failure",
        }
    }

    /// [`Self::diagnostic_detail`] made safe for logs. Every detail is built
    /// from static text, HTTP statuses and vendor result codes; as a defence
    /// in depth any word containing `://` (a URL, possibly pre-signed) is
    /// replaced and the text is capped at [`MAX_LOGGED_DETAIL`] bytes.
    #[must_use]
    pub(crate) fn logged_detail(&self) -> String {
        redact_detail(self.diagnostic_detail())
    }

    #[must_use]
    pub fn public_response(&self) -> (StatusCode, Value) {
        match self {
            // 401 is reserved for a bad bridge token; 403 asks for re-enrollment.
            Self::ReauthRequired => (StatusCode::FORBIDDEN, json!({"error":"reauth_required"})),
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
            Self::SessionExpired | Self::Timeout => {
                (StatusCode::BAD_GATEWAY, json!({"error":"upstream_failed"}))
            }
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
        if !matches!(self, Self::ReauthRequired | Self::CameraNotFound) {
            tracing::error!(
                error_type = self.diagnostic_code(),
                detail = %self.logged_detail(),
                "request failed"
            );
        }
        let (status, value) = self.public_response();
        (status, Json(value)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{BridgeError, MAX_LOGGED_DETAIL};

    #[test]
    fn logged_details_redact_urls_and_are_capped() {
        let error = BridgeError::Upstream(
            "media failed at https://img.ezvizlife.com/p.jpg?Signature=secret now".into(),
        );
        assert_eq!(error.logged_detail(), "media failed at [url] now");
        let long = BridgeError::Upstream(format!("{}é", "x".repeat(MAX_LOGGED_DETAIL - 1)));
        let logged = long.logged_detail();
        assert!(logged.ends_with("...") && logged.len() <= MAX_LOGGED_DETAIL + 3);
        let plain = BridgeError::Upstream("EZVIZ API returned HTTP 502".into());
        assert_eq!(plain.logged_detail(), "EZVIZ API returned HTTP 502");
        assert_eq!(
            BridgeError::Timeout.logged_detail(),
            "upstream deadline exceeded"
        );
    }
}
