use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{
    error::BridgeError,
    media_format::{MediaFormat, PROBE_BYTES},
    scenetrove::PullResult,
};

pub fn receipt_path(destination: &Path, key: &str) -> PathBuf {
    destination.join(format!(
        ".ezviz-ack-{}.json",
        hex::encode(Sha256::digest(key.as_bytes()))
    ))
}

pub fn recover_receipt(path: &Path, destination: &Path) -> Result<Option<PullResult>, BridgeError> {
    if !path.exists() {
        return Ok(None);
    }
    let result: PullResult = serde_json::from_slice(&fs::read(path)?)?;
    verify_committed(&result, destination)?;
    Ok(Some(result))
}

fn verify_committed(result: &PullResult, destination: &Path) -> Result<(), BridgeError> {
    if result.path.parent() != Some(destination) {
        return Err(invalid_receipt());
    }
    let metadata = fs::symlink_metadata(&result.path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(invalid_receipt());
    }
    let mut file = fs::File::open(&result.path)?;
    let mut prefix = [0_u8; PROBE_BYTES];
    let prefix_bytes = file.read(&mut prefix)?;
    if MediaFormat::detect(&prefix[..prefix_bytes]).is_none() {
        return Err(invalid_receipt());
    }
    let mut digest = Sha256::new();
    digest.update(&prefix[..prefix_bytes]);
    let mut bytes = prefix_bytes as u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes.saturating_add(read as u64);
        digest.update(&buffer[..read]);
    }
    if bytes != result.bytes || hex::encode(digest.finalize()) != result.sha256 {
        return Err(invalid_receipt());
    }
    Ok(())
}

fn invalid_receipt() -> BridgeError {
    BridgeError::Recording("ACK receipt does not match committed local media".into())
}

pub async fn remove_receipt(receipt: &Path, directory: &Path) -> Result<(), BridgeError> {
    match tokio::fs::remove_file(receipt).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}
