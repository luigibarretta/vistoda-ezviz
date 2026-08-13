from __future__ import annotations

import io
import json
import threading
from pathlib import Path
from unittest.mock import Mock

import pytest
from pyezvizapi.exceptions import EzvizAuthVerificationCode

from ezviz_vtm_bridge.config import CameraConfig
from ezviz_vtm_bridge.errors import ConfigurationError, StreamStoppedError
from ezviz_vtm_bridge.transport import PyezvizTransport, StopAwareWriter


class SnapshotClient:
    def __init__(self, payload: bytes) -> None:
        self.payload = payload

    def save_image(self, _serial: str, output: io.BytesIO, **_kwargs: object) -> None:
        output.write(self.payload)


def test_snapshot_strips_vendor_prefix_and_suffix() -> None:
    jpeg = b"\xff\xd8\xffimage\xff\xd9"
    transport = PyezvizTransport(SnapshotClient(b"prefix" + jpeg + b"suffix"), Path("x"), 10)
    assert transport.snapshot_jpeg(CameraConfig("front", "ABC")) == jpeg


def test_snapshot_rejects_invalid_payload() -> None:
    transport = PyezvizTransport(SnapshotClient(b"not-an-image"), Path("x"), 10)
    with pytest.raises(ValueError, match="JPEG"):
        transport.snapshot_jpeg(CameraConfig("front", "ABC"))


def test_stop_aware_writer_cancels_before_write() -> None:
    output = io.BytesIO()
    stop = threading.Event()
    writer = StopAwareWriter(output, stop)
    assert writer.write(b"ok") == 2
    writer.flush()
    stop.set()
    with pytest.raises(StreamStoppedError):
        writer.write(b"blocked")
    with pytest.raises(StreamStoppedError):
        writer.flush()


def test_atomic_token_write_uses_private_mode(tmp_path: Path) -> None:
    path = tmp_path / "tokens" / "token.json"
    PyezvizTransport._atomic_token_write(path, {"session_id": "secret"})
    assert json.loads(path.read_text(encoding="utf-8"))["session_id"] == "secret"
    assert path.stat().st_mode & 0o777 == 0o600


def test_token_loader_rejects_incomplete_file(tmp_path: Path) -> None:
    path = tmp_path / "token.json"
    path.write_text('{"session_id":"long-but-incomplete-value"}', encoding="utf-8")
    path.chmod(0o600)
    with pytest.raises(ConfigurationError, match="incomplete"):
        PyezvizTransport.from_token_file(path, 10)


def test_token_loader_refreshes_and_persists(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "token.json"
    token = {"session_id": "s" * 20, "rf_session_id": "r" * 20, "api_url": "api.example"}
    path.write_text(json.dumps(token), encoding="utf-8")
    path.chmod(0o600)
    client = Mock()
    client.login.return_value = {**token, "session_id": "n" * 20}
    constructor = Mock(return_value=client)
    monkeypatch.setattr("ezviz_vtm_bridge.transport.EzvizClient", constructor)
    transport = PyezvizTransport.from_token_file(path, 12.8)
    assert transport.client is client
    constructor.assert_called_once_with(token=token, timeout=12)
    assert json.loads(path.read_text(encoding="utf-8"))["session_id"] == "n" * 20


def test_transport_stream_passes_bounded_vendor_options(monkeypatch: pytest.MonkeyPatch) -> None:
    called: dict[str, object] = {}

    def fake_copy(client, serial, output, **kwargs):  # type: ignore[no-untyped-def]
        called.update(client=client, serial=serial, output=output, **kwargs)
        output.write(b"media")

    monkeypatch.setattr("ezviz_vtm_bridge.transport.copy_cloud_stream_to_mpegps", fake_copy)
    client = Mock()
    transport = PyezvizTransport(client, Path("unused"), 17)
    output = io.BytesIO()
    transport.stream_mpeg_ps(CameraConfig("front", "ABC"), output, threading.Event())
    assert output.getvalue() == b"media"
    assert called["serial"] == "ABC"
    assert called["timeout"] == 17
    assert called["duration_seconds"] is None
    transport.close()
    client.close_session.assert_called_once()


def test_transport_close_tolerates_client_without_close() -> None:
    PyezvizTransport(object(), Path("unused"), 10).close()


def test_enroll_handles_mfa_and_persists_only_token(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    token = {"session_id": "s", "rf_session_id": "r", "api_url": "api.example"}
    first_client = Mock()
    first_client.login.side_effect = EzvizAuthVerificationCode("MFA")
    second_client = Mock()
    second_client.login.side_effect = [EzvizAuthVerificationCode("MFA"), token]
    constructor = Mock(side_effect=[first_client, second_client])
    monkeypatch.setattr("ezviz_vtm_bridge.transport.EzvizClient", constructor)
    path = tmp_path / "token.json"
    with pytest.raises(EzvizAuthVerificationCode):
        PyezvizTransport.enroll(
            account="owner@example.test",
            password="not-persisted",  # noqa: S106 - synthetic test value
            api_region="apiieu.ezvizlife.com",
            token_path=path,
            timeout_seconds=25,
        )
    assert not path.exists()
    result = PyezvizTransport.enroll(
        account="owner@example.test",
        password="not-persisted",  # noqa: S106 - synthetic test value
        api_region="apiieu.ezvizlife.com",
        token_path=path,
        timeout_seconds=25,
        mfa_code=123456,
    )
    assert result == token
    persisted = path.read_text(encoding="utf-8")
    assert "not-persisted" not in persisted
    assert json.loads(persisted) == token
    first_client.close_session.assert_called_once()
    second_client.login.assert_any_call(123456)
    second_client.close_session.assert_called_once()
