mod job;
mod model;
mod recovery;

pub use model::RecordingManifest;

use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use tokio::{sync::Mutex, task::JoinSet};

use crate::{error::BridgeError, hub::RawStreamHub, metrics::Metrics, storage::atomic_write_json};

const MAX_ACK_TOMBSTONES: usize = 4_096;

#[derive(Clone, Deserialize, Serialize)]
struct AckTombstone {
    recording_id: String,
    idempotency_key: Option<String>,
    acknowledged_at: String,
}

#[derive(Default, Deserialize, Serialize)]
struct Journal {
    #[serde(default = "schema_version")]
    schema_version: u8,
    #[serde(default)]
    recordings: Vec<RecordingManifest>,
    #[serde(default)]
    idempotency: BTreeMap<String, String>,
    #[serde(default)]
    acknowledgements: VecDeque<AckTombstone>,
}

#[derive(Default)]
pub(super) struct State {
    manifests: BTreeMap<String, RecordingManifest>,
    idempotency: BTreeMap<String, String>,
    acknowledgements: VecDeque<AckTombstone>,
}

pub struct RecordingManager {
    pub(super) directory: PathBuf,
    journal: PathBuf,
    pub(super) hubs: BTreeMap<String, Arc<RawStreamHub>>,
    pub(super) metrics: Metrics,
    max_duration: u64,
    pub(super) max_bytes: u64,
    quota_bytes: u64,
    pub(super) state: Mutex<State>,
    tasks: Mutex<JoinSet<()>>,
}

impl RecordingManager {
    pub fn load(
        directory: PathBuf,
        hubs: BTreeMap<String, Arc<RawStreamHub>>,
        metrics: Metrics,
        max_duration: u64,
        max_bytes: u64,
        quota_bytes: u64,
    ) -> Result<Arc<Self>, BridgeError> {
        fs::create_dir_all(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let journal_path = directory.join("recordings.json");
        let mut state = recovery::load_state(&journal_path)?;
        recovery::recover(&directory, &mut state)?;
        let manager = Arc::new(Self {
            directory,
            journal: journal_path,
            hubs,
            metrics,
            max_duration,
            max_bytes,
            quota_bytes,
            state: Mutex::new(state),
            tasks: Mutex::new(JoinSet::new()),
        });
        manager.persist_initial()?;
        Ok(manager)
    }

    pub async fn start(
        self: &Arc<Self>,
        camera: &str,
        duration_seconds: u64,
        idempotency_key: &str,
    ) -> Result<RecordingManifest, BridgeError> {
        if !self.hubs.contains_key(camera) {
            return Err(BridgeError::Recording(
                "camera alias is not configured".into(),
            ));
        }
        if !(1..=self.max_duration).contains(&duration_seconds) {
            return Err(BridgeError::Recording(format!(
                "duration must be between 1 and {} seconds",
                self.max_duration
            )));
        }
        if !(8..=128).contains(&idempotency_key.len()) {
            return Err(BridgeError::Recording(
                "Idempotency-Key must contain 8 to 128 characters".into(),
            ));
        }
        let mut state = self.state.lock().await;
        if state
            .acknowledgements
            .iter()
            .any(|item| item.idempotency_key.as_deref() == Some(idempotency_key))
        {
            return Err(BridgeError::Recording(
                "Idempotency-Key belongs to an acknowledged recording".into(),
            ));
        }
        if let Some(existing_id) = state.idempotency.get(idempotency_key) {
            let existing = state.manifests.get(existing_id).ok_or_else(|| {
                BridgeError::Recording("recording journal is inconsistent".into())
            })?;
            if existing.camera != camera || existing.requested_duration_seconds != duration_seconds
            {
                return Err(BridgeError::Recording(
                    "Idempotency-Key was reused with a different request".into(),
                ));
            }
            return Ok(existing.clone());
        }
        if recovery::spool_bytes(&self.directory)? + self.max_bytes > self.quota_bytes {
            return Err(BridgeError::Capacity(
                "recording quota does not have safe headroom".into(),
            ));
        }
        let manifest = RecordingManifest::pending(camera, duration_seconds);
        state
            .idempotency
            .insert(idempotency_key.to_owned(), manifest.recording_id.clone());
        state
            .manifests
            .insert(manifest.recording_id.clone(), manifest.clone());
        self.persist(&state)?;
        drop(state);
        let manager = Arc::clone(self);
        let id = manifest.recording_id.clone();
        self.tasks
            .lock()
            .await
            .spawn(async move { manager.record(&id).await });
        Ok(manifest)
    }

    pub async fn get(&self, id: &str) -> Option<RecordingManifest> {
        self.state.lock().await.manifests.get(id).cloned()
    }

    pub async fn list(&self) -> Vec<RecordingManifest> {
        let mut values: Vec<_> = self
            .state
            .lock()
            .await
            .manifests
            .values()
            .cloned()
            .collect();
        values.sort_by(|left, right| right.requested_at.cmp(&left.requested_at));
        values
    }

    pub async fn media_path(&self, id: &str) -> Option<PathBuf> {
        let state = self.state.lock().await;
        let manifest = state.manifests.get(id)?;
        if manifest.status != "ready" {
            return None;
        }
        let path = self.directory.join(format!("{id}.mpegps"));
        path.is_file().then_some(path)
    }

    pub async fn acknowledge(&self, id: &str) -> Result<bool, BridgeError> {
        let mut state = self.state.lock().await;
        let Some(manifest) = state.manifests.get(id).cloned() else {
            return Ok(false);
        };
        if matches!(manifest.status.as_str(), "pending" | "recording") {
            return Err(BridgeError::RecordingActive);
        }
        let key = state
            .idempotency
            .iter()
            .find_map(|(key, value)| (value == id).then(|| key.clone()));
        if manifest.status == "ready" {
            let path = self.directory.join(format!("{id}.mpegps"));
            match fs::remove_file(path) {
                Ok(()) => fs::File::open(&self.directory)?.sync_all()?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        state.manifests.remove(id);
        state.idempotency.retain(|_, value| value != id);
        state.acknowledgements.push_back(AckTombstone {
            recording_id: id.to_owned(),
            idempotency_key: key,
            acknowledged_at: model::utc_now(),
        });
        while state.acknowledgements.len() > MAX_ACK_TOMBSTONES {
            state.acknowledgements.pop_front();
        }
        self.persist(&state)?;
        Ok(true)
    }

    pub async fn close(&self) {
        self.tasks.lock().await.abort_all();
        while self.tasks.lock().await.join_next().await.is_some() {}
    }

    fn persist_initial(&self) -> Result<(), BridgeError> {
        let state = self
            .state
            .try_lock()
            .map_err(|_| BridgeError::Recording("state lock failed".into()))?;
        self.persist(&state)
    }

    pub(super) fn persist(&self, state: &State) -> Result<(), BridgeError> {
        let journal = Journal {
            schema_version: schema_version(),
            recordings: state.manifests.values().cloned().collect(),
            idempotency: state.idempotency.clone(),
            acknowledgements: state.acknowledgements.clone(),
        };
        atomic_write_json(&self.journal, &journal)
    }
}

const fn schema_version() -> u8 {
    2
}
