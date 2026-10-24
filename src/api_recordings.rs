use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use tokio_util::io::ReaderStream;

use crate::{
    api::{Runtime, no_store, response},
    error::BridgeError,
    pagination, recording_playback,
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
        .route(
            "/v1/recordings/{recording_id}/playback.mp4",
            get(playback_recording),
        )
}

/// A live capture (`duration_seconds`) or one microSD interval
/// (`source: "sd"` with epoch-second `start` and `end`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordingRequest {
    duration_seconds: Option<u64>,
    source: Option<String>,
    start: Option<i64>,
    end: Option<i64>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordingPage {
    page: Option<usize>,
    page_size: Option<usize>,
    camera: Option<String>,
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
    let manifest = match (payload.source.as_deref(), payload.duration_seconds) {
        (None | Some("live"), Some(duration))
            if payload.start.is_none() && payload.end.is_none() =>
        {
            runtime.recordings.start(&camera, duration, key).await?
        }
        (Some("sd"), None) => {
            let (Some(start), Some(end)) = (payload.start, payload.end) else {
                return Err(BridgeError::Recording(
                    "a microSD recording needs start and end".into(),
                ));
            };
            if !runtime.config.cameras.contains_key(&camera) {
                return Err(BridgeError::CameraNotFound);
            }
            // Reject impossible bounds before asking the cloud for the zone.
            let length = end.checked_sub(start).unwrap_or(0);
            let limit = runtime.config.max_recording_seconds;
            if length < 1 || u64::try_from(length).unwrap_or(u64::MAX) > limit {
                return Err(BridgeError::Recording(format!(
                    "microSD interval must last between 1 and {limit} seconds"
                )));
            }
            let (begin_local, end_local) = runtime
                .devices
                .playback_times(&camera, (start, end))
                .await?;
            let interval = crate::recordings::SdInterval {
                start,
                end,
                begin_local,
                end_local,
            };
            runtime.recordings.start_sd(&camera, interval, key).await?
        }
        _ => {
            return Err(BridgeError::Recording(
                "send duration_seconds for live, or source sd with start and end".into(),
            ));
        }
    };
    Ok((StatusCode::ACCEPTED, Json(manifest)))
}

async fn list_recordings(
    State(runtime): State<Arc<Runtime>>,
    Query(query): Query<RecordingPage>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut recordings = runtime.recordings.list().await;
    if let Some(camera) = query.camera {
        recordings.retain(|item| item.camera == camera);
    }
    let (recordings, pagination) = pagination::page(&recordings, query.page, query.page_size)
        .ok_or(StatusCode::BAD_REQUEST)?;
    let storage = runtime
        .recordings
        .storage_descriptor()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(serde_json::json!({
        "recordings": recordings,
        "pagination": pagination,
        "storage": storage
    })))
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
    let (path, media_type) = runtime
        .recordings
        .media(&id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let mut result = response(
        StatusCode::OK,
        if media_type == "video/mp2t" {
            "video/mp2t"
        } else {
            "video/mpeg"
        },
        Body::from_stream(ReaderStream::new(file)),
    );
    no_store(result.headers_mut());
    Ok(result)
}

async fn playback_recording(
    State(runtime): State<Arc<Runtime>>,
    Path(id): Path<String>,
) -> Result<Response, StatusCode> {
    recording_playback::playback(runtime, &id).await
}
