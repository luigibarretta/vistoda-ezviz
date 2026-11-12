//! AES-256-GCM sealed files.
//!
//! A sealed file is the JSON envelope `{"version", "nonce", "ciphertext"}`
//! (standard base64, 96-bit random nonce, no associated data), written
//! atomically as an owner-private file. Version [`VERSION_KEY_FILE`] is sealed
//! with the [`KeyFile`] sub-key; version [`VERSION_LEGACY_KEY`] is only opened
//! (with the key given to [`SealedFile::with_legacy_key`]) and re-sealed.
//!
//! [`SealedFile::migrating_plaintext`] adopts a provider's former plaintext
//! file: it is sealed, read back and only then removed, so an enrolled session
//! is never lost. A plaintext file always wins over an existing sealed one: it
//! is either newer (an earlier release ran after a downgrade and rotated the
//! session) or identical (a crash between sealing and removal).
//!
//! The key file lives next to the sealed data, so the sealing protects against
//! a partial read of the data directory and against holders of the provider
//! API token; anyone who can read the whole directory (for example a full
//! backup) can still open it.

mod envelope;
mod key;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use envelope::{decrypt, parse};
pub use envelope::{open, seal};
pub use key::{KEY_FILE_NAME, KeyFile, SealingKey};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::private_file::{self, FileError, PrivateFileError, ReadPolicy};

/// Envelope sealed with the sub-key of a [`KeyFile`].
pub const VERSION_KEY_FILE: u8 = 2;
/// Envelope sealed with a legacy key; opened once and re-sealed.
pub const VERSION_LEGACY_KEY: u8 = 1;
const DEFAULT_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Error)]
pub enum SealError {
    #[error("{0}")]
    File(#[from] FileError),
    #[error("sealed file encoding is invalid")]
    Encoding,
    #[error("sealed file envelope version {0} is not supported")]
    UnsupportedVersion(u8),
    #[error("sealed file authentication failed")]
    Authentication,
    #[error("sealed file key is unusable: {0}")]
    Key(String),
    /// The sealed file exists but its key file does not: never "not
    /// enrolled", since a new enrollment would silently replace the data.
    #[error("sealed file exists but its key file {} is missing", .0.display())]
    MissingKey(PathBuf),
}

/// What [`SealedFile::migrate`] did with the former plaintext file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Migration {
    Nothing,
    /// The plaintext was sealed; no sealed file existed.
    Sealed,
    /// The plaintext replaced an existing sealed file.
    Replaced,
}

/// Envelope version and plaintext of an opened file.
type Opened = (u8, Zeroizing<Vec<u8>>);

/// One sealed file, its key and its optional migration sources.
#[derive(Clone, Debug)]
pub struct SealedFile {
    path: PathBuf,
    key: KeyFile,
    legacy_key: Option<SealingKey>,
    plaintext: Option<PathBuf>,
    max_bytes: u64,
}

impl SealedFile {
    #[must_use]
    pub const fn new(path: PathBuf, key: KeyFile) -> Self {
        Self {
            path,
            key,
            legacy_key: None,
            plaintext: None,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// Also opens (and re-seals) [`VERSION_LEGACY_KEY`] envelopes.
    #[must_use]
    pub fn with_legacy_key(mut self, key: SealingKey) -> Self {
        self.legacy_key = Some(key);
        self
    }

    /// Seals the former plaintext file `legacy` on first use (see module docs).
    #[must_use]
    pub fn migrating_plaintext(mut self, legacy: PathBuf) -> Self {
        self.plaintext = Some(legacy);
        self
    }

    /// Size limit for the sealed file and the plaintext it migrates.
    #[must_use]
    pub const fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the sealed file exists (after any pending migration).
    pub fn exists(&self) -> Result<bool, SealError> {
        self.migrate()?;
        present(&self.path)
    }

    /// The plaintext, `None` when nothing is stored.
    pub fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SealError> {
        self.migrate()?;
        let Some((version, plaintext)) = self.read_sealed()? else {
            return Ok(None);
        };
        if version == VERSION_LEGACY_KEY {
            // Until this succeeds the legacy envelope stays and is retried on
            // the next load; the plaintext is usable either way.
            let path = self.path.display();
            match self.save(&plaintext) {
                Ok(()) => tracing::info!(%path, "legacy envelope re-sealed"),
                Err(error) => tracing::warn!(%error, %path, "legacy envelope not re-sealed yet"),
            }
        }
        Ok(Some(plaintext))
    }

    /// Seals `plaintext` and atomically replaces the file.
    pub fn save(&self, plaintext: &[u8]) -> Result<(), SealError> {
        let bytes = seal(&self.key.for_sealing()?, VERSION_KEY_FILE, plaintext)?;
        Ok(private_file::write_atomic(&self.path, &bytes)?)
    }

    /// Removes the sealed file (not the key file); absent is fine.
    pub fn remove(&self) -> Result<(), SealError> {
        Ok(private_file::remove(&self.path)?)
    }

    /// Seals the former plaintext file, if one is configured and present,
    /// over any existing sealed file (see the module docs).
    pub fn migrate(&self) -> Result<Migration, SealError> {
        let Some(legacy) = self.plaintext.as_deref() else {
            return Ok(Migration::Nothing);
        };
        let plaintext = match private_file::read(legacy, ReadPolicy::new(self.max_bytes)) {
            Ok(plaintext) => plaintext,
            // Absent, or a concurrent migration finished first.
            Err(error) if error.is_not_found() => return Ok(Migration::Nothing),
            Err(error) => return Err(error.into()),
        };
        let replaced = present(&self.path)?;
        self.save(&plaintext)?;
        match self.read_sealed()? {
            Some((_, sealed)) if *sealed == *plaintext => {}
            _ => return Err(SealError::Encoding),
        }
        private_file::remove(legacy)?;
        tracing::info!(path = %self.path.display(), replaced, "plaintext file sealed and removed");
        Ok(if replaced {
            Migration::Replaced
        } else {
            Migration::Sealed
        })
    }

    fn read_sealed(&self) -> Result<Option<Opened>, SealError> {
        let policy = ReadPolicy::new(self.max_bytes);
        let Some(raw) = private_file::read_optional(&self.path, policy)? else {
            return Ok(None);
        };
        let envelope = parse(&raw)?;
        let key = match (envelope.version, &self.legacy_key) {
            (VERSION_KEY_FILE, _) => self.key.existing().map_err(|error| match error {
                SealError::File(file) if file.is_not_found() => {
                    SealError::MissingKey(self.key.path().to_path_buf())
                }
                other => other,
            })?,
            (VERSION_LEGACY_KEY, Some(legacy)) => legacy.clone(),
            (version, _) => return Err(SealError::UnsupportedVersion(version)),
        };
        Ok(Some((envelope.version, decrypt(&key, &envelope)?)))
    }
}

fn present(path: &Path) -> Result<bool, SealError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(FileError::Private(PrivateFileError::Io(error)).into()),
    }
}
