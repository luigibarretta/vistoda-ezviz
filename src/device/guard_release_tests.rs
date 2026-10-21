//! The 24-hour latch is not taken for lookups EZVIZ never processed.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::CloudKeyGuard;
use crate::error::BridgeError;

#[tokio::test]
async fn a_refused_session_does_not_latch_the_lookup() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let guard = CloudKeyGuard::load(directory.path().join("attempts.json"));
    let calls = AtomicUsize::new(0);
    for error in [BridgeError::ReauthRequired, BridgeError::SessionExpired] {
        let result = guard
            .fetch("SERIAL", 1_000, || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(error)
            })
            .await;
        assert!(result.is_err());
        assert_eq!(guard.blocked_until("SERIAL", 1_000), None);
    }
    let ok = guard.fetch("SERIAL", 1_000, || async { Ok("CODE".to_owned()) });
    assert!(ok.await.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
