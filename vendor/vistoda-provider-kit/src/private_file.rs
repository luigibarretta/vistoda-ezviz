//! Owner-private files: checked reads and atomic writes.
//!
//! Reads go through `auth_modules::secrets::files`, which refuses symlinks,
//! non-regular files, any group/other permission bit, oversized and empty
//! files and (unless the policy says [`FileOwner::Any`]) files owned by
//! another user, and detects a path swapped between its checks. Writes create
//! a fresh `0600` temporary file in the target directory (`create_new`), sync
//! it, rename it over the target and sync the directory.

use std::fs::{self, DirBuilder, File};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

pub use auth_modules::secrets::files::{FileOwner, PrivateFileError};
use auth_modules::secrets::files::{
    PrivateFileOptions, read_private_file_with, write_private_atomic,
};
use serde::Serialize;
use thiserror::Error;
use zeroize::Zeroizing;

#[derive(Debug, Error)]
pub enum FileError {
    #[error("{0}")]
    Private(#[from] PrivateFileError),
    #[error("private file is shorter than {minimum} bytes")]
    TooShort { minimum: usize },
    #[error("private file is not valid UTF-8")]
    NotUtf8,
    #[error("private file could not be encoded: {0}")]
    Encoding(#[from] serde_json::Error),
}

impl FileError {
    /// True when the file (or a directory on its path) does not exist.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Private(PrivateFileError::Io(error)) if error.kind() == io::ErrorKind::NotFound)
    }
}

/// How a private file is read.
#[derive(Clone, Copy, Debug)]
pub struct ReadPolicy {
    max_bytes: u64,
    min_bytes: usize,
    owner: FileOwner,
    trim: bool,
}

impl ReadPolicy {
    /// Files written by this process: owned by its effective UID.
    #[must_use]
    pub const fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            min_bytes: 1,
            owner: FileOwner::CurrentProcess,
            trim: false,
        }
    }

    #[must_use]
    pub const fn min_bytes(mut self, min_bytes: usize) -> Self {
        self.min_bytes = min_bytes;
        self
    }

    /// `FileOwner::Any` for operator-provisioned secrets (for example a
    /// mounted API token) whose owner the process does not control.
    #[must_use]
    pub const fn owner(mut self, owner: FileOwner) -> Self {
        self.owner = owner;
        self
    }

    /// Removes surrounding ASCII whitespace before the length check.
    #[must_use]
    pub const fn trimmed(mut self) -> Self {
        self.trim = true;
        self
    }
}

/// Reads a private file under `policy`.
pub fn read(path: &Path, policy: ReadPolicy) -> Result<Zeroizing<Vec<u8>>, FileError> {
    let mut options = PrivateFileOptions::new(policy.max_bytes).owner(policy.owner);
    if policy.trim {
        options = options.trim_ascii_whitespace();
    }
    let bytes = Zeroizing::new(read_private_file_with(path, &options)?.into_inner());
    if bytes.len() < policy.min_bytes {
        return Err(FileError::TooShort {
            minimum: policy.min_bytes,
        });
    }
    Ok(bytes)
}

/// Like [`read`], `None` when the file does not exist.
pub fn read_optional(
    path: &Path,
    policy: ReadPolicy,
) -> Result<Option<Zeroizing<Vec<u8>>>, FileError> {
    match read(path, policy) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.is_not_found() => Ok(None),
        Err(error) => Err(error),
    }
}

/// Reads a private UTF-8 file (for example an API token).
pub fn read_string(path: &Path, policy: ReadPolicy) -> Result<Zeroizing<String>, FileError> {
    let bytes = read(path, policy)?;
    std::str::from_utf8(&bytes)
        .map(|text| Zeroizing::new(text.to_owned()))
        .map_err(|_| FileError::NotUtf8)
}

/// Atomically replaces `path` with a `0600` file holding `bytes`. The parent
/// directory must exist (see [`create_parent`]).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), FileError> {
    Ok(write_private_atomic(path, bytes)?)
}

/// Creates the missing directories above `path` with mode `0700`; existing
/// directories keep their mode.
pub fn create_parent(path: &Path) -> Result<(), FileError> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent(path))
        .map_err(|error| PrivateFileError::Io(error).into())
}

/// [`write_atomic`] of `value` as JSON followed by a newline.
pub fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), FileError> {
    let mut bytes = Zeroizing::new(serde_json::to_vec(value)?);
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// Publishes `bytes` at `path` only if nothing exists there yet.
///
/// The complete file is written under a temporary name and hard-linked into
/// place, so two racing writers can never replace each other's content.
/// Returns whether this call created the file.
pub fn create_new(path: &Path, bytes: &[u8]) -> Result<bool, FileError> {
    let name = path.file_name().ok_or(PrivateFileError::InvalidPath)?;
    let mut staged_name = std::ffi::OsString::from(".");
    staged_name.push(name);
    staged_name.push(".new");
    let staged = path.with_file_name(staged_name);
    // A leftover staging file from a crash is ours to replace. A concurrent
    // writer may replace or remove it meanwhile; callers read the published
    // file back, so they always use what is on disk.
    write_atomic(&staged, bytes)?;
    let linked = fs::hard_link(&staged, path);
    let _ignored = fs::remove_file(&staged);
    match linked {
        Ok(()) => {
            sync_parent(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(PrivateFileError::Io(error).into()),
    }
}

/// Removes `path` (absent is fine) and syncs its directory.
pub fn remove(path: &Path) -> Result<(), FileError> {
    match fs::remove_file(path) {
        Ok(()) => sync_parent(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PrivateFileError::Io(error).into()),
    }
}

/// Renames `from` over `to` and syncs the directory of `to`.
pub fn rename(from: &Path, to: &Path) -> Result<(), FileError> {
    fs::rename(from, to).map_err(PrivateFileError::Io)?;
    sync_parent(to)
}

fn parent(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn sync_parent(path: &Path) -> Result<(), FileError> {
    File::open(parent(path))
        .and_then(|directory| directory.sync_all())
        .map_err(|error| PrivateFileError::Io(error).into())
}
