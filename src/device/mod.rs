//! Read-only camera device APIs: video encryption, microSD status and the
//! SD-card record index. Nothing here formats, reboots or toggles a camera.

mod key;
mod model;
mod records;
mod search;
mod service;
mod storage;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_service;

pub use key::{KeySource, VideoKey, candidate_keys, matches_hash, resolve_video_key, twice_md5};
pub use model::{DeviceStatus, RecordSearchVersion, parse_device_status, parse_utc_offset};
pub use records::{
    MAX_RECORDS, MAX_V2_PAGES, RecordQuery, SdRecord, V2_PAGE_SIZE, day_window, decode_payload,
    normalize, parse_common, parse_v2_page,
};
pub use service::DeviceService;
pub use storage::{StorageReport, StorageState, parse_storage_status};

use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::{config::CameraConfig, error::BridgeError};

/// Storage status is fetched at most once per camera in this window.
pub const STORAGE_TTL: Duration = Duration::from_secs(600);
/// Encryption state and key availability are re-checked after this window.
pub const ENCRYPTION_TTL: Duration = Duration::from_secs(600);
/// Failed lookups are retried no sooner than this.
pub const FAILURE_TTL: Duration = Duration::from_secs(60);
/// Record index cache for today and yesterday (UTC, with one day of margin).
pub const RECORDS_RECENT_TTL: Duration = Duration::from_secs(60);
/// Record index cache for older days.
pub const RECORDS_PAST_TTL: Duration = Duration::from_secs(600);

/// Read-only EZVIZ calls used by the device APIs.
#[async_trait]
pub trait DeviceSource: Send + Sync {
    /// `STATUS` and capability data from the resource page list.
    async fn device_status(&self, camera: &CameraConfig) -> Result<DeviceStatus, BridgeError>;
    /// Cloud copy of the verification code (`/api/device/query/encryptkey`).
    async fn cloud_verification_code(
        &self,
        camera: &CameraConfig,
    ) -> Result<Zeroizing<String>, BridgeError>;
    /// Raw `/api/device/queryStorageStatus` response.
    async fn storage_status(&self, camera: &CameraConfig) -> Result<Value, BridgeError>;
    /// One raw record-index response.
    async fn record_page(
        &self,
        camera: &CameraConfig,
        query: &RecordQuery,
    ) -> Result<Value, BridgeError>;
}

/// `GET /v1/cameras/{camera}/encryption` response body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EncryptionReport {
    pub video_encrypted: Option<bool>,
    pub key_source: KeySource,
}

/// `GET /v1/cameras/{camera}/sd-records` response body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RecordList {
    pub records: Vec<SdRecord>,
}
