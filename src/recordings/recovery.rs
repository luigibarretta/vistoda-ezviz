use std::{fs, io::Read, path::Path};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{Journal, RecordingManifest, State, model::utc_now};
use crate::error::BridgeError;

const PACK_START: &[u8] = b"\x00\x00\x01\xba";
const TS_PACKET_BYTES: usize = 188;

pub(super) fn spool_bytes(directory: &Path) -> Result<u64, BridgeError> {
    let mut total = 0_u64;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_some_and(|ext| matches!(ext.to_str(), Some("mpegps" | "ts")))
        {
            total = total.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(total)
}

pub(super) fn load_state(path: &Path) -> Result<State, BridgeError> {
    if !path.exists() {
        return Ok(State::default());
    }
    let journal: Journal = serde_json::from_slice(&fs::read(path)?)
        .map_err(|_| BridgeError::Recording("recording journal is invalid".into()))?;
    Ok(State {
        manifests: journal
            .recordings
            .into_iter()
            .map(|item| (item.recording_id.clone(), item))
            .collect(),
        idempotency: journal.idempotency,
        acknowledgements: journal.acknowledgements,
    })
}

pub(super) fn recover(directory: &Path, state: &mut State) -> Result<(), BridgeError> {
    for manifest in state.manifests.values_mut() {
        let path = media_path(directory, manifest);
        if matches!(manifest.status.as_str(), "pending" | "recording") {
            match inspect_media(&path) {
                Ok(Some((bytes, digest))) => {
                    manifest.status = "ready".into();
                    manifest.media_type = if path.extension().is_some_and(|value| value == "ts") {
                        "video/mp2t".into()
                    } else {
                        "video/mpeg".into()
                    };
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
    let mut prefix = [0_u8; TS_PACKET_BYTES * 3];
    let read = file.read(&mut prefix)?;
    let valid_ps = read >= PACK_START.len() && prefix.starts_with(PACK_START);
    let valid_transport_stream = read >= TS_PACKET_BYTES * 3
        && prefix[0] == 0x47
        && prefix[TS_PACKET_BYTES] == 0x47
        && prefix[TS_PACKET_BYTES * 2] == 0x47;
    if !valid_ps && !valid_transport_stream {
        return Err(BridgeError::Recording(
            "recovered media has no MPEG-PS or MPEG-TS header".into(),
        ));
    }
    let mut digest = Sha256::new();
    digest.update(&prefix[..read]);
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

pub(super) fn media_path(directory: &Path, manifest: &RecordingManifest) -> std::path::PathBuf {
    let preferred = if manifest.media_type == "video/mp2t" {
        "ts"
    } else {
        "mpegps"
    };
    let preferred = directory.join(format!("{}.{}", manifest.recording_id, preferred));
    if preferred.is_file() {
        return preferred;
    }
    let fallback_extension = if manifest.media_type == "video/mp2t" {
        "mpegps"
    } else {
        "ts"
    };
    let fallback = directory.join(format!("{}.{}", manifest.recording_id, fallback_extension));
    if fallback.is_file() {
        fallback
    } else {
        preferred
    }
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
        let mut manifest = RecordingManifest::pending("front", 5);
        manifest.media_type = "video/mpeg".into();
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
