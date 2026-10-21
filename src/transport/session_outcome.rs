//! Classifies the answer to a session refresh.

use reqwest::StatusCode;
use serde_json::Value;

use crate::{error::BridgeError, transport::http::meta_code};

/// HTTP 401/403 or a non-server-error envelope code means EZVIZ refused the
/// refresh token; server errors, throttling and unreadable bodies are transient.
pub(super) fn refreshed_session(status: StatusCode, value: &Value) -> Result<&Value, BridgeError> {
    let code = meta_code(value);
    let server_side = |code: i64| (500..600).contains(&code);
    if status.is_success() && code == Some(200) {
        return value
            .get("sessionInfo")
            .ok_or_else(|| BridgeError::Upstream("refresh response omitted sessionInfo".into()));
    }
    let transient = status.is_server_error()
        || status == StatusCode::TOO_MANY_REQUESTS
        || code.is_none_or(server_side);
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) || !transient {
        return Err(BridgeError::ReauthRequired);
    }
    Err(BridgeError::Upstream(format!(
        "EZVIZ session refresh returned HTTP {}",
        status.as_u16()
    )))
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;
    use serde_json::json;

    use super::refreshed_session;
    use crate::error::BridgeError;

    #[test]
    fn refresh_outcomes_separate_rejection_from_transient_failures() {
        let ok = json!({"meta": {"code": 200}, "sessionInfo": {}});
        assert!(refreshed_session(StatusCode::OK, &ok).is_ok());
        for (status, body) in [
            (StatusCode::OK, json!({"meta": {"code": 403}})),
            (StatusCode::OK, json!({"meta": {"code": 1002}})),
            (StatusCode::UNAUTHORIZED, json!(null)),
            (StatusCode::FORBIDDEN, json!({"meta": {"code": 500}})),
        ] {
            let result = refreshed_session(status, &body);
            assert!(matches!(result, Err(BridgeError::ReauthRequired)), "{body}");
        }
        for (status, body) in [
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"meta": {"code": 403}}),
            ),
            (StatusCode::TOO_MANY_REQUESTS, json!(null)),
            (StatusCode::OK, json!({"meta": {"code": 504}})),
            (StatusCode::OK, json!(null)),
        ] {
            let result = refreshed_session(status, &body);
            assert!(matches!(result, Err(BridgeError::Upstream(_))), "{body}");
        }
    }
}
