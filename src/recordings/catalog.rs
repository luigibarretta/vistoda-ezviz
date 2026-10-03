//! Read-side manifest queries and consumer acknowledgement for the recording manager.

use std::{fs, path::PathBuf};

use super::{
    AckTombstone, MAX_ACK_TOMBSTONES, RecordingManager, RecordingManifest, model, recovery,
};
use crate::error::BridgeError;

impl RecordingManager {
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

    pub fn storage_descriptor(&self) -> Result<serde_json::Value, BridgeError> {
        let used_bytes = recovery::spool_bytes(&self.directory)?;
        Ok(serde_json::json!({
            "directory": self.directory.to_string_lossy(),
            "scope": "addon_private",
            "used_bytes": used_bytes,
            "quota_bytes": self.quota_bytes,
            "available_bytes": self.quota_bytes.saturating_sub(used_bytes)
        }))
    }

    pub async fn media_path(&self, id: &str) -> Option<PathBuf> {
        self.media(id).await.map(|(path, _)| path)
    }

    pub async fn media(&self, id: &str) -> Option<(PathBuf, String)> {
        let state = self.state.lock().await;
        let manifest = state.manifests.get(id)?;
        if manifest.status != "ready" {
            return None;
        }
        let path = recovery::media_path(&self.directory, manifest);
        path.is_file().then(|| (path, manifest.media_type.clone()))
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
            let path = recovery::media_path(&self.directory, &manifest);
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
}
