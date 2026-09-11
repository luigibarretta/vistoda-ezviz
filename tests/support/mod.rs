use std::{
    collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, header},
};
use ezviz_vtm_bridge::{
    api::Runtime,
    config::{BridgeConfig, CameraConfig},
    error::BridgeError,
    transport::{CameraTransport, ChunkConsumer},
};
use tempfile::TempDir;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

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

pub struct TestSystem {
    pub directory: TempDir,
    pub runtime: Arc<Runtime>,
}

impl TestSystem {
    pub fn new() -> Self {
        Self::with_live_session_limit(90)
    }

    pub fn with_live_session_limit(max_live_session_seconds: u64) -> Self {
        Self::configured(max_live_session_seconds, false)
    }

    #[allow(dead_code)]
    pub fn with_encrypted_camera() -> Self {
        Self::configured(90, true)
    }

    fn configured(max_live_session_seconds: u64, decrypt_video: bool) -> Self {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let token_path = directory.path().join("api-token");
        fs::write(&token_path, TOKEN).unwrap_or_else(|error| panic!("{error}"));
        fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| panic!("{error}"));
        let cameras = BTreeMap::from([(
            "front".into(),
            CameraConfig {
                serial: "never-exposed".into(),
                channel: 1,
                substream: false,
                decrypt_video,
                media_key_file: decrypt_video.then(|| directory.path().join("media-key")),
            },
        )]);
        let config = BridgeConfig {
            bind_host: "127.0.0.1".into(),
            bind_port: 8765,
            api_token_file: token_path,
            ezviz_token_file: PathBuf::from("unused"),
            cameras,
            data_dir: directory.path().join("data"),
            ffmpeg_path: PathBuf::from("/usr/bin/false"),
            upstream_timeout_seconds: 20,
            max_live_session_seconds,
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

    pub fn request(method: &str, path: &str, body: Body) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)
            .unwrap_or_else(|error| panic!("{error}"))
    }
}
