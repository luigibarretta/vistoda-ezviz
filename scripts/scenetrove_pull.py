#!/usr/bin/env python3
"""Pull one finite bridge recording into a SceneTrove MPEG-PS device folder."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from contextlib import suppress
from datetime import datetime
from pathlib import Path
from typing import Any, BinaryIO

from ezviz_vtm_bridge.config import read_secret_file
from ezviz_vtm_bridge.recordings import PACK_START

JSON_LIMIT = 1024 * 1024


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Prevent a redirect from forwarding the bridge credential elsewhere."""

    def redirect_request(self, *_args: Any, **_kwargs: Any) -> None:
        return None


def validated_base_url(value: str) -> str:
    parsed = urllib.parse.urlsplit(value)
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise ValueError("base URL must be an absolute HTTP(S) URL")
    if parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError("base URL cannot contain credentials, query or fragment")
    return value.rstrip("/")


def request_json(
    opener: urllib.request.OpenerDirector,
    request: urllib.request.Request,
    *,
    timeout: float,
) -> dict[str, Any]:
    with opener.open(request, timeout=timeout) as response:
        payload = response.read(JSON_LIMIT + 1)
    if len(payload) > JSON_LIMIT:
        raise ValueError("bridge JSON response exceeded its bound")
    value = json.loads(payload)
    if not isinstance(value, dict):
        raise ValueError("bridge JSON response is not an object")
    return value


def write_media(
    source: BinaryIO, destination: Path, *, max_bytes: int
) -> tuple[int, str]:
    descriptor = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    digest = hashlib.sha256()
    written = 0
    prefix = bytearray()
    try:
        with os.fdopen(descriptor, "wb") as output:
            while chunk := source.read(64 * 1024):
                written += len(chunk)
                if written > max_bytes:
                    raise ValueError("recording exceeded download byte bound")
                if len(prefix) < len(PACK_START):
                    prefix.extend(chunk[: len(PACK_START) - len(prefix)])
                digest.update(chunk)
                output.write(chunk)
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        destination.unlink(missing_ok=True)
        raise
    if bytes(prefix) != PACK_START:
        destination.unlink(missing_ok=True)
        raise ValueError("recording does not start at an MPEG-PS pack")
    return written, digest.hexdigest()


def pull(  # noqa: PLR0913 - the consumer contract stays explicit
    *,
    base_url: str,
    camera: str,
    duration_seconds: int,
    idempotency_key: str,
    token: str,
    destination: Path,
    timeout_seconds: float,
    max_bytes: int,
) -> dict[str, Any]:
    base = validated_base_url(base_url)
    authorization = {"Authorization": f"Bearer {token}"}
    opener = urllib.request.build_opener(NoRedirect())
    body = json.dumps({"duration_seconds": duration_seconds}).encode()
    create = urllib.request.Request(  # noqa: S310 - validated base URL
        f"{base}/v1/cameras/{urllib.parse.quote(camera, safe='')}/recordings",
        data=body,
        headers={
            **authorization,
            "Content-Type": "application/json",
            "Idempotency-Key": idempotency_key,
        },
        method="POST",
    )
    manifest = request_json(opener, create, timeout=timeout_seconds)
    recording_id = str(manifest.get("recording_id", ""))
    if not recording_id:
        raise ValueError("bridge omitted recording_id")
    deadline = time.monotonic() + timeout_seconds
    while manifest.get("status") not in {"ready", "failed"}:
        if time.monotonic() >= deadline:
            raise TimeoutError("recording did not finish before deadline")
        time.sleep(0.5)
        status = urllib.request.Request(  # noqa: S310 - validated base URL
            f"{base}/v1/recordings/{urllib.parse.quote(recording_id, safe='')}",
            headers=authorization,
        )
        manifest = request_json(opener, status, timeout=timeout_seconds)
    if manifest.get("status") != "ready":
        raise RuntimeError(f"bridge recording failed: {manifest.get('error_code', 'unknown')}")
    expected_bytes = int(manifest.get("bytes", -1))
    expected_digest = str(manifest.get("sha256", ""))
    if not 0 < expected_bytes <= max_bytes or len(expected_digest) != 64:
        raise ValueError("recording manifest bounds or digest are invalid")
    completed = datetime.fromisoformat(str(manifest["completed_at"]).replace("Z", "+00:00"))
    filename = f"hiv{completed:%Y%m%dT%H%M%SZ}-{recording_id}.mp4"
    destination.mkdir(mode=0o700, parents=True, exist_ok=True)
    final_path = destination / filename
    partial = destination / f".{filename}.partial"
    media = urllib.request.Request(  # noqa: S310 - validated base URL
        f"{base}/v1/recordings/{urllib.parse.quote(recording_id, safe='')}/media",
        headers=authorization,
    )
    try:
        with opener.open(media, timeout=timeout_seconds) as response:
            actual_bytes, actual_digest = write_media(response, partial, max_bytes=max_bytes)
        if actual_bytes != expected_bytes or actual_digest != expected_digest:
            raise ValueError("recording media does not match its manifest")
        os.replace(partial, final_path)
        directory = os.open(destination, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        with suppress(FileNotFoundError):
            partial.unlink()
    return {
        "recording_id": recording_id,
        "path": str(final_path),
        "bytes": actual_bytes,
        "sha256": actual_digest,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--camera", required=True)
    parser.add_argument("--duration", type=int, required=True)
    parser.add_argument("--idempotency-key", required=True)
    parser.add_argument("--token-file", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--max-bytes", type=int, default=256 * 1024 * 1024)
    arguments = parser.parse_args()
    result = pull(
        base_url=arguments.base_url,
        camera=arguments.camera,
        duration_seconds=arguments.duration,
        idempotency_key=arguments.idempotency_key,
        token=read_secret_file(arguments.token_file, minimum_length=32),
        destination=arguments.destination,
        timeout_seconds=arguments.timeout,
        max_bytes=arguments.max_bytes,
    )
    sys.stdout.write(json.dumps(result, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
