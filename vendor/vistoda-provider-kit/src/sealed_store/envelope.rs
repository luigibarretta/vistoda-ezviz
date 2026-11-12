//! The sealed-file envelope `{"version", "nonce", "ciphertext"}`.

use aes_gcm::aead::{Aead, AeadCore, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{SealError, SealingKey};

#[derive(Deserialize, Serialize)]
pub(super) struct Envelope {
    pub(super) version: u8,
    nonce: String,
    ciphertext: String,
}

/// Seals `plaintext` into an envelope of `version`.
pub fn seal(key: &SealingKey, version: u8, plaintext: &[u8]) -> Result<Vec<u8>, SealError> {
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = key
        .cipher()
        .encrypt(&nonce, plaintext)
        .map_err(|_| SealError::Authentication)?;
    serde_json::to_vec(&Envelope {
        version,
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    })
    .map_err(|_| SealError::Encoding)
}

/// Opens an envelope with `key` whatever its version; returns the version too.
pub fn open(key: &SealingKey, envelope: &[u8]) -> Result<(u8, Zeroizing<Vec<u8>>), SealError> {
    let envelope = parse(envelope)?;
    Ok((envelope.version, decrypt(key, &envelope)?))
}

pub(super) fn parse(bytes: &[u8]) -> Result<Envelope, SealError> {
    serde_json::from_slice(bytes).map_err(|_| SealError::Encoding)
}

pub(super) fn decrypt(
    key: &SealingKey,
    envelope: &Envelope,
) -> Result<Zeroizing<Vec<u8>>, SealError> {
    let nonce: [u8; 12] = STANDARD
        .decode(&envelope.nonce)
        .map_err(|_| SealError::Encoding)?
        .try_into()
        .map_err(|_| SealError::Encoding)?;
    let ciphertext = STANDARD
        .decode(&envelope.ciphertext)
        .map_err(|_| SealError::Encoding)?;
    key.cipher()
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
        .map(Zeroizing::new)
        .map_err(|_| SealError::Authentication)
}
