"""Fail-closed bridge configuration."""

from __future__ import annotations

import json
import os
import stat
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .errors import ConfigurationError


def read_secret_file(path: Path, *, minimum_length: int = 1) -> str:
    """Read a secret after refusing group/world permissions and symlinks."""
    try:
        metadata = path.lstat()
    except OSError as error:
        raise ConfigurationError(f"secret file is unavailable: {path.name}") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise ConfigurationError(f"secret path is not a regular file: {path.name}")
    if metadata.st_mode & 0o077:
        raise ConfigurationError(f"secret file permissions are too broad: {path.name}")
    try:
        value = path.read_text(encoding="utf-8").strip()
    except OSError as error:
        raise ConfigurationError(f"secret file cannot be read: {path.name}") from error
    if len(value) < minimum_length:
        raise ConfigurationError(f"secret file is empty or too short: {path.name}")
    return value


@dataclass(frozen=True, slots=True)
class CameraConfig:
    """One camera, addressed externally only by alias."""

    alias: str
    serial: str
    decrypt_video: bool = False
    media_key_file: Path | None = None

    @classmethod
    def from_mapping(cls, alias: str, value: Any) -> CameraConfig:
        if not alias or not alias.replace("-", "").replace("_", "").isalnum():
            raise ConfigurationError("camera aliases must be alphanumeric with '-' or '_'")
        if not isinstance(value, dict):
            raise ConfigurationError(f"camera {alias} must be an object")
        serial = value.get("serial")
        if not isinstance(serial, str) or not serial.strip():
            raise ConfigurationError(f"camera {alias} has no serial")
        media_key = value.get("media_key_file")
        if media_key is not None and not isinstance(media_key, str):
            raise ConfigurationError(f"camera {alias} media_key_file must be a string")
        decrypt = value.get("decrypt_video", False)
        if not isinstance(decrypt, bool):
            raise ConfigurationError(f"camera {alias} decrypt_video must be boolean")
        return cls(
            alias=alias,
            serial=serial.strip(),
            decrypt_video=decrypt,
            media_key_file=Path(media_key) if media_key else None,
        )


@dataclass(frozen=True, slots=True)
class BridgeConfig:
    """All runtime bounds are explicit and validated."""

    bind_host: str
    bind_port: int
    api_token_file: Path
    ezviz_token_file: Path
    cameras: dict[str, CameraConfig]
    data_dir: Path
    ffmpeg_path: str
    upstream_timeout_seconds: float
    idle_grace_seconds: float
    queue_chunks: int
    cross_thread_chunks: int
    max_subscribers: int
    max_recording_seconds: int
    max_recording_bytes: int
    recording_quota_bytes: int
    snapshot_cache_seconds: float
    snapshot_stale_seconds: float

    @classmethod
    def from_env(cls, environ: dict[str, str] | None = None) -> BridgeConfig:
        env = os.environ if environ is None else environ
        cameras_path = Path(env.get("EZVIZ_BRIDGE_CAMERAS_FILE", "/config/cameras.json"))
        try:
            raw_cameras = json.loads(cameras_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise ConfigurationError("camera configuration is unavailable or invalid") from error
        if not isinstance(raw_cameras, dict) or not raw_cameras:
            raise ConfigurationError("at least one camera must be configured")
        cameras = {
            alias: CameraConfig.from_mapping(alias, value)
            for alias, value in raw_cameras.items()
        }

        def integer(name: str, default: int, minimum: int, maximum: int) -> int:
            try:
                value = int(env.get(name, str(default)))
            except ValueError as error:
                raise ConfigurationError(f"{name} must be an integer") from error
            if not minimum <= value <= maximum:
                raise ConfigurationError(f"{name} must be between {minimum} and {maximum}")
            return value

        def number(name: str, default: float, minimum: float, maximum: float) -> float:
            try:
                value = float(env.get(name, str(default)))
            except ValueError as error:
                raise ConfigurationError(f"{name} must be numeric") from error
            if not minimum <= value <= maximum:
                raise ConfigurationError(f"{name} must be between {minimum} and {maximum}")
            return value

        return cls(
            bind_host=env.get("EZVIZ_BRIDGE_BIND_HOST", "0.0.0.0"),  # noqa: S104
            bind_port=integer("EZVIZ_BRIDGE_BIND_PORT", 8765, 1024, 65535),
            api_token_file=Path(env.get("EZVIZ_BRIDGE_API_TOKEN_FILE", "/run/secrets/api_token")),
            ezviz_token_file=Path(env.get("EZVIZ_BRIDGE_EZVIZ_TOKEN_FILE", "/data/token.json")),
            cameras=cameras,
            data_dir=Path(env.get("EZVIZ_BRIDGE_DATA_DIR", "/data")),
            ffmpeg_path=env.get("EZVIZ_BRIDGE_FFMPEG_PATH", "/usr/bin/ffmpeg"),
            upstream_timeout_seconds=number("EZVIZ_BRIDGE_UPSTREAM_TIMEOUT", 20, 5, 60),
            idle_grace_seconds=number("EZVIZ_BRIDGE_IDLE_GRACE", 15, 0, 120),
            queue_chunks=integer("EZVIZ_BRIDGE_QUEUE_CHUNKS", 32, 2, 256),
            cross_thread_chunks=integer("EZVIZ_BRIDGE_THREAD_QUEUE_CHUNKS", 16, 2, 128),
            max_subscribers=integer("EZVIZ_BRIDGE_MAX_SUBSCRIBERS", 8, 1, 64),
            max_recording_seconds=integer("EZVIZ_BRIDGE_MAX_RECORDING_SECONDS", 120, 5, 600),
            max_recording_bytes=integer(
                "EZVIZ_BRIDGE_MAX_RECORDING_BYTES", 256 * 1024 * 1024, 1024 * 1024, 2**31
            ),
            recording_quota_bytes=integer(
                "EZVIZ_BRIDGE_RECORDING_QUOTA_BYTES", 2 * 1024 * 1024 * 1024, 1024 * 1024, 2**40
            ),
            snapshot_cache_seconds=number("EZVIZ_BRIDGE_SNAPSHOT_CACHE", 3, 0, 30),
            snapshot_stale_seconds=number("EZVIZ_BRIDGE_SNAPSHOT_STALE", 900, 30, 3600),
        )
