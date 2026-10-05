//! Stateful synthetic EZVIZ control source shared by the control API tests.
#![allow(dead_code, clippy::struct_excessive_bools)]

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use axum::{body::Body, response::Response};
use ezviz_vtm_bridge::{
    api::router,
    config::CameraConfig,
    controls::{ControlSource, PtzAction, PtzDirection, VendorWrite},
    device::{DeviceSource, DeviceStatus, RecordQuery},
    error::BridgeError,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use zeroize::Zeroizing;

use crate::support::TestSystem;

pub const SERIAL: &str = "never-exposed";

pub struct State {
    pub switches: BTreeMap<u16, bool>,
    pub defence: bool,
    pub detection: u8,
    pub sensitivity: i64,
    pub group_mode: u8,
    /// Accept writes without applying them (an unconfirmed change).
    pub sticky: bool,
    pub ptz_supported: bool,
    pub fail_stop_once: bool,
    pub writes: Vec<VendorWrite>,
    pub group_writes: Vec<u8>,
    pub ptz: Vec<(PtzDirection, PtzAction)>,
    pub pages: usize,
    pub algorithm_reads: usize,
    pub group_reads: usize,
}

pub struct FakeControls {
    pub state: Mutex<State>,
}

impl FakeControls {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                switches: BTreeMap::from([(7, false), (21, true), (3, true), (458, true)]),
                defence: true,
                detection: 1,
                sensitivity: 50,
                group_mode: 2,
                sticky: false,
                ptz_supported: true,
                fail_stop_once: false,
                writes: Vec::new(),
                group_writes: Vec::new(),
                ptz: Vec::new(),
                pages: 0,
                algorithm_reads: 0,
                group_reads: 0,
            }),
        })
    }

    pub fn with<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        change(
            &mut self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

fn page(state: &State) -> Value {
    let switches: Vec<Value> = state
        .switches
        .iter()
        .map(|(kind, enable)| json!({"type": kind, "enable": enable}))
        .collect();
    let ptz = if state.ptz_supported { "1" } else { "0" };
    let support = json!({"1": "1", "40": "1", "62": "1", "61": "3", "154": ptz}).to_string();
    json!({
        "page": {"hasNext": false},
        "deviceInfos": [{"deviceSerial": SERIAL, "status": 1, "version": "V1.0",
                         "supportExt": support}],
        "STATUS": {SERIAL: {"globalStatus": u8::from(state.defence),
                   "encryptPwd": "0123456789abcdef0123456789abcdef",
                   "optionals": {"Alarm_DetectHumanCar": {"type": state.detection},
                                 "powerRemaining": 64, "batteryCameraWorkMode": 0}}},
        "SWITCH": {SERIAL: switches},
        "TIME_PLAN": {SERIAL: [{"type": 2, "enable": 0}]},
        "UPGRADE": {SERIAL: {"isNeedUpgrade": 0}}
    })
}

#[async_trait]
impl ControlSource for FakeControls {
    async fn control_page(&self, offset: usize, _: usize) -> Result<Value, BridgeError> {
        assert_eq!(offset, 0);
        Ok(self.with(|state| {
            state.pages += 1;
            page(state)
        }))
    }

    async fn algorithm_config(&self, camera: &CameraConfig) -> Result<Value, BridgeError> {
        assert_eq!(camera.serial, SERIAL);
        Ok(self.with(|state| {
            state.algorithm_reads += 1;
            json!({"resultCode": "0", "algorithmConfig": {"algorithmList": [
                {"type": "3", "value": state.sensitivity.to_string(), "channel": 1}]}})
        }))
    }

    async fn group_defence_mode(&self) -> Result<Value, BridgeError> {
        Ok(self.with(|state| {
            state.group_reads += 1;
            json!({"meta": {"code": 200}, "mode": state.group_mode.to_string()})
        }))
    }

    async fn write(&self, _: &CameraConfig, write: VendorWrite) -> Result<(), BridgeError> {
        self.with(|state| {
            state.writes.push(write);
            if state.sticky {
                return;
            }
            match write {
                VendorWrite::Switch { kind, enable } => {
                    state.switches.insert(kind, enable);
                }
                VendorWrite::Defence(enable) => state.defence = enable,
                VendorWrite::DetectionMode(code) => state.detection = code,
                VendorWrite::Sensitivity { value, .. } => state.sensitivity = value,
            }
        });
        Ok(())
    }

    async fn set_group_defence_mode(&self, mode: u8) -> Result<(), BridgeError> {
        self.with(|state| {
            state.group_writes.push(mode);
            if !state.sticky {
                state.group_mode = mode;
            }
        });
        Ok(())
    }

    async fn ptz(
        &self,
        _: &CameraConfig,
        direction: PtzDirection,
        action: PtzAction,
    ) -> Result<(), BridgeError> {
        self.with(|state| {
            state.ptz.push((direction, action));
            if action == PtzAction::Stop && state.fail_stop_once {
                state.fail_stop_once = false;
                return Err(BridgeError::Upstream("stop failed".into()));
            }
            Ok(())
        })
    }
}

/// Device source whose cloud verification-code lookup is only counted: the
/// control routes must never reach it.
#[derive(Default)]
pub struct CountingKeys {
    pub cloud_code_requests: AtomicUsize,
}

impl CountingKeys {
    pub fn requests(&self) -> usize {
        self.cloud_code_requests.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DeviceSource for CountingKeys {
    async fn device_status(&self, _: &CameraConfig) -> Result<DeviceStatus, BridgeError> {
        Ok(DeviceStatus::default())
    }

    async fn cloud_verification_code(
        &self,
        _: &CameraConfig,
    ) -> Result<Zeroizing<String>, BridgeError> {
        self.cloud_code_requests.fetch_add(1, Ordering::SeqCst);
        Err(BridgeError::Upstream("refused".into()))
    }

    async fn storage_status(&self, _: &CameraConfig) -> Result<Value, BridgeError> {
        Ok(json!({}))
    }

    async fn record_page(&self, _: &CameraConfig, _: &RecordQuery) -> Result<Value, BridgeError> {
        Ok(json!({}))
    }
}

/// Sends one authenticated JSON request; `Value::Null` sends no body.
pub async fn call(system: &TestSystem, method: &str, path: &str, body: &Value) -> Response {
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    router(Arc::clone(&system.runtime))
        .oneshot(TestSystem::request(method, path, body))
        .await
        .unwrap_or_else(|error| panic!("{error}"))
}

pub fn attached() -> (TestSystem, Arc<FakeControls>, Arc<CountingKeys>) {
    let system = TestSystem::new();
    let fake = FakeControls::new();
    let keys = Arc::new(CountingKeys::default());
    system.runtime.controls.attach(Arc::clone(&fake) as _);
    system.runtime.devices.attach(Arc::clone(&keys) as _);
    (system, fake, keys)
}
