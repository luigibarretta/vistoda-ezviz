use axum::http::{HeaderMap, header::AUTHORIZATION};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use subtle::ConstantTimeEq;

use crate::error::BridgeError;

#[derive(Clone)]
pub struct ApiAuthenticator {
    token: Box<[u8]>,
}

impl ApiAuthenticator {
    pub fn new(token: String) -> Result<Self, BridgeError> {
        if token.len() < 32
            || matches!(
                token.to_ascii_lowercase().as_str(),
                "change-me" | "changeme" | "password"
            )
        {
            return Err(BridgeError::Configuration(
                "bridge API token is missing, short, or a default".into(),
            ));
        }
        Ok(Self {
            token: token.into_bytes().into_boxed_slice(),
        })
    }

    #[must_use]
    pub fn accepts(&self, headers: &HeaderMap) -> bool {
        let Some(value) = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
        else {
            return false;
        };
        if let Some(candidate) = value.strip_prefix("Bearer ") {
            return constant_time(candidate.as_bytes(), &self.token);
        }
        let Some(encoded) = value.strip_prefix("Basic ") else {
            return false;
        };
        let Ok(decoded) = STANDARD.decode(encoded) else {
            return false;
        };
        let Some(separator) = decoded.iter().position(|byte| *byte == b':') else {
            return false;
        };
        constant_time(&decoded[..separator], b"homeassistant")
            && constant_time(&decoded[separator + 1..], &self.token)
    }
}

fn constant_time(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn accepts_bearer_and_home_assistant_basic_only() {
        let auth = ApiAuthenticator::new("x".repeat(32)).unwrap_or_else(|error| panic!("{error}"));
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", "x".repeat(32)))
                .unwrap_or_else(|error| panic!("{error}")),
        );
        assert!(auth.accepts(&headers));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!(
                "Basic {}",
                STANDARD.encode(format!("homeassistant:{}", "x".repeat(32)))
            ))
            .unwrap_or_else(|error| panic!("{error}")),
        );
        assert!(auth.accepts(&headers));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Basic broken"));
        assert!(!auth.accepts(&headers));
    }
}
