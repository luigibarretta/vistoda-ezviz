//! One EZVIZ control request with a single session refresh.
//! Errors never carry URLs, serials, codes or session identifiers.

use reqwest::{Method, StatusCode};
use serde_json::Value;
use url::Url;

use crate::{
    error::BridgeError,
    transport::{
        EzvizToken, EzvizTransport, alarms::SESSION_EXPIRED, client::success_code, http::meta_code,
    },
};

/// Request body variants used by the control endpoints.
pub(super) enum Body<'a> {
    Empty,
    /// `application/x-www-form-urlencoded` fields.
    Form(&'a [(&'a str, String)]),
    /// Legacy `/api/...` form that also repeats the session fields.
    Legacy(&'a [(&'a str, String)]),
}

impl EzvizTransport {
    /// Sends one request. `query` is appended verbatim after URL-safe
    /// normalization, which keeps `{`, `}` and `:` as EZVIZ expects.
    pub(super) async fn control_json(
        &self,
        method: Method,
        path: &str,
        query: Option<&str>,
        body: &Body<'_>,
    ) -> Result<Value, BridgeError> {
        for attempt in 0..=1 {
            let token = self.token.read().await.clone();
            let mut url = Url::parse(&format!("https://{}{path}", token.api_url))
                .map_err(|_| BridgeError::Upstream("EZVIZ API host is invalid".into()))?;
            url.set_query(query);
            let request = self.request(method.clone(), url.as_str(), &token);
            let request = match body {
                Body::Empty => request,
                Body::Form(fields) => request.form(fields),
                Body::Legacy(fields) => request.form(&legacy_fields(&token, fields)),
            };
            let response = request
                .send()
                .await
                .map_err(|error| BridgeError::Http(error.without_url()))?;
            let status = response.status();
            let value: Value = if status == StatusCode::UNAUTHORIZED {
                Value::Null
            } else {
                response
                    .json()
                    .await
                    .map_err(|error| BridgeError::Http(error.without_url()))?
            };
            let expired =
                status == StatusCode::UNAUTHORIZED || meta_code(&value) == Some(SESSION_EXPIRED);
            if expired && attempt == 0 {
                self.refresh_session().await?;
                continue;
            }
            if expired {
                return Err(BridgeError::Authentication);
            }
            if !status.is_success() {
                return Err(BridgeError::Upstream(format!(
                    "EZVIZ control API returned HTTP {}",
                    status.as_u16()
                )));
            }
            return Ok(value);
        }
        Err(BridgeError::Authentication)
    }
}

fn legacy_fields(token: &EzvizToken, fields: &[(&str, String)]) -> Vec<(String, String)> {
    let mut form: Vec<(String, String)> = fields
        .iter()
        .map(|(name, value)| ((*name).to_owned(), value.clone()))
        .collect();
    form.extend([
        ("clientType".to_owned(), "3".to_owned()),
        ("netType".to_owned(), "WIFI".to_owned()),
        (
            "featureCode".to_owned(),
            token.feature_code.clone().unwrap_or_default(),
        ),
        ("sessionId".to_owned(), token.session_id.clone()),
    ]);
    form
}

/// `meta.code == 200` envelope check for `/v3` endpoints.
pub(super) fn require_meta_ok(value: &Value) -> Result<(), BridgeError> {
    if meta_code(value) == Some(200) {
        Ok(())
    } else {
        Err(BridgeError::Upstream(
            "EZVIZ refused the control request".into(),
        ))
    }
}

/// `resultCode == 0` check for legacy `/api` endpoints.
pub(super) fn require_result_ok(value: &Value) -> Result<(), BridgeError> {
    if success_code(value.get("resultCode")) {
        Ok(())
    } else {
        Err(BridgeError::Upstream(
            "EZVIZ refused the control request".into(),
        ))
    }
}

/// `value` query string for a devconfig key: JSON with quotes encoded and
/// braces and colons literal, exactly as pyezvizapi sends it.
pub(super) fn key_value_query(key: &str, value: &str) -> String {
    let encoded: String = value
        .chars()
        .map(|character| match character {
            '"' => "%22".to_owned(),
            ' ' => "%20".to_owned(),
            other => other.to_string(),
        })
        .collect();
    format!("key={key}&value={encoded}")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{key_value_query, require_meta_ok, require_result_ok};

    #[test]
    fn devconfig_query_keeps_braces_and_colons() {
        assert_eq!(
            key_value_query("Alarm_DetectHumanCar", "{\"type\":3}"),
            "key=Alarm_DetectHumanCar&value={%22type%22:3}"
        );
    }

    #[test]
    fn envelopes_are_checked() {
        assert!(require_meta_ok(&json!({"meta": {"code": 200}})).is_ok());
        assert!(require_meta_ok(&json!({"meta": {"code": 504}})).is_err());
        assert!(require_result_ok(&json!({"resultCode": "0"})).is_ok());
        assert!(require_result_ok(&json!({"resultCode": "-1"})).is_err());
    }
}
