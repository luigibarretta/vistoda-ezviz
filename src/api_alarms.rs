//! Cursor/long-poll alarm feed and stored alarm pictures.

use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    alarms::{AlarmBatch, MAX_WAIT_SECONDS, is_token},
    api::{Runtime, no_store, response},
    error::BridgeError,
};

pub fn routes() -> Router<Arc<Runtime>> {
    Router::new()
        .route("/v1/cameras/{camera}/alarms", get(list_alarms))
        .route(
            "/v1/cameras/{camera}/alarms/{alarm_id}/picture.jpg",
            get(alarm_picture),
        )
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AlarmQuery {
    after: Option<u64>,
    wait: Option<u64>,
}

async fn list_alarms(
    State(runtime): State<Arc<Runtime>>,
    Path(camera): Path<String>,
    query: Result<Query<AlarmQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, BridgeError> {
    let Ok(Query(query)) = query else {
        return Ok(bad_request("after and wait must be non-negative integers"));
    };
    let wait = query.wait.unwrap_or(0);
    if wait > MAX_WAIT_SECONDS {
        return Ok(bad_request("wait must be between 0 and 25 seconds"));
    }
    let batch: AlarmBatch = runtime
        .alarms
        .batch(&camera, query.after, Duration::from_secs(wait))
        .await?;
    let mut result = Json(batch).into_response();
    no_store(result.headers_mut());
    Ok(result)
}

async fn alarm_picture(
    State(runtime): State<Arc<Runtime>>,
    Path((camera, alarm_id)): Path<(String, String)>,
) -> Result<Response, BridgeError> {
    if !is_token(&alarm_id) {
        return Ok(not_found());
    }
    let Some(picture) = runtime.alarms.picture(&camera, &alarm_id).await? else {
        return Ok(not_found());
    };
    let mut result = response(StatusCode::OK, "image/jpeg", Body::from(picture));
    no_store(result.headers_mut());
    Ok(result)
}

fn bad_request(detail: &'static str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":"invalid_alarm_request", "detail":detail})),
    )
        .into_response()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error":"alarm_picture_not_found"})),
    )
        .into_response()
}
