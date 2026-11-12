//! Keys of sealed files.
//!
//! A [`KeyFile`] holds 32 random bytes in an owner-private file (`store.key`
//! next to the sealed files). Each sealed file uses the HKDF-SHA256 sub-key of
//! that master key for its own [`KeyLabel`] (`auth_modules::keys`), so one key
//! file can protect several files without sharing a cipher key between them.
//! [`SealingKey::from_label_and_material`] is the older derivation
//! `SHA-256(label || 0x00 || material)`, kept to open legacy envelopes.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use aes_gcm::aead::{OsRng, rand_core::RngCore};
use aes_gcm::{Aes256Gcm, KeyInit};
use auth_modules::keys::{KeyLabel, MasterKey, SUBKEY_BYTES};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::SealError;
use crate::private_file::{self, ReadPolicy};

/// File name of the master key next to the sealed files it protects.
pub const KEY_FILE_NAME: &str = "store.key";
const MASTER_KEY_BYTES: usize = 32;

/// A 32-byte AES-256-GCM key, wiped on drop.
#[derive(Clone)]
pub struct SealingKey(Zeroizing<[u8; SUBKEY_BYTES]>);

impl SealingKey {
    /// `SHA-256(domain_label || 0x00 || material)`. Blink sealed its store
    /// this way before `store.key` existed (label
    /// `vistoda-blink-provider-store-v1`, material = workload token); a key
    /// derived from a secret that other components also hold is only kept to
    /// open such legacy envelopes.
    #[must_use]
    pub fn from_label_and_material(domain_label: &str, material: &[u8]) -> Self {
        let mut digest = Sha256::new();
        digest.update(domain_label.as_bytes());
        digest.update([0_u8]);
        digest.update(material);
        Self(Zeroizing::new(digest.finalize().into()))
    }

    pub(super) fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new(&(*self.0).into())
    }
}

impl fmt::Debug for SealingKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SealingKey([REDACTED])")
    }
}

/// The sub-key for one label of an owner-private master key file; loaded (or
/// created) once and cached.
#[derive(Clone)]
pub struct KeyFile {
    path: PathBuf,
    label: KeyLabel,
    cached: Arc<Mutex<Option<SealingKey>>>,
}

impl KeyFile {
    #[must_use]
    pub fn new(path: PathBuf, label: KeyLabel) -> Self {
        Self {
            path,
            label,
            cached: Arc::default(),
        }
    }

    /// The [`KEY_FILE_NAME`] file in the directory of `sealed`.
    #[must_use]
    pub fn beside(sealed: &Path, label: KeyLabel) -> Self {
        Self::new(sealed.with_file_name(KEY_FILE_NAME), label)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Key for opening: never creates key material, so a lost key file fails
    /// closed instead of being silently replaced.
    pub fn existing(&self) -> Result<SealingKey, SealError> {
        self.resolve(false)
    }

    /// Key for sealing: creates the master key file on first use.
    pub fn for_sealing(&self) -> Result<SealingKey, SealError> {
        self.resolve(true)
    }

    fn resolve(&self, create: bool) -> Result<SealingKey, SealError> {
        let mut cached = self.cached.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(key) = cached.as_ref() {
            return Ok(key.clone());
        }
        let policy = ReadPolicy::new(MASTER_KEY_BYTES as u64);
        let material = match private_file::read(&self.path, policy) {
            Ok(material) => material,
            Err(error) if create && error.is_not_found() => {
                let mut fresh = Zeroizing::new([0_u8; MASTER_KEY_BYTES]);
                OsRng.fill_bytes(&mut *fresh);
                private_file::create_new(&self.path, &*fresh)?;
                // Derive from what is on disk: a concurrent creator may have
                // won, and a failed write must never seal with a key that
                // only existed in memory.
                private_file::read(&self.path, policy)?
            }
            Err(error) => return Err(error.into()),
        };
        let key = derive(&material, self.label)?;
        *cached = Some(key.clone());
        drop(cached);
        Ok(key)
    }
}

impl fmt::Debug for KeyFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeyFile")
            .field("path", &self.path)
            .field("label", &self.label.as_str())
            .finish_non_exhaustive()
    }
}

fn derive(material: &[u8], label: KeyLabel) -> Result<SealingKey, SealError> {
    if material.len() != MASTER_KEY_BYTES {
        return Err(SealError::Key("store key must be exactly 32 bytes".into()));
    }
    let master = MasterKey::from_bytes(material.to_vec())
        .map_err(|error| SealError::Key(error.to_string()))?;
    Ok(SealingKey(Zeroizing::new(
        *master.derive(label).expose_secret(),
    )))
}
