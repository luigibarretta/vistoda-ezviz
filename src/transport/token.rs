use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

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
            .and_then(usable_auth_address)
        {
            return Ok(address);
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

fn usable_auth_address(value: &str) -> Option<String> {
    let address = with_https(value.trim());
    let host = Url::parse(&address).ok()?.host_str()?.to_ascii_lowercase();
    (!matches!(host.as_str(), "" | "none" | "null")).then_some(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(auth: &str) -> EzvizToken {
        EzvizToken {
            session_id: "s".repeat(20),
            rf_session_id: "r".repeat(20),
            api_url: "apiieu.ezvizlife.com".into(),
            username: None,
            feature_code: Some("f".repeat(32)),
            service_urls: Some(serde_json::json!({"authAddr": auth})),
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn nullish_legacy_auth_address_uses_regional_fallback() {
        assert_eq!(
            token("https://none")
                .auth_address()
                .unwrap_or_else(|error| panic!("{error}")),
            "https://euauth.ezvizlife.com"
        );
        assert_eq!(
            token("auth.example.test")
                .auth_address()
                .unwrap_or_else(|error| panic!("{error}")),
            "https://auth.example.test"
        );
    }
}
