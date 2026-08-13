use std::{fs, io::Read, path::Path};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{State, model::utc_now};
use crate::error::BridgeError;

const PACK_START: &[u8] = b"\x00\x00\x01\xba";

pub(super) fn recover(directory: &Path, state: &mut State) -> Result<(), BridgeError> {
    for manifest in state.manifests.values_mut() {
        let path = directory.join(format!("{}.mpegps", manifest.recording_id));
        if matches!(manifest.status.as_str(), "pending" | "recording") {
            match inspect_media(&path) {
                Ok(Some((bytes, digest))) => {
                    manifest.status = "ready".into();
                    manifest
                        .started_at
                        .get_or_insert_with(|| manifest.requested_at.clone());
                    manifest.completed_at = Some(utc_now());
                    manifest
                        .actual_duration_seconds
                        .get_or_insert(manifest.requested_duration_seconds as f64);
                    manifest.bytes = Some(bytes);
                    manifest.sha256 = Some(digest);
                    manifest.error_code = None;
                }
                Ok(None) => manifest.fail("interrupted"),
                Err(_) => manifest.fail("invalid_media"),
            }
        } else if manifest.status == "ready" && !path.is_file() {
            manifest.fail("missing_media");
            manifest.bytes = None;
            manifest.sha256 = None;
        }
    }
    remove_partial_files(directory)?;
    Ok(())
}

fn inspect_media(path: &Path) -> Result<Option<(u64, String)>, BridgeError> {
    if !path.is_file() {
        return Ok(None);
    }
    let mut file = fs::File::open(path)?;
    let mut prefix = [0_u8; 4];
    if file.read_exact(&mut prefix).is_err() || prefix != PACK_START {
        return Err(BridgeError::Recording(
            "recovered media has no MPEG-PS pack header".into(),
        ));
    }
    let mut digest = Sha256::new();
    digest.update(prefix);
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(Some((
        file.metadata()?.len(),
        hex::encode(digest.finalize()),
    )))
}

fn remove_partial_files(directory: &Path) -> Result<(), BridgeError> {
    let mut changed = false;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(id) = name
            .strip_prefix('.')
            .and_then(|name| name.strip_suffix(".partial"))
        else {
            continue;
        };
        if Uuid::parse_str(id).is_ok() && entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
            changed = true;
        }
    }
    if changed {
        fs::File::open(directory)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};

    use super::*;
    use crate::recordings::{RecordingManifest, State};

    fn state(manifest: RecordingManifest) -> State {
        State {
            manifests: BTreeMap::from([(manifest.recording_id.clone(), manifest)]),
            idempotency: BTreeMap::new(),
            acknowledgements: VecDeque::new(),
        }
    }

    #[test]
    fn complete_published_media_is_promoted_after_crash() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let manifest = RecordingManifest::pending("front", 5);
        let id = manifest.recording_id.clone();
        fs::write(
            directory.path().join(format!("{id}.mpegps")),
            b"\x00\x00\x01\xba-recovered",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let mut state = state(manifest);
        recover(directory.path(), &mut state).unwrap_or_else(|error| panic!("{error}"));
        let recovered = state
            .manifests
            .get(&id)
            .unwrap_or_else(|| panic!("missing"));
        assert_eq!(recovered.status, "ready");
        assert_eq!(recovered.bytes, Some(14));
        assert!(recovered.sha256.is_some());
    }

    #[test]
    fn interrupted_and_partial_outputs_are_cleaned() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let manifest = RecordingManifest::pending("front", 5);
        let id = manifest.recording_id.clone();
        let partial = directory.path().join(format!(".{id}.partial"));
        fs::write(&partial, b"incomplete").unwrap_or_else(|error| panic!("{error}"));
        let mut state = state(manifest);
        recover(directory.path(), &mut state).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(state.manifests[&id].status, "failed");
        assert_eq!(
            state.manifests[&id].error_code.as_deref(),
            Some("interrupted")
        );
        assert!(!partial.exists());
    }
}
