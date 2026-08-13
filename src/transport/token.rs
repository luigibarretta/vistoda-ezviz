use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::BridgeError;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EzvizToken {
    pub session_id: String,
    pub rf_session_id: String,
    pub api_url: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub feature_code: Option<String>,
    #[serde(default)]
    pub service_urls: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl EzvizToken {
    pub fn validate(&self) -> Result<(), BridgeError> {
        if self.session_id.len() < 20
            || self.rf_session_id.len() < 20
            || self.api_url.trim().is_empty()
        {
            return Err(BridgeError::Configuration(
                "EZVIZ token file is incomplete".into(),
            ));
        }
        Ok(())
    }

    pub fn auth_address(&self) -> Result<String, BridgeError> {
        if let Some(address) = self
            .service_urls
            .as_ref()
            .and_then(|value| value.get("authAddr"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "null" && *value != "none")
        {
            return Ok(with_https(address));
        }
        let Some(region) = self
            .api_url
            .strip_prefix("apii")
            .and_then(|value| value.strip_suffix(".ezvizlife.com"))
            .filter(|value| !value.is_empty())
        else {
            return Err(BridgeError::Upstream(
                "EZVIZ token has no usable auth service".into(),
            ));
        };
        Ok(format!("https://{region}auth.ezvizlife.com"))
    }
}

fn with_https(value: &str) -> String {
    if value.starts_with("https://") || value.starts_with("http://") {
        value.trim_end_matches('/').to_owned()
    } else {
        format!("https://{}", value.trim_end_matches('/'))
    }
}
