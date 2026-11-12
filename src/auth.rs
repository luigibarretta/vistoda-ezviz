//! The bridge API token, checked by the provider kit.
//!
//! `vistoda_provider_kit::auth` compares in constant time: bearer everywhere,
//! Home Assistant Basic (`homeassistant` + token) only where the caller allows
//! it, i.e. the Generic Camera snapshot and live routes (`api_session`).

use std::path::Path;

pub use vistoda_provider_kit::auth::ApiToken as ApiAuthenticator;

use crate::error::BridgeError;

/// Reads the private API token file: no symlink, no group/other permission,
/// at least 32 bytes after trimming, and not a well-known default.
pub fn load(path: &Path) -> Result<ApiAuthenticator, BridgeError> {
    ApiAuthenticator::from_file(path)
        .map_err(|error| BridgeError::Configuration(format!("bridge API token: {error}")))
}
