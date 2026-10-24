//! In-process fake EZVIZ server for the session tests: plain HTTP on
//! loopback (production always uses HTTPS). Named `_tests` so the catalog
//! source guard treats its encryptkey route as test code.

use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Form, Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde_json::{Value, json};

use crate::{session_health::SessionHealth, transport::EzvizTransport};

pub(super) const PAD: &str = "-0123456789abcdef";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Refresh {
    Rotate,
    Reject,
    Fail,
}

pub(super) struct Fake {
    session: Mutex<String>,
    refresh: Mutex<String>,
    refreshes: AtomicUsize,
    mode: Mutex<Refresh>,
    pub(super) delay: Mutex<Duration>,
    /// Expired sessions get HTTP 401 instead of a 200 `meta.code` 99997.
    pub(super) expire_with_401: Mutex<bool>,
}

pub(super) fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

async fn login(
    State(fake): State<Arc<Fake>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let count = fake.refreshes.fetch_add(1, Ordering::SeqCst) + 1;
    let delay = *lock(&fake.delay);
    tokio::time::sleep(delay).await;
    let mode = *lock(&fake.mode);
    if mode == Refresh::Fail {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    // A refresh token works once: reuse is refused, as EZVIZ does.
    if mode == Refresh::Reject || form.get("refreshSessionId") != Some(&*lock(&fake.refresh)) {
        return Json(json!({"meta": {"code": 403}})).into_response();
    }
    let session = format!("session-{count}{PAD}");
    let refresh = format!("refresh-{count}{PAD}");
    *lock(&fake.session) = session.clone();
    *lock(&fake.refresh) = refresh.clone();
    let info = json!({"sessionId": session, "refreshSessionId": refresh});
    Json(json!({"meta": {"code": 200}, "sessionInfo": info})).into_response()
}

async fn defence(State(fake): State<Arc<Fake>>, headers: HeaderMap) -> Response {
    let current = lock(&fake.session).clone();
    if headers
        .get("sessionid")
        .and_then(|value| value.to_str().ok())
        == Some(&current)
    {
        return Json(json!({"meta": {"code": 200}, "mode": "2"})).into_response();
    }
    if *lock(&fake.expire_with_401) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({"meta": {"code": 99_997}})).into_response()
}

async fn encrypt_key(State(fake): State<Arc<Fake>>, headers: HeaderMap) -> Response {
    let current = lock(&fake.session).clone();
    if headers
        .get("sessionid")
        .and_then(|value| value.to_str().ok())
        == Some(&current)
    {
        return Json(json!({"resultCode": "0", "encryptkey": "ABCDEF"})).into_response();
    }
    StatusCode::UNAUTHORIZED.into_response()
}

async fn system_info() -> Json<Value> {
    Json(json!({"meta": {"code": 200}, "systemConfigInfo": {"authAddr": "https://none"}}))
}

pub(super) struct Harness {
    pub(super) fake: Arc<Fake>,
    pub(super) transport: Arc<EzvizTransport>,
    pub(super) health: SessionHealth,
    pub(super) path: PathBuf,
    _directory: tempfile::TempDir,
}

pub(super) async fn harness(mode: Refresh) -> Harness {
    let fake = Arc::new(Fake {
        session: Mutex::new(format!("session-0{PAD}")),
        refresh: Mutex::new(format!("refresh-0{PAD}")),
        refreshes: AtomicUsize::new(0),
        mode: Mutex::new(mode),
        delay: Mutex::new(Duration::ZERO),
        expire_with_401: Mutex::new(false),
    });
    let app = Router::new()
        .route("/v3/apigateway/login", put(login))
        .route("/v3/userdevices/v1/group/defenceMode", get(defence))
        .route("/v3/configurations/system/info", get(system_info))
        .route("/big", get(|| async { "x".repeat(4096) }))
        .route("/v3/broken", get(|| async { StatusCode::BAD_GATEWAY }))
        .route(
            "/v3/always-expired",
            get(|| async { Json(json!({"meta": {"code": 99_997}})) }),
        )
        .route("/api/device/query/encryptkey", post(encrypt_key))
        .with_state(Arc::clone(&fake));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("{error}"));
    tokio::spawn(async move { axum::serve(listener, app).await });
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("token.json");
    // The stored session has expired server-side; its refresh token is valid.
    let token = json!({"session_id": format!("expired-session{PAD}"),
        "rf_session_id": format!("refresh-0{PAD}"), "api_url": address.to_string(),
        "feature_code": "f".repeat(32)});
    std::fs::write(&path, token.to_string()).unwrap_or_else(|error| panic!("{error}"));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .unwrap_or_else(|error| panic!("{error}"));
    let health = SessionHealth::default();
    let transport = EzvizTransport::load(path.clone(), 5, health.clone())
        .unwrap_or_else(|error| panic!("{error}"))
        .over_plain_http();
    Harness {
        fake,
        transport: Arc::new(transport),
        health,
        path,
        _directory: directory,
    }
}

pub(super) fn stored_refresh_token(path: &Path) -> String {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{error}"));
    let value: Value = serde_json::from_str(&raw).unwrap_or_else(|error| panic!("{error}"));
    value["rf_session_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

impl Harness {
    pub(super) fn refreshes(&self) -> usize {
        self.fake.refreshes.load(Ordering::SeqCst)
    }
}
