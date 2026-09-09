use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use tokio_util::io::ReaderStream;

use crate::{
    api::{Runtime, no_store, response},
    error::BridgeError,
};

pub fn routes() -> Router<Arc<Runtime>> {
    Router::new()
        .route("/v1/recordings", get(list_recordings))
        .route("/v1/cameras/{camera}/recordings", post(create_recording))
        .route(
            "/v1/recordings/{recording_id}",
            get(get_recording).delete(delete_recording),
        )
        .route(
            "/v1/recordings/{recording_id}/media",
            get(download_recording),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordingRequest {
    duration_seconds: u64,
}

async fn create_recording(
    State(runtime): State<Arc<Runtime>>,
    Path(camera): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<RecordingRequest>,
) -> Result<impl IntoResponse, BridgeError> {
    let key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let manifest = runtime
        .recordings
        .start(&camera, payload.duration_seconds, key)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(manifest)))
}

async fn list_recordings(State(runtime): State<Arc<Runtime>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "recordings": runtime.recordings.list().await
    }))
}

async fn get_recording(
    State(runtime): State<Arc<Runtime>>,
    Path(id): Path<String>,
) -> Result<Json<crate::recordings::RecordingManifest>, StatusCode> {
    runtime
        .recordings
        .get(&id)
        .await
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn delete_recording(
    State(runtime): State<Arc<Runtime>>,
    Path(id): Path<String>,
) -> Result<StatusCode, BridgeError> {
    runtime.recordings.acknowledge(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn download_recording(
    State(runtime): State<Arc<Runtime>>,
    Path(id): Path<String>,
) -> Result<Response, StatusCode> {
    let path = runtime
        .recordings
        .media_path(&id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let mut result = response(
        StatusCode::OK,
        "video/mpeg",
        Body::from_stream(ReaderStream::new(file)),
    );
    no_store(result.headers_mut());
    Ok(result)
}
