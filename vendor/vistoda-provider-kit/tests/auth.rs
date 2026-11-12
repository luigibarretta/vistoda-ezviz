#![allow(clippy::expect_used)]

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use tower::ServiceExt as _;
use vistoda_provider_kit::auth::{ApiToken, AuthPolicy, TokenError, bearer_matches, require_auth};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn token() -> ApiToken {
    ApiToken::new(TOKEN).expect("token")
}

fn headers(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(value).expect("header"),
    );
    headers
}

fn basic(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
}

#[test]
fn weak_tokens_are_rejected() {
    assert!(matches!(ApiToken::new("short"), Err(TokenError::Weak)));
    assert!(matches!(ApiToken::new(""), Err(TokenError::Weak)));
    assert!(ApiToken::new(TOKEN).is_ok());
    assert_eq!(format!("{:?}", token()), "ApiToken([REDACTED])");
}

#[test]
fn bearer_and_basic_are_checked_exactly() {
    let token = token();
    assert!(token.accepts(&headers(&format!("Bearer {TOKEN}")), false));
    assert!(!token.accepts(&headers(&format!("Bearer {TOKEN}x")), false));
    assert!(!token.accepts(&headers(&format!("bearer {TOKEN}")), false));
    assert!(!token.accepts(&HeaderMap::new(), true));
    let home_assistant = headers(&basic("homeassistant", TOKEN));
    assert!(token.accepts(&home_assistant, true));
    assert!(
        !token.accepts(&home_assistant, false),
        "Basic only where allowed"
    );
    assert!(!token.accepts(&headers(&basic("admin", TOKEN)), true));
    assert!(!token.accepts(&headers(&basic("homeassistant", "wrong")), true));
    assert!(!token.accepts(&headers("Basic !!!"), true));
    assert!(!token.accepts(&headers(&format!("Basic {}", STANDARD.encode(TOKEN))), true));
    assert!(bearer_matches(
        &headers(&format!("Bearer {TOKEN}")),
        TOKEN.as_bytes()
    ));
    assert!(!bearer_matches(
        &headers(&basic("homeassistant", TOKEN)),
        TOKEN.as_bytes()
    ));
}

#[test]
fn token_files_must_be_private_and_are_trimmed() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("api_token");
    std::fs::write(&path, format!("{TOKEN}\n")).expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).expect("chmod");
    assert!(matches!(
        ApiToken::from_file(&path),
        Err(TokenError::File(_))
    ));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    let loaded = ApiToken::from_file(&path).expect("token file");
    assert_eq!(loaded.expose_secret(), TOKEN.as_bytes());
}

fn router(policy: AuthPolicy) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/metrics", get(|| async { "metrics" }))
        .route("/v1/cameras/front/snapshot.jpg", get(|| async { "jpeg" }))
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(policy),
            require_auth,
        ))
}

async fn status(router: &Router, path: &str, authorization: Option<&str>) -> StatusCode {
    let mut request = Request::builder().uri(path);
    if let Some(value) = authorization {
        request = request.header(header::AUTHORIZATION, value);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    response.status()
}

#[tokio::test]
async fn the_middleware_protects_every_route_but_health() {
    let router = router(AuthPolicy::new(token()));
    assert_eq!(status(&router, "/healthz", None).await, StatusCode::OK);
    assert_eq!(
        status(&router, "/metrics", None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(&router, "/missing", None).await,
        StatusCode::UNAUTHORIZED
    );
    let bearer = format!("Bearer {TOKEN}");
    assert_eq!(
        status(&router, "/metrics", Some(&bearer)).await,
        StatusCode::OK
    );
    let basic = basic("homeassistant", TOKEN);
    let snapshot = "/v1/cameras/front/snapshot.jpg";
    assert_eq!(
        status(&router, snapshot, Some(&basic)).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn basic_paths_and_custom_rejections() {
    let policy = AuthPolicy::new(token())
        .basic_paths(|path| path.ends_with("/snapshot.jpg"))
        .rejection(|| StatusCode::FORBIDDEN.into_response());
    let router = router(policy);
    let basic = basic("homeassistant", TOKEN);
    let snapshot = "/v1/cameras/front/snapshot.jpg";
    assert_eq!(
        status(&router, snapshot, Some(&basic)).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&router, "/metrics", Some(&basic)).await,
        StatusCode::FORBIDDEN
    );
    let response = router
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_default_rejection_is_a_json_bearer_challenge() {
    let response = router(AuthPolicy::new(token()))
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
}
