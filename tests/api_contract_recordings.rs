//! Recording ACK contract: verified download, idempotent ACK and active-recording guard.

mod support;

use std::{fs, sync::Arc, time::Duration};

use axum::{body::Body, http::StatusCode};
use ezviz_vtm_bridge::api::router;
use http_body_util::BodyExt;
use serde_json::Value;
use tokio::time::sleep;
use tower::ServiceExt;

use support::{TestSystem, json};

#[tokio::test]
async fn recording_ack_is_idempotent_after_verified_download() {
    let system = TestSystem::new();
    let app = router(Arc::clone(&system.runtime));
    let mut create = TestSystem::request(
        "POST",
        "/v1/cameras/front/recordings",
        Body::from(r#"{"duration_seconds":1}"#),
    );
    create.headers_mut().insert(
        "idempotency-key",
        "contract-test-001"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    );
    let accepted = app
        .clone()
        .oneshot(create)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let id = json(accepted).await["recording_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    loop {
        let manifest = system
            .runtime
            .recordings
            .get(&id)
            .await
            .unwrap_or_else(|| panic!("manifest disappeared"));
        if manifest.status == "ready" {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        sleep(Duration::from_millis(20)).await;
    }
    let media = app
        .clone()
        .oneshot(TestSystem::request(
            "GET",
            &format!("/v1/recordings/{id}/media"),
            Body::empty(),
        ))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(media.status(), StatusCode::OK);
    let bytes = media
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .to_bytes();
    assert!(bytes.starts_with(b"\x00\x00\x01\xba"));
    for _ in 0..2 {
        let ack = app
            .clone()
            .oneshot(TestSystem::request(
                "DELETE",
                &format!("/v1/recordings/{id}"),
                Body::empty(),
            ))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(ack.status(), StatusCode::NO_CONTENT);
    }
    assert!(system.runtime.recordings.media_path(&id).await.is_none());
    assert!(
        system
            .runtime
            .recordings
            .start("front", 1, "contract-test-001")
            .await
            .is_err()
    );
    let journal: Value = serde_json::from_slice(
        &fs::read(
            system
                .directory
                .path()
                .join("data/recordings/recordings.json"),
        )
        .unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(journal["schema_version"], 2);
    assert_eq!(
        journal["acknowledgements"].as_array().map(Vec::len),
        Some(1)
    );
    system.runtime.close().await;
}

#[tokio::test]
async fn unknown_recording_ack_is_idempotent() {
    let system = TestSystem::new();
    let response = router(Arc::clone(&system.runtime))
        .oneshot(TestSystem::request(
            "DELETE",
            "/v1/recordings/00000000-0000-4000-8000-000000000099",
            Body::empty(),
        ))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    system.runtime.close().await;
}

#[tokio::test]
async fn active_recording_cannot_be_acknowledged() {
    let system = TestSystem::new();
    let manifest = system
        .runtime
        .recordings
        .start("front", 5, "active-test-001")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let response = router(Arc::clone(&system.runtime))
        .oneshot(TestSystem::request(
            "DELETE",
            &format!("/v1/recordings/{}", manifest.recording_id),
            Body::empty(),
        ))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(response.status(), StatusCode::CONFLICT);
    system.runtime.close().await;
}
