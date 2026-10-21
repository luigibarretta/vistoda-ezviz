//! Single-flight cache cell with separate success and failure lifetimes, a
//! read deadline and a bounded stale fallback.

use std::{future::Future, time::Duration};

use tokio::{sync::Mutex, time::Instant};

use crate::{error::BridgeError, metrics::Metrics};

struct Entry<T> {
    expires: Instant,
    /// `Err` records a recent failure and its diagnostic category.
    value: Result<T, &'static str>,
}

/// The last successful value, kept apart so a failure cannot erase it.
struct Good<T> {
    value: T,
    at: Instant,
}

/// Concurrent readers wait for one fetch; the lock is held across that fetch
/// on purpose so a burst of requests produces a single vendor call.
pub(super) struct TtlCell<T> {
    name: &'static str,
    entry: Mutex<Option<Entry<T>>>,
    good: std::sync::Mutex<Option<Good<T>>>,
    ttl: Duration,
    failure_ttl: Duration,
    stale_for: Duration,
    metrics: Metrics,
}

/// Outcome of a cache read.
pub(super) enum Cached<T> {
    Value(T),
    /// The last good value, served because the current read failed.
    Stale(T),
    /// No usable value; carries the failure's diagnostic category.
    Failed(&'static str),
}

impl<T: Clone> TtlCell<T> {
    pub(super) const fn new(
        name: &'static str,
        ttl: Duration,
        failure_ttl: Duration,
        stale_for: Duration,
        metrics: Metrics,
    ) -> Self {
        Self {
            name,
            entry: Mutex::const_new(None),
            good: std::sync::Mutex::new(None),
            ttl,
            failure_ttl,
            stale_for,
            metrics,
        }
    }

    /// Cached value, or one fetch when expired or when `force` is set. Waiting
    /// and fetching both stop at `deadline`; then the stale fallback applies.
    /// A forced read never answers with a stale value.
    pub(super) async fn get<F, Fut>(&self, force: bool, deadline: Instant, fetch: F) -> Cached<T>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<T, BridgeError>> + Send,
    {
        let Ok(mut entry) = tokio::time::timeout_at(deadline, self.entry.lock()).await else {
            return self.fallback(force, "timeout").await;
        };
        let fresh = entry
            .as_ref()
            .filter(|entry| !force && entry.expires > Instant::now());
        if let Some(cached) = fresh {
            return match &cached.value {
                Ok(value) => Cached::Value(value.clone()),
                Err(category) => self.fallback(force, category).await,
            };
        }
        if Instant::now() >= deadline {
            // An earlier read used the whole budget; do not record a failure.
            drop(entry);
            return self.fallback(force, "timeout").await;
        }
        let result = tokio::time::timeout_at(deadline, fetch())
            .await
            .unwrap_or(Err(BridgeError::Timeout));
        let (value, ttl) = match result {
            Ok(value) => {
                *self.lock_good() = Some(Good {
                    value: value.clone(),
                    at: Instant::now(),
                });
                (Ok(value), self.ttl)
            }
            Err(error) => {
                let category = error.diagnostic_code();
                tracing::warn!(
                    error_type = category,
                    read = self.name,
                    "EZVIZ control read failed"
                );
                let labels = [("read", self.name), ("category", category)];
                self.metrics
                    .increment_labeled("control_read_failures_total", &labels)
                    .await;
                (Err(category), self.failure_ttl)
            }
        };
        *entry = Some(Entry {
            expires: Instant::now() + ttl,
            value: value.clone(),
        });
        drop(entry);
        match value {
            Ok(value) => Cached::Value(value),
            Err(category) => self.fallback(force, category).await,
        }
    }

    /// Serves the last good value for `stale_for` unless the session is gone.
    async fn fallback(&self, force: bool, category: &'static str) -> Cached<T> {
        let usable = !force && category != "reauth_required";
        let stale = usable
            .then(|| {
                self.lock_good()
                    .as_ref()
                    .filter(|good| good.at.elapsed() <= self.stale_for)
                    .map(|good| good.value.clone())
            })
            .flatten();
        match stale {
            Some(value) => {
                let labels = [("read", self.name)];
                self.metrics
                    .increment_labeled("control_stale_reads_total", &labels)
                    .await;
                Cached::Stale(value)
            }
            None => Cached::Failed(category),
        }
    }

    /// Forgets the cached and the last good value: the state is unknown.
    pub(super) async fn invalidate(&self) {
        *self.entry.lock().await = None;
        *self.lock_good() = None;
    }

    fn lock_good(&self) -> std::sync::MutexGuard<'_, Option<Good<T>>> {
        self.good
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
