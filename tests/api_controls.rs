mod controls_fake;
mod support;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use ezviz_vtm_bridge::{
    api::router,
    controls::{PtzAction, PtzDirection},
};
use serde_json::{Value, json};
use tower::ServiceExt;

use controls_fake::{attached, call};
use support::{TestSystem, json};

#[tokio::test]
async fn control_routes_require_authentication_known_cameras_and_a_runtime() {
    let (system, _, _) = attached();
    for (method, path) in [
        ("GET", "/v1/cameras/front/controls"),
        ("PUT", "/v1/cameras/front/controls"),
        ("POST", "/v1/cameras/front/ptz"),
        ("GET", "/v1/account/defence"),
        ("PUT", "/v1/account/defence"),
    ] {
        let anonymous = router(Arc::clone(&system.runtime))
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("{error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED, "{path}");
        if path.contains("front") {
            let unknown = call(&system, method, &path.replace("front", "rear"), &json!({})).await;
            assert_eq!(unknown.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }
    let detached = TestSystem::new();
    let read = call(&detached, "GET", "/v1/cameras/front/controls", &Value::Null).await;
    assert_eq!(read.status(), StatusCode::SERVICE_UNAVAILABLE);
    let defence = call(&detached, "GET", "/v1/account/defence", &Value::Null).await;
    assert_eq!(defence.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn controls_follow_the_contract_and_are_cached() {
    let (system, fake, _) = attached();
    let response = call(&system, "GET", "/v1/cameras/front/controls", &Value::Null).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let body = json(response).await;
    assert_eq!(
        body,
        json!({
            "online": true, "defence_enabled": true, "alarm_schedule_enabled": false,
            "detection_mode": "human_shape",
            "sensitivity": {"value": 50, "min": 0, "max": 100},
            "switches": {"privacy": false, "sleep": true, "status_light": true},
            "ptz": true,
            "battery": {"percent": 64, "work_mode": "power_saving"},
            "firmware": {"version": "V1.0", "update_available": false}
        })
    );
    let text = body.to_string();
    assert!(!text.contains("never-exposed") && !text.contains("0123456789abcdef"));
    for _ in 0..3 {
        call(&system, "GET", "/v1/cameras/front/controls", &Value::Null).await;
    }
    assert_eq!(
        fake.with(|state| (state.pages, state.algorithm_reads)),
        (1, 1)
    );
}

#[tokio::test]
async fn ptz_validates_the_direction_and_sends_one_step() {
    let (system, fake, _) = attached();
    for body in [
        json!({}),
        json!({"direction": "UP"}),
        json!({"direction": "zoom"}),
        json!({"direction": "up", "speed": 9}),
    ] {
        let response = call(&system, "POST", "/v1/cameras/front/ptz", &body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
    }
    assert!(fake.with(|state| state.ptz.is_empty()));
    fake.with(|state| state.fail_stop_once = true);
    let moved = call(
        &system,
        "POST",
        "/v1/cameras/front/ptz",
        &json!({"direction": "left"}),
    )
    .await;
    assert_eq!(moved.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        fake.with(|state| state.ptz.clone()),
        [
            (PtzDirection::Left, PtzAction::Start),
            (PtzDirection::Left, PtzAction::Stop),
            (PtzDirection::Left, PtzAction::Stop)
        ]
    );
}

#[tokio::test]
async fn ptz_is_refused_when_the_camera_lacks_it() {
    let (system, fake, _) = attached();
    fake.with(|state| state.ptz_supported = false);
    let response = call(
        &system,
        "POST",
        "/v1/cameras/front/ptz",
        &json!({"direction": "up"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        json(response).await,
        json!({"error": "unsupported_control"})
    );
    assert!(fake.with(|state| state.ptz.is_empty()));
}

#[tokio::test(start_paused = true)]
async fn account_defence_checks_the_expected_mode_and_reads_back() {
    let (system, fake, _) = attached();
    let read = json(call(&system, "GET", "/v1/account/defence", &Value::Null).await).await;
    assert_eq!(read, json!({"mode": "away"}));
    let stale = json!({"mode": "home", "expected_mode": "sleep"});
    let conflict = call(&system, "PUT", "/v1/account/defence", &stale).await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        json(conflict).await,
        json!({"error": "conflict", "current": "away"})
    );
    for invalid in [
        json!({"mode": "disarmed", "expected_mode": "away"}),
        json!({"mode": "home"}),
    ] {
        let bad = call(&system, "PUT", "/v1/account/defence", &invalid).await;
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST, "{invalid}");
    }
    let change = json!({"mode": "home", "expected_mode": "away"});
    let changed = call(&system, "PUT", "/v1/account/defence", &change).await;
    assert_eq!(changed.status(), StatusCode::OK);
    assert_eq!(json(changed).await, json!({"mode": "home"}));
    assert_eq!(fake.with(|state| state.group_writes.clone()), [1]);
    fake.with(|state| state.sticky = true);
    let back = json!({"mode": "sleep", "expected_mode": "home"});
    let unconfirmed = call(&system, "PUT", "/v1/account/defence", &back).await;
    assert_eq!(unconfirmed.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(json(unconfirmed).await, json!({"error": "unconfirmed"}));
    // The failed change is rolled back exactly once to the previous mode.
    assert_eq!(fake.with(|state| state.group_writes.clone()), [1, 3, 1]);
}

#[tokio::test(start_paused = true)]
async fn control_routes_never_request_the_cloud_verification_code() {
    let (system, _, keys) = attached();
    let switch = json!({"key": "switch.privacy", "value": true, "expected_value": false});
    let defence = json!({"mode": "sleep", "expected_mode": "away"});
    for (method, path, body) in [
        ("GET", "/v1/cameras/front/controls", Value::Null),
        ("PUT", "/v1/cameras/front/controls", switch),
        (
            "POST",
            "/v1/cameras/front/ptz",
            json!({"direction": "down"}),
        ),
        ("GET", "/v1/account/defence", Value::Null),
        ("PUT", "/v1/account/defence", defence),
    ] {
        let response = call(&system, method, path, &body).await;
        assert!(response.status().is_success(), "{method} {path}");
    }
    assert_eq!(keys.requests(), 0);
}
