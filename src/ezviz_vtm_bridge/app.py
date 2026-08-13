"""Authenticated HTTP API for bridge consumers."""

from __future__ import annotations

import asyncio
import json
import logging
from collections.abc import Awaitable, Callable
from contextlib import suppress
from dataclasses import dataclass
from pathlib import Path

from aiohttp import web

from . import __version__
from .auth import AUTHENTICATOR, ApiAuthenticator, authentication_middleware
from .config import BridgeConfig, read_secret_file
from .errors import CapacityError, RecordingError
from .metrics import Metrics
from .recordings import RecordingManager
from .snapshots import SnapshotService
from .streams import StreamManager
from .transport import CameraTransport

LOGGER = logging.getLogger("ezviz_vtm_bridge")


@dataclass(slots=True)
class Runtime:
    config: BridgeConfig
    transport: CameraTransport
    metrics: Metrics
    streams: StreamManager
    snapshots: SnapshotService
    recordings: RecordingManager


RUNTIME = web.AppKey("runtime", Runtime)


@web.middleware
async def safe_error_middleware(
    request: web.Request, handler: Callable[[web.Request], Awaitable[web.StreamResponse]]
) -> web.StreamResponse:
    try:
        return await handler(request)
    except web.HTTPException:
        raise
    except KeyError:
        raise web.HTTPNotFound(
            text='{"error":"camera_not_found"}', content_type="application/json"
        ) from None
    except CapacityError:
        raise web.HTTPTooManyRequests(
            text='{"error":"capacity"}', content_type="application/json"
        ) from None
    except RecordingError as error:
        raise web.HTTPBadRequest(
            text=json.dumps(
                {"error": "invalid_recording_request", "detail": str(error)},
                separators=(",", ":"),
            ),
            content_type="application/json",
        ) from None
    except Exception as error:  # noqa: BLE001
        LOGGER.error("request failed", extra={"error_type": type(error).__name__})
        raise web.HTTPInternalServerError(
            text='{"error":"internal_error"}', content_type="application/json"
        ) from None


async def health(_: web.Request) -> web.Response:
    return web.json_response({"status": "ok", "version": __version__})


async def metrics(request: web.Request) -> web.Response:
    runtime = request.app[RUNTIME]
    return web.Response(text=runtime.metrics.render(), content_type="text/plain")


async def snapshot(request: web.Request) -> web.Response:
    runtime = request.app[RUNTIME]
    alias = request.match_info["camera"]
    if alias not in runtime.config.cameras:
        raise KeyError(alias)
    image = await runtime.snapshots.get(alias)
    return web.Response(
        body=image,
        content_type="image/jpeg",
        headers={"Cache-Control": "no-store", "X-Content-Type-Options": "nosniff"},
    )


async def _stream(request: web.Request, *, mpeg_ts: bool) -> web.StreamResponse:
    runtime = request.app[RUNTIME]
    alias = request.match_info["camera"]
    hubs = runtime.streams.ts if mpeg_ts else runtime.streams.raw
    if alias not in hubs:
        raise KeyError(alias)
    response = web.StreamResponse(
        status=200,
        headers={
            "Content-Type": "video/mp2t" if mpeg_ts else "video/mpeg",
            "Cache-Control": "no-store",
            "X-Content-Type-Options": "nosniff",
        },
    )
    await response.prepare(request)
    runtime.metrics.increment("stream_requests_total", alias)
    try:
        async for chunk in hubs[alias].iter_chunks():
            await response.write(chunk)
    except (ConnectionError, asyncio.CancelledError):
        pass
    finally:
        with suppress(ConnectionError):
            await response.write_eof()
    return response


async def raw_stream(request: web.Request) -> web.StreamResponse:
    return await _stream(request, mpeg_ts=False)


async def ts_stream(request: web.Request) -> web.StreamResponse:
    return await _stream(request, mpeg_ts=True)


async def create_recording(request: web.Request) -> web.Response:
    runtime = request.app[RUNTIME]
    alias = request.match_info["camera"]
    idempotency_key = request.headers.get("Idempotency-Key", "")
    try:
        payload = await request.json()
    except (ValueError, web.HTTPException):
        raise RecordingError("request body must be JSON") from None
    if not isinstance(payload, dict) or set(payload) != {"duration_seconds"}:
        raise RecordingError("request must contain only duration_seconds")
    duration = payload["duration_seconds"]
    if not isinstance(duration, int) or isinstance(duration, bool):
        raise RecordingError("duration_seconds must be an integer")
    manifest = await runtime.recordings.start(alias, duration, idempotency_key)
    return web.json_response(manifest.public(), status=202)


async def get_recording(request: web.Request) -> web.Response:
    manifest = request.app[RUNTIME].recordings.get(request.match_info["recording_id"])
    if manifest is None:
        raise web.HTTPNotFound(text='{"error":"not_found"}', content_type="application/json")
    return web.json_response(manifest.public())


async def download_recording(request: web.Request) -> web.StreamResponse:
    path = request.app[RUNTIME].recordings.media_path(request.match_info["recording_id"])
    if path is None:
        raise web.HTTPNotFound(text='{"error":"not_ready"}', content_type="application/json")
    return web.FileResponse(
        path,
        headers={
            "Content-Type": "video/mpeg",
            "Cache-Control": "private, no-store",
            "X-Content-Type-Options": "nosniff",
        },
    )


async def close_runtime(app: web.Application) -> None:
    runtime = app[RUNTIME]
    await runtime.recordings.close()
    await runtime.streams.close()
    runtime.transport.close()


def create_app(config: BridgeConfig, transport: CameraTransport) -> web.Application:
    api_token = read_secret_file(config.api_token_file, minimum_length=32)
    metrics_store = Metrics()
    streams = StreamManager(
        config.cameras,
        transport,
        metrics_store,
        subscriber_queue_chunks=config.queue_chunks,
        thread_queue_chunks=config.cross_thread_chunks,
        max_subscribers=config.max_subscribers,
        idle_grace_seconds=config.idle_grace_seconds,
        ffmpeg_path=config.ffmpeg_path,
    )
    runtime = Runtime(
        config=config,
        transport=transport,
        metrics=metrics_store,
        streams=streams,
        snapshots=SnapshotService(
            config.cameras,
            transport,
            metrics_store,
            config.snapshot_cache_seconds,
            config.snapshot_stale_seconds,
        ),
        recordings=RecordingManager(
            config.data_dir / "recordings",
            streams.raw,
            metrics_store,
            max_duration_seconds=config.max_recording_seconds,
            max_recording_bytes=config.max_recording_bytes,
            quota_bytes=config.recording_quota_bytes,
        ),
    )
    app = web.Application(middlewares=[safe_error_middleware, authentication_middleware])
    app[RUNTIME] = runtime
    app[AUTHENTICATOR] = ApiAuthenticator(api_token)
    app.add_routes(
        [
            web.get("/healthz", health),
            web.get("/metrics", metrics),
            web.get("/v1/cameras/{camera}/snapshot.jpg", snapshot),
            web.get("/v1/cameras/{camera}/live.mpegps", raw_stream),
            web.get("/v1/cameras/{camera}/live.ts", ts_stream),
            web.post("/v1/cameras/{camera}/recordings", create_recording),
            web.get("/v1/recordings/{recording_id}", get_recording),
            web.get("/v1/recordings/{recording_id}/media", download_recording),
        ]
    )
    app.on_cleanup.append(close_runtime)
    return app


def token_path_from_config(config: BridgeConfig) -> Path:
    return config.ezviz_token_file
