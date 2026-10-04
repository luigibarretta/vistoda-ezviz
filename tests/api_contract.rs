mod support;

use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use ezviz_vtm_bridge::{api::router, bootstrap};
use http_body_util::BodyExt;
use tokio::time::{Instant, timeout};
use tower::ServiceExt;

use support::{TestSystem, json};

#[tokio::test]
async fn live_stream_ends_at_the_configured_session_limit() {
    let system = TestSystem::with_live_session_limit(1);
    let response = router(Arc::clone(&system.runtime))
        .oneshot(TestSystem::request(
            "GET",
            "/v1/cameras/front/live.mpegps",
            Body::empty(),
        ))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(response.status(), StatusCode::OK);

    let started = Instant::now();
    let media = timeout(Duration::from_secs(2), response.into_body().collect())
        .await
        .unwrap_or_else(|error| panic!("live response exceeded its session limit: {error}"))
        .unwrap_or_else(|error| panic!("{error}"))
        .to_bytes();

    assert!(media.starts_with(b"\x00\x00\x01\xba"));
    assert!(started.elapsed() >= Duration::from_millis(900));
    system.runtime.close().await;
}

#[tokio::test]
async fn encrypted_camera_rejects_mpeg_ps_but_keeps_ts_contract() {
    let system = TestSystem::with_encrypted_camera();
    let response = router(Arc::clone(&system.runtime))
        .oneshot(TestSystem::request(
            "GET",
            "/v1/cameras/front/live.mpegps",
            Body::empty(),
        ))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json(response).await["error"], "unsupported_media");
    system.runtime.close().await;
}

#[tokio::test]
async fn program_stream_route_rejects_runtime_detected_encryption() {
    let system = TestSystem::with_detected_encryption();
    let response = router(Arc::clone(&system.runtime))
        .oneshot(TestSystem::request(
            "GET",
            "/v1/cameras/front/live.mpegps",
            Body::empty(),
        ))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json(response).await["error"], "unsupported_media");
    system.runtime.close().await;
}

#[tokio::test]
async fn health_is_public_and_media_requires_authentication() {
    let system = TestSystem::new();
    let app = router(Arc::clone(&system.runtime));
    let health = app
        .clone()
        .oneshot(
            Request::get("/healthz")
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("{error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(json(health).await["status"], "ok");
    let unauthorized = app
        .oneshot(
            Request::get("/metrics")
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("{error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        unauthorized.headers()[header::WWW_AUTHENTICATE],
        "Basic realm=\"ezviz-vtm-bridge\""
    );
    system.runtime.close().await;
}

#[tokio::test]
async fn enrollment_bootstrap_is_private_and_reports_its_phase() {
    let system = TestSystem::new();
    let (ready, _receiver) = tokio::sync::watch::channel(false);
    let app =
        bootstrap::router(&system.runtime.config, ready).unwrap_or_else(|error| panic!("{error}"));
    let health = app
        .clone()
        .oneshot(
            Request::get("/healthz")
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("{error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(json(health).await["phase"], "enrollment_required");
    let unauthorized = app
        .clone()
        .oneshot(
            Request::get("/metrics")
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("{error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let authorized = app
        .oneshot(TestSystem::request("GET", "/metrics", Body::empty()))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(authorized.status(), StatusCode::OK);
    system.runtime.close().await;
}
