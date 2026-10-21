//! EZVIZ cloud enrollment calls: password/MFA login, SMS challenge and service URL discovery.

use md5::{Digest as _, Md5};
use reqwest::{Client, StatusCode};
use serde_json::Value;

use super::LoginOutcome;
use crate::{
    error::BridgeError,
    transport::{
        EzvizToken,
        authed::{http_error, read_json},
        http::mobile_headers,
    },
};

/// Region redirects (`meta.code` 1100) followed per login; EZVIZ sends one.
const MAX_REGION_REDIRECTS: usize = 2;
/// Default API domain; region codes expand to `apii{region}` under it.
const EZVIZ_DOMAIN: &str = ".ezvizlife.com";
/// The only domains this bridge sends EZVIZ credentials or sessions to. The
/// Russian region rewrites `ezvizlife` to `ezvizru` (pyEzvizApi c713642f,
/// `ezviz_lan_ezviz_access_replacement_domain`).
const EZVIZ_DOMAINS: [&str; 2] = [EZVIZ_DOMAIN, ".ezvizru.com"];

pub(super) async fn login(
    client: &Client,
    account: &str,
    password: &str,
    region: &str,
    feature: &str,
    mfa: Option<&str>,
) -> Result<LoginOutcome, BridgeError> {
    let mut region = normalize_region(region)?;
    for _ in 0..=MAX_REGION_REDIRECTS {
        match login_once(client, account, password, &region, feature, mfa).await? {
            Attempt::Done(outcome) => return Ok(outcome),
            Attempt::Redirect(next) => region = normalize_region(&next)?,
        }
    }
    Err(BridgeError::Upstream(
        "EZVIZ login redirected too many times".into(),
    ))
}

enum Attempt {
    Done(LoginOutcome),
    Redirect(String),
}

async fn login_once(
    client: &Client,
    account: &str,
    password: &str,
    region: &str,
    feature: &str,
    mfa: Option<&str>,
) -> Result<Attempt, BridgeError> {
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
        .await
        .map_err(http_error)?;
    if !response.status().is_success() {
        return Err(login_error(mfa));
    }
    let value = read_json(response, true).await?;
    match value
        .get("meta")
        .and_then(|meta| meta.get("code"))
        .and_then(Value::as_i64)
    {
        Some(200) => Ok(Attempt::Done(LoginOutcome::Token(EzvizToken {
            session_id: required(&value, &["loginSession", "sessionId"])?,
            rf_session_id: required(&value, &["loginSession", "rfSessionId"])?,
            username: Some(required(&value, &["loginUser", "username"])?),
            api_url: normalize_region(&required(&value, &["loginArea", "apiDomain"])?)?,
            feature_code: Some(feature.to_owned()),
            service_urls: None,
            extra: std::collections::BTreeMap::new(),
        }))),
        Some(6002) => Ok(Attempt::Done(LoginOutcome::MfaRequired(region.to_owned()))),
        Some(1100) => Ok(Attempt::Redirect(required(
            &value,
            &["loginArea", "apiDomain"],
        )?)),
        _ => Err(login_error(mfa)),
    }
}

pub(super) async fn send_mfa(
    client: &Client,
    account: &str,
    region: &str,
    feature: &str,
) -> Result<(), BridgeError> {
    let response = client
        .post(format!(
            "https://{}/v3/sms/nologin/checkcode",
            normalize_region(region)?
        ))
        .headers(mobile_headers(feature))
        .form(&[("from", account), ("bizType", "TERMINAL_BIND")])
        .send()
        .await
        .map_err(http_error)?;
    if response.status() != StatusCode::OK {
        return Err(BridgeError::InvalidCredentials);
    }
    let value = read_json(response, true).await?;
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

pub(super) async fn service_urls(
    client: &Client,
    token: &EzvizToken,
) -> Result<Value, BridgeError> {
    let mut headers = mobile_headers(token.feature_code.as_deref().unwrap_or(""));
    headers.insert(
        "sessionid",
        token
            .session_id
            .parse()
            .map_err(|_| BridgeError::Upstream("EZVIZ session id is not a header".into()))?,
    );
    let response = client
        .get(format!(
            "https://{}/v3/configurations/system/info",
            normalize_region(&token.api_url)?
        ))
        .headers(headers)
        .send()
        .await
        .map_err(http_error)?;
    let value = read_json(response, true).await?;
    value
        .get("systemConfigInfo")
        .cloned()
        .ok_or_else(|| BridgeError::Upstream("system info omitted service URLs".into()))
}

const fn login_error(mfa: Option<&str>) -> BridgeError {
    if mfa.is_some() {
        BridgeError::InvalidOtp
    } else {
        BridgeError::InvalidCredentials
    }
}

fn required(value: &Value, path: &[&str]) -> Result<String, BridgeError> {
    let missing = || BridgeError::Upstream("EZVIZ login response is incomplete".into());
    path.iter()
        .try_fold(value, |current, key| current.get(*key).ok_or_else(missing))?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(missing)
}

/// `eu` becomes `apiieu.ezvizlife.com`; a full domain must be a plain
/// `*.ezvizlife.com` or `*.ezvizru.com` host, so credentials never leave EZVIZ.
fn normalize_region(value: &str) -> Result<String, BridgeError> {
    let value = value.trim();
    let host = if value.contains('.') {
        value
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_ascii_lowercase()
    } else {
        format!("apii{}{EZVIZ_DOMAIN}", value.to_ascii_lowercase())
    };
    let plain = host
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-.".contains(&byte));
    let label = EZVIZ_DOMAINS
        .iter()
        .find_map(|domain| host.strip_suffix(domain))
        .unwrap_or_default();
    if plain && !label.is_empty() && !label.starts_with('.') && !label.contains("..") {
        Ok(host)
    } else {
        Err(BridgeError::Upstream(
            "EZVIZ API domain is not an EZVIZ host".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_region;

    #[test]
    fn api_domains_are_limited_to_ezvizlife_hosts() {
        for (input, host) in [
            ("eu", "apiieu.ezvizlife.com"),
            ("apiius.ezvizlife.com", "apiius.ezvizlife.com"),
            ("https://apiisa.ezvizlife.com/", "apiisa.ezvizlife.com"),
            ("apiirus.ezvizru.com", "apiirus.ezvizru.com"),
        ] {
            assert_eq!(normalize_region(input).ok().as_deref(), Some(host));
        }
        for input in [
            "evil.example",
            "ezvizlife.com",
            "ezvizru.com",
            "evil.ezvizru.com.example",
            ".ezvizlife.com",
            "apiieu.ezvizlife.com.evil.example",
            "evil.example/x.ezvizlife.com",
            "user@apiieu.ezvizlife.com",
            "apiieu.ezvizlife.com:8443",
            "http://apiieu.ezvizlife.com",
            "e/u",
        ] {
            assert!(normalize_region(input).is_err(), "{input}");
        }
    }
}
