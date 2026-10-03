use std::path::{Path, PathBuf};

use md5::{Digest as _, Md5};
use reqwest::Client;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{error::BridgeError, storage::atomic_write_json, transport::EzvizToken};

#[path = "enroll_api.rs"]
mod api;
use api::{login, send_mfa, service_urls};

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

fn validate_code(code: &str) -> Result<(), BridgeError> {
    if code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(())
    } else {
        Err(BridgeError::InvalidOtp)
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
