use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::header::HeaderMap;
use serde_json::Value;

use crate::error::BridgeError;

pub(super) fn mobile_headers(feature_code: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("featurecode", feature_code),
        ("clienttype", "3"),
        ("nettype", "WIFI"),
        ("customno", "1000001"),
        ("clientno", "web_site"),
        ("appid", "ys7"),
        ("language", "en_GB"),
        ("lang", "en"),
        ("user-agent", "okhttp/3.12.1"),
    ] {
        if let (Ok(name), Ok(value)) = (name.parse::<reqwest::header::HeaderName>(), value.parse())
        {
            headers.insert(name, value);
        }
    }
    headers
}

pub(super) fn meta_code(value: &Value) -> Option<i64> {
    value.get("meta")?.get("code")?.as_i64()
}

pub(super) fn required_string(value: &Value, key: &str) -> Result<String, BridgeError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| BridgeError::Upstream(format!("EZVIZ response omitted {key}")))
}

pub(super) fn value_u16(value: Option<&Value>) -> Result<u16, BridgeError> {
    let raw = value
        .and_then(|value| {
            value
                .as_u64()
                .map(|number| number.to_string())
                .or_else(|| value.as_str().map(str::to_owned))
        })
        .ok_or_else(|| BridgeError::Upstream("VTM server port is missing".into()))?;
    raw.parse()
        .map_err(|_| BridgeError::Upstream("VTM server port is invalid".into()))
}

pub(super) fn find_resource<'a>(page: &'a Value, serial: &str) -> Result<&'a Value, BridgeError> {
    page.get("resourceInfos")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("deviceSerial").and_then(Value::as_str) == Some(serial))
        })
        .ok_or_else(|| BridgeError::Upstream("camera resource was not found".into()))
}

pub(super) fn jwt_sign(session: &str) -> Result<String, BridgeError> {
    let payload = session
        .split('.')
        .nth(1)
        .ok_or_else(|| BridgeError::Upstream("session token is not a JWT".into()))?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| BridgeError::Upstream("session JWT is invalid".into()))?;
    let claims: Value = serde_json::from_slice(&decoded)?;
    required_string(&claims, "s")
}
