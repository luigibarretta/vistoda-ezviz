"""Fresh snapshot service with short coalescing cache."""

from __future__ import annotations

import asyncio
import time
from dataclasses import dataclass

from .config import CameraConfig
from .metrics import Metrics
from .transport import CameraTransport


@dataclass(slots=True)
class CachedSnapshot:
    data: bytes
    created_monotonic: float


class SnapshotService:
    def __init__(
        self,
        cameras: dict[str, CameraConfig],
        transport: CameraTransport,
        metrics: Metrics,
        cache_seconds: float,
    ) -> None:
        self._cameras = cameras
        self._transport = transport
        self._metrics = metrics
        self._cache_seconds = cache_seconds
        self._cache: dict[str, CachedSnapshot] = {}
        self._locks = {alias: asyncio.Lock() for alias in cameras}

    async def get(self, alias: str) -> bytes:
        camera = self._cameras[alias]
        cached = self._cache.get(alias)
        now = time.monotonic()
        if cached and now - cached.created_monotonic <= self._cache_seconds:
            self._metrics.increment("snapshot_cache_hits_total", alias)
            return cached.data
        async with self._locks[alias]:
            cached = self._cache.get(alias)
            now = time.monotonic()
            if cached and now - cached.created_monotonic <= self._cache_seconds:
                self._metrics.increment("snapshot_cache_hits_total", alias)
                return cached.data
            image = await asyncio.to_thread(self._transport.snapshot_jpeg, camera)
            self._cache[alias] = CachedSnapshot(image, time.monotonic())
            self._metrics.increment("snapshots_total", alias)
            return image
