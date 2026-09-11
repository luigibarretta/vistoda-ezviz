mod support;

use std::sync::Arc;

use axum::{body::Body, http::StatusCode};
use ezviz_vtm_bridge::api::router;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use support::TestSystem;

async fn json(response: axum::response::Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .to_bytes();
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("{error}"))
}

#[tokio::test]
async fn camera_identity_is_authenticated_and_alias_scoped() {
    let system = TestSystem::new();
    assert!(system.directory.path().is_dir());
    let app = router(Arc::clone(&system.runtime));
    let identity = app
        .clone()
        .oneshot(TestSystem::request(
            "GET",
            "/v1/cameras/front/identity",
            Body::empty(),
        ))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(identity.status(), StatusCode::OK);
    let payload = json(identity).await;
    assert_eq!(payload["camera"], "front");
    assert_eq!(payload["source_id"], "never-exposed:1");
    let missing = app
        .oneshot(TestSystem::request(
            "GET",
            "/v1/cameras/missing/identity",
            Body::empty(),
        ))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    system.runtime.close().await;
}

#[tokio::test]
async fn cached_live_url_rejects_retarget_before_upstream() {
    let system = TestSystem::new();
    let response = router(Arc::clone(&system.runtime))
        .oneshot(TestSystem::request(
            "GET",
            "/v1/cameras/front/live.ts?expected_binding=stale",
            Body::empty(),
        ))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(json(response).await["error"], "camera_not_found");
    system.runtime.close().await;
}
