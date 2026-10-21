//! Process-wide state of the EZVIZ session, shared by the transport, the API
//! middleware, `/healthz` and `/metrics`. Never holds a secret.

use std::{
    fmt::Write as _,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
};

/// What `/healthz` reports as `ezviz_session`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// The last authenticated EZVIZ call or refresh succeeded.
    Ok,
    /// Not yet confirmed, or the last refresh failed transiently.
    Degraded,
    /// EZVIZ rejected the refresh: the account must be enrolled again.
    ReauthRequired,
}

impl SessionState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Degraded => "degraded",
            Self::ReauthRequired => "reauth_required",
        }
    }

    const fn code(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Degraded => 1,
            Self::ReauthRequired => 2,
        }
    }

    const fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Ok,
            2 => Self::ReauthRequired,
            _ => Self::Degraded,
        }
    }
}

#[derive(Default)]
struct Inner {
    state: AtomicU8,
    refresh_ok: AtomicU64,
    refresh_failed: AtomicU64,
    session_rejected: AtomicU64,
    persist_failed: AtomicU64,
}

/// Cheap clonable handle; every method is lock-free.
#[derive(Clone)]
pub struct SessionHealth {
    inner: Arc<Inner>,
}

impl Default for SessionHealth {
    /// Starts degraded until the first refresh or authenticated call succeeds.
    fn default() -> Self {
        let inner = Inner::default();
        inner
            .state
            .store(SessionState::Degraded.code(), Ordering::SeqCst);
        Self {
            inner: Arc::new(inner),
        }
    }
}

impl SessionHealth {
    /// A session that is already known to work (tests and fakes).
    #[must_use]
    pub fn healthy() -> Self {
        let health = Self::default();
        health.set(SessionState::Ok);
        health
    }

    #[must_use]
    pub fn state(&self) -> SessionState {
        SessionState::from_code(self.inner.state.load(Ordering::SeqCst))
    }

    #[must_use]
    pub fn reauth_required(&self) -> bool {
        self.state() == SessionState::ReauthRequired
    }

    pub fn set(&self, state: SessionState) {
        self.inner.state.store(state.code(), Ordering::SeqCst);
    }

    /// An authenticated call succeeded with the current session.
    pub fn confirmed(&self) {
        self.set(SessionState::Ok);
    }

    pub fn refresh_succeeded(&self) {
        self.inner.refresh_ok.fetch_add(1, Ordering::Relaxed);
        self.set(SessionState::Ok);
    }

    /// `rejected`: EZVIZ refused the refresh token itself.
    pub fn refresh_failed(&self, rejected: bool) {
        self.inner.refresh_failed.fetch_add(1, Ordering::Relaxed);
        if rejected {
            self.session_rejected();
        } else if !self.reauth_required() {
            self.set(SessionState::Degraded);
        }
    }

    /// EZVIZ refused the session (refresh rejected, or still expired after it).
    pub fn session_rejected(&self) {
        self.inner.session_rejected.fetch_add(1, Ordering::Relaxed);
        self.set(SessionState::ReauthRequired);
    }

    /// The token file could not be written (the session stays in memory).
    pub fn persist_failed(&self) {
        self.inner.persist_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Prometheus lines, prefixed like every other bridge metric.
    #[must_use]
    pub fn render(&self) -> String {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        let mut output = String::new();
        let _ok = writeln!(
            output,
            "ezviz_bridge_session_refreshes_total{{result=\"ok\"}} {}",
            load(&self.inner.refresh_ok)
        );
        let _failed = writeln!(
            output,
            "ezviz_bridge_session_refreshes_total{{result=\"failed\"}} {}",
            load(&self.inner.refresh_failed)
        );
        let _rejected = writeln!(
            output,
            "ezviz_bridge_session_auth_failures_total {}",
            load(&self.inner.session_rejected)
        );
        let _persist = writeln!(
            output,
            "ezviz_bridge_session_persist_failures_total {}",
            load(&self.inner.persist_failed)
        );
        let _state = writeln!(
            output,
            "ezviz_bridge_session_reauth_required {}",
            u8::from(self.reauth_required())
        );
        output
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionHealth, SessionState};

    #[test]
    fn rejection_is_sticky_until_a_success() {
        let health = SessionHealth::default();
        assert_eq!(health.state(), SessionState::Degraded);
        health.refresh_failed(true);
        health.refresh_failed(false);
        assert!(health.reauth_required());
        let text = health.render();
        assert!(text.contains("session_refreshes_total{result=\"failed\"} 2"));
        assert!(text.contains("session_auth_failures_total 1"));
        assert!(text.contains("session_reauth_required 1"));
        health.refresh_succeeded();
        assert_eq!(health.state(), SessionState::Ok);
    }
}
