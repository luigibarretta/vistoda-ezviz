"""One-upstream, bounded raw media fan-out."""

from __future__ import annotations

import asyncio
import queue
import threading
import time
from collections.abc import AsyncIterator
from contextlib import suppress
from typing import Final

from .config import CameraConfig
from .errors import CapacityError, StreamStoppedError
from .metrics import Metrics
from .transport import CameraTransport

_END: Final = object()


def _force_end(target: asyncio.Queue[bytes | object], end: object = _END) -> None:
    """Terminate a bounded subscriber even when its queue is full."""
    if target.full():
        with suppress(asyncio.QueueEmpty):
            target.get_nowait()
    target.put_nowait(end)


class BoundedThreadWriter:
    """Synchronous writer backed by a bounded cross-thread queue."""

    def __init__(
        self,
        target: queue.Queue[bytes | object],
        stop_event: threading.Event,
        *,
        chunk_bytes: int = 64 * 1024,
    ) -> None:
        self._target = target
        self._stop = stop_event
        self._chunk_bytes = chunk_bytes

    def write(self, data: bytes) -> int:
        view = memoryview(data)
        for offset in range(0, len(view), self._chunk_bytes):
            chunk = bytes(view[offset : offset + self._chunk_bytes])
            while not self._stop.is_set():
                try:
                    self._target.put(chunk, timeout=0.25)
                    break
                except queue.Full:
                    continue
            else:
                raise StreamStoppedError("stream stopped")
        return len(data)

    def flush(self) -> None:
        if self._stop.is_set():
            raise StreamStoppedError("stream stopped")


class RawStreamHub:
    """Own exactly one blocking vendor stream for one camera."""

    def __init__(  # noqa: PLR0913 - independent resource bounds stay explicit
        self,
        camera: CameraConfig,
        transport: CameraTransport,
        metrics: Metrics,
        *,
        subscriber_queue_chunks: int,
        thread_queue_chunks: int,
        max_subscribers: int,
        idle_grace_seconds: float,
    ) -> None:
        self.camera = camera
        self._transport = transport
        self._metrics = metrics
        self._subscriber_queue_chunks = subscriber_queue_chunks
        self._thread_queue: queue.Queue[bytes | object] = queue.Queue(thread_queue_chunks)
        self._max_subscribers = max_subscribers
        self._idle_grace = idle_grace_seconds
        self._subscribers: set[asyncio.Queue[bytes | object]] = set()
        self._lock = asyncio.Lock()
        self._stop_event = threading.Event()
        self._run_task: asyncio.Task[None] | None = None
        self._idle_task: asyncio.Task[None] | None = None
        self._closed = False
        self._next_start = 0.0
        self._failures = 0

    @property
    def subscriber_count(self) -> int:
        return len(self._subscribers)

    @property
    def running(self) -> bool:
        return self._run_task is not None and not self._run_task.done()

    async def iter_chunks(self) -> AsyncIterator[bytes]:
        subscriber: asyncio.Queue[bytes | object] = asyncio.Queue(
            self._subscriber_queue_chunks
        )
        async with self._lock:
            if self._closed:
                raise RuntimeError("stream hub is closed")
            if len(self._subscribers) >= self._max_subscribers:
                raise CapacityError("maximum stream subscribers reached")
            self._subscribers.add(subscriber)
            self._cancel_idle_locked()
            self._ensure_running_locked()
            self._update_gauges()
        try:
            while True:
                item = await subscriber.get()
                if item is _END:
                    break
                yield item  # type: ignore[misc]
        finally:
            async with self._lock:
                self._subscribers.discard(subscriber)
                self._update_gauges()
                if not self._subscribers and self.running:
                    self._idle_task = asyncio.create_task(self._stop_after_idle())

    def _ensure_running_locked(self) -> None:
        if self.running:
            return
        self._stop_event = threading.Event()
        self._thread_queue = queue.Queue(self._thread_queue.maxsize)
        self._run_task = asyncio.create_task(self._run(), name=f"raw-stream-{self.camera.alias}")

    def _cancel_idle_locked(self) -> None:
        if self._idle_task is not None:
            self._idle_task.cancel()
            self._idle_task = None

    async def _stop_after_idle(self) -> None:
        try:
            await asyncio.sleep(self._idle_grace)
            async with self._lock:
                if not self._subscribers:
                    self._stop_event.set()
        except asyncio.CancelledError:
            return

    async def _run(self) -> None:
        delay = max(0.0, self._next_start - time.monotonic())
        if delay:
            await asyncio.sleep(delay)
        self._metrics.increment("upstream_starts_total", self.camera.alias)
        self._metrics.gauge("upstream_active", self.camera.alias, 1)
        producer = asyncio.create_task(asyncio.to_thread(self._produce))
        failed = False
        try:
            while True:
                item = await asyncio.to_thread(self._thread_queue.get)
                if item is _END:
                    break
                self._publish(item)  # type: ignore[arg-type]
        except asyncio.CancelledError:
            self._stop_event.set()
            raise
        finally:
            self._stop_event.set()
            try:
                await asyncio.wait_for(producer, timeout=5)
            except (TimeoutError, asyncio.CancelledError):
                producer.cancel()
                failed = True
            except Exception:  # noqa: BLE001 - vendor errors are reduced to metrics
                failed = True
            if failed:
                self._failures += 1
                self._metrics.increment("upstream_failures_total", self.camera.alias)
                backoff = 600.0 if self._failures >= 3 else float(2**self._failures)
                self._next_start = time.monotonic() + backoff
            else:
                self._failures = 0
                self._next_start = 0.0
            self._metrics.gauge("upstream_active", self.camera.alias, 0)
            self._finish_subscribers()

    def _produce(self) -> None:
        writer = BoundedThreadWriter(self._thread_queue, self._stop_event)
        try:
            self._transport.stream_mpeg_ps(self.camera, writer, self._stop_event)
        except StreamStoppedError:
            pass
        finally:
            try:
                self._thread_queue.put_nowait(_END)
            except queue.Full:
                with suppress(queue.Empty):
                    self._thread_queue.get_nowait()
                with suppress(queue.Full):
                    self._thread_queue.put_nowait(_END)

    def _publish(self, chunk: bytes) -> None:
        slow: list[asyncio.Queue[bytes | object]] = []
        for subscriber in tuple(self._subscribers):
            try:
                subscriber.put_nowait(chunk)
            except asyncio.QueueFull:
                slow.append(subscriber)
        for subscriber in slow:
            self._subscribers.discard(subscriber)
            _force_end(subscriber)
            self._metrics.increment("slow_subscribers_total", self.camera.alias)
        if slow:
            self._update_gauges()

    def _finish_subscribers(self) -> None:
        for subscriber in tuple(self._subscribers):
            _force_end(subscriber)
        self._subscribers.clear()
        self._update_gauges()

    def _update_gauges(self) -> None:
        self._metrics.gauge("raw_subscribers", self.camera.alias, len(self._subscribers))

    async def close(self) -> None:
        async with self._lock:
            self._closed = True
            self._cancel_idle_locked()
            self._stop_event.set()
            task = self._run_task
        if task is not None:
            try:
                await asyncio.wait_for(task, timeout=8)
            except TimeoutError:
                task.cancel()
                with suppress(asyncio.CancelledError):
                    await task
