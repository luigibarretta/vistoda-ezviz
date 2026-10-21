use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware,
    response::Response,
    routing::get,
};
use serde_json::json;

use crate::{
    alarms::AlarmFeed, auth::ApiAuthenticator, config::BridgeConfig, controls::ControlService,
    device::DeviceService, enrollment::EnrollmentManager, error::BridgeError, hub::RawStreamHub,
    metrics::Metrics, recordings::RecordingManager, remux::MpegTsHub,
    session_health::SessionHealth, snapshot::SnapshotService, storage::ensure_private_regular,
    transport::CameraTransport,
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
    pub alarms: Arc<AlarmFeed>,
    pub devices: Arc<DeviceService>,
    pub controls: Arc<ControlService>,
    /// EZVIZ session state shared with the transport.
    pub session: SessionHealth,
    enrollment: OnceLock<EnrollmentManager>,
}

impl Runtime {
    /// A runtime whose session is already known to work (tests and fakes).
    pub fn build(
        config: BridgeConfig,
        transport: Arc<dyn CameraTransport>,
    ) -> Result<Arc<Self>, BridgeError> {
        Self::build_with_session(config, transport, SessionHealth::healthy())
    }

    pub fn build_with_session(
        config: BridgeConfig,
        transport: Arc<dyn CameraTransport>,
        session: SessionHealth,
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
        let alarms = AlarmFeed::load(
            &crate::alarms::data_directory(&config.data_dir),
            &config.cameras,
            metrics.clone(),
        );
        let devices = DeviceService::new(&config.cameras);
        let controls = ControlService::new(&config.cameras, &metrics);
        Ok(Arc::new(Self {
            config,
            auth,
            metrics,
            raw,
            ts,
            snapshots,
            recordings,
            alarms,
            devices,
            controls,
            session,
            enrollment: OnceLock::new(),
        }))
    }

    /// Enables runtime re-enrollment; later calls are ignored.
    pub(crate) fn attach_enrollment(&self, manager: EnrollmentManager) {
        let _ignored = self.enrollment.set(manager);
    }

    pub(crate) fn enrollment(&self) -> Result<&EnrollmentManager, BridgeError> {
        self.enrollment
            .get()
            .ok_or(BridgeError::UpstreamUnavailable)
    }

    pub async fn close(&self) {
        self.alarms.close().await;
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
        .route("/healthz", get(crate::api_session::health))
        .route("/metrics", get(metrics))
        .route("/v1/cameras/{camera}/identity", get(camera_identity))
        .route("/v1/cameras/{camera}/snapshot.jpg", get(snapshot))
        .route("/v1/cameras/{camera}/live.mpegps", get(raw_stream))
        .route("/v1/cameras/{camera}/live.ts", get(ts_stream))
        .merge(crate::api_recordings::routes())
        .merge(crate::api_alarms::routes())
        .merge(crate::api_device::routes())
        .merge(crate::api_controls::routes())
        .merge(crate::api_session::routes())
        .layer(middleware::from_fn_with_state(
            Arc::clone(&runtime),
            crate::api_session::authenticate,
        ))
        .with_state(runtime)
}

async fn metrics(State(runtime): State<Arc<Runtime>>) -> Response {
    response(
        StatusCode::OK,
        "text/plain; charset=utf-8",
        runtime.metrics.render().await + &runtime.session.render(),
    )
}

async fn camera_identity(
    State(runtime): State<Arc<Runtime>>,
    Path(camera): Path<String>,
) -> Result<Json<serde_json::Value>, BridgeError> {
    let configured = runtime
        .config
        .cameras
        .get(&camera)
        .ok_or(BridgeError::CameraNotFound)?;
    Ok(Json(json!({
        "camera": camera,
        "source_id": format!("{}:{}", configured.serial, configured.channel),
    })))
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
