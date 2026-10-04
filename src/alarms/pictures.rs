//! Private on-disk JPEG storage for alarm pictures, keyed by alarm ID.

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

use uuid::Uuid;

use super::model::is_token;
use crate::error::BridgeError;

/// Upper bound for one stored JPEG.
pub const MAX_PICTURE_BYTES: usize = 4 * 1024 * 1024;

pub struct PictureDir {
    path: PathBuf,
}

impl PictureDir {
    pub const fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn file(&self, id: &str) -> Option<PathBuf> {
        is_token(id).then(|| self.path.join(format!("{id}.jpg")))
    }

    /// Atomically publishes a validated JPEG and returns its size.
    pub fn write(&self, id: &str, jpeg: &[u8]) -> Result<u64, BridgeError> {
        let target = self
            .file(id)
            .ok_or_else(|| BridgeError::Upstream("alarm ID is not a safe token".into()))?;
        if jpeg.len() > MAX_PICTURE_BYTES || !jpeg.starts_with(&[0xff, 0xd8, 0xff]) {
            return Err(BridgeError::Upstream(
                "alarm picture is not a bounded JPEG".into(),
            ));
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.path)?;
        let temporary = self.path.join(format!(".{id}.{}", Uuid::new_v4()));
        let result = publish(&temporary, &target, jpeg, &self.path);
        if result.is_err() {
            let _ignored = fs::remove_file(&temporary);
        }
        result?;
        Ok(jpeg.len() as u64)
    }

    pub async fn read(&self, id: &str) -> Option<Vec<u8>> {
        let path = self.file(id)?;
        let data = tokio::fs::read(path).await.ok()?;
        (data.len() <= MAX_PICTURE_BYTES && data.starts_with(&[0xff, 0xd8, 0xff])).then_some(data)
    }

    pub fn exists(&self, id: &str) -> bool {
        self.file(id).is_some_and(|path| path.is_file())
    }

    pub fn remove(&self, id: &str) {
        if let Some(path) = self.file(id) {
            let _ignored = fs::remove_file(path);
        }
    }

    /// Deletes files that no longer belong to a retained alarm.
    pub fn remove_orphans(&self, keep: &BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(&self.path) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let retained = name
                .strip_suffix(".jpg")
                .is_some_and(|id| keep.contains(id));
            if !retained && entry.file_type().is_ok_and(|kind| kind.is_file()) {
                let _ignored = fs::remove_file(entry.path());
            }
        }
    }
}

fn publish(temporary: &Path, target: &Path, jpeg: &[u8], parent: &Path) -> Result<(), BridgeError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temporary)?;
    file.write_all(jpeg)?;
    file.sync_all()?;
    fs::rename(temporary, target)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
