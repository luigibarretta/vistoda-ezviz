use std::{collections::BTreeMap, env, path::PathBuf};

use serde::Deserialize;

use crate::error::BridgeError;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CameraConfig {
    pub serial: String,
    #[serde(default)]
    pub decrypt_video: bool,
    pub media_key_file: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct BridgeConfig {
    pub bind_host: String,
    pub bind_port: u16,
    pub api_token_file: PathBuf,
    pub ezviz_token_file: PathBuf,
    pub cameras: BTreeMap<String, CameraConfig>,
    pub data_dir: PathBuf,
    pub ffmpeg_path: PathBuf,
    pub upstream_timeout_seconds: u64,
    pub max_live_session_seconds: u64,
    pub idle_grace_seconds: u64,
    pub queue_chunks: usize,
    pub max_subscribers: usize,
    pub max_recording_seconds: u64,
    pub max_recording_bytes: u64,
    pub recording_quota_bytes: u64,
    pub snapshot_cache_seconds: u64,
    pub snapshot_stale_seconds: u64,
}

impl BridgeConfig {
    pub fn from_env() -> Result<Self, BridgeError> {
        let cameras_file = path("EZVIZ_BRIDGE_CAMERAS_FILE", "/config/cameras.json");
        let raw = std::fs::read_to_string(&cameras_file).map_err(|_| {
            BridgeError::Configuration("camera configuration is unavailable or invalid".into())
        })?;
        let cameras: BTreeMap<String, CameraConfig> = serde_json::from_str(&raw).map_err(|_| {
            BridgeError::Configuration("camera configuration is unavailable or invalid".into())
        })?;
        validate_cameras(&cameras)?;
        Ok(Self {
            bind_host: value("EZVIZ_BRIDGE_BIND_HOST", "0.0.0.0"),
            bind_port: u16::try_from(integer("EZVIZ_BRIDGE_BIND_PORT", 8765, 1024, 65_535)?)
                .map_err(|_| BridgeError::Configuration("bind port is invalid".into()))?,
            api_token_file: path("EZVIZ_BRIDGE_API_TOKEN_FILE", "/run/secrets/api_token"),
            ezviz_token_file: path("EZVIZ_BRIDGE_EZVIZ_TOKEN_FILE", "/data/token.json"),
            cameras,
            data_dir: path("EZVIZ_BRIDGE_DATA_DIR", "/data"),
            ffmpeg_path: path("EZVIZ_BRIDGE_FFMPEG_PATH", "/usr/bin/ffmpeg"),
            upstream_timeout_seconds: integer("EZVIZ_BRIDGE_UPSTREAM_TIMEOUT", 20, 5, 60)?,
            max_live_session_seconds: integer(
                "EZVIZ_BRIDGE_MAX_LIVE_SESSION_SECONDS",
                90,
                30,
                900,
            )?,
            idle_grace_seconds: integer("EZVIZ_BRIDGE_IDLE_GRACE", 15, 0, 120)?,
            queue_chunks: usize::try_from(integer("EZVIZ_BRIDGE_QUEUE_CHUNKS", 32, 2, 256)?)
                .map_err(|_| BridgeError::Configuration("queue size is invalid".into()))?,
            max_subscribers: usize::try_from(integer("EZVIZ_BRIDGE_MAX_SUBSCRIBERS", 8, 1, 64)?)
                .map_err(|_| BridgeError::Configuration("subscriber limit is invalid".into()))?,
            max_recording_seconds: integer("EZVIZ_BRIDGE_MAX_RECORDING_SECONDS", 120, 5, 600)?,
            max_recording_bytes: integer(
                "EZVIZ_BRIDGE_MAX_RECORDING_BYTES",
                256 * 1024 * 1024,
                1024 * 1024,
                2_147_483_648,
            )?,
            recording_quota_bytes: integer(
                "EZVIZ_BRIDGE_RECORDING_QUOTA_BYTES",
                2 * 1024 * 1024 * 1024,
                1024 * 1024,
                1_099_511_627_776,
            )?,
            snapshot_cache_seconds: integer("EZVIZ_BRIDGE_SNAPSHOT_CACHE", 3, 0, 300)?,
            snapshot_stale_seconds: integer("EZVIZ_BRIDGE_SNAPSHOT_STALE", 900, 30, 3600)?,
        })
    }
}

fn validate_cameras(cameras: &BTreeMap<String, CameraConfig>) -> Result<(), BridgeError> {
    if cameras.is_empty() {
        return Err(BridgeError::Configuration(
            "at least one camera must be configured".into(),
        ));
    }
    for (alias, camera) in cameras {
        if alias.is_empty()
            || !alias
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(BridgeError::Configuration(
                "camera aliases must be alphanumeric with '-' or '_'".into(),
            ));
        }
        if camera.serial.trim().is_empty() {
            return Err(BridgeError::Configuration(format!(
                "camera {alias} has no serial"
            )));
        }
        if camera.decrypt_video && camera.media_key_file.is_none() {
            return Err(BridgeError::Configuration(format!(
                "camera {alias} requires media_key_file when decrypt_video is enabled"
            )));
        }
    }
    Ok(())
}

fn value(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn path(name: &str, default: &str) -> PathBuf {
    PathBuf::from(value(name, default))
}

fn integer(name: &str, default: u64, minimum: u64, maximum: u64) -> Result<u64, BridgeError> {
    let raw = value(name, &default.to_string());
    let parsed = raw
        .parse::<u64>()
        .map_err(|_| BridgeError::Configuration(format!("{name} must be an integer")))?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(BridgeError::Configuration(format!(
            "{name} must be between {minimum} and {maximum}"
        )));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_are_strictly_validated() {
        let mut cameras = BTreeMap::new();
        cameras.insert(
            "../serial".into(),
            CameraConfig {
                serial: "secret".into(),
                decrypt_video: false,
                media_key_file: None,
            },
        );
        assert!(validate_cameras(&cameras).is_err());
    }
}
