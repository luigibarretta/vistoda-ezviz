//! Stale serving, read deadlines and the 403 `reauth_required` contract.

mod controls_fake;
mod support;

use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use ezviz_vtm_bridge::api::router;
use serde_json::{Value, json};
use tower::ServiceExt;

use controls_fake::{Fault, attached, call};
use support::{TestSystem, json};

const CONTROLS: &str = "/v1/cameras/front/controls";
const DEFENCE: &str = "/v1/account/defence";

async fn metrics(system: &TestSystem) -> String {
    let response = call(system, "GET", "/metrics", &Value::Null).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test(start_paused = true)]
async fn failed_reads_serve_the_last_good_value_marked_stale_for_ten_minutes() {
    let (system, fake, _) = attached();
    let fresh = json(call(&system, "GET", CONTROLS, &Value::Null).await).await;
    assert_eq!(fresh["stale"], json!(false));
    let mode = json(call(&system, "GET", DEFENCE, &Value::Null).await).await;
    assert_eq!(mode, json!({"mode": "away", "stale": false}));
    fake.with(|state| state.fault = Fault::Upstream);
    tokio::time::advance(Duration::from_secs(61)).await;
    let stale = json(call(&system, "GET", CONTROLS, &Value::Null).await).await;
    assert_eq!(stale["stale"], json!(true));
    assert_eq!(stale["defence_enabled"], fresh["defence_enabled"]);
    let mode = json(call(&system, "GET", DEFENCE, &Value::Null).await).await;
    assert_eq!(mode, json!({"mode": "away", "stale": true}));
    let text = metrics(&system).await;
    assert!(text.contains(
        "ezviz_bridge_control_read_failures_total{read=\"snapshot\",category=\"upstream\"} 1"
    ));
    assert!(text.contains("ezviz_bridge_control_stale_reads_total{read=\"defence\"} 1"));
    // Past the stale window the failure is reported again.
    tokio::time::advance(Duration::from_secs(10 * 60)).await;
    for path in [CONTROLS, DEFENCE] {
        let failed = call(&system, "GET", path, &Value::Null).await;
        assert_eq!(failed.status(), StatusCode::BAD_GATEWAY, "{path}");
        assert_eq!(json(failed).await, json!({"error": "upstream_failed"}));
    }
    fake.with(|state| state.fault = Fault::None);
    tokio::time::advance(Duration::from_secs(31)).await;
    let back = json(call(&system, "GET", CONTROLS, &Value::Null).await).await;
    assert_eq!(back["stale"], json!(false));
}

#[tokio::test(start_paused = true)]
async fn writes_never_use_a_stale_value() {
    let (system, fake, _) = attached();
    call(&system, "GET", CONTROLS, &Value::Null).await;
    fake.with(|state| state.fault = Fault::Upstream);
    let switch = json!({"key": "switch.privacy", "value": true, "expected_value": false});
    let write = call(&system, "PUT", CONTROLS, &switch).await;
    assert_eq!(write.status(), StatusCode::BAD_GATEWAY);
    let defence = json!({"mode": "home", "expected_mode": "away"});
    let write = call(&system, "PUT", DEFENCE, &defence).await;
    assert_eq!(write.status(), StatusCode::BAD_GATEWAY);
    assert!(fake.with(|state| state.writes.is_empty() && state.group_writes.is_empty()));
}

#[tokio::test(start_paused = true)]
async fn a_hanging_ezviz_read_ends_before_home_assistant_gives_up() {
    let (system, fake, _) = attached();
    call(&system, "GET", DEFENCE, &Value::Null).await;
    fake.with(|state| state.fault = Fault::Hang);
    tokio::time::advance(Duration::from_secs(61)).await;
    let started = tokio::time::Instant::now();
    let first = call(&system, "GET", CONTROLS, &Value::Null).await;
    assert_eq!(first.status(), StatusCode::BAD_GATEWAY);
    let mode = json(call(&system, "GET", DEFENCE, &Value::Null).await).await;
    assert_eq!(mode, json!({"mode": "away", "stale": true}));
    // Two sequential reads, each bounded by the 15 s deadline.
    assert!(started.elapsed() <= Duration::from_secs(30));
    assert!(metrics(&system).await.contains("category=\"timeout\"} 1"));
}

#[tokio::test(start_paused = true)]
async fn an_expired_ezviz_session_is_403_reauth_required_never_stale() {
    let (system, fake, _) = attached();
    call(&system, "GET", CONTROLS, &Value::Null).await;
    fake.with(|state| state.fault = Fault::Reauth);
    tokio::time::advance(Duration::from_secs(61)).await;
    for path in [CONTROLS, DEFENCE] {
        let response = call(&system, "GET", path, &Value::Null).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        assert_eq!(json(response).await, json!({"error": "reauth_required"}));
    }
}

#[tokio::test]
async fn a_refused_session_blocks_ezviz_routes_but_not_local_ones() {
    let (system, _, _) = attached();
    let health = json(call(&system, "GET", "/healthz", &Value::Null).await).await;
    assert_eq!(health["ezviz_session"], json!("ok"));
    system.runtime.session.session_rejected();
    for path in [
        "/v1/cameras/front/identity",
        "/v1/cameras/front/snapshot.jpg",
        "/v1/cameras/front/alarms",
        CONTROLS,
        DEFENCE,
    ] {
        let response = call(&system, "GET", path, &Value::Null).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        assert_eq!(json(response).await, json!({"error": "reauth_required"}));
    }
    let recordings = call(&system, "GET", "/v1/recordings", &Value::Null).await;
    assert_eq!(recordings.status(), StatusCode::OK);
    let health = json(call(&system, "GET", "/healthz", &Value::Null).await).await;
    assert_eq!(health["status"], json!("ok"));
    assert_eq!(health["ezviz_session"], json!("reauth_required"));
    // 401 still means only a wrong bridge token.
    let anonymous = router(Arc::clone(&system.runtime))
        .oneshot(
            Request::builder()
                .uri("/v1/cameras/front/identity")
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("{error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let text = metrics(&system).await;
    assert!(text.contains("ezviz_bridge_session_reauth_required 1"));
    assert!(text.contains("ezviz_bridge_api_auth_failures_total 1"));
}

#[tokio::test]
async fn runtime_enrollment_is_unavailable_until_attached() {
    let (system, _, _) = attached();
    let body = json!({"account": "user@example.test", "password": "secret"});
    let response = call(&system, "POST", "/v1/enrollments", &body).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
