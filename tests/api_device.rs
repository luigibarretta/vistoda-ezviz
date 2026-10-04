mod support;

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ezviz_vtm_bridge::{
    api::router,
    config::CameraConfig,
    device::{DeviceSource, DeviceStatus, RecordQuery, RecordSearchVersion, twice_md5},
    error::BridgeError,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use zeroize::Zeroizing;

use support::{TestSystem, json};

struct Device;

#[async_trait]
impl DeviceSource for Device {
    async fn device_status(&self, _: &CameraConfig) -> Result<DeviceStatus, BridgeError> {
        Ok(DeviceStatus {
            video_encrypted: Some(true),
            encrypt_pwd_hash: Some(twice_md5("ABCDEF")),
            utc_offset_seconds: Some(0),
            record_search: Some(RecordSearchVersion::Common),
        })
    }

    async fn cloud_verification_code(
        &self,
        _: &CameraConfig,
    ) -> Result<Zeroizing<String>, BridgeError> {
        // The device endpoints must never trigger an EZVIZ code email.
        panic!("the cloud verification code must not be requested")
    }

    async fn storage_status(&self, _: &CameraConfig) -> Result<Value, BridgeError> {
        Ok(json!({"resultCode": "0", "storageStatus": {"storageList": [
            {"index": 1, "status": 2, "capacity": 0}]}}))
    }

    async fn record_page(
        &self,
        _: &CameraConfig,
        query: &RecordQuery,
    ) -> Result<Value, BridgeError> {
        assert_eq!(query.start, "2026-10-04T00:00:00");
        assert_eq!(query.stop, "2026-10-04T23:59:59");
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&[8, 0, 0, 8, 1, 0, 1, 0], 6);
        Ok(
            json!({"meta": {"code": 200}, "baseDay": "2026-10-04", "searchCount": 1,
                  "isFinished": 1, "data": STANDARD.encode(packed)}),
        )
    }
}

async fn get(system: &TestSystem, path: &str) -> axum::response::Response {
    router(Arc::clone(&system.runtime))
        .oneshot(TestSystem::request("GET", path, Body::empty()))
        .await
        .unwrap_or_else(|error| panic!("{error}"))
}

fn attached() -> TestSystem {
    let system = TestSystem::new();
    system.runtime.devices.attach(Arc::new(Device));
    system
}

#[tokio::test]
async fn device_routes_require_authentication_and_known_cameras() {
    let system = attached();
    for path in [
        "/v1/cameras/front/encryption",
        "/v1/cameras/front/storage",
        "/v1/cameras/front/sd-records?date=2026-10-04",
    ] {
        let anonymous = router(Arc::clone(&system.runtime))
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("{error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED, "{path}");
        let unknown = get(&system, &path.replace("front", "rear")).await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn encryption_and_storage_follow_the_contract() {
    let system = attached();
    let response = get(&system, "/v1/cameras/front/encryption").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let body = json(response).await;
    assert_eq!(body, json!({"video_encrypted": true, "key_source": "none"}));
    assert!(!body.to_string().contains("ABCDEF"));
    let storage = json(get(&system, "/v1/cameras/front/storage").await).await;
    assert_eq!(storage, json!({"status": "unformatted"}));
}

#[tokio::test]
async fn sd_records_validate_the_day_and_return_epoch_seconds() {
    let system = attached();
    for path in [
        "/v1/cameras/front/sd-records",
        "/v1/cameras/front/sd-records?date=2026-10-4",
        "/v1/cameras/front/sd-records?date=2026-10-04&end=2026-10-05",
        "/v1/cameras/front/sd-records?date=2026-10-04T00:00:00",
    ] {
        assert_eq!(
            get(&system, path).await.status(),
            StatusCode::BAD_REQUEST,
            "{path}"
        );
    }
    let body = json(get(&system, "/v1/cameras/front/sd-records?date=2026-10-04").await).await;
    let start = 1_791_072_000 + 8 * 3600;
    assert_eq!(
        body,
        json!({"records": [{"start": start, "end": start + 60, "type": "event"}]})
    );
}

#[tokio::test]
async fn unattached_upstream_is_unavailable() {
    let system = TestSystem::new();
    let storage = get(&system, "/v1/cameras/front/storage").await;
    assert_eq!(storage.status(), StatusCode::SERVICE_UNAVAILABLE);
    let records = get(&system, "/v1/cameras/front/sd-records?date=2026-10-04").await;
    assert_eq!(records.status(), StatusCode::SERVICE_UNAVAILABLE);
    let encryption = get(&system, "/v1/cameras/front/encryption").await;
    assert_eq!(encryption.status(), StatusCode::SERVICE_UNAVAILABLE);
}
