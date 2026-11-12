//! The EZVIZ token file, sealed with the provider kit.
//!
//! The configured path (`/data/token.json` by default) used to hold the EZVIZ
//! session in plaintext. The session now lives in the sibling `token.sealed`
//! (AES-256-GCM under an HKDF sub-key of `store.key` in the same directory,
//! `vistoda_provider_kit::sealed_store`); a plaintext file left by an earlier
//! release is sealed on first use, read back and only then removed. The
//! configured path keeps naming the plaintext file, so deployments and the
//! enrollment CLI are unchanged.

use std::path::{Path, PathBuf};

use vistoda_provider_kit::{
    KeyLabel, private_file,
    sealed_store::{KeyFile, SealError, SealedFile},
};

use crate::{error::BridgeError, transport::EzvizToken};

const LABEL: KeyLabel = KeyLabel::new("vistoda-ezviz/ezviz-token/v1");
const MAX_BYTES: u64 = 256 * 1024;

/// The sealed file that replaces the plaintext file at `path`.
#[must_use]
pub fn sealed_path(path: &Path) -> PathBuf {
    path.with_extension("sealed")
}

fn store(path: &Path) -> SealedFile {
    let sealed = sealed_path(path);
    SealedFile::new(sealed.clone(), KeyFile::beside(&sealed, LABEL))
        .migrating_plaintext(path.to_path_buf())
        .with_max_bytes(MAX_BYTES)
}

fn rejected(error: &SealError) -> BridgeError {
    BridgeError::Configuration(format!("EZVIZ token store: {error}"))
}

/// Whether an enrolled session is stored (after any pending migration).
pub fn exists(path: &Path) -> Result<bool, BridgeError> {
    store(path).exists().map_err(|error| rejected(&error))
}

pub fn load(path: &Path) -> Result<EzvizToken, BridgeError> {
    let plaintext = store(path)
        .load()
        .map_err(|error| rejected(&error))?
        .ok_or_else(|| BridgeError::Configuration("EZVIZ token file is unavailable".into()))?;
    let token: EzvizToken = serde_json::from_slice(&plaintext)
        .map_err(|_| BridgeError::Configuration("EZVIZ token file is invalid".into()))?;
    token.validate()?;
    Ok(token)
}

/// Seals `token`; like the plaintext file before it, a missing directory is
/// created (mode `0700`), for example `./data` of a first CLI enrollment.
pub fn save(path: &Path, token: &EzvizToken) -> Result<(), BridgeError> {
    let plaintext = zeroize::Zeroizing::new(serde_json::to_vec(token)?);
    private_file::create_parent(path)
        .map_err(SealError::from)
        .and_then(|()| store(path).save(&plaintext))
        .map_err(|error| rejected(&error))
}

/// Moves a session enrolled at `staged` to `path` and returns it.
pub fn adopt(staged: &Path, path: &Path) -> Result<EzvizToken, BridgeError> {
    let token = load(staged)?;
    private_file::rename(&sealed_path(staged), &sealed_path(path))
        .map_err(|error| BridgeError::Configuration(format!("EZVIZ token store: {error}")))?;
    Ok(token)
}
