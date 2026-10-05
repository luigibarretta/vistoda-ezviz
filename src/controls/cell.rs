//! Single-flight cache cell with separate success and failure lifetimes.

use std::{future::Future, time::Duration};

use tokio::{sync::Mutex, time::Instant};

use crate::error::BridgeError;

struct Entry<T> {
    expires: Instant,
    /// `None` records a recent failure.
    value: Option<T>,
}

/// Concurrent readers wait for one fetch; the lock is held across that fetch
/// on purpose so a burst of requests produces a single vendor call.
pub(super) struct TtlCell<T> {
    entry: Mutex<Option<Entry<T>>>,
    ttl: Duration,
    failure_ttl: Duration,
}

/// Outcome of a cache read.
pub(super) enum Cached<T> {
    Value(T),
    /// A fetch failed now or within the failure lifetime.
    Failed,
}

impl<T: Clone> TtlCell<T> {
    pub(super) const fn new(ttl: Duration, failure_ttl: Duration) -> Self {
        Self {
            entry: Mutex::const_new(None),
            ttl,
            failure_ttl,
        }
    }

    /// Cached value, or one fetch when expired or when `force` is set.
    pub(super) async fn get<F, Fut>(&self, force: bool, fetch: F) -> Cached<T>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<T, BridgeError>> + Send,
    {
        let mut entry = self.entry.lock().await;
        let fresh = entry
            .as_ref()
            .filter(|entry| !force && entry.expires > Instant::now());
        if let Some(cached) = fresh {
            return cached.value.clone().map_or(Cached::Failed, Cached::Value);
        }
        let result = fetch().await;
        if let Err(error) = &result {
            tracing::warn!(
                error_type = error.diagnostic_code(),
                "EZVIZ control read failed"
            );
        }
        let value = result.ok();
        let ttl = if value.is_some() {
            self.ttl
        } else {
            self.failure_ttl
        };
        *entry = Some(Entry {
            expires: Instant::now() + ttl,
            value: value.clone(),
        });
        value.map_or(Cached::Failed, Cached::Value)
    }

    /// Forgets the cached value so the next read fetches again.
    pub(super) async fn invalidate(&self) {
        *self.entry.lock().await = None;
    }
}
