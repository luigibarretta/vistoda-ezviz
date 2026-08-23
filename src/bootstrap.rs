use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;

use crate::{
    VERSION,
    auth::ApiAuthenticator,
    config::BridgeConfig,
    enrollment::{EnrollmentManager, EnrollmentStart, VerifyEnrollment},
    error::BridgeError,
    storage::ensure_private_regular,
};

struct BootstrapRuntime {
    auth: ApiAuthenticator,
    enrollment: EnrollmentManager,
}

pub fn router(
    config: &BridgeConfig,
    ready: tokio::sync::watch::Sender<bool>,
) -> Result<Router, BridgeError> {
    let token = ensure_private_regular(&config.api_token_file, 32)?;
    let runtime = Arc::new(BootstrapRuntime {
        auth: ApiAuthenticator::new(token)?,
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
            Arc::clone(&runtime),
            authenticate,
        ))
        .with_state(runtime))
}

async fn authenticate(
    State(runtime): State<Arc<BootstrapRuntime>>,
    request: Request,
    next: Next,
) -> Response {
    if request.uri().path() == "/healthz" || runtime.auth.accepts(request.headers()) {
        return next.run(request).await;
    }
    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error":"unauthorized"})),
    )
        .into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"vistoda-ezviz\""),
    );
    response
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
