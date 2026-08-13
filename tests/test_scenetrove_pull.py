from __future__ import annotations

import io
from pathlib import Path

import pytest

import scripts.scenetrove_pull as puller
from ezviz_vtm_bridge.recordings import PACK_START


def test_base_url_is_fail_closed() -> None:
    assert puller.validated_base_url("https://bridge.example/internal/") == (
        "https://bridge.example/internal"
    )
    for invalid in (
        "file:///etc/passwd",
        "bridge.local",
        "https://user:secret@bridge.example",
        "https://bridge.example?token=secret",
    ):
        with pytest.raises(ValueError):
            puller.validated_base_url(invalid)


def test_write_media_verifies_pack_and_digest(tmp_path: Path) -> None:
    payload = PACK_START + b"media"
    path = tmp_path / "capture.partial"
    written, digest = puller.write_media(io.BytesIO(payload), path, max_bytes=1024)
    assert written == len(payload)
    assert len(digest) == 64
    assert path.read_bytes() == payload


def test_write_media_removes_invalid_or_oversized_partial(tmp_path: Path) -> None:
    invalid = tmp_path / "invalid.partial"
    with pytest.raises(ValueError, match="pack"):
        puller.write_media(io.BytesIO(b"invalid"), invalid, max_bytes=1024)
    assert not invalid.exists()
    oversized = tmp_path / "oversized.partial"
    with pytest.raises(ValueError, match="byte bound"):
        puller.write_media(io.BytesIO(PACK_START + b"x" * 20), oversized, max_bytes=8)
    assert not oversized.exists()
