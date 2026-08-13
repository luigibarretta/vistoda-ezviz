use std::{
    collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use ezviz_vtm_bridge::{
    api::{Runtime, router},
    config::{BridgeConfig, CameraConfig},
    error::BridgeError,
    transport::{CameraTransport, ChunkConsumer},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

struct FakeTransport;

#[async_trait]
impl CameraTransport for FakeTransport {
    async fn snapshot_jpeg(&self, _: &CameraConfig) -> Result<Vec<u8>, BridgeError> {
        Ok(vec![0xff, 0xd8, 0xff, 0xe0, 0xff, 0xd9])
    }

    async fn stream_mpeg_ps(
        &self,
        _: &CameraConfig,
        cancel: CancellationToken,
        output: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError> {
        while !cancel.is_cancelled() {
            if !output.consume(b"\x00\x00\x01\xba-media".to_vec()) {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }
}

struct TestSystem {
    directory: TempDir,
    runtime: Arc<Runtime>,
}

impl TestSystem {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let token_path = directory.path().join("api-token");
        fs::write(&token_path, TOKEN).unwrap_or_else(|error| panic!("{error}"));
        fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| panic!("{error}"));
        let mut cameras = BTreeMap::new();
        cameras.insert(
            "front".into(),
            CameraConfig {
                serial: "never-exposed".into(),
                decrypt_video: false,
                media_key_file: None,
            },
        );
        let config = BridgeConfig {
            bind_host: "127.0.0.1".into(),
            bind_port: 8765,
            api_token_file: token_path,
            ezviz_token_file: PathBuf::from("unused"),
            cameras,
            data_dir: directory.path().join("data"),
            ffmpeg_path: PathBuf::from("/usr/bin/false"),
            upstream_timeout_seconds: 20,
            idle_grace_seconds: 0,
            queue_chunks: 8,
            max_subscribers: 4,
            max_recording_seconds: 10,
            max_recording_bytes: 1024 * 1024,
            recording_quota_bytes: 4 * 1024 * 1024,
            snapshot_cache_seconds: 3,
            snapshot_stale_seconds: 30,
        };
        let runtime = Runtime::build(config, Arc::new(FakeTransport))
            .unwrap_or_else(|error| panic!("{error}"));
        Self { directory, runtime }
    }

    fn request(method: &str, path: &str, body: Body) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)
            .unwrap_or_else(|error| panic!("{error}"))
    }
}

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
