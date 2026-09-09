use std::{collections::BTreeMap, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;

use crate::{
    VERSION, auth::ApiAuthenticator, config::BridgeConfig, error::BridgeError, hub::RawStreamHub,
    metrics::Metrics, recordings::RecordingManager, remux::MpegTsHub, snapshot::SnapshotService,
    storage::ensure_private_regular, transport::CameraTransport,
};

#[path = "api_stream.rs"]
mod stream_api;
use stream_api::{raw_stream, ts_stream};

pub struct Runtime {
    pub config: BridgeConfig,
    pub auth: ApiAuthenticator,
    pub metrics: Metrics,
    pub raw: BTreeMap<String, Arc<RawStreamHub>>,
    pub ts: BTreeMap<String, Arc<MpegTsHub>>,
    pub snapshots: SnapshotService,
    pub recordings: Arc<RecordingManager>,
}

impl Runtime {
    pub fn build(
        config: BridgeConfig,
        transport: Arc<dyn CameraTransport>,
    ) -> Result<Arc<Self>, BridgeError> {
        let token = ensure_private_regular(&config.api_token_file, 32)?;
        let auth = ApiAuthenticator::new(token)?;
        let metrics = Metrics::default();
        let mut raw = BTreeMap::new();
        let mut ts = BTreeMap::new();
        for (alias, camera) in &config.cameras {
            let raw_hub = RawStreamHub::new(
                alias.clone(),
                camera.clone(),
                Arc::clone(&transport),
                metrics.clone(),
                config.queue_chunks,
                config.max_subscribers,
                Duration::from_secs(config.idle_grace_seconds),
            );
            let ts_hub = MpegTsHub::new(
                alias.clone(),
                Arc::clone(&raw_hub),
                metrics.clone(),
                config.ffmpeg_path.clone(),
                config.queue_chunks,
                config.max_subscribers,
                Duration::from_secs(config.idle_grace_seconds),
            );
            raw.insert(alias.clone(), raw_hub);
            ts.insert(alias.clone(), ts_hub);
        }
        let snapshots = SnapshotService::new(
            config.cameras.clone(),
            transport,
            metrics.clone(),
            Duration::from_secs(config.snapshot_cache_seconds),
            Duration::from_secs(config.snapshot_stale_seconds),
        );
        let recordings = RecordingManager::load(
            config.data_dir.join("recordings"),
            raw.clone(),
            metrics.clone(),
            config.max_recording_seconds,
            config.max_recording_bytes,
            config.recording_quota_bytes,
        )?;
        Ok(Arc::new(Self {
            config,
            auth,
            metrics,
            raw,
            ts,
            snapshots,
            recordings,
        }))
    }

    pub async fn close(&self) {
        self.recordings.close().await;
        for hub in self.ts.values() {
            hub.close().await;
        }
        for hub in self.raw.values() {
            hub.close().await;
        }
    }
}

pub fn router(runtime: Arc<Runtime>) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/metrics", get(metrics))
        .route("/v1/cameras/{camera}/snapshot.jpg", get(snapshot))
        .route("/v1/cameras/{camera}/live.mpegps", get(raw_stream))
        .route("/v1/cameras/{camera}/live.ts", get(ts_stream))
        .merge(crate::api_recordings::routes())
        .layer(middleware::from_fn_with_state(
            Arc::clone(&runtime),
            authenticate,
        ))
        .with_state(runtime)
}

async fn authenticate(
    State(runtime): State<Arc<Runtime>>,
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
        HeaderValue::from_static("Basic realm=\"ezviz-vtm-bridge\""),
    );
    response
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status":"ok", "version":VERSION}))
}

async fn metrics(State(runtime): State<Arc<Runtime>>) -> Response {
    response(
        StatusCode::OK,
        "text/plain; charset=utf-8",
        runtime.metrics.render().await,
    )
}

async fn snapshot(
    State(runtime): State<Arc<Runtime>>,
    Path(camera): Path<String>,
) -> Result<Response, BridgeError> {
    let image = runtime.snapshots.get(&camera).await?;
    let mut result = response(
        StatusCode::OK,
        "image/jpeg",
        Body::from(image.as_ref().clone()),
    );
    no_store(result.headers_mut());
    Ok(result)
}

pub(super) fn response(
    status: StatusCode,
    content_type: &'static str,
    body: impl Into<Body>,
) -> Response {
    let mut response = Response::new(body.into());
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}
pub(super) fn no_store(headers: &mut HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
}
