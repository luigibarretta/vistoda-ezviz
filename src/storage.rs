//! Private state files through the provider kit
//! (`vistoda_provider_kit::private_file`): atomic `0600` JSON writes and
//! checked secret reads, with the bridge's error type.

use std::path::Path;

use serde::Serialize;
use vistoda_provider_kit::private_file::{self, FileError, FileOwner, ReadPolicy};

use crate::error::BridgeError;

/// Operator-provided secrets are small; anything larger is not a secret file.
const MAX_SECRET_BYTES: u64 = 64 * 1024;

/// Atomically replaces `path` with `value` as JSON (mode `0600`, parent
/// directory created `0700` when missing).
pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), BridgeError> {
    private_file::create_parent(path)
        .and_then(|()| private_file::write_json(path, value))
        .map_err(|error| rejected(path, &error))
}

/// Reads a trimmed secret that must be a private regular file of at least
/// `minimum_length` bytes. Any owner is accepted: these files are provisioned
/// by the operator or the add-on runner.
pub fn ensure_private_regular(path: &Path, minimum_length: usize) -> Result<String, BridgeError> {
    let policy = ReadPolicy::new(MAX_SECRET_BYTES)
        .owner(FileOwner::Any)
        .trimmed()
        .min_bytes(minimum_length);
    private_file::read_string(path, policy)
        .map(|value| value.as_str().to_owned())
        .map_err(|error| rejected(path, &error))
}

fn rejected(path: &Path, error: &FileError) -> BridgeError {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<secret>");
    BridgeError::Configuration(format!("private file {name}: {error}"))
}
