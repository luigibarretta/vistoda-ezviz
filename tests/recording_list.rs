mod support;

use std::sync::Arc;

use axum::{body::Body, http::StatusCode};
use ezviz_vtm_bridge::api::router;
use http_body_util::BodyExt;
use tower::ServiceExt;

use support::TestSystem;

#[tokio::test]
async fn archive_inventory_is_authenticated_and_starts_empty() {
    let system = TestSystem::new();
    assert!(system.directory.path().join("data/recordings").is_dir());
    let app = router(Arc::clone(&system.runtime));
    let unauthorized = app
        .clone()
        .oneshot(
            axum::http::Request::get("/v1/recordings")
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("{error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(TestSystem::request(
            "GET",
            "/v1/recordings?page=1&page_size=10&camera=front",
            Body::empty(),
        ))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .to_bytes();
    let payload: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(payload["recordings"], serde_json::json!([]));
    assert_eq!(payload["pagination"]["page"], 1);
    assert_eq!(payload["pagination"]["page_size"], 10);
    assert_eq!(payload["pagination"]["total_items"], 0);
    let invalid = app
        .oneshot(TestSystem::request(
            "GET",
            "/v1/recordings?page=1&page_size=101",
            Body::empty(),
        ))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    system.runtime.close().await;
}
