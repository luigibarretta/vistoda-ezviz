"""Lazy shared MPEG-TS copy-remux."""

from __future__ import annotations

import asyncio
import time
from collections.abc import AsyncIterator
from contextlib import suppress
from typing import Final

from .errors import CapacityError
from .hub import RawStreamHub, _force_end
from .metrics import Metrics

_END = object()
_REMUX_ARGUMENTS: Final = (
    "-hide_banner",
    "-loglevel",
    "error",
    "-analyzeduration",
    "500000",
    "-probesize",
    "1000000",
    "-fflags",
    "+genpts+nobuffer",
    "-f",
    "mpeg",
    "-i",
    "pipe:0",
    "-map",
    "0:v:0?",
    "-map",
    "0:a:0?",
    "-c",
    "copy",
    "-muxdelay",
    "0",
    "-flush_packets",
    "1",
    "-f",
    "mpegts",
    "pipe:1",
)
ProcessPipes = tuple[asyncio.StreamWriter, asyncio.StreamReader, asyncio.StreamReader]


class MpegTsHub:
    """Derive one bounded FFmpeg remux from a raw hub."""

    def __init__(  # noqa: PLR0913 - remux lifecycle bounds stay explicit
        self,
        raw: RawStreamHub,
        metrics: Metrics,
        *,
        ffmpeg_path: str,
        subscriber_queue_chunks: int,
        max_subscribers: int,
        idle_grace_seconds: float,
    ) -> None:
        self.camera = raw.camera
        self._raw = raw
        self._metrics = metrics
        self._ffmpeg = ffmpeg_path
        self._queue_chunks = subscriber_queue_chunks
        self._max_subscribers = max_subscribers
        self._idle_grace = idle_grace_seconds
        self._subscribers: set[asyncio.Queue[bytes | object]] = set()
        self._lock = asyncio.Lock()
        self._run_task: asyncio.Task[None] | None = None
        self._idle_task: asyncio.Task[None] | None = None
        self._closed = False

    async def iter_chunks(self) -> AsyncIterator[bytes]:
        subscriber: asyncio.Queue[bytes | object] = asyncio.Queue(self._queue_chunks)
        async with self._lock:
            if self._closed:
                raise RuntimeError("MPEG-TS hub is closed")
            if len(self._subscribers) >= self._max_subscribers:
                raise CapacityError("maximum MPEG-TS subscribers reached")
            self._subscribers.add(subscriber)
            if self._idle_task is not None:
                self._idle_task.cancel()
                self._idle_task = None
            if self._run_task is None or self._run_task.done():
                self._run_task = asyncio.create_task(
                    self._run(), name=f"mpegts-{self.camera.alias}"
                )
            self._update_gauge()
        try:
            while True:
                item = await subscriber.get()
                if item is _END:
                    break
                yield item  # type: ignore[misc]
        finally:
            async with self._lock:
                self._subscribers.discard(subscriber)
                self._update_gauge()
                if not self._subscribers and self._run_task and not self._run_task.done():
                    self._idle_task = asyncio.create_task(self._cancel_after_idle())

    async def _cancel_after_idle(self) -> None:
        try:
            await asyncio.sleep(self._idle_grace)
            async with self._lock:
                if not self._subscribers and self._run_task is not None:
                    self._run_task.cancel()
        except asyncio.CancelledError:
            return

    async def _run(self) -> None:
        started = time.monotonic()
        process = await self._start_process()
        if process is None:
            return
        self._metrics.increment("remux_starts_total", self.camera.alias)
        self._metrics.gauge("remux_active", self.camera.alias, 1)
        pipes = await self._validated_pipes(process)
        if pipes is None:
            return
        await self._relay(process, pipes, started)

    async def _validated_pipes(self, process: asyncio.subprocess.Process) -> ProcessPipes | None:
        if process.stdin is None or process.stdout is None or process.stderr is None:
            process.kill()
            await process.wait()
            self._metrics.increment("remux_failures_total", self.camera.alias)
            self._metrics.gauge("remux_active", self.camera.alias, 0)
            self._finish_subscribers()
            return None
        return process.stdin, process.stdout, process.stderr

    async def _relay(
        self,
        process: asyncio.subprocess.Process,
        pipes: ProcessPipes,
        started: float,
    ) -> None:
        stdin, stdout, stderr = pipes

        async def feed() -> None:
            try:
                async for chunk in self._raw.iter_chunks():
                    stdin.write(chunk)
                    await stdin.drain()
            finally:
                stdin.close()

        async def distribute() -> None:
            first_chunk = True
            while chunk := await stdout.read(64 * 1024):
                if first_chunk:
                    self._metrics.gauge(
                        "remux_startup_seconds",
                        self.camera.alias,
                        round(time.monotonic() - started, 6),
                    )
                    first_chunk = False
                self._publish(chunk)

        async def discard_stderr() -> None:
            while await stderr.read(4096):
                pass

        tasks = [asyncio.create_task(feed()), asyncio.create_task(distribute())]
        stderr_task = asyncio.create_task(discard_stderr())
        try:
            await asyncio.gather(*tasks)
            if await process.wait() != 0:
                self._metrics.increment("remux_failures_total", self.camera.alias)
        except asyncio.CancelledError:
            raise
        except Exception:  # noqa: BLE001 - process failures are reduced to metrics
            self._metrics.increment("remux_failures_total", self.camera.alias)
        finally:
            for task in tasks:
                task.cancel()
            stderr_task.cancel()
            if process.returncode is None:
                process.terminate()
                try:
                    await asyncio.wait_for(process.wait(), timeout=3)
                except TimeoutError:
                    process.kill()
                    await process.wait()
            self._metrics.gauge("remux_active", self.camera.alias, 0)
            self._finish_subscribers()

    async def _start_process(self) -> asyncio.subprocess.Process | None:
        try:
            return await asyncio.create_subprocess_exec(
                self._ffmpeg,
                *_REMUX_ARGUMENTS,
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
        except OSError:
            self._metrics.increment("remux_failures_total", self.camera.alias)
            self._finish_subscribers()
            return None

    def _publish(self, chunk: bytes) -> None:
        slow: list[asyncio.Queue[bytes | object]] = []
        for subscriber in tuple(self._subscribers):
            try:
                subscriber.put_nowait(chunk)
            except asyncio.QueueFull:
                slow.append(subscriber)
        for subscriber in slow:
            self._subscribers.discard(subscriber)
            _force_end(subscriber, _END)
            self._metrics.increment("slow_subscribers_total", self.camera.alias)
        if slow:
            self._update_gauge()

    def _finish_subscribers(self) -> None:
        for subscriber in tuple(self._subscribers):
            _force_end(subscriber, _END)
        self._subscribers.clear()
        self._update_gauge()

    def _update_gauge(self) -> None:
        self._metrics.gauge("ts_subscribers", self.camera.alias, len(self._subscribers))

    async def close(self) -> None:
        async with self._lock:
            self._closed = True
            if self._idle_task is not None:
                self._idle_task.cancel()
            task = self._run_task
            if task is not None:
                task.cancel()
        if task is not None:
            with suppress(asyncio.CancelledError):
                await task
