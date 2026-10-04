//! Per-camera caches and request coalescing for the device APIs.

use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
    time::Duration,
};

use chrono::{NaiveDate, Utc};
use tokio::{
    sync::{Mutex, Semaphore},
    time::Instant,
};

use super::{
    DeviceSource, ENCRYPTION_TTL, EncryptionReport, FAILURE_TTL, KeySource, RECORDS_PAST_TTL,
    RECORDS_RECENT_TTL, RecordList, STORAGE_TTL, StorageReport, parse_storage_status,
    resolve_video_key, search::lookup_day,
};
use crate::{config::CameraConfig, error::BridgeError};

/// Upstream record searches allowed at once across all cameras.
pub(super) const MAX_RECORD_LOOKUPS: usize = 2;
/// Cached days per camera.
const MAX_CACHED_DAYS: usize = 8;

pub(super) struct Cached<T> {
    pub(super) value: T,
    pub(super) expires: Instant,
}

pub(super) struct Slot {
    config: CameraConfig,
    pub(super) storage: Mutex<Option<Cached<StorageReport>>>,
    encryption: Mutex<Option<Cached<EncryptionReport>>>,
    records: Mutex<BTreeMap<NaiveDate, Cached<RecordList>>>,
}

/// Per-camera caches and request coalescing for the device APIs.
pub struct DeviceService {
    cameras: BTreeMap<String, Slot>,
    source: OnceLock<Arc<dyn DeviceSource>>,
    record_permits: Semaphore,
}

impl DeviceService {
    #[must_use]
    pub fn new(cameras: &BTreeMap<String, CameraConfig>) -> Arc<Self> {
        let cameras = cameras
            .iter()
            .map(|(alias, config)| {
                let slot = Slot {
                    config: config.clone(),
                    storage: Mutex::new(None),
                    encryption: Mutex::new(None),
                    records: Mutex::new(BTreeMap::new()),
                };
                (alias.clone(), slot)
            })
            .collect();
        Arc::new(Self {
            cameras,
            source: OnceLock::new(),
            record_permits: Semaphore::new(MAX_RECORD_LOOKUPS),
        })
    }

    /// Connects the upstream source once; later calls are ignored.
    pub fn attach(&self, source: Arc<dyn DeviceSource>) {
        let _ignored = self.source.set(source);
    }

    pub(super) fn slot(&self, camera: &str) -> Result<&Slot, BridgeError> {
        self.cameras.get(camera).ok_or(BridgeError::CameraNotFound)
    }

    fn source(&self) -> Result<&Arc<dyn DeviceSource>, BridgeError> {
        self.source.get().ok_or(BridgeError::UpstreamUnavailable)
    }

    /// Cached microSD status; vendor failures become `unknown`.
    pub async fn storage(&self, camera: &str) -> Result<StorageReport, BridgeError> {
        let slot = self.slot(camera)?;
        let mut cached = slot.storage.lock().await;
        if let Some(entry) = cached
            .as_ref()
            .filter(|entry| entry.expires > Instant::now())
        {
            return Ok(entry.value.clone());
        }
        let (value, ttl) = match self.source()?.storage_status(&slot.config).await {
            Ok(raw) => (parse_storage_status(&raw), STORAGE_TTL),
            Err(error) => {
                tracing::warn!(
                    error_type = error.diagnostic_code(),
                    "storage status lookup failed"
                );
                (StorageReport::unknown(), FAILURE_TTL)
            }
        };
        *cached = Some(Cached {
            value: value.clone(),
            expires: Instant::now() + ttl,
        });
        Ok(value)
    }

    /// Cached encryption state and usable key source.
    pub async fn encryption(&self, camera: &str) -> Result<EncryptionReport, BridgeError> {
        let slot = self.slot(camera)?;
        let mut cached = slot.encryption.lock().await;
        if let Some(entry) = cached
            .as_ref()
            .filter(|entry| entry.expires > Instant::now())
        {
            return Ok(entry.value.clone());
        }
        let source = self.source()?;
        let status = source.device_status(&slot.config).await.ok();
        let key = resolve_video_key(source.as_ref(), &slot.config, status.as_ref()).await;
        let value = EncryptionReport {
            video_encrypted: status.as_ref().and_then(|status| status.video_encrypted),
            key_source: key.as_ref().map_or(KeySource::None, |key| key.source),
        };
        let ttl = if status.is_some() {
            ENCRYPTION_TTL
        } else {
            FAILURE_TTL
        };
        *cached = Some(Cached {
            value: value.clone(),
            expires: Instant::now() + ttl,
        });
        Ok(value)
    }

    /// Record index for one camera-local day, newest protocol first.
    pub async fn sd_records(
        &self,
        camera: &str,
        day: NaiveDate,
    ) -> Result<RecordList, BridgeError> {
        let slot = self.slot(camera)?;
        // Holding the per-camera map serializes lookups for one camera.
        let mut cache = slot.records.lock().await;
        let now = Instant::now();
        cache.retain(|_, entry| entry.expires > now);
        if let Some(entry) = cache.get(&day) {
            return Ok(entry.value.clone());
        }
        let source = self.source()?;
        let _permit = self
            .record_permits
            .acquire()
            .await
            .map_err(|_| BridgeError::UpstreamUnavailable)?;
        let list = lookup_day(source.as_ref(), &slot.config, day).await?;
        while cache.len() >= MAX_CACHED_DAYS {
            cache.pop_first();
        }
        cache.insert(
            day,
            Cached {
                value: list.clone(),
                expires: Instant::now() + record_ttl(day, Utc::now().date_naive()),
            },
        );
        Ok(list)
    }
}

/// Recent days may still gain clips; older days change only on overwrite.
#[must_use]
pub(super) fn record_ttl(day: NaiveDate, utc_today: NaiveDate) -> Duration {
    // One day of margin covers every camera time zone.
    if day < utc_today - chrono::Days::new(1) {
        RECORDS_PAST_TTL
    } else {
        RECORDS_RECENT_TTL
    }
}
