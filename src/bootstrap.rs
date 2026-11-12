use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;
use vistoda_provider_kit::auth::{AuthPolicy, require_auth, unauthorized};

use crate::{
    VERSION,
    config::BridgeConfig,
    enrollment::{EnrollmentManager, EnrollmentStart, VerifyEnrollment},
    error::BridgeError,
};

struct BootstrapRuntime {
    enrollment: EnrollmentManager,
}

pub fn router(
    config: &BridgeConfig,
    ready: tokio::sync::watch::Sender<bool>,
) -> Result<Router, BridgeError> {
    let policy = AuthPolicy::new(crate::auth::load(&config.api_token_file)?)
        .rejection(|| unauthorized("Bearer realm=\"vistoda-ezviz\""));
    let runtime = Arc::new(BootstrapRuntime {
        enrollment: EnrollmentManager::new(
            config.ezviz_token_file.clone(),
            config.upstream_timeout_seconds,
            ready,
        ),
    });
    Ok(Router::new()
        .route("/healthz", get(health))
        .route("/metrics", get(metrics))
        .route("/v1/enrollments", post(start))
        .route("/v1/enrollments/{enrollment}", post(verify).delete(cancel))
        .layer(middleware::from_fn_with_state(
            Arc::new(policy),
            require_auth,
        ))
        .with_state(runtime))
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status":"ok", "phase":"enrollment_required", "version":VERSION}))
}

async fn metrics() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "ezviz_bridge_enrollment_required 1\n",
    )
        .into_response()
}

async fn start(
    State(runtime): State<Arc<BootstrapRuntime>>,
    Json(input): Json<EnrollmentStart>,
) -> Result<Json<crate::enrollment::EnrollmentStarted>, BridgeError> {
    Ok(Json(runtime.enrollment.start(input).await?))
}

async fn verify(
    State(runtime): State<Arc<BootstrapRuntime>>,
    Path(enrollment): Path<String>,
    Json(input): Json<VerifyEnrollment>,
) -> Result<Json<crate::enrollment::EnrollmentVerified>, BridgeError> {
    Ok(Json(runtime.enrollment.verify(&enrollment, input).await?))
}

async fn cancel(
    State(runtime): State<Arc<BootstrapRuntime>>,
    Path(enrollment): Path<String>,
) -> StatusCode {
    runtime.enrollment.cancel(&enrollment).await;
    StatusCode::NO_CONTENT
}
