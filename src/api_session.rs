//! Bridge-token authentication, EZVIZ session reporting and runtime
//! re-enrollment.
//!
//! HTTP 401 always means a wrong bridge token. When EZVIZ refused the stored
//! session, every EZVIZ-backed route answers 403 `{"error":"reauth_required"}`
//! so Home Assistant starts its re-authentication flow; that flow enrolls
//! through `/v1/enrollments`, which the runtime serves too. Locally stored
//! recordings stay readable.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::json;

use crate::{
    VERSION,
    api::{Runtime, no_store},
    enrollment::{EnrollmentStart, EnrollmentStarted, EnrollmentVerified, VerifyEnrollment},
    error::BridgeError,
};

pub fn routes() -> Router<Arc<Runtime>> {
    Router::new()
        .route("/v1/enrollments", post(start))
        .route("/v1/enrollments/{enrollment}", post(verify).delete(cancel))
}

/// Routes that need a working EZVIZ session.
fn needs_session(path: &str) -> bool {
    path.starts_with("/v1/")
        && !path.starts_with("/v1/recordings")
        && !path.starts_with("/v1/enrollments")
}

pub async fn authenticate(
    State(runtime): State<Arc<Runtime>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    if path == "/healthz" {
        return next.run(request).await;
    }
    if !runtime.auth.accepts(request.headers()) {
        runtime
            .metrics
            .increment_labeled("api_auth_failures_total", &[])
            .await;
        let mut response = (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        )
            .into_response();
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"ezviz-vtm-bridge\""),
        );
        return response;
    }
    if runtime.session.reauth_required() && needs_session(&path) {
        let (status, body) = BridgeError::ReauthRequired.public_response();
        let mut response = (status, Json(body)).into_response();
        no_store(response.headers_mut());
        return response;
    }
    next.run(request).await
}

/// Liveness stays 200 while degraded, so the Supervisor does not restart a
/// bridge that only waits for EZVIZ or for a new enrollment.
pub async fn health(State(runtime): State<Arc<Runtime>>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "version": VERSION,
        "ezviz_session": runtime.session.state().as_str(),
    }))
}

async fn start(
    State(runtime): State<Arc<Runtime>>,
    Json(input): Json<EnrollmentStart>,
) -> Result<Json<EnrollmentStarted>, BridgeError> {
    let manager = runtime.enrollment()?;
    Ok(Json(manager.start(input).await?))
}

async fn verify(
    State(runtime): State<Arc<Runtime>>,
    Path(enrollment): Path<String>,
    Json(input): Json<VerifyEnrollment>,
) -> Result<Json<EnrollmentVerified>, BridgeError> {
    let manager = runtime.enrollment()?;
    Ok(Json(manager.verify(&enrollment, input).await?))
}

async fn cancel(
    State(runtime): State<Arc<Runtime>>,
    Path(enrollment): Path<String>,
) -> Result<StatusCode, BridgeError> {
    runtime.enrollment()?.cancel(&enrollment).await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::needs_session;

    #[test]
    fn local_recordings_and_enrollment_do_not_need_the_session() {
        assert!(needs_session("/v1/cameras/front/identity"));
        assert!(needs_session("/v1/account/defence"));
        assert!(needs_session("/v1/cameras/front/recordings"));
        assert!(!needs_session("/v1/recordings/abc/media"));
        assert!(!needs_session("/v1/enrollments"));
        assert!(!needs_session("/metrics"));
    }
}
