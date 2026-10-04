//! Rate guard for the cloud verification-code lookup.
//!
//! EZVIZ risk control may answer `/api/device/query/encryptkey` from a
//! third-party terminal by emailing or texting the account owner a login
//! code. Vistoda therefore allows at most one request per camera every 24
//! hours, across every caller and across restarts: the attempt time is
//! persisted before the request is sent, and a successful code is kept only
//! in zeroizing memory for the process lifetime. Serials are stored as
//! truncated SHA-256 identifiers; codes are never written or logged.

use std::{collections::BTreeMap, future::Future, path::PathBuf, sync::Mutex};

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{error::BridgeError, storage::atomic_write_json};

/// Minimum spacing between two cloud lookups for one camera.
pub const CLOUD_KEY_INTERVAL_SECONDS: i64 = 24 * 3600;

#[derive(Default)]
struct GuardState {
    attempts: BTreeMap<String, i64>,
    codes: BTreeMap<String, Zeroizing<String>>,
}

/// Process-wide cache and persisted 24-hour latch for cloud codes.
pub struct CloudKeyGuard {
    path: Option<PathBuf>,
    state: Mutex<GuardState>,
}

fn key_id(serial: &str) -> String {
    hex::encode(Sha256::digest(serial.as_bytes()))[..16].to_owned()
}

impl CloudKeyGuard {
    /// Loads persisted attempt times; an unreadable file starts empty.
    #[must_use]
    pub fn load(path: PathBuf) -> Self {
        let attempts = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            state: Mutex::new(GuardState {
                attempts,
                codes: BTreeMap::new(),
            }),
        }
    }

    /// Guard without persistence, for tests and tools.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            path: None,
            state: Mutex::new(GuardState::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, GuardState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Code fetched earlier in this process, without any network access.
    #[must_use]
    pub fn cached(&self, serial: &str) -> Option<Zeroizing<String>> {
        self.state().codes.get(&key_id(serial)).cloned()
    }

    /// Epoch second until which no new lookup is allowed, if latched.
    #[must_use]
    pub fn blocked_until(&self, serial: &str, now: i64) -> Option<i64> {
        let until = self.state().attempts.get(&key_id(serial))? + CLOUD_KEY_INTERVAL_SECONDS;
        (until > now).then_some(until)
    }

    /// Records and persists an attempt before any request is sent.
    fn begin(&self, serial: &str, now: i64) -> Result<(), BridgeError> {
        let id = key_id(serial);
        // Check and record under one lock so concurrent callers cannot both pass.
        let mut state = self.state();
        if state
            .attempts
            .get(&id)
            .is_some_and(|at| at + CLOUD_KEY_INTERVAL_SECONDS > now)
        {
            return Err(BridgeError::Upstream(
                "cloud verification code lookup is paused for 24 hours".into(),
            ));
        }
        state.attempts.insert(id, now);
        state
            .attempts
            .retain(|_, at| now - *at < CLOUD_KEY_INTERVAL_SECONDS);
        // Persist before the request so a crash cannot cause a repeat.
        self.path
            .as_ref()
            .map_or(Ok(()), |path| atomic_write_json(path, &state.attempts))
    }

    /// Returns the cached code or performs at most one guarded lookup.
    pub async fn fetch<F, Fut>(
        &self,
        serial: &str,
        now: i64,
        request: F,
    ) -> Result<Zeroizing<String>, BridgeError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<String, BridgeError>> + Send,
    {
        if let Some(code) = self.cached(serial) {
            return Ok(code);
        }
        self.begin(serial, now)?;
        match request().await {
            Ok(code) => {
                let code = Zeroizing::new(code);
                self.state().codes.insert(key_id(serial), code.clone());
                Ok(code)
            }
            Err(error) => {
                tracing::warn!(
                    error_type = error.diagnostic_code(),
                    "cloud verification code lookup failed; paused for 24 hours"
                );
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const NOW: i64 = 1_791_072_000;

    async fn attempt(guard: &CloudKeyGuard, now: i64, calls: &AtomicUsize, ok: bool) -> bool {
        guard
            .fetch("SERIAL", now, || async {
                calls.fetch_add(1, Ordering::SeqCst);
                if ok {
                    Ok("ABCDEF".to_owned())
                } else {
                    Err(BridgeError::Upstream("risk control".into()))
                }
            })
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn failure_latches_for_a_day_and_survives_restart() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("cloud-key-attempts.json");
        let calls = AtomicUsize::new(0);
        let guard = CloudKeyGuard::load(path.clone());
        assert!(!attempt(&guard, NOW, &calls, false).await);
        assert!(!attempt(&guard, NOW + 60, &calls, true).await);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(!raw.contains("SERIAL"));

        let restarted = CloudKeyGuard::load(path.clone());
        assert_eq!(
            restarted.blocked_until("SERIAL", NOW + 3600),
            Some(NOW + CLOUD_KEY_INTERVAL_SECONDS)
        );
        assert!(!attempt(&restarted, NOW + 3600, &calls, true).await);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(attempt(&restarted, NOW + CLOUD_KEY_INTERVAL_SECONDS, &calls, true).await);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn success_is_cached_in_memory_and_still_spaced_after_restart() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("cloud-key-attempts.json");
        let calls = AtomicUsize::new(0);
        let guard = CloudKeyGuard::load(path.clone());
        assert!(attempt(&guard, NOW, &calls, true).await);
        assert!(attempt(&guard, NOW + 7200, &calls, true).await);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            guard.cached("SERIAL").as_deref().map(String::as_str),
            Some("ABCDEF")
        );
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap_or_default()
                .contains("ABCDEF")
        );

        let restarted = CloudKeyGuard::load(path);
        assert!(restarted.cached("SERIAL").is_none());
        assert!(!attempt(&restarted, NOW + 7200, &calls, true).await);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
