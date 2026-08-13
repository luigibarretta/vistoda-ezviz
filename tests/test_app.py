from __future__ import annotations

import asyncio
from base64 import b64encode

import pytest
from aiohttp import web
from aiohttp.test_utils import TestClient, TestServer

from ezviz_vtm_bridge.app import create_app, safe_error_middleware
from ezviz_vtm_bridge.config import BridgeConfig
from ezviz_vtm_bridge.errors import CapacityError

from .fakes import JPEG, FakeTransport

TOKEN = "a" * 48


async def client_for(config: BridgeConfig, transport: FakeTransport) -> TestClient:
    client = TestClient(TestServer(create_app(config, transport)))
    await client.start_server()
    return client


async def test_health_is_minimal_and_public(config: BridgeConfig) -> None:
    client = await client_for(config, FakeTransport())
    try:
        response = await client.get("/healthz")
        assert response.status == 200
        assert await response.json() == {"status": "ok", "version": "0.1.0"}
    finally:
        await client.close()


async def test_media_and_metrics_require_exact_bearer(config: BridgeConfig) -> None:
    client = await client_for(config, FakeTransport())
    try:
        for path in ("/metrics", "/v1/cameras/front-door/snapshot.jpg"):
            response = await client.get(path)
            assert response.status == 401
            assert response.headers["WWW-Authenticate"] == 'Basic realm="ezviz-vtm-bridge"'
            assert "a" * 16 not in await response.text()
        response = await client.get(
            "/v1/cameras/front-door/snapshot.jpg",
            headers={"Authorization": f"Bearer {TOKEN}"},
        )
        assert response.status == 200
        assert await response.read() == JPEG
        assert response.headers["Cache-Control"] == "no-store"
        basic = b64encode(f"homeassistant:{TOKEN}".encode()).decode()
        response = await client.get(
            "/v1/cameras/front-door/snapshot.jpg",
            headers={"Authorization": f"Basic {basic}"},
        )
        assert response.status == 200
    finally:
        await client.close()


async def test_unknown_camera_does_not_disclose_configuration(config: BridgeConfig) -> None:
    client = await client_for(config, FakeTransport())
    try:
        response = await client.get(
            "/v1/cameras/unknown/snapshot.jpg",
            headers={"Authorization": f"Bearer {TOKEN}"},
        )
        assert response.status == 404
        body = await response.text()
        assert "TEST-SERIAL" not in body
    finally:
        await client.close()


async def test_raw_stream_is_chunked_and_disconnects_cleanly(config: BridgeConfig) -> None:
    transport = FakeTransport()
    client = await client_for(config, transport)
    try:
        response = await client.get(
            "/v1/cameras/front-door/live.mpegps",
            headers={"Authorization": f"Bearer {TOKEN}"},
        )
        assert response.status == 200
        assert response.headers["Content-Type"] == "video/mpeg"
        assert await response.content.readexactly(16)
        response.close()
        await asyncio.sleep(0.05)
        assert transport.active == 0
    finally:
        await client.close()


async def test_recording_api_is_idempotent(config: BridgeConfig) -> None:
    transport = FakeTransport()
    client = await client_for(config, transport)
    headers = {"Authorization": f"Bearer {TOKEN}", "Idempotency-Key": "api-request-001"}
    try:
        first = await client.post(
            "/v1/cameras/front-door/recordings",
            headers=headers,
            json={"duration_seconds": 1},
        )
        assert first.status == 202
        first_manifest = await first.json()
        second = await client.post(
            "/v1/cameras/front-door/recordings",
            headers=headers,
            json={"duration_seconds": 1},
        )
        assert (await second.json())["recording_id"] == first_manifest["recording_id"]
        recording_id = first_manifest["recording_id"]
        for _ in range(300):
            status = await client.get(
                f"/v1/recordings/{recording_id}",
                headers={"Authorization": f"Bearer {TOKEN}"},
            )
            manifest = await status.json()
            if manifest["status"] in {"ready", "failed"}:
                break
            await asyncio.sleep(0.01)
        assert manifest["status"] == "ready"
        media = await client.get(
            f"/v1/recordings/{recording_id}/media",
            headers={"Authorization": f"Bearer {TOKEN}"},
        )
        assert media.status == 200
        assert (await media.read()).startswith(b"\x00\x00\x01\xba")
    finally:
        await client.close()


async def test_recording_rejects_extra_fields(config: BridgeConfig) -> None:
    client = await client_for(config, FakeTransport())
    try:
        response = await client.post(
            "/v1/cameras/front-door/recordings",
            headers={"Authorization": f"Bearer {TOKEN}", "Idempotency-Key": "api-request-002"},
            json={"duration_seconds": 1, "upstream_url": "http://attacker"},
        )
        assert response.status == 400
    finally:
        await client.close()


async def test_metrics_and_missing_recordings(config: BridgeConfig) -> None:
    client = await client_for(config, FakeTransport())
    headers = {"Authorization": f"Bearer {TOKEN}"}
    try:
        metrics_response = await client.get("/metrics", headers=headers)
        assert metrics_response.status == 200
        assert await metrics_response.text() == ""
        missing = await client.get("/v1/recordings/missing", headers=headers)
        assert missing.status == 404
        media = await client.get("/v1/recordings/missing/media", headers=headers)
        assert media.status == 404
    finally:
        await client.close()


async def test_safe_error_middleware_reduces_internal_failures() -> None:
    async def at_capacity(_request: web.Request) -> web.StreamResponse:
        raise CapacityError("secret capacity detail")

    async def unexpected(_request: web.Request) -> web.StreamResponse:
        raise RuntimeError("secret upstream detail")

    request = object()
    with pytest.raises(web.HTTPTooManyRequests) as capacity_error:
        await safe_error_middleware(request, at_capacity)  # type: ignore[arg-type]
    assert capacity_error.value.text == '{"error":"capacity"}'
    with pytest.raises(web.HTTPInternalServerError) as internal_error:
        await safe_error_middleware(request, unexpected)  # type: ignore[arg-type]
    assert internal_error.value.text == '{"error":"internal_error"}'


@pytest.mark.parametrize(
    ("body", "content_type"),
    [
        (b"not-json", "application/json"),
        (b'{"duration_seconds":true}', "application/json"),
    ],
)
async def test_recording_rejects_invalid_json_types(
    config: BridgeConfig, body: bytes, content_type: str
) -> None:
    client = await client_for(config, FakeTransport())
    try:
        response = await client.post(
            "/v1/cameras/front-door/recordings",
            headers={
                "Authorization": f"Bearer {TOKEN}",
                "Idempotency-Key": "api-request-003",
                "Content-Type": content_type,
            },
            data=body,
        )
        assert response.status == 400
    finally:
        await client.close()
