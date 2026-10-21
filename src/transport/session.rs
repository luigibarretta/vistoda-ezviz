//! The EZVIZ session: one in-memory token, its private file and a
//! single-flight refresh.
//!
//! EZVIZ rotates the refresh token on every refresh, so two concurrent
//! refreshes with the same token would race and one would be rejected. All
//! refreshes and token-file writes are serialized by one lock; a caller that
//! saw an older generation reuses the refresh that happened meanwhile. The
//! refresh itself runs in a detached task that owns the lock, so a cancelled
//! HTTP request cannot stop it between EZVIZ rotating the token and the new
//! token reaching the file.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use reqwest::Client;
use tokio::{
    sync::{Mutex, RwLock},
    time::Instant,
};

use crate::{
    error::BridgeError,
    session_health::{SessionHealth, SessionState},
    storage::{atomic_write_json, ensure_private_regular},
    transport::{
        EzvizToken,
        authed::{http_error, read_json},
        http::{mobile_headers, required_string},
        session_outcome::refreshed_session,
    },
};

/// A transient refresh failure is not retried sooner than this.
const TRANSIENT_COOLDOWN: Duration = Duration::from_secs(10);
/// A refresh token EZVIZ refused is not offered again sooner than this.
const REJECTED_COOLDOWN: Duration = Duration::from_secs(30 * 60);

struct Failure {
    generation: u64,
    at: Instant,
    rejected: bool,
}

pub(super) struct SessionStore {
    token: RwLock<EzvizToken>,
    /// Incremented whenever the session identifiers change.
    generation: AtomicU64,
    path: PathBuf,
    /// Serializes refreshes and every token-file write.
    lock: Arc<Mutex<()>>,
    failure: std::sync::Mutex<Option<Failure>>,
    pub(super) health: SessionHealth,
}

impl SessionStore {
    pub(super) fn load(path: PathBuf, health: SessionHealth) -> Result<Self, BridgeError> {
        let token = read_token(&path)?;
        Ok(Self {
            token: RwLock::new(token),
            generation: AtomicU64::new(0),
            path,
            lock: Arc::new(Mutex::new(())),
            failure: std::sync::Mutex::new(None),
            health,
        })
    }

    /// The token and its generation, read consistently.
    pub(super) async fn current(&self) -> (EzvizToken, u64) {
        let token = self.token.read().await;
        (token.clone(), self.generation.load(Ordering::SeqCst))
    }

    pub(super) async fn token(&self) -> EzvizToken {
        self.token.read().await.clone()
    }

    /// Refreshes once for generation `seen`; concurrent callers share it.
    pub(super) async fn refresh(
        self: &Arc<Self>,
        client: &Client,
        scheme: &'static str,
        seen: u64,
    ) -> Result<(), BridgeError> {
        let guard = Arc::clone(&self.lock).lock_owned().await;
        if self.generation.load(Ordering::SeqCst) != seen {
            return Ok(());
        }
        if let Some(error) = self.recent_failure(seen) {
            return Err(error);
        }
        let store = Arc::clone(self);
        let client = client.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let result = store.rotate(&client, scheme).await;
            store.record(seen, &result);
            result
        })
        .await?
    }

    /// Stores a field that does not change the session identifiers.
    pub(super) async fn update(
        &self,
        change: impl FnOnce(&mut EzvizToken),
    ) -> Result<(), BridgeError> {
        let _guard = self.lock.lock().await;
        let mut token = self.token().await;
        change(&mut token);
        self.publish(token, false).await
    }

    /// Replaces the session with a freshly enrolled token file.
    pub(super) async fn adopt(&self, staged: &Path) -> Result<(), BridgeError> {
        let _guard = self.lock.lock().await;
        let token = read_token(staged)?;
        std::fs::rename(staged, &self.path)?;
        let mut slot = self.token.write().await;
        *slot = token;
        self.generation.fetch_add(1, Ordering::SeqCst);
        drop(slot);
        *self.lock_failure() = None;
        self.health.set(SessionState::Ok);
        Ok(())
    }

    async fn rotate(&self, client: &Client, scheme: &str) -> Result<(), BridgeError> {
        let current = self.token().await;
        let feature = current
            .feature_code
            .clone()
            .ok_or_else(|| BridgeError::Configuration("EZVIZ token has no feature code".into()))?;
        let mut headers = mobile_headers(&feature);
        if let Ok(value) = current.session_id.parse() {
            headers.insert("sessionid", value);
        }
        let response = client
            .put(format!(
                "{scheme}://{}/v3/apigateway/login",
                current.api_url
            ))
            .headers(headers)
            .form(&[
                ("refreshSessionId", current.rf_session_id.as_str()),
                ("featureCode", feature.as_str()),
            ])
            .send()
            .await
            .map_err(http_error)?;
        let status = response.status();
        let value = read_json(response, false).await?;
        let session = refreshed_session(status, &value)?;
        let mut updated = current;
        updated.session_id = required_string(session, "sessionId")?;
        updated.rf_session_id = required_string(session, "refreshSessionId")?;
        self.publish(updated, true).await
    }

    /// File first, then memory; callers hold `lock`, which orders the writes.
    /// A failed file write still updates memory: EZVIZ has already rotated the
    /// refresh token, so the old one in memory would be useless. The failure is
    /// logged and counted; the next successful write repairs the file.
    async fn publish(&self, token: EzvizToken, rotated: bool) -> Result<(), BridgeError> {
        if let Err(error) = atomic_write_json(&self.path, &token) {
            tracing::error!(
                error_type = error.diagnostic_code(),
                "EZVIZ token file could not be written; keeping the session in memory"
            );
            self.health.persist_failed();
        }
        let mut slot = self.token.write().await;
        *slot = token;
        if rotated {
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }

    fn record(&self, generation: u64, result: &Result<(), BridgeError>) {
        let mut failure = self.lock_failure();
        match result {
            Ok(()) => {
                *failure = None;
                self.health.refresh_succeeded();
            }
            Err(error) => {
                let rejected = matches!(error, BridgeError::ReauthRequired);
                tracing::warn!(
                    error_type = error.diagnostic_code(),
                    "EZVIZ session refresh failed"
                );
                *failure = Some(Failure {
                    generation,
                    at: Instant::now(),
                    rejected,
                });
                self.health.refresh_failed(rejected);
            }
        }
    }

    fn recent_failure(&self, generation: u64) -> Option<BridgeError> {
        let failure = self.lock_failure();
        let failure = failure
            .as_ref()
            .filter(|item| item.generation == generation)?;
        if failure.rejected {
            (failure.at.elapsed() < REJECTED_COOLDOWN).then_some(BridgeError::ReauthRequired)
        } else {
            (failure.at.elapsed() < TRANSIENT_COOLDOWN)
                .then(|| BridgeError::Upstream("EZVIZ session refresh failed moments ago".into()))
        }
    }

    fn lock_failure(&self) -> std::sync::MutexGuard<'_, Option<Failure>> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn read_token(path: &Path) -> Result<EzvizToken, BridgeError> {
    let raw = ensure_private_regular(path, 20)?;
    let token: EzvizToken = serde_json::from_str(&raw)
        .map_err(|_| BridgeError::Configuration("EZVIZ token file is invalid".into()))?;
    token.validate()?;
    Ok(token)
}
