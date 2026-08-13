from __future__ import annotations

from dataclasses import replace
from pathlib import Path

import pytest

from ezviz_vtm_bridge.config import BridgeConfig, CameraConfig


@pytest.fixture
def camera() -> CameraConfig:
    return CameraConfig(alias="front-door", serial="TEST-SERIAL")


@pytest.fixture
def config(tmp_path: Path, camera: CameraConfig) -> BridgeConfig:
    api_token = tmp_path / "api-token"
    api_token.write_text("a" * 48, encoding="utf-8")
    api_token.chmod(0o600)
    return BridgeConfig(
        bind_host="127.0.0.1",
        bind_port=8765,
        api_token_file=api_token,
        ezviz_token_file=tmp_path / "ezviz-token.json",
        cameras={camera.alias: camera},
        data_dir=tmp_path / "data",
        ffmpeg_path="/usr/bin/false",
        upstream_timeout_seconds=20,
        idle_grace_seconds=0.01,
        queue_chunks=8,
        cross_thread_chunks=4,
        max_subscribers=4,
        max_recording_seconds=10,
        max_recording_bytes=1024 * 1024,
        recording_quota_bytes=2 * 1024 * 1024,
        snapshot_cache_seconds=2,
    )


@pytest.fixture
def short_recording_config(config: BridgeConfig) -> BridgeConfig:
    return replace(config, max_recording_seconds=2)
