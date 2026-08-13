from __future__ import annotations

import asyncio
import queue
import threading
from pathlib import Path

import pytest

from ezviz_vtm_bridge.config import CameraConfig
from ezviz_vtm_bridge.errors import CapacityError, StreamStoppedError
from ezviz_vtm_bridge.hub import BoundedThreadWriter, RawStreamHub, _force_end
from ezviz_vtm_bridge.metrics import Metrics
from ezviz_vtm_bridge.remux import MpegTsHub

from .fakes import PACK, FakeTransport


def test_bounded_writer_honors_stop_and_flush() -> None:
    target: queue.Queue[bytes | object] = queue.Queue(2)
    stop = threading.Event()
    writer = BoundedThreadWriter(target, stop, chunk_bytes=2)
    assert writer.write(b"abcd") == 4
    assert target.get_nowait() == b"ab"
    assert target.get_nowait() == b"cd"
    writer.flush()
    stop.set()
    with pytest.raises(StreamStoppedError):
        writer.write(b"blocked")
    with pytest.raises(StreamStoppedError):
        writer.flush()


async def test_force_end_replaces_data_in_full_queue() -> None:
    target: asyncio.Queue[bytes | object] = asyncio.Queue(1)
    target.put_nowait(b"stale")
    _force_end(target)
    assert await target.get() != b"stale"


def make_hub(
    camera: CameraConfig,
    transport: FakeTransport,
    *,
    max_subscribers: int = 4,
    queue_chunks: int = 8,
    idle_grace: float = 0.01,
) -> RawStreamHub:
    return RawStreamHub(
        camera,
        transport,
        Metrics(),
        subscriber_queue_chunks=queue_chunks,
        thread_queue_chunks=4,
        max_subscribers=max_subscribers,
        idle_grace_seconds=idle_grace,
    )


async def take(hub: RawStreamHub, count: int, *, delay: float = 0) -> list[bytes]:
    result = []
    async for chunk in hub.iter_chunks():
        result.append(chunk)
        if delay:
            await asyncio.sleep(delay)
        if len(result) >= count:
            return result
    return result


async def test_two_consumers_share_one_upstream(camera: CameraConfig) -> None:
    transport = FakeTransport()
    hub = make_hub(camera, transport)
    first, second = await asyncio.gather(take(hub, 5), take(hub, 5))
    assert all(chunk.startswith(PACK) for chunk in first + second)
    assert transport.starts == 1
    assert transport.max_active == 1
    await asyncio.sleep(0.03)
    await hub.close()
    assert transport.active == 0


async def test_capacity_is_fail_closed(camera: CameraConfig) -> None:
    transport = FakeTransport(interval=0.02)
    hub = make_hub(camera, transport, max_subscribers=1)
    entered = asyncio.Event()

    async def hold() -> None:
        async for _chunk in hub.iter_chunks():
            entered.set()
            await asyncio.sleep(0.1)
            return

    task = asyncio.create_task(hold())
    await entered.wait()
    with pytest.raises(CapacityError):
        iterator = hub.iter_chunks()
        await anext(iterator)
    await task
    await hub.close()


async def test_slow_subscriber_is_evicted_without_stopping_fast_one(
    camera: CameraConfig,
) -> None:
    transport = FakeTransport(interval=0)
    metrics = Metrics()
    hub = RawStreamHub(
        camera,
        transport,
        metrics,
        subscriber_queue_chunks=2,
        thread_queue_chunks=4,
        max_subscribers=2,
        idle_grace_seconds=0.01,
    )
    slow = hub.iter_chunks()
    first_slow = asyncio.create_task(anext(slow))
    fast = await take(hub, 10)
    assert await first_slow
    assert len(fast) == 10
    await slow.aclose()
    await hub.close()
    assert "slow_subscribers_total" in metrics.render()


async def test_upstream_stops_after_idle_grace(camera: CameraConfig) -> None:
    transport = FakeTransport()
    hub = make_hub(camera, transport, idle_grace=0.02)
    assert len(await take(hub, 1)) == 1
    await asyncio.sleep(0.06)
    assert transport.active == 0
    await hub.close()


def fake_ffmpeg(tmp_path: Path) -> str:
    script = tmp_path / "fake-ffmpeg"
    script.write_text(
        "#!/usr/bin/env python3\n"
        "import sys\n"
        "while data := sys.stdin.buffer.read(65536):\n"
        "    sys.stdout.buffer.write(data)\n"
        "    sys.stdout.buffer.flush()\n",
        encoding="utf-8",
    )
    script.chmod(0o755)
    return str(script)


async def test_mpeg_ts_consumers_share_one_remux(
    tmp_path: Path, camera: CameraConfig
) -> None:
    transport = FakeTransport()
    metrics = Metrics()
    raw = RawStreamHub(
        camera,
        transport,
        metrics,
        subscriber_queue_chunks=8,
        thread_queue_chunks=4,
        max_subscribers=4,
        idle_grace_seconds=0.01,
    )
    ts = MpegTsHub(
        raw,
        metrics,
        ffmpeg_path=fake_ffmpeg(tmp_path),
        subscriber_queue_chunks=8,
        max_subscribers=4,
        idle_grace_seconds=0.01,
    )
    first, second = await asyncio.gather(take(ts, 4), take(ts, 4))  # type: ignore[arg-type]
    assert first and second
    assert transport.starts == 1
    assert "remux_starts_total" in metrics.render()
    await ts.close()
    await raw.close()


async def test_mpeg_ts_capacity_and_idle_shutdown(
    tmp_path: Path, camera: CameraConfig
) -> None:
    transport = FakeTransport(interval=0.01)
    metrics = Metrics()
    raw = make_hub(camera, transport)
    ts = MpegTsHub(
        raw,
        metrics,
        ffmpeg_path=fake_ffmpeg(tmp_path),
        subscriber_queue_chunks=4,
        max_subscribers=1,
        idle_grace_seconds=0.01,
    )
    iterator = ts.iter_chunks()
    assert await anext(iterator)
    other = ts.iter_chunks()
    with pytest.raises(CapacityError):
        await anext(other)
    await iterator.aclose()
    await asyncio.sleep(0.05)
    assert "ezviz_bridge_remux_active{camera=\"front-door\"} 0" in metrics.render()
    await ts.close()
    await raw.close()


async def test_failed_upstream_finishes_subscribers_and_records_failure(
    camera: CameraConfig,
) -> None:
    class FailedTransport(FakeTransport):
        def stream_mpeg_ps(self, camera, output, stop_event):  # type: ignore[no-untyped-def]
            del camera, output, stop_event
            raise OSError("signed-url-must-not-escape")

    metrics = Metrics()
    hub = RawStreamHub(
        camera,
        FailedTransport(),
        metrics,
        subscriber_queue_chunks=4,
        thread_queue_chunks=4,
        max_subscribers=2,
        idle_grace_seconds=0,
    )
    assert await take(hub, 1) == []
    assert "upstream_failures_total" in metrics.render()
    await hub.close()
