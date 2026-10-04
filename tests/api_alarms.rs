mod support;

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use ezviz_vtm_bridge::{
    alarms::AlarmSource, api::router, config::CameraConfig, error::BridgeError,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use support::{TestSystem, json};

const JPEG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 1, 2, 3, 0xff, 0xd9];

struct Alarms;

#[async_trait]
impl AlarmSource for Alarms {
    async fn alarm_summary(&self) -> Result<Value, BridgeError> {
        Ok(
            json!({"summaries":[{"deviceSerial":"never-exposed","total":1,
            "topMessage":{"msgId":"evt-1"}}]}),
        )
    }

    async fn alarm_list(&self, _: &str, _: &str, _: &str) -> Result<Value, BridgeError> {
        Ok(
            json!({"hasNext":false,"message":[{"msgId":"evt-1","channel":1,
            "time":1_700_000_000_000_i64,"title":"Doorbell","subType":2701,
            "pic":"https://pictures.example.invalid/evt-1","picCrypt":0,
            "ext":{"alarmType":0}}]}),
        )
    }

    async fn alarm_picture(&self, _: &str, _: usize) -> Result<Vec<u8>, BridgeError> {
        Ok(JPEG.to_vec())
    }

    async fn picture_key(&self, _: &CameraConfig) -> Result<String, BridgeError> {
        Err(BridgeError::Upstream("unused".into()))
    }
}

async fn get(system: &TestSystem, path: &str) -> axum::response::Response {
    router(Arc::clone(&system.runtime))
        .oneshot(TestSystem::request("GET", path, Body::empty()))
        .await
        .unwrap_or_else(|error| panic!("{error}"))
}

#[tokio::test]
async fn alarm_routes_require_authentication_and_validate_queries() {
    let system = TestSystem::new();
    let anonymous = router(Arc::clone(&system.runtime))
        .oneshot(
            Request::get("/v1/cameras/front/alarms")
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("{error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    for path in [
        "/v1/cameras/front/alarms?wait=26",
        "/v1/cameras/front/alarms?after=-1",
        "/v1/cameras/front/alarms?after=1&extra=1",
    ] {
        let response = get(&system, path).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(json(response).await["error"], "invalid_alarm_request");
    }
    let missing = get(&system, "/v1/cameras/unknown/alarms").await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let empty = get(&system, "/v1/cameras/front/alarms?after=0&wait=0").await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert_eq!(empty.headers()[header::CACHE_CONTROL], "no-store");
    let body = json(empty).await;
    assert_eq!(body["camera"], "front");
    assert_eq!(body["next_sequence"], 0);
    assert_eq!(body["events"], json!([]));
    assert_eq!(
        body["generation"].as_str().map(str::len),
        Some(36),
        "generation is a UUID"
    );
    for path in [
        "/v1/cameras/front/alarms/evt-1/picture.jpg",
        "/v1/cameras/front/alarms/bad.id/picture.jpg",
    ] {
        assert_eq!(get(&system, path).await.status(), StatusCode::NOT_FOUND);
    }
    system.runtime.close().await;
}

#[tokio::test]
async fn polled_alarms_are_listed_with_pictures_and_not_replayed() {
    let system = TestSystem::new();
    system
        .runtime
        .alarms
        .spawn(Arc::new(Alarms), Duration::from_millis(20));
    let mut recent = Value::Null;
    for _ in 0..200 {
        recent = json(get(&system, "/v1/cameras/front/alarms").await).await;
        if recent["next_sequence"] == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let event = &recent["events"][0];
    assert_eq!(event["id"], "evt-1");
    assert_eq!(event["sequence"], 1);
    assert_eq!(event["occurred_at"], 1_700_000_000);
    assert_eq!(event["category"], "doorbell");
    assert_eq!(event["title"], "Doorbell");
    assert_eq!(event["has_picture"], true);
    let fields: Vec<_> = event
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    assert_eq!(
        fields,
        [
            "alarm_type",
            "category",
            "has_picture",
            "id",
            "occurred_at",
            "sequence",
            "title"
        ]
    );
    let primed = json(get(&system, "/v1/cameras/front/alarms?after=0").await).await;
    assert_eq!(
        primed["events"],
        json!([]),
        "startup history is not replayed"
    );
    assert_eq!(primed["next_sequence"], 1);
    let picture = get(&system, "/v1/cameras/front/alarms/evt-1/picture.jpg").await;
    assert_eq!(picture.status(), StatusCode::OK);
    assert_eq!(picture.headers()[header::CONTENT_TYPE], "image/jpeg");
    let bytes = picture
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .to_bytes();
    assert_eq!(bytes.as_ref(), JPEG);
    let metrics = get(&system, "/metrics").await;
    let text = metrics
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .to_bytes();
    let text = String::from_utf8_lossy(&text);
    assert!(text.contains("ezviz_bridge_alarm_polls_total{camera=\"front\"}"));
    assert!(text.contains("ezviz_bridge_alarm_pictures_stored_total{camera=\"front\"} 1"));
    assert!(!text.contains("never-exposed"));
    system.runtime.close().await;
}
