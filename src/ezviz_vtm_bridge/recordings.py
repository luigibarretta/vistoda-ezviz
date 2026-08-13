"""Finite, immutable MPEG-PS recordings for SceneTrove."""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import tempfile
from contextlib import suppress
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any
from uuid import uuid4

from .errors import CapacityError, RecordingError
from .hub import RawStreamHub
from .metrics import Metrics

PACK_START = b"\x00\x00\x01\xba"


def utc_now() -> str:
    return datetime.now(UTC).isoformat().replace("+00:00", "Z")


def write_all(descriptor: int, payload: bytes) -> None:
    """Write a complete media chunk, including after a short os.write()."""
    view = memoryview(payload)
    written = 0
    while written < len(view):
        count = os.write(descriptor, view[written:])
        if count <= 0:
            raise OSError("media write made no progress")
        written += count


@dataclass(slots=True)
class RecordingManifest:
    schema_version: int
    recording_id: str
    camera: str
    status: str
    requested_at: str
    started_at: str | None
    completed_at: str | None
    requested_duration_seconds: int
    actual_duration_seconds: float | None
    media_type: str
    bytes: int | None
    sha256: str | None
    error_code: str | None

    def public(self) -> dict[str, Any]:
        return asdict(self)


class RecordingManager:
    """Own recording jobs, journal and bounded spool."""

    def __init__(  # noqa: PLR0913 - independent storage bounds stay explicit
        self,
        directory: Path,
        hubs: dict[str, RawStreamHub],
        metrics: Metrics,
        *,
        max_duration_seconds: int,
        max_recording_bytes: int,
        quota_bytes: int,
    ) -> None:
        self._directory = directory
        self._directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(self._directory, 0o700)
        self._journal = self._directory / "recordings.json"
        self._hubs = hubs
        self._metrics = metrics
        self._max_duration = max_duration_seconds
        self._max_bytes = max_recording_bytes
        self._quota = quota_bytes
        self._manifests: dict[str, RecordingManifest] = {}
        self._idempotency: dict[str, str] = {}
        self._tasks: set[asyncio.Task[None]] = set()
        self._lock = asyncio.Lock()
        self._load()

    def _load(self) -> None:
        if not self._journal.exists():
            return
        try:
            raw = json.loads(self._journal.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise RecordingError("recording journal is invalid") from error
        for item in raw.get("recordings", []):
            manifest = RecordingManifest(**item)
            if manifest.status in {"pending", "recording"}:
                manifest.status = "failed"
                manifest.completed_at = utc_now()
                manifest.error_code = "interrupted"
            self._manifests[manifest.recording_id] = manifest
        idempotency = raw.get("idempotency", {})
        if isinstance(idempotency, dict):
            self._idempotency = {
                str(key): str(value)
                for key, value in idempotency.items()
                if str(value) in self._manifests
            }
        self._persist()

    async def start(
        self, camera: str, duration_seconds: int, idempotency_key: str
    ) -> RecordingManifest:
        if camera not in self._hubs:
            raise RecordingError("camera alias is not configured")
        if not 1 <= duration_seconds <= self._max_duration:
            raise RecordingError(f"duration must be between 1 and {self._max_duration} seconds")
        if not 8 <= len(idempotency_key) <= 128:
            raise RecordingError("Idempotency-Key must contain 8 to 128 characters")
        async with self._lock:
            if existing_id := self._idempotency.get(idempotency_key):
                existing = self._manifests[existing_id]
                if (
                    existing.camera != camera
                    or existing.requested_duration_seconds != duration_seconds
                ):
                    raise RecordingError("Idempotency-Key was reused with a different request")
                return existing
            if self._spool_bytes() + self._max_bytes > self._quota:
                raise CapacityError("recording quota does not have safe headroom")
            recording_id = str(uuid4())
            manifest = RecordingManifest(
                schema_version=1,
                recording_id=recording_id,
                camera=camera,
                status="pending",
                requested_at=utc_now(),
                started_at=None,
                completed_at=None,
                requested_duration_seconds=duration_seconds,
                actual_duration_seconds=None,
                media_type="video/mpeg",
                bytes=None,
                sha256=None,
                error_code=None,
            )
            self._manifests[recording_id] = manifest
            self._idempotency[idempotency_key] = recording_id
            self._persist()
            task = asyncio.create_task(self._record(manifest), name=f"recording-{recording_id}")
            self._tasks.add(task)
            task.add_done_callback(self._tasks.discard)
            return manifest

    def get(self, recording_id: str) -> RecordingManifest | None:
        return self._manifests.get(recording_id)

    def media_path(self, recording_id: str) -> Path | None:
        manifest = self.get(recording_id)
        if manifest is None or manifest.status != "ready":
            return None
        path = self._directory / f"{recording_id}.mpegps"
        return path if path.is_file() else None

    async def _record(self, manifest: RecordingManifest) -> None:  # noqa: PLR0915
        final_path = self._directory / f"{manifest.recording_id}.mpegps"
        temporary_path = self._directory / f".{manifest.recording_id}.partial"
        digest = hashlib.sha256()
        written = 0
        started_monotonic: float | None = None
        align_buffer = bytearray()
        descriptor: int | None = None
        try:
            descriptor = os.open(temporary_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            manifest.status = "recording"
            self._persist()
            async with asyncio.timeout(manifest.requested_duration_seconds + 45):
                async for source_chunk in self._hubs[manifest.camera].iter_chunks():
                    chunk = source_chunk
                    if started_monotonic is None:
                        align_buffer.extend(chunk)
                        marker = align_buffer.find(PACK_START)
                        if marker < 0:
                            if len(align_buffer) > 1024 * 1024:
                                raise RecordingError("MPEG-PS pack header was not found")
                            continue
                        chunk = bytes(align_buffer[marker:])
                        align_buffer.clear()
                        started_monotonic = asyncio.get_running_loop().time()
                        manifest.started_at = utc_now()
                    if written + len(chunk) > self._max_bytes:
                        raise CapacityError("recording reached the configured byte limit")
                    write_all(descriptor, chunk)
                    digest.update(chunk)
                    written += len(chunk)
                    elapsed = asyncio.get_running_loop().time() - started_monotonic
                    if elapsed >= manifest.requested_duration_seconds:
                        break
            if started_monotonic is None or written == 0:
                raise RecordingError("recording contained no aligned MPEG-PS media")
            os.fsync(descriptor)
            os.close(descriptor)
            descriptor = None
            os.replace(temporary_path, final_path)
            directory_fd = os.open(self._directory, os.O_RDONLY)
            try:
                os.fsync(directory_fd)
            finally:
                os.close(directory_fd)
            manifest.status = "ready"
            manifest.completed_at = utc_now()
            manifest.actual_duration_seconds = round(
                asyncio.get_running_loop().time() - started_monotonic, 3
            )
            manifest.bytes = written
            manifest.sha256 = digest.hexdigest()
            self._metrics.increment("recordings_ready_total", manifest.camera)
        except (Exception, asyncio.CancelledError) as error:
            manifest.status = "failed"
            manifest.completed_at = utc_now()
            manifest.error_code = self._safe_error_code(error)
            self._metrics.increment("recordings_failed_total", manifest.camera)
            if descriptor is not None:
                os.close(descriptor)
            temporary_path.unlink(missing_ok=True)
            if isinstance(error, asyncio.CancelledError):
                raise
        finally:
            self._persist()

    @staticmethod
    def _safe_error_code(error: BaseException) -> str:
        if isinstance(error, TimeoutError):
            return "timeout"
        if isinstance(error, CapacityError):
            return "capacity"
        if isinstance(error, RecordingError):
            return "invalid_media"
        return "upstream_failure"

    def _spool_bytes(self) -> int:
        return sum(path.stat().st_size for path in self._directory.glob("*.mpegps"))

    def _persist(self) -> None:
        payload = {
            "schema_version": 1,
            "recordings": [
                manifest.public()
                for manifest in sorted(self._manifests.values(), key=lambda item: item.recording_id)
            ],
            "idempotency": dict(sorted(self._idempotency.items())),
        }
        descriptor, temporary_name = tempfile.mkstemp(prefix=".recordings.", dir=self._directory)
        try:
            os.fchmod(descriptor, 0o600)
            with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
                json.dump(payload, handle, separators=(",", ":"), sort_keys=True)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary_name, self._journal)
        except BaseException:
            with suppress(FileNotFoundError):
                os.unlink(temporary_name)
            raise

    async def close(self) -> None:
        tasks = tuple(self._tasks)
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
