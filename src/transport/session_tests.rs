//! Session expiry, refresh and token rotation against the fake EZVIZ server.

use std::time::Duration;

use serde_json::json;

use super::session_fake_tests::{PAD, Refresh, harness, lock, stored_refresh_token};
use crate::{
    controls::ControlSource,
    error::BridgeError,
    session_health::SessionState,
    transport::{authed::read_limited, media::download_presigned},
};

#[tokio::test]
async fn an_expired_session_refreshes_once_and_persists_the_rotated_token() {
    for expire_with_401 in [false, true] {
        let system = harness(Refresh::Rotate).await;
        *lock(&system.fake.expire_with_401) = expire_with_401;
        let value = system.transport.group_defence_mode().await;
        assert_eq!(
            value.ok().map(|value| value["mode"].clone()),
            Some(json!("2"))
        );
        assert_eq!(system.refreshes(), 1);
        assert_eq!(
            stored_refresh_token(&system.path),
            format!("refresh-1{PAD}")
        );
        assert_eq!(system.health.state(), SessionState::Ok);
        assert!(
            system
                .health
                .render()
                .contains("refreshes_total{result=\"ok\"} 1")
        );
    }
}

#[tokio::test]
async fn concurrent_expiries_share_one_refresh() {
    let system = harness(Refresh::Rotate).await;
    let calls = (0..8).map(|_| system.transport.group_defence_mode());
    let results = futures_util::future::join_all(calls).await;
    assert!(results.iter().all(Result::is_ok));
    // A second refresh would have reused the rotated token and been refused.
    assert_eq!(system.refreshes(), 1);
}

#[tokio::test]
async fn a_cancelled_request_cannot_lose_the_rotated_token() {
    let system = harness(Refresh::Rotate).await;
    *lock(&system.fake.delay) = Duration::from_millis(300);
    let call = system.transport.group_defence_mode();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), call)
            .await
            .is_err()
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        stored_refresh_token(&system.path),
        format!("refresh-1{PAD}")
    );
    *lock(&system.fake.delay) = Duration::ZERO;
    assert!(system.transport.group_defence_mode().await.is_ok());
    assert_eq!(system.refreshes(), 1);
}

#[tokio::test]
async fn a_refused_refresh_requires_reauth_and_is_not_retried() {
    let system = harness(Refresh::Reject).await;
    for _ in 0..3 {
        let result = system.transport.group_defence_mode().await;
        assert!(matches!(result, Err(BridgeError::ReauthRequired)));
    }
    assert_eq!(system.refreshes(), 1);
    assert!(system.health.reauth_required());
    assert_eq!(
        stored_refresh_token(&system.path),
        format!("refresh-0{PAD}")
    );
}

#[tokio::test]
async fn a_failing_start_serves_degraded_instead_of_exiting() {
    let system = harness(Refresh::Fail).await;
    assert_eq!(system.refreshes(), 0, "loading must not contact EZVIZ");
    let start = system.transport.start();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(system.refreshes(), 1);
    assert_eq!(system.health.state(), SessionState::Degraded);
    assert!(!start.is_finished(), "a transient failure keeps retrying");
    start.abort();
    let result = system.transport.group_defence_mode().await;
    assert!(matches!(result, Err(BridgeError::Upstream(_))));
    assert_eq!(system.refreshes(), 1, "the refresh cooldown applies");
}

#[tokio::test]
async fn a_successful_start_refreshes_and_discovers_services() {
    let system = harness(Refresh::Rotate).await;
    let start = system.transport.start();
    tokio::time::timeout(Duration::from_secs(5), start)
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(system.health.state(), SessionState::Ok);
    let raw = std::fs::read_to_string(&system.path).unwrap_or_else(|error| panic!("{error}"));
    assert!(raw.contains("refresh-1") && raw.contains("authAddr"));
}

#[tokio::test]
async fn media_downloads_require_https() {
    let system = harness(Refresh::Rotate).await;
    let url = format!(
        "http://{}/v3/configurations/system/info",
        system.transport.token().await.api_url
    );
    let result = download_presigned(&system.transport.media, &url, 1024).await;
    assert!(matches!(result, Err(BridgeError::Upstream(_))));
}

#[tokio::test]
async fn bodies_are_read_within_a_bound() {
    let system = harness(Refresh::Rotate).await;
    let url = format!("http://{}/big", system.transport.token().await.api_url);
    let fetch = || async {
        let response = system.transport.media.get(&url).send().await;
        response.unwrap_or_else(|error| panic!("{error}"))
    };
    assert!(read_limited(fetch().await, 1024).await.is_err());
    let body = read_limited(fetch().await, 8192).await;
    assert_eq!(body.map(|bytes| bytes.len()).ok(), Some(4096));
}

#[tokio::test]
async fn a_server_error_does_not_confirm_the_session() {
    let system = harness(Refresh::Rotate).await;
    let result = system.transport.message_json("/v3/broken", &[]).await;
    assert!(matches!(result, Err(BridgeError::Upstream(_))));
    assert_eq!(system.health.state(), SessionState::Degraded);
}

#[tokio::test]
async fn an_endpoint_refusing_a_fresh_session_fails_alone() {
    let system = harness(Refresh::Rotate).await;
    let result = system
        .transport
        .message_json("/v3/always-expired", &[])
        .await;
    assert!(matches!(result, Err(BridgeError::SessionExpired)));
    assert_eq!(system.refreshes(), 1);
    // The refresh worked: no re-authentication is requested.
    assert_eq!(system.health.state(), SessionState::Ok);
}

#[tokio::test]
async fn the_cloud_key_lookup_shares_the_session_rules() {
    let system = harness(Refresh::Rotate).await;
    let key = system.transport.camera_key("SERIAL").await;
    assert_eq!(key.ok().as_deref().map(String::as_str), Some("ABCDEF"));
    assert_eq!(system.refreshes(), 1);
    let refused = harness(Refresh::Reject).await;
    let now = chrono::Utc::now().timestamp();
    let result = refused.transport.camera_key("SERIAL").await;
    assert!(matches!(result, Err(BridgeError::ReauthRequired)));
    // EZVIZ never processed the lookup, so the 24-hour latch is not taken.
    assert_eq!(
        refused.transport.cloud_keys.blocked_until("SERIAL", now),
        None
    );
}
