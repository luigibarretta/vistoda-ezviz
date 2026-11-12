//! API authentication of the provider bridges.
//!
//! Every route except the public ones (by default only `/healthz`) needs
//! `Authorization: Bearer <token>`, compared in constant time. Routes chosen
//! with [`AuthPolicy::basic_paths`] (Home Assistant's Generic Camera snapshot
//! and stream URLs, which cannot send a bearer) also accept HTTP Basic with
//! the fixed user [`BASIC_USERNAME`] and the token as password.
//!
//! [`require_auth`] is meant as a layer on the whole router
//! (`axum::middleware::from_fn_with_state(Arc::new(policy), require_auth)`),
//! so a new handler cannot forget to authenticate.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use auth_modules::secrets::constant_time_eq;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::private_file::{self, FileError, FileOwner, ReadPolicy};

pub const MIN_TOKEN_BYTES: usize = 32;
/// Basic user name Home Assistant's Generic Camera is configured with.
pub const BASIC_USERNAME: &str = "homeassistant";
const MAX_TOKEN_BYTES: u64 = 4096;
const DEFAULT_TOKENS: [&str; 3] = ["change-me", "changeme", "password"];

#[derive(Debug, Error)]
pub enum TokenError {
    #[error("API token is missing, shorter than 32 bytes, or a default")]
    Weak,
    #[error("API token file is unusable: {0}")]
    File(#[from] FileError),
}

/// The bridge API token, wiped on drop.
#[derive(Clone)]
pub struct ApiToken(Zeroizing<Vec<u8>>);

impl ApiToken {
    /// Rejects tokens shorter than [`MIN_TOKEN_BYTES`] and well-known
    /// placeholders. The token is used as given (callers trim file content).
    pub fn new(token: impl Into<Vec<u8>>) -> Result<Self, TokenError> {
        let token = Zeroizing::new(token.into());
        let default = DEFAULT_TOKENS
            .iter()
            .any(|value| token.eq_ignore_ascii_case(value.as_bytes()));
        if token.len() < MIN_TOKEN_BYTES || default {
            return Err(TokenError::Weak);
        }
        Ok(Self(token))
    }

    /// Reads an operator-provisioned token file: private mode and no symlink
    /// are required, any owner is accepted, surrounding whitespace is trimmed.
    pub fn from_file(path: &Path) -> Result<Self, TokenError> {
        let policy = ReadPolicy::new(MAX_TOKEN_BYTES)
            .owner(FileOwner::Any)
            .trimmed()
            .min_bytes(MIN_TOKEN_BYTES);
        let token = private_file::read(path, policy)?;
        Self::new(token.to_vec())
    }

    #[must_use]
    pub fn expose_secret(&self) -> &[u8] {
        &self.0
    }

    /// Constant-time comparison with `candidate`.
    #[must_use]
    pub fn matches(&self, candidate: &[u8]) -> bool {
        constant_time_eq(candidate, &self.0)
    }

    /// Bearer always; Basic (`homeassistant` + token) only when `allow_basic`.
    #[must_use]
    pub fn accepts(&self, headers: &HeaderMap, allow_basic: bool) -> bool {
        let Some(value) = authorization(headers) else {
            return false;
        };
        if let Some(candidate) = value.strip_prefix("Bearer ") {
            return self.matches(candidate.as_bytes());
        }
        let Some(encoded) = value.strip_prefix("Basic ").filter(|_| allow_basic) else {
            return false;
        };
        let Ok(decoded) = STANDARD.decode(encoded).map(Zeroizing::new) else {
            return false;
        };
        let Some(separator) = decoded.iter().position(|byte| *byte == b':') else {
            return false;
        };
        let (user, password) = decoded.split_at(separator);
        // Both halves are compared so the outcome does not reveal which failed.
        let user_ok = constant_time_eq(user, BASIC_USERNAME.as_bytes());
        let password_ok = self.matches(&password[1..]);
        user_ok && password_ok
    }
}

impl fmt::Debug for ApiToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApiToken([REDACTED])")
    }
}

/// Constant-time check of `Authorization: Bearer <expected>`.
#[must_use]
pub fn bearer_matches(headers: &HeaderMap, expected: &[u8]) -> bool {
    authorization(headers)
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|candidate| constant_time_eq(candidate.as_bytes(), expected))
}

fn authorization(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
}

/// Which routes are public or accept Basic, and what a rejection looks like.
#[derive(Clone)]
pub struct AuthPolicy {
    token: ApiToken,
    public: fn(&str) -> bool,
    basic: fn(&str) -> bool,
    reject: fn() -> Response,
}

impl AuthPolicy {
    /// Bearer on every route except `/healthz`; 401 with a bearer challenge.
    #[must_use]
    pub fn new(token: ApiToken) -> Self {
        Self {
            token,
            public: health_only,
            basic: |_| false,
            reject: || unauthorized("Bearer"),
        }
    }

    #[must_use]
    pub fn public_paths(mut self, public: fn(&str) -> bool) -> Self {
        self.public = public;
        self
    }

    #[must_use]
    pub fn basic_paths(mut self, basic: fn(&str) -> bool) -> Self {
        self.basic = basic;
        self
    }

    /// The provider's own 401 response (for example its error type).
    #[must_use]
    pub fn rejection(mut self, reject: fn() -> Response) -> Self {
        self.reject = reject;
        self
    }

    #[must_use]
    pub const fn token(&self) -> &ApiToken {
        &self.token
    }

    /// Whether a request for `path` with `headers` may proceed.
    #[must_use]
    pub fn authorizes(&self, path: &str, headers: &HeaderMap) -> bool {
        (self.public)(path) || self.token.accepts(headers, (self.basic)(path))
    }

    /// The configured rejection response.
    #[must_use]
    pub fn reject(&self) -> Response {
        (self.reject)()
    }
}

impl fmt::Debug for AuthPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("AuthPolicy").finish_non_exhaustive()
    }
}

/// The default public route set: `/healthz` only.
#[must_use]
pub fn health_only(path: &str) -> bool {
    path == "/healthz"
}

/// Router-wide authentication middleware.
pub async fn require_auth(
    State(policy): State<Arc<AuthPolicy>>,
    request: Request,
    next: Next,
) -> Response {
    if policy.authorizes(request.uri().path(), request.headers()) {
        next.run(request).await
    } else {
        policy.reject()
    }
}

/// `401 {"error":"unauthorized"}` with the given `WWW-Authenticate` value.
#[must_use]
pub fn unauthorized(challenge: &'static str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::WWW_AUTHENTICATE, challenge),
        ],
        r#"{"error":"unauthorized"}"#,
    )
        .into_response()
}
