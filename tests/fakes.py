from __future__ import annotations

import threading
import time

from ezviz_vtm_bridge.config import CameraConfig
from ezviz_vtm_bridge.errors import StreamStoppedError
from ezviz_vtm_bridge.transport import BinarySink

PACK = b"\x00\x00\x01\xba" + bytes(range(32))
JPEG = b"\xff\xd8\xff\xe0fake-jpeg\xff\xd9"


class FakeTransport:
    def __init__(self, *, interval: float = 0.001, finite_chunks: int | None = None) -> None:
        self.interval = interval
        self.finite_chunks = finite_chunks
        self.starts = 0
        self.active = 0
        self.max_active = 0
        self.snapshot_calls = 0
        self.closed = False

    def snapshot_jpeg(self, camera: CameraConfig) -> bytes:
        del camera
        self.snapshot_calls += 1
        return JPEG

    def stream_mpeg_ps(
        self, camera: CameraConfig, output: BinarySink, stop_event: threading.Event
    ) -> None:
        del camera
        self.starts += 1
        self.active += 1
        self.max_active = max(self.max_active, self.active)
        try:
            count = 0
            while not stop_event.is_set():
                output.write(PACK + count.to_bytes(4, "big"))
                output.flush()
                count += 1
                if self.finite_chunks is not None and count >= self.finite_chunks:
                    return
                time.sleep(self.interval)
            raise StreamStoppedError("stopped")
        finally:
            self.active -= 1

    def close(self) -> None:
        self.closed = True
