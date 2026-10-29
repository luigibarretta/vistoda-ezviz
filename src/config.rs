use std::{collections::BTreeMap, env, path::PathBuf};

use serde::Deserialize;

use crate::error::BridgeError;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CameraConfig {
    pub serial: String,
    #[serde(default = "default_channel")]
    pub channel: u16,
    #[serde(default)]
    pub substream: bool,
    /// Forces the encrypted-RTP path; cameras whose cloud status reports
    /// video encryption use it automatically.
    #[serde(default)]
    pub decrypt_video: bool,
    /// Optional private verification-code file. Without it, the cloud copy
    /// is used when it matches the camera's `encryptPwd` hash.
    pub media_key_file: Option<PathBuf>,
}

const fn default_channel() -> u16 {
    1
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
    /// Alarm summary poll interval; zero disables the alarm feed poller.
    pub alarm_poll_seconds: u64,
    /// Alarms kept per camera (oldest evicted first, with their pictures).
    pub alarm_history_limit: usize,
    /// Picture storage for all cameras together, split evenly per camera.
    pub alarm_picture_budget_bytes: u64,
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
            alarm_poll_seconds: alarm_interval()?,
            alarm_history_limit: usize::try_from(clamped(
                "EZVIZ_BRIDGE_ALARM_HISTORY_LIMIT",
                200,
                50,
                2000,
            ))
            .map_err(|_| BridgeError::Configuration("alarm history limit is invalid".into()))?,
            alarm_picture_budget_bytes: clamped(
                "EZVIZ_BRIDGE_ALARM_PICTURE_BUDGET_MIB",
                256,
                64,
                2048,
            ) * 1024
                * 1024,
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
        if camera.serial.is_empty()
            || camera.serial.len() > 64
            || !camera
                .serial
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(BridgeError::Configuration(format!(
                "camera {alias} has an invalid serial"
            )));
        }
        if !(1..=256).contains(&camera.channel) {
            return Err(BridgeError::Configuration(format!(
                "camera {alias} channel must be between 1 and 256"
            )));
        }
    }
    Ok(())
}

fn alarm_interval() -> Result<u64, BridgeError> {
    let seconds = integer("EZVIZ_BRIDGE_ALARM_POLL_SECONDS", 15, 0, 300)?;
    if (1..10).contains(&seconds) {
        return Err(BridgeError::Configuration(
            "EZVIZ_BRIDGE_ALARM_POLL_SECONDS must be 0 or between 10 and 300".into(),
        ));
    }
    Ok(seconds)
}

/// Reads a bounded integer that must never stop startup: a missing value
/// takes the default; an invalid or out-of-range one is replaced (default or
/// nearest bound) and the replacement is logged.
fn clamped(name: &str, default: u64, minimum: u64, maximum: u64) -> u64 {
    let (value, adjusted) = bounded(env::var(name).ok().as_deref(), default, minimum, maximum);
    if adjusted {
        tracing::warn!(
            variable = name,
            value,
            "configuration value replaced to stay in range"
        );
    }
    value
}

/// The value to use for `raw` and whether it had to be replaced.
fn bounded(raw: Option<&str>, default: u64, minimum: u64, maximum: u64) -> (u64, bool) {
    match raw.map(|raw| raw.trim().parse::<u64>()) {
        None => (default, false),
        Some(Err(_)) => (default, true),
        Some(Ok(parsed)) => {
            let value = parsed.clamp(minimum, maximum);
            (value, value != parsed)
        }
    }
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
#[path = "config_tests.rs"]
mod tests;
