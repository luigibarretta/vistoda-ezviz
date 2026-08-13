from __future__ import annotations

import asyncio
import hashlib
import os
from pathlib import Path

import pytest

from ezviz_vtm_bridge.config import CameraConfig
from ezviz_vtm_bridge.errors import CapacityError, RecordingError
from ezviz_vtm_bridge.hub import RawStreamHub
from ezviz_vtm_bridge.metrics import Metrics
from ezviz_vtm_bridge.recordings import RecordingManager, write_all

from .fakes import PACK, FakeTransport


def test_write_all_retries_short_writes(monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[bytes] = []

    def short_write(_descriptor: int, payload: bytes | memoryview) -> int:
        materialized = bytes(payload)
        calls.append(materialized)
        return min(2, len(materialized))

    monkeypatch.setattr(os, "write", short_write)
    write_all(123, b"abcde")
    assert calls == [b"abcde", b"cde", b"e"]


def test_write_all_rejects_no_progress(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(os, "write", lambda _descriptor, _payload: 0)
    with pytest.raises(OSError, match="no progress"):
        write_all(123, b"media")


def build_manager(
    path: Path,
    camera: CameraConfig,
    *,
    max_bytes: int = 1024 * 1024,
    quota: int = 2 * 1024 * 1024,
) -> tuple[RecordingManager, RawStreamHub]:
    metrics = Metrics()
    hub = RawStreamHub(
        camera,
        FakeTransport(interval=0.001),
        metrics,
        subscriber_queue_chunks=16,
        thread_queue_chunks=8,
        max_subscribers=2,
        idle_grace_seconds=0,
    )
    manager = RecordingManager(
        path,
        {camera.alias: hub},
        metrics,
        max_duration_seconds=5,
        max_recording_bytes=max_bytes,
        quota_bytes=quota,
    )
    return manager, hub


async def wait_ready(manager: RecordingManager, recording_id: str) -> None:
    for _ in range(300):
        manifest = manager.get(recording_id)
        assert manifest is not None
        if manifest.status in {"ready", "failed"}:
            return
        await asyncio.sleep(0.01)
    raise AssertionError("recording did not finish")


async def test_recording_is_aligned_atomic_and_digest_verified(
    tmp_path: Path, camera: CameraConfig
) -> None:
    manager, hub = build_manager(tmp_path / "recordings", camera)
    manifest = await manager.start(camera.alias, 1, "request-0001")
    await wait_ready(manager, manifest.recording_id)
    ready = manager.get(manifest.recording_id)
    assert ready is not None and ready.status == "ready"
    media = manager.media_path(manifest.recording_id)
    assert media is not None
    data = media.read_bytes()
    assert data.startswith(PACK[:4])
    assert ready.bytes == len(data)
    assert ready.sha256 == hashlib.sha256(data).hexdigest()
    assert not list(media.parent.glob("*.partial"))
    await manager.close()
    await hub.close()


async def test_idempotency_returns_same_recording_and_rejects_changed_request(
    tmp_path: Path, camera: CameraConfig
) -> None:
    manager, hub = build_manager(tmp_path / "recordings", camera)
    first = await manager.start(camera.alias, 1, "request-0002")
    second = await manager.start(camera.alias, 1, "request-0002")
    assert second.recording_id == first.recording_id
    with pytest.raises(RecordingError, match="different"):
        await manager.start(camera.alias, 2, "request-0002")
    await manager.close()
    await hub.close()


async def test_recording_quota_requires_worst_case_headroom(
    tmp_path: Path, camera: CameraConfig
) -> None:
    directory = tmp_path / "recordings"
    directory.mkdir()
    (directory / "existing.mpegps").write_bytes(b"x" * 100)
    manager, hub = build_manager(directory, camera, max_bytes=100, quota=199)
    with pytest.raises(CapacityError, match="headroom"):
        await manager.start(camera.alias, 1, "request-0003")
    await manager.close()
    await hub.close()


def test_restart_marks_incomplete_job_failed(tmp_path: Path, camera: CameraConfig) -> None:
    directory = tmp_path / "recordings"
    directory.mkdir()
    (directory / "recordings.json").write_text(
        '{"schema_version":1,"recordings":[{'
        '"schema_version":1,"recording_id":"old","camera":"front-door",'
        '"status":"recording","requested_at":"now","started_at":"now",'
        '"completed_at":null,"requested_duration_seconds":1,'
        '"actual_duration_seconds":null,"media_type":"video/mpeg",'
        '"bytes":null,"sha256":null,"error_code":null}],"idempotency":{}}',
        encoding="utf-8",
    )
    manager, _hub = build_manager(directory, camera)
    recovered = manager.get("old")
    assert recovered is not None
    assert recovered.status == "failed"
    assert recovered.error_code == "interrupted"


async def test_recording_request_validation(tmp_path: Path, camera: CameraConfig) -> None:
    manager, hub = build_manager(tmp_path / "recordings", camera)
    with pytest.raises(RecordingError, match="configured"):
        await manager.start("missing", 1, "request-1000")
    with pytest.raises(RecordingError, match="duration"):
        await manager.start(camera.alias, 99, "request-1001")
    with pytest.raises(RecordingError, match="8 to 128"):
        await manager.start(camera.alias, 1, "short")
    assert manager.get("missing") is None
    assert manager.media_path("missing") is None
    await manager.close()
    await hub.close()


async def test_recording_fails_at_byte_bound(tmp_path: Path, camera: CameraConfig) -> None:
    manager, hub = build_manager(
        tmp_path / "recordings", camera, max_bytes=len(PACK) + 1, quota=1024
    )
    manifest = await manager.start(camera.alias, 1, "request-1002")
    await wait_ready(manager, manifest.recording_id)
    failed = manager.get(manifest.recording_id)
    assert failed is not None
    assert failed.status == "failed"
    assert failed.error_code == "capacity"
    assert manager.media_path(manifest.recording_id) is None
    await manager.close()
    await hub.close()


def test_invalid_recording_journal_fails_closed(tmp_path: Path, camera: CameraConfig) -> None:
    directory = tmp_path / "recordings"
    directory.mkdir()
    (directory / "recordings.json").write_text("invalid", encoding="utf-8")
    with pytest.raises(RecordingError, match="journal"):
        build_manager(directory, camera)
