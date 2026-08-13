"""Replaceable EZVIZ vendor transport."""

from __future__ import annotations

import io
import json
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol

from pyezvizapi.client import EzvizClient
from pyezvizapi.cloud_stream import copy_cloud_stream_to_mpegps
from pyezvizapi.exceptions import EzvizAuthVerificationCode

from .config import CameraConfig, read_secret_file
from .errors import ConfigurationError, StreamStoppedError
from .storage import atomic_write_json


class BinarySink(Protocol):
    """Minimal file-like target accepted by the vendor stream functions."""

    def write(self, data: bytes) -> int: ...

    def flush(self) -> None: ...


class CameraTransport(Protocol):
    """The only vendor-specific boundary used by the service."""

    def snapshot_jpeg(self, camera: CameraConfig) -> bytes:
        """Return a fresh decodable JPEG."""

    def stream_mpeg_ps(
        self, camera: CameraConfig, output: BinarySink, stop_event: threading.Event
    ) -> None:
        """Write MPEG-PS until stopped or the upstream fails."""

    def close(self) -> None:
        """Release vendor resources."""


class StopAwareWriter:
    """Turn an external stop event into a synchronous writer cancellation."""

    def __init__(self, output: BinarySink, stop_event: threading.Event) -> None:
        self._output = output
        self._stop = stop_event

    def write(self, data: bytes) -> int:
        if self._stop.is_set():
            raise StreamStoppedError("stream stopped")
        return self._output.write(data)

    def flush(self) -> None:
        if self._stop.is_set():
            raise StreamStoppedError("stream stopped")
        self._output.flush()


@dataclass(slots=True)
class PyezvizTransport:
    """Pinned pyezvizapi implementation."""

    client: Any
    token_path: Path
    timeout_seconds: float

    @classmethod
    def enroll(  # noqa: PLR0913 - explicit enrollment inputs avoid hidden state
        cls,
        *,
        account: str,
        password: str,
        api_region: str,
        token_path: Path,
        timeout_seconds: float,
        mfa_code: int | None = None,
    ) -> dict[str, Any]:
        """Create a dedicated session without persisting account credentials."""
        client = EzvizClient(
            account=account,
            password=password,
            url=api_region,
            timeout=int(timeout_seconds),
        )
        try:
            try:
                token = client.login()
            except EzvizAuthVerificationCode:
                if mfa_code is None:
                    raise
                token = client.login(mfa_code)
            cls._atomic_token_write(token_path, token)
            return token
        finally:
            close = getattr(client, "close_session", None)
            if callable(close):
                close()

    @classmethod
    def from_token_file(cls, token_path: Path, timeout_seconds: float) -> PyezvizTransport:
        raw = read_secret_file(token_path, minimum_length=20)
        try:
            token = json.loads(raw)
        except json.JSONDecodeError as error:
            raise ConfigurationError("EZVIZ token file is invalid") from error
        required = {"session_id", "rf_session_id", "api_url"}
        if not isinstance(token, dict) or not required.issubset(token):
            raise ConfigurationError("EZVIZ token file is incomplete")
        client = EzvizClient(token=token, timeout=int(timeout_seconds))
        refreshed = client.login()
        cls._atomic_token_write(token_path, refreshed)
        return cls(client=client, token_path=token_path, timeout_seconds=timeout_seconds)

    @staticmethod
    def _atomic_token_write(path: Path, token: dict[str, Any]) -> None:
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        atomic_write_json(path, token)

    def snapshot_jpeg(self, camera: CameraConfig) -> bytes:
        output = io.BytesIO()
        self.client.save_image(camera.serial, output, channel=1, decrypt=True)
        image = output.getvalue()
        start = image.find(b"\xff\xd8\xff")
        end = image.rfind(b"\xff\xd9")
        if start < 0 or end < start:
            raise ValueError("snapshot payload is not a JPEG")
        return image[start : end + 2]

    def stream_mpeg_ps(
        self, camera: CameraConfig, output: BinarySink, stop_event: threading.Event
    ) -> None:
        media_key = None
        if camera.media_key_file is not None:
            media_key = read_secret_file(camera.media_key_file, minimum_length=6)
        copy_cloud_stream_to_mpegps(
            self.client,
            camera.serial,
            StopAwareWriter(output, stop_event),  # type: ignore[arg-type]
            channel=1,
            timeout=self.timeout_seconds,
            duration_seconds=None,
            max_packets=None,
            decrypt_video=camera.decrypt_video,
            media_key=media_key,
        )

    def close(self) -> None:
        close = getattr(self.client, "close_session", None)
        if callable(close):
            close()
