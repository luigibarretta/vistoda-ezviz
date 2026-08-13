use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use serde::Serialize;
use uuid::Uuid;

use crate::error::BridgeError;

pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), BridgeError> {
    let parent = path
        .parent()
        .ok_or_else(|| BridgeError::Configuration("state path has no parent".into()))?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let temporary = temporary_path(path)?;
    let result = write_and_publish(&temporary, path, value);
    if result.is_err() {
        let _ignored = fs::remove_file(&temporary);
    }
    result
}

fn write_and_publish<T: Serialize>(
    temporary: &Path,
    path: &Path,
    value: &T,
) -> Result<(), BridgeError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temporary)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    File::open(
        path.parent()
            .ok_or_else(|| BridgeError::Configuration("state path has no parent".into()))?,
    )?
    .sync_all()?;
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf, BridgeError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| BridgeError::Configuration("state filename is invalid".into()))?;
    Ok(path.with_file_name(format!(".{name}.{}", Uuid::new_v4())))
}

pub fn ensure_private_regular(path: &Path, minimum_length: usize) -> Result<String, BridgeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        BridgeError::Configuration(format!(
            "secret file is unavailable: {}",
            display_name(path)
        ))
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(BridgeError::Configuration(format!(
            "secret path is not a regular file: {}",
            display_name(path)
        )));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(BridgeError::Configuration(format!(
            "secret file permissions are too broad: {}",
            display_name(path)
        )));
    }
    let value = fs::read_to_string(path)?.trim().to_owned();
    if value.len() < minimum_length {
        return Err(BridgeError::Configuration(format!(
            "secret file is empty or too short: {}",
            display_name(path)
        )));
    }
    Ok(value)
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<secret>")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_json_is_private_and_complete() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("state.json");
        atomic_write_json(&path, &serde_json::json!({"complete": true}))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            fs::read_to_string(&path).unwrap_or_default(),
            "{\"complete\":true}\n"
        );
        let mode = fs::metadata(path)
            .unwrap_or_else(|error| panic!("{error}"))
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn atomic_json_preserves_existing_directory_mode() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o750))
            .unwrap_or_else(|error| panic!("{error}"));
        atomic_write_json(&directory.path().join("receipt.json"), &true)
            .unwrap_or_else(|error| panic!("{error}"));
        let mode = fs::metadata(directory.path())
            .unwrap_or_else(|error| panic!("{error}"))
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o750);
    }

    #[test]
    fn secret_reader_rejects_short_or_broad_files() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("token");
        fs::write(&path, "short").unwrap_or_else(|error| panic!("{error}"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(ensure_private_regular(&path, 32).is_err());
        fs::write(&path, "x".repeat(32)).unwrap_or_else(|error| panic!("{error}"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(ensure_private_regular(&path, 32).is_err());
    }
}
