use std::path::{Path, PathBuf};

use md5::{Digest as _, Md5};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{error::BridgeError, storage::atomic_write_json, transport::EzvizToken};

use super::http::mobile_headers;

enum LoginOutcome {
    Token(EzvizToken),
    MfaRequired(String),
}

pub enum EnrollmentState {
    Complete,
    Mfa(PendingEnrollment),
}

pub struct PendingEnrollment {
    client: Client,
    account: String,
    password: Zeroizing<String>,
    region: String,
    feature: String,
    token_path: PathBuf,
}

pub async fn begin(
    account: String,
    password: Zeroizing<String>,
    region: &str,
    token_path: PathBuf,
    timeout_seconds: u64,
) -> Result<EnrollmentState, BridgeError> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_seconds))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let feature = hex::encode(Md5::digest(Uuid::new_v4().as_bytes()));
    match login(&client, &account, &password, region, &feature, None).await? {
        LoginOutcome::Token(token) => {
            persist(&client, token, &token_path).await?;
            Ok(EnrollmentState::Complete)
        }
        LoginOutcome::MfaRequired(actual_region) => {
            send_mfa(&client, &account, &actual_region, &feature).await?;
            Ok(EnrollmentState::Mfa(PendingEnrollment {
                client,
                account,
                password,
                region: actual_region,
                feature,
                token_path,
            }))
        }
    }
}

impl PendingEnrollment {
    pub async fn complete(self, code: &str) -> Result<(), BridgeError> {
        validate_code(code)?;
        let token = match login(
            &self.client,
            &self.account,
            &self.password,
            &self.region,
            &self.feature,
            Some(code),
        )
        .await?
        {
            LoginOutcome::Token(token) => token,
            LoginOutcome::MfaRequired(_) => return Err(BridgeError::InvalidOtp),
        };
        persist(&self.client, token, &self.token_path).await
    }
}

pub async fn enroll(
    account: &str,
    password: &str,
    region: &str,
    token_path: &Path,
    timeout_seconds: u64,
    mfa_reader: impl FnOnce() -> Result<String, BridgeError>,
) -> Result<(), BridgeError> {
    match begin(
        account.to_owned(),
        Zeroizing::new(password.to_owned()),
        region,
        token_path.to_owned(),
        timeout_seconds,
    )
    .await?
    {
        EnrollmentState::Complete => Ok(()),
        EnrollmentState::Mfa(pending) => pending.complete(&mfa_reader()?).await,
    }
}

async fn persist(
    client: &Client,
    mut token: EzvizToken,
    token_path: &Path,
) -> Result<(), BridgeError> {
    token.service_urls = Some(service_urls(client, &token).await?);
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
        return Err(login_error(mfa));
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
        _ => Err(login_error(mfa)),
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
        return Err(BridgeError::InvalidCredentials);
    }
    let value: Value = response.json().await?;
    if value
        .get("meta")
        .and_then(|meta| meta.get("code"))
        .and_then(Value::as_i64)
        != Some(200)
    {
        return Err(BridgeError::InvalidCredentials);
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

fn validate_code(code: &str) -> Result<(), BridgeError> {
    if code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(())
    } else {
        Err(BridgeError::InvalidOtp)
    }
}

const fn login_error(mfa: Option<&str>) -> BridgeError {
    if mfa.is_some() {
        BridgeError::InvalidOtp
    } else {
        BridgeError::InvalidCredentials
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_only_six_digit_mfa_codes() {
        assert!(validate_code("123456").is_ok());
        assert!(validate_code("12345").is_err());
        assert!(validate_code("12345a").is_err());
    }
}
