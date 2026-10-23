//! The one place that decides when an EZVIZ session has expired.
//!
//! Every session-bearing JSON call goes through [`EzvizTransport::authed`]:
//! HTTP 401 or `meta.code` 99997 triggers one single-flight refresh and one
//! retry. A refused refresh is `ReauthRequired`; an endpoint that still
//! refuses a freshly refreshed session fails with `SessionExpired`. Bodies are
//! read with a size bound, and errors never carry URLs (session identifiers
//! travel in some query strings).

use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde_json::Value;

use crate::{
    error::BridgeError,
    transport::{EzvizToken, EzvizTransport, http::meta_code},
};

/// Server-side session expiry reported inside an HTTP 200 envelope.
pub(super) const SESSION_EXPIRED: i64 = 99_997;
/// Upper bound for one EZVIZ JSON response body.
const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;

/// HTTP status and decoded body of one accepted (non-expired) response.
pub(super) struct Reply {
    pub(super) status: StatusCode,
    pub(super) value: Value,
}

#[must_use]
pub(super) fn session_expired(status: StatusCode, value: &Value) -> bool {
    status == StatusCode::UNAUTHORIZED || meta_code(value) == Some(SESSION_EXPIRED)
}

/// Drops the URL (and so any session identifier) from a transport error.
pub(super) fn http_error(error: reqwest::Error) -> BridgeError {
    BridgeError::Http(error.without_url())
}

impl EzvizTransport {
    /// Sends `build(request, token)` to `https://{api_url}{path}` with the
    /// session headers, refreshing and retrying once when the session expired.
    pub(super) async fn authed<F>(
        &self,
        method: Method,
        path: &str,
        build: F,
    ) -> Result<Reply, BridgeError>
    where
        F: Fn(RequestBuilder, &EzvizToken) -> RequestBuilder + Send + Sync,
    {
        for attempt in 0..2 {
            let (token, generation) = self.session.current().await;
            let url = format!("{}://{}{path}", self.scheme, token.api_url);
            let request = build(self.request(method.clone(), &url, &token), &token);
            let response = request.send().await.map_err(http_error)?;
            let status = response.status();
            let value = if status == StatusCode::UNAUTHORIZED {
                Value::Null
            } else {
                read_json(response, status.is_success()).await?
            };
            if !session_expired(status, &value) {
                // Only a real success proves the session; a 5xx proves nothing.
                if status.is_success() && matches!(meta_code(&value), None | Some(200)) {
                    self.session.health.confirmed();
                }
                return Ok(Reply { status, value });
            }
            if attempt == 0 {
                self.refresh_session(generation).await?;
            }
        }
        // The refresh succeeded, yet this endpoint still refuses the session:
        // fail this request only. Declaring the account revoked here would
        // send Home Assistant into a re-authentication loop.
        tracing::warn!("EZVIZ refused a freshly refreshed session");
        Err(BridgeError::SessionExpired)
    }
}

/// Reads at most `max` bytes; larger bodies are refused, not truncated.
pub(super) async fn read_limited(
    mut response: Response,
    max: usize,
) -> Result<Vec<u8>, BridgeError> {
    let too_large = || BridgeError::Upstream("EZVIZ response exceeds the size limit".into());
    if response
        .content_length()
        .is_some_and(|length| length > max as u64)
    {
        return Err(too_large());
    }
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(http_error)? {
        if data.len() + chunk.len() > max {
            return Err(too_large());
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

/// Bounded JSON body. `strict` is set for success statuses; an unreadable
/// error body becomes `null` so the HTTP status stays the reported failure.
pub(super) async fn read_json(response: Response, strict: bool) -> Result<Value, BridgeError> {
    let bytes = read_limited(response, MAX_JSON_BYTES).await?;
    match serde_json::from_slice(&bytes) {
        Ok(value) => Ok(value),
        Err(error) if strict => Err(error.into()),
        Err(_) => Ok(Value::Null),
    }
}
