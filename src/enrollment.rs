use std::{path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};
use tokio::{sync::Mutex, time::Instant};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    error::BridgeError,
    transport::{EnrollmentState, PendingEnrollment, begin},
};

const ENROLLMENT_TTL: Duration = Duration::from_secs(120);
const START_COOLDOWN: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentStart {
    account: String,
    password: Zeroizing<String>,
    #[serde(default = "default_region")]
    api_region: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyEnrollment {
    code: Zeroizing<String>,
}

#[derive(Serialize)]
pub struct EnrollmentStarted {
    enrollment_id: String,
    next_step: &'static str,
    expires_in: u64,
}

#[derive(Serialize)]
pub struct EnrollmentVerified {
    status: &'static str,
}

struct Pending {
    id: Uuid,
    provider: PendingEnrollment,
    expires_at: Instant,
}

#[derive(Default)]
struct State {
    pending: Option<Pending>,
    last_start: Option<Instant>,
    in_flight: bool,
}

pub struct EnrollmentManager {
    token_path: PathBuf,
    timeout_seconds: u64,
    state: Mutex<State>,
    ready: tokio::sync::watch::Sender<bool>,
}

impl EnrollmentManager {
    pub fn new(
        token_path: PathBuf,
        timeout_seconds: u64,
        ready: tokio::sync::watch::Sender<bool>,
    ) -> Self {
        Self {
            token_path,
            timeout_seconds,
            state: Mutex::new(State::default()),
            ready,
        }
    }

    pub async fn start(&self, input: EnrollmentStart) -> Result<EnrollmentStarted, BridgeError> {
        validate_start(&input)?;
        let mut state = self.state.lock().await;
        expire(&mut state);
        if state.pending.is_some() || state.in_flight {
            return Err(BridgeError::EnrollmentBusy);
        }
        if state
            .last_start
            .is_some_and(|last| last + START_COOLDOWN > Instant::now())
        {
            return Err(BridgeError::EnrollmentBusy);
        }
        state.last_start = Some(Instant::now());
        state.in_flight = true;
        drop(state);

        let id = Uuid::new_v4();
        let outcome = begin(
            input.account,
            input.password,
            &input.api_region,
            self.token_path.clone(),
            self.timeout_seconds,
        )
        .await;

        let mut state = self.state.lock().await;
        state.in_flight = false;
        let response = match outcome {
            Ok(EnrollmentState::Complete) => {
                let _unused = self.ready.send(true);
                Ok(started(id, "complete", 0))
            }
            Ok(EnrollmentState::Mfa(provider)) => {
                state.pending = Some(Pending {
                    id,
                    provider,
                    expires_at: Instant::now() + ENROLLMENT_TTL,
                });
                Ok(started(id, "otp", ENROLLMENT_TTL.as_secs()))
            }
            Err(error) => Err(error),
        };
        drop(state);
        response
    }

    pub async fn verify(
        &self,
        enrollment_id: &str,
        input: VerifyEnrollment,
    ) -> Result<EnrollmentVerified, BridgeError> {
        let id = Uuid::parse_str(enrollment_id).map_err(|_| BridgeError::EnrollmentExpired)?;
        let pending = {
            let mut state = self.state.lock().await;
            expire(&mut state);
            match state.pending.take() {
                Some(pending) if pending.id == id => pending,
                Some(pending) => {
                    state.pending = Some(pending);
                    return Err(BridgeError::EnrollmentExpired);
                }
                None => return Err(BridgeError::EnrollmentExpired),
            }
        };
        pending.provider.complete(&input.code).await?;
        let _unused = self.ready.send(true);
        Ok(EnrollmentVerified { status: "complete" })
    }

    pub async fn cancel(&self, enrollment_id: &str) {
        let Ok(id) = Uuid::parse_str(enrollment_id) else {
            return;
        };
        let mut state = self.state.lock().await;
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            state.pending = None;
        }
    }
}

fn started(id: Uuid, next_step: &'static str, expires_in: u64) -> EnrollmentStarted {
    EnrollmentStarted {
        enrollment_id: id.to_string(),
        next_step,
        expires_in,
    }
}

fn expire(state: &mut State) {
    if state
        .pending
        .as_ref()
        .is_some_and(|pending| pending.expires_at <= Instant::now())
    {
        state.pending = None;
    }
}

fn validate_start(input: &EnrollmentStart) -> Result<(), BridgeError> {
    if input.account.trim().is_empty()
        || input.password.len() < 4
        || input.api_region.trim().is_empty()
        || input.api_region.len() > 128
    {
        return Err(BridgeError::InvalidCredentials);
    }
    Ok(())
}

fn default_region() -> String {
    "eu".into()
}
