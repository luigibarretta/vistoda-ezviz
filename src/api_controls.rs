//! Native camera controls and the account defence mode.
//!
//! Reads are served from cached EZVIZ data; writes happen only here, on an
//! explicit request, and are verified by a read-back.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};

use crate::{
    api::{Runtime, no_store},
    controls::{ControlError, ControlRequest, DefenceUpdate, PtzRequest},
};

pub fn routes() -> Router<Arc<Runtime>> {
    Router::new()
        .route(
            "/v1/cameras/{camera}/controls",
            get(read_controls).put(write_control),
        )
        .route("/v1/cameras/{camera}/ptz", post(ptz))
        .route("/v1/account/defence", get(read_defence).put(write_defence))
}

fn json_response(value: impl serde::Serialize) -> Response {
    let mut result = Json(value).into_response();
    no_store(result.headers_mut());
    result
}

fn body<T>(input: Result<Json<T>, JsonRejection>) -> Result<T, ControlError> {
    input
        .map(|Json(value)| value)
        .map_err(|_| ControlError::Invalid("request body does not match the schema"))
}

async fn read_controls(
    State(runtime): State<Arc<Runtime>>,
    Path(camera): Path<String>,
) -> Result<Response, ControlError> {
    Ok(json_response(runtime.controls.controls(&camera).await?))
}

async fn write_control(
    State(runtime): State<Arc<Runtime>>,
    Path(camera): Path<String>,
    input: Result<Json<ControlRequest>, JsonRejection>,
) -> Result<Response, ControlError> {
    if !runtime.config.cameras.contains_key(&camera) {
        return Err(ControlError::CameraNotFound);
    }
    let request = body(input)?;
    Ok(json_response(
        runtime.controls.update(&camera, &request).await?,
    ))
}

async fn ptz(
    State(runtime): State<Arc<Runtime>>,
    Path(camera): Path<String>,
    input: Result<Json<PtzRequest>, JsonRejection>,
) -> Result<Response, ControlError> {
    if !runtime.config.cameras.contains_key(&camera) {
        return Err(ControlError::CameraNotFound);
    }
    runtime.controls.ptz(&camera, body(input)?).await?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    no_store(response.headers_mut());
    Ok(response)
}

async fn read_defence(State(runtime): State<Arc<Runtime>>) -> Result<Response, ControlError> {
    Ok(json_response(runtime.controls.account_defence().await?))
}

async fn write_defence(
    State(runtime): State<Arc<Runtime>>,
    input: Result<Json<DefenceUpdate>, JsonRejection>,
) -> Result<Response, ControlError> {
    let update = body(input)?;
    Ok(json_response(
        runtime.controls.set_account_defence(update).await?,
    ))
}
