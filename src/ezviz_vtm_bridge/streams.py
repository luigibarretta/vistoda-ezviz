"""Composition of raw and remuxed per-camera streams."""

from __future__ import annotations

import asyncio

from .config import CameraConfig
from .hub import RawStreamHub
from .metrics import Metrics
from .remux import MpegTsHub
from .transport import CameraTransport


class StreamManager:
    """Resolve configured camera hubs without exposing serials."""

    def __init__(  # noqa: PLR0913 - reviewed resource bounds remain explicit
        self,
        cameras: dict[str, CameraConfig],
        transport: CameraTransport,
        metrics: Metrics,
        *,
        subscriber_queue_chunks: int,
        thread_queue_chunks: int,
        max_subscribers: int,
        idle_grace_seconds: float,
        ffmpeg_path: str,
    ) -> None:
        self.raw: dict[str, RawStreamHub] = {}
        self.ts: dict[str, MpegTsHub] = {}
        for alias, camera in cameras.items():
            raw = RawStreamHub(
                camera,
                transport,
                metrics,
                subscriber_queue_chunks=subscriber_queue_chunks,
                thread_queue_chunks=thread_queue_chunks,
                max_subscribers=max_subscribers,
                idle_grace_seconds=idle_grace_seconds,
            )
            self.raw[alias] = raw
            self.ts[alias] = MpegTsHub(
                raw,
                metrics,
                ffmpeg_path=ffmpeg_path,
                subscriber_queue_chunks=subscriber_queue_chunks,
                max_subscribers=max_subscribers,
                idle_grace_seconds=idle_grace_seconds,
            )

    async def close(self) -> None:
        await asyncio.gather(*(hub.close() for hub in self.ts.values()))
        await asyncio.gather(*(hub.close() for hub in self.raw.values()))
