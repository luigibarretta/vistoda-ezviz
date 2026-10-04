//! Video verification-code resolution for encrypted cameras.
//!
//! The 7.6.1 app validates a typed verification code with
//! `MD5Util.getTwiceMD5String(code)` against `STATUS.encryptPwd`
//! (`MessageDecryptManager`), and its "device verification code" screen shows
//! the value returned by `/api/device/query/encryptkey`
//! (`GetDeviceEncryptKeyTask`). Vistoda therefore accepts either source only
//! when it matches that hash; without a hash the RTP decryptor still fails
//! closed on the first parameter set.

use md5::{Digest as _, Md5};
use serde::Serialize;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use super::{DeviceSource, model::DeviceStatus};
use crate::{config::CameraConfig, storage::ensure_private_regular};

/// Where the usable video key came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeySource {
    Option,
    Cloud,
    None,
}

/// A resolved verification code. It has no `Debug` and zeroizes on drop.
pub struct VideoKey {
    pub source: KeySource,
    code: Zeroizing<String>,
}

impl VideoKey {
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }
}

/// Lower-case hex `MD5(hex(MD5(code)))`, as stored in `STATUS.encryptPwd`.
#[must_use]
pub fn twice_md5(code: &str) -> String {
    let first = hex::encode(Md5::digest(code.as_bytes()));
    hex::encode(Md5::digest(first.as_bytes()))
}

/// Checks a candidate against the optional cloud hash; `None` cannot reject.
#[must_use]
pub fn matches_hash(code: &str, hash: Option<&str>) -> bool {
    let valid_length = !code.is_empty() && code.len() <= 64;
    hash.map_or(valid_length, |expected| {
        valid_length
            && bool::from(
                twice_md5(code)
                    .as_bytes()
                    .ct_eq(expected.to_ascii_lowercase().as_bytes()),
            )
    })
}

/// Ordered usable verification codes, option first.
///
/// With a cloud `encryptPwd` hash only matching codes are returned (the cloud
/// copy only when the option does not match). Without a hash both the option
/// code and the cloud copy are returned, so callers can try each; image
/// headers and the RTP parameter set reject a wrong one. Neither the code nor
/// the serial is ever logged.
pub async fn candidate_keys(
    source: &dyn DeviceSource,
    camera: &CameraConfig,
    status: Option<&DeviceStatus>,
) -> Vec<VideoKey> {
    let hash = status.and_then(|status| status.encrypt_pwd_hash.as_deref());
    let mut keys: Vec<VideoKey> = option_key(camera, hash).into_iter().collect();
    if hash.is_some() && !keys.is_empty() {
        return keys;
    }
    if let Some(cloud) = cloud_key(source, camera, hash).await
        && !keys.iter().any(|key| key.code() == cloud.code())
    {
        keys.push(cloud);
    }
    keys
}

/// The app option's code when it is readable and matches the camera.
fn option_key(camera: &CameraConfig, hash: Option<&str>) -> Option<VideoKey> {
    let path = camera.media_key_file.as_ref()?;
    match ensure_private_regular(path, 1).map(Zeroizing::new) {
        Ok(code) if matches_hash(&code, hash) => Some(VideoKey {
            source: KeySource::Option,
            code,
        }),
        Ok(_) => {
            tracing::warn!("configured verification code does not match the camera");
            None
        }
        Err(error) => {
            tracing::warn!(
                error_type = error.diagnostic_code(),
                "configured verification code is unavailable"
            );
            None
        }
    }
}

/// The cloud copy of the verification code when it matches the camera.
async fn cloud_key(
    source: &dyn DeviceSource,
    camera: &CameraConfig,
    hash: Option<&str>,
) -> Option<VideoKey> {
    match source.cloud_verification_code(camera).await {
        Ok(code) if matches_hash(&code, hash) => Some(VideoKey {
            source: KeySource::Cloud,
            code,
        }),
        Ok(_) => {
            tracing::warn!("cloud verification code does not match the camera");
            None
        }
        Err(error) => {
            tracing::warn!(
                error_type = error.diagnostic_code(),
                "cloud verification code is unavailable"
            );
            None
        }
    }
}

/// First usable code, as reported by the encryption endpoint.
pub async fn resolve_video_key(
    source: &dyn DeviceSource,
    camera: &CameraConfig,
    status: Option<&DeviceStatus>,
) -> Option<VideoKey> {
    candidate_keys(source, camera, status)
        .await
        .into_iter()
        .next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twice_md5_matches_the_app_digest() {
        // md5("ABCDEF") = 8827a41122a5028b9808c7bf84b9fcf6, hashed again as hex text.
        assert_eq!(twice_md5("ABCDEF"), "aa37ad52a5a65df39791a95818fb3298");
    }

    #[test]
    fn hash_check_is_case_insensitive_and_rejects_mismatches() {
        let hash = twice_md5("ABCDEF").to_ascii_uppercase();
        assert!(matches_hash("ABCDEF", Some(&hash)));
        assert!(!matches_hash("ABCDEG", Some(&hash)));
        assert!(matches_hash("ABCDEF", None));
        assert!(!matches_hash("", None));
        assert!(!matches_hash(&"A".repeat(65), None));
    }
}
