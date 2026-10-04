//! Read-only camera device endpoints: encryption, microSD status and the
//! SD-card record index. None of them changes camera state.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::json;

use crate::{
    api::{Runtime, no_store},
    error::BridgeError,
};

pub fn routes() -> Router<Arc<Runtime>> {
    Router::new()
        .route("/v1/cameras/{camera}/encryption", get(encryption))
        .route("/v1/cameras/{camera}/storage", get(storage))
        .route("/v1/cameras/{camera}/sd-records", get(sd_records))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordsQuery {
    date: String,
}

fn json_response(value: impl serde::Serialize) -> Response {
    let mut result = Json(value).into_response();
    no_store(result.headers_mut());
    result
}

async fn encryption(
    State(runtime): State<Arc<Runtime>>,
    Path(camera): Path<String>,
) -> Result<Response, BridgeError> {
    Ok(json_response(runtime.devices.encryption(&camera).await?))
}

async fn storage(
    State(runtime): State<Arc<Runtime>>,
    Path(camera): Path<String>,
) -> Result<Response, BridgeError> {
    Ok(json_response(runtime.devices.storage(&camera).await?))
}

async fn sd_records(
    State(runtime): State<Arc<Runtime>>,
    Path(camera): Path<String>,
    query: Result<Query<RecordsQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, BridgeError> {
    if !runtime.config.cameras.contains_key(&camera) {
        return Err(BridgeError::CameraNotFound);
    }
    let Some(day) = query.ok().and_then(|Query(query)| parse_day(&query.date)) else {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_records_request",
                        "detail":"date must be one calendar day as YYYY-MM-DD"})),
        )
            .into_response());
    };
    Ok(json_response(
        runtime.devices.sd_records(&camera, day).await?,
    ))
}

/// Strict `YYYY-MM-DD` parser for one camera-local calendar day.
fn parse_day(value: &str) -> Option<NaiveDate> {
    (value.len() == 10)
        .then(|| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::parse_day;

    #[test]
    fn day_parameter_is_strict() {
        assert!(parse_day("2026-10-04").is_some());
        for invalid in [
            "2026-10-4",
            "2026-13-01",
            "2026-10-04T00:00:00",
            "",
            "04/10/2026",
        ] {
            assert!(parse_day(invalid).is_none(), "{invalid}");
        }
    }
}
