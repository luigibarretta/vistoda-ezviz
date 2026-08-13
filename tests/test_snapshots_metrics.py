from __future__ import annotations

import asyncio

from ezviz_vtm_bridge.config import CameraConfig
from ezviz_vtm_bridge.metrics import Metrics
from ezviz_vtm_bridge.snapshots import SnapshotService

from .fakes import JPEG, FakeTransport


async def test_concurrent_snapshots_are_coalesced(camera: CameraConfig) -> None:
    transport = FakeTransport()
    metrics = Metrics()
    service = SnapshotService(
        {camera.alias: camera}, transport, metrics, cache_seconds=10, stale_seconds=900
    )
    images = await asyncio.gather(*(service.get(camera.alias) for _ in range(8)))
    assert images == [JPEG] * 8
    assert transport.snapshot_calls == 1
    rendered = metrics.render()
    assert 'camera="front-door"' in rendered
    assert "TEST-SERIAL" not in rendered


async def test_snapshot_uses_bounded_stale_copy_on_refresh_failure(
    camera: CameraConfig,
) -> None:
    transport = FakeTransport()
    metrics = Metrics()
    service = SnapshotService(
        {camera.alias: camera}, transport, metrics, cache_seconds=0, stale_seconds=900
    )
    assert await service.get(camera.alias) == JPEG

    def fail(_camera: CameraConfig) -> bytes:
        raise RuntimeError("vendor unavailable")

    transport.snapshot_jpeg = fail  # type: ignore[method-assign]
    assert await service.get(camera.alias) == JPEG
    assert "snapshot_stale_fallbacks_total" in metrics.render()


def test_metrics_are_sorted_and_low_cardinality() -> None:
    metrics = Metrics()
    metrics.increment("requests_total", "z")
    metrics.increment("requests_total", "a", 2)
    metrics.gauge("active", "a", 1)
    assert metrics.render().splitlines() == [
        'ezviz_bridge_requests_total{camera="a"} 2',
        'ezviz_bridge_requests_total{camera="z"} 1',
        'ezviz_bridge_active{camera="a"} 1',
    ]
