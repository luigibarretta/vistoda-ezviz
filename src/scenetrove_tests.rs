use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::StatusCode,
    response::Response,
    routing::{get, post},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::*;

const MEDIA: &[u8] = b"\x00\x00\x01\xba-scene-trove-media";

#[derive(Default)]
struct MockState {
    posts: AtomicUsize,
    deletes: AtomicUsize,
    local_commit_seen: AtomicBool,
    expected_local: std::sync::Mutex<Option<PathBuf>>,
}

async fn create(State(state): State<Arc<MockState>>) -> Json<Value> {
    state.posts.fetch_add(1, Ordering::Relaxed);
    Json(manifest())
}

async fn status() -> Json<Value> {
    Json(manifest())
}

async fn media() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(MEDIA))
        .unwrap_or_else(|error| panic!("{error}"))
}

async fn acknowledge(State(state): State<Arc<MockState>>) -> StatusCode {
    state.deletes.fetch_add(1, Ordering::Relaxed);
    let exists = state
        .expected_local
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|path| path.is_file());
    state.local_commit_seen.store(exists, Ordering::Relaxed);
    StatusCode::NO_CONTENT
}

fn manifest() -> Value {
    json!({
        "recording_id":"00000000-0000-4000-8000-000000000001",
        "status":"ready",
        "completed_at":"2026-08-14T01:02:03Z",
        "bytes":MEDIA.len(),
        "sha256":hex::encode(Sha256::digest(MEDIA)),
        "error_code":null
    })
}

async fn server(state: Arc<MockState>) -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route("/v1/cameras/front/recordings", post(create))
        .route("/v1/recordings/{id}", get(status).delete(acknowledge))
        .route("/v1/recordings/{id}/media", get(media))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("{error}"));
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
    });
    (format!("http://{address}"), task)
}

fn request(directory: &Path, base_url: String, key: &str) -> PullRequest {
    let token_file = directory.join("token");
    fs::write(&token_file, "x".repeat(32)).unwrap_or_else(|error| panic!("{error}"));
    fs::set_permissions(&token_file, fs::Permissions::from_mode(0o600))
        .unwrap_or_else(|error| panic!("{error}"));
    PullRequest {
        base_url,
        camera: "front".into(),
        duration_seconds: 1,
        idempotency_key: key.into(),
        token_file,
        destination: directory.join("media"),
        timeout_seconds: 5,
        max_bytes: 1024,
    }
}

#[tokio::test]
async fn ack_happens_only_after_local_commit() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let state = Arc::new(MockState::default());
    let (url, task) = server(Arc::clone(&state)).await;
    let request = request(directory.path(), url, "normal-commit-001");
    let expected = request
        .destination
        .join("hiv20260814T010203Z-00000000-0000-4000-8000-000000000001.mp4");
    *state
        .expected_local
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(expected);
    let result = pull(request)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        fs::read(result.path).unwrap_or_else(|error| panic!("{error}")),
        MEDIA
    );
    assert!(state.local_commit_seen.load(Ordering::Relaxed));
    assert_eq!(state.deletes.load(Ordering::Relaxed), 1);
    task.abort();
}

#[tokio::test]
async fn receipt_recovers_crash_window_without_new_recording() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let state = Arc::new(MockState::default());
    let (url, task) = server(Arc::clone(&state)).await;
    let request = request(directory.path(), url, "crash-window-001");
    fs::create_dir_all(&request.destination).unwrap_or_else(|error| panic!("{error}"));
    let media_path = request.destination.join("existing.mp4");
    fs::write(&media_path, MEDIA).unwrap_or_else(|error| panic!("{error}"));
    *state
        .expected_local
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(media_path.clone());
    let result = PullResult {
        recording_id: "already-committed".into(),
        path: media_path,
        bytes: MEDIA.len() as u64,
        sha256: hex::encode(Sha256::digest(MEDIA)),
    };
    let receipt = receipt_path(&request.destination, &request.idempotency_key);
    atomic_write_json(&receipt, &result).unwrap_or_else(|error| panic!("{error}"));
    pull(request)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(state.posts.load(Ordering::Relaxed), 0);
    assert_eq!(state.deletes.load(Ordering::Relaxed), 1);
    assert!(!receipt.exists());
    task.abort();
}

#[tokio::test]
async fn corrupt_local_commit_blocks_remote_ack() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let state = Arc::new(MockState::default());
    let (url, task) = server(Arc::clone(&state)).await;
    let request = request(directory.path(), url, "corrupt-commit-001");
    fs::create_dir_all(&request.destination).unwrap_or_else(|error| panic!("{error}"));
    let media_path = request.destination.join("corrupt.mp4");
    fs::write(&media_path, b"\x00\x00\x01\xba-corrupt").unwrap_or_else(|error| panic!("{error}"));
    let result = PullResult {
        recording_id: "must-not-ack".into(),
        path: media_path,
        bytes: MEDIA.len() as u64,
        sha256: hex::encode(Sha256::digest(MEDIA)),
    };
    atomic_write_json(
        &receipt_path(&request.destination, &request.idempotency_key),
        &result,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(pull(request).await.is_err());
    assert_eq!(state.posts.load(Ordering::Relaxed), 0);
    assert_eq!(state.deletes.load(Ordering::Relaxed), 0);
    task.abort();
}
