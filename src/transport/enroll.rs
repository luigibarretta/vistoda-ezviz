use std::path::Path;

use md5::{Digest as _, Md5};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use uuid::Uuid;

use crate::{error::BridgeError, storage::atomic_write_json, transport::EzvizToken};

use super::http::mobile_headers;

enum LoginOutcome {
    Token(EzvizToken),
    MfaRequired(String),
}

pub async fn enroll(
    account: &str,
    password: &str,
    region: &str,
    token_path: &Path,
    timeout_seconds: u64,
    mfa_reader: impl FnOnce() -> Result<String, BridgeError>,
) -> Result<(), BridgeError> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_seconds))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let feature = hex::encode(Md5::digest(Uuid::new_v4().as_bytes()));
    let mut token = match login(&client, account, password, region, &feature, None).await? {
        LoginOutcome::Token(token) => token,
        LoginOutcome::MfaRequired(actual_region) => {
            send_mfa(&client, account, &actual_region, &feature).await?;
            let code = mfa_reader()?;
            if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(BridgeError::Configuration(
                    "MFA code must contain six digits".into(),
                ));
            }
            match login(
                &client,
                account,
                password,
                &actual_region,
                &feature,
                Some(&code),
            )
            .await?
            {
                LoginOutcome::Token(token) => token,
                LoginOutcome::MfaRequired(_) => return Err(BridgeError::Authentication),
            }
        }
    };
    token.service_urls = Some(service_urls(&client, &token).await?);
    atomic_write_json(token_path, &token)
}

async fn login(
    client: &Client,
    account: &str,
    password: &str,
    region: &str,
    feature: &str,
    mfa: Option<&str>,
) -> Result<LoginOutcome, BridgeError> {
    let region = normalize_region(region);
    let password_digest = hex::encode(Md5::digest(password.as_bytes()));
    let response = client
        .post(format!("https://{region}/v3/users/login/v5"))
        .headers(mobile_headers(feature))
        .form(&[
            ("account", account),
            ("password", password_digest.as_str()),
            ("featureCode", feature),
            ("msgType", if mfa.is_some() { "3" } else { "0" }),
            ("bizType", if mfa.is_some() { "TERMINAL_BIND" } else { "" }),
            ("cuName", "SGFzc2lv"),
            ("smsCode", mfa.unwrap_or("")),
        ])
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(BridgeError::Authentication);
    }
    let value: Value = response.json().await?;
    match value
        .get("meta")
        .and_then(|meta| meta.get("code"))
        .and_then(Value::as_i64)
    {
        Some(200) => Ok(LoginOutcome::Token(EzvizToken {
            session_id: required(&value, &["loginSession", "sessionId"])?,
            rf_session_id: required(&value, &["loginSession", "rfSessionId"])?,
            username: Some(required(&value, &["loginUser", "username"])?),
            api_url: required(&value, &["loginArea", "apiDomain"])?,
            feature_code: Some(feature.to_owned()),
            service_urls: None,
            extra: std::collections::BTreeMap::new(),
        })),
        Some(6002) => Ok(LoginOutcome::MfaRequired(region)),
        Some(1100) => {
            let redirected = required(&value, &["loginArea", "apiDomain"])?;
            Box::pin(login(client, account, password, &redirected, feature, mfa)).await
        }
        _ => Err(BridgeError::Authentication),
    }
}

async fn send_mfa(
    client: &Client,
    account: &str,
    region: &str,
    feature: &str,
) -> Result<(), BridgeError> {
    let response = client
        .post(format!(
            "https://{}/v3/sms/nologin/checkcode",
            normalize_region(region)
        ))
        .headers(mobile_headers(feature))
        .form(&[("from", account), ("bizType", "TERMINAL_BIND")])
        .send()
        .await?;
    if response.status() != StatusCode::OK {
        return Err(BridgeError::Authentication);
    }
    let value: Value = response.json().await?;
    if value
        .get("meta")
        .and_then(|meta| meta.get("code"))
        .and_then(Value::as_i64)
        != Some(200)
    {
        return Err(BridgeError::Authentication);
    }
    Ok(())
}

async fn service_urls(client: &Client, token: &EzvizToken) -> Result<Value, BridgeError> {
    let mut headers = mobile_headers(token.feature_code.as_deref().unwrap_or(""));
    headers.insert(
        "sessionid",
        token
            .session_id
            .parse()
            .map_err(|_| BridgeError::Authentication)?,
    );
    let response = client
        .get(format!(
            "https://{}/v3/configurations/system/info",
            token.api_url
        ))
        .headers(headers)
        .send()
        .await?;
    let value: Value = response.json().await?;
    value
        .get("systemConfigInfo")
        .cloned()
        .ok_or_else(|| BridgeError::Upstream("system info omitted service URLs".into()))
}

fn required(value: &Value, path: &[&str]) -> Result<String, BridgeError> {
    path.iter()
        .try_fold(value, |current, key| {
            current.get(*key).ok_or(BridgeError::Authentication)
        })?
        .as_str()
        .map(str::to_owned)
        .ok_or(BridgeError::Authentication)
}
fn normalize_region(value: &str) -> String {
    if value.contains('.') {
        value
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_owned()
    } else {
        format!("apii{value}.ezvizlife.com")
    }
}
