//! Cached control reads, shared by the read, write and PTZ endpoints.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    sync::{Arc, OnceLock},
};

use tokio::{sync::Mutex, time::Instant};

use super::{
    CONTROLS_FAILURE_TTL, CONTROLS_TTL, CameraData, ControlError, ControlSource, Controls,
    DEFENCE_TTL, DefenceMode, MAX_PAGES, PAGE_SIZE, READ_DEADLINE, STALE_FOR, Sensitivity,
    Snapshot, VERIFY_DELAYS,
    cell::{Cached, TtlCell},
    controls_from, merge_page, page_has_next, parse_sensitivity, sensitivity_kind,
};
use crate::{config::CameraConfig, error::BridgeError, metrics::Metrics};

/// Which cached reads a control read must bypass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refresh {
    Cached,
    Status,
    All,
}

/// Read-back result after a write.
pub(super) enum Confirmation<T> {
    Confirmed(T),
    /// A read-back succeeded and still showed another value.
    Mismatch,
    /// No read-back succeeded.
    Unknown,
}

/// Native camera controls with account-wide caching and serialized writes.
pub struct ControlService {
    cameras: BTreeMap<String, CameraConfig>,
    serials: BTreeSet<String>,
    source: OnceLock<Arc<dyn ControlSource>>,
    pub(super) snapshot: TtlCell<Arc<Snapshot>>,
    sensitivity: BTreeMap<String, TtlCell<Option<Sensitivity>>>,
    pub(super) defence: TtlCell<Option<DefenceMode>>,
    /// One write or PTZ step at a time across the account.
    pub(super) writes: Mutex<()>,
}

impl ControlService {
    #[must_use]
    pub fn new(cameras: &BTreeMap<String, CameraConfig>, metrics: &Metrics) -> Arc<Self> {
        let sensitivity = cameras
            .keys()
            .map(|alias| (alias.clone(), cell("sensitivity", CONTROLS_TTL, metrics)))
            .collect();
        Arc::new(Self {
            cameras: cameras.clone(),
            serials: cameras
                .values()
                .map(|camera| camera.serial.clone())
                .collect(),
            source: OnceLock::new(),
            snapshot: cell("snapshot", CONTROLS_TTL, metrics),
            sensitivity,
            defence: cell("defence", DEFENCE_TTL, metrics),
            writes: Mutex::new(()),
        })
    }

    /// Connects the upstream source once; later calls are ignored.
    pub fn attach(&self, source: Arc<dyn ControlSource>) {
        let _ignored = self.source.set(source);
    }

    pub(super) fn camera(&self, alias: &str) -> Result<&CameraConfig, ControlError> {
        self.cameras.get(alias).ok_or(ControlError::CameraNotFound)
    }

    pub(super) fn source(&self) -> Result<Arc<dyn ControlSource>, ControlError> {
        self.source.get().cloned().ok_or(ControlError::Unavailable)
    }

    /// Cached controls of one configured camera.
    pub async fn controls(&self, alias: &str) -> Result<Controls, ControlError> {
        let config = self.camera(alias)?;
        let source = self.source()?;
        self.read(source.as_ref(), alias, config, Refresh::Cached)
            .await
    }

    pub(super) async fn read(
        &self,
        source: &dyn ControlSource,
        alias: &str,
        config: &CameraConfig,
        refresh: Refresh,
    ) -> Result<Controls, ControlError> {
        let deadline = Instant::now() + READ_DEADLINE;
        let force = refresh != Refresh::Cached;
        let (snapshot, mut stale) = self.snapshot(source, force, deadline).await?;
        let empty = CameraData::default();
        let data = snapshot.get(&config.serial).unwrap_or(&empty);
        let sensitivity = match sensitivity_kind(&data.device) {
            Some(kind) => {
                let force = refresh == Refresh::All;
                let read = self.sensitivity(source, alias, config, kind, force, deadline);
                let (value, old) = read.await;
                stale |= old;
                value
            }
            None => None,
        };
        let mut controls = controls_from(data, sensitivity);
        controls.stale = stale;
        Ok(controls)
    }

    /// The account-wide snapshot and whether it is a stale fallback.
    pub(super) async fn snapshot(
        &self,
        source: &dyn ControlSource,
        force: bool,
        deadline: Instant,
    ) -> Result<(Arc<Snapshot>, bool), ControlError> {
        let fetch = || fetch_snapshot(source, &self.serials);
        match self.snapshot.get(force, deadline, fetch).await {
            Cached::Value(snapshot) => Ok((snapshot, false)),
            Cached::Stale(snapshot) => Ok((snapshot, true)),
            Cached::Failed(category) => Err(ControlError::failed(category)),
        }
    }

    /// Sensitivity read failures hide the value instead of failing the read.
    async fn sensitivity(
        &self,
        source: &dyn ControlSource,
        alias: &str,
        config: &CameraConfig,
        kind: u8,
        force: bool,
        deadline: Instant,
    ) -> (Option<Sensitivity>, bool) {
        let Some(cell) = self.sensitivity.get(alias) else {
            return (None, false);
        };
        let fetch = move || async move {
            let raw = source.algorithm_config(config).await?;
            Ok(parse_sensitivity(&raw, kind, config.channel))
        };
        match cell.get(force, deadline, fetch).await {
            Cached::Value(value) => (value, false),
            Cached::Stale(value) => (value, true),
            Cached::Failed(_) => (None, false),
        }
    }

    /// Drops every cached read after an unconfirmed or failed write.
    pub(super) async fn invalidate(&self) {
        self.snapshot.invalidate().await;
        self.defence.invalidate().await;
        for cell in self.sensitivity.values() {
            cell.invalidate().await;
        }
    }
}

fn cell<T: Clone>(name: &'static str, ttl: std::time::Duration, metrics: &Metrics) -> TtlCell<T> {
    TtlCell::new(name, ttl, CONTROLS_FAILURE_TTL, STALE_FOR, metrics.clone())
}

/// Reads back up to `VERIFY_DELAYS.len()` times, waiting before each read.
pub(super) async fn confirm<T, F, Fut>(mut read: F, matches: impl Fn(&T) -> bool) -> Confirmation<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ControlError>>,
{
    let mut outcome = Confirmation::Unknown;
    for delay in VERIFY_DELAYS {
        tokio::time::sleep(delay).await;
        match read().await {
            Ok(value) if matches(&value) => return Confirmation::Confirmed(value),
            Ok(_) => outcome = Confirmation::Mismatch,
            Err(_) => {}
        }
    }
    outcome
}

async fn fetch_snapshot(
    source: &dyn ControlSource,
    serials: &BTreeSet<String>,
) -> Result<Arc<Snapshot>, BridgeError> {
    let mut snapshot = Snapshot::new();
    let mut offset = 0_usize;
    for _ in 0..MAX_PAGES {
        let page = source.control_page(offset, PAGE_SIZE).await?;
        merge_page(&mut snapshot, &page, serials);
        if !page_has_next(&page) {
            break;
        }
        offset = offset.saturating_add(PAGE_SIZE);
    }
    Ok(Arc::new(snapshot))
}
