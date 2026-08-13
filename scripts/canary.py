#!/usr/bin/env python3
"""Opt-in deployed-service canary; read the bridge API token from stdin."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path


def request(url: str, token: str, timeout: float) -> urllib.request.addinfourl:
    value = urllib.request.Request(  # noqa: S310 - explicit operator-provided HTTP(S) URL
        url, headers={"Authorization": f"Bearer {token}"}
    )
    return urllib.request.urlopen(value, timeout=timeout)  # noqa: S310 - operator URL


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--camera", required=True)
    parser.add_argument("--seconds", type=int, default=20)
    parser.add_argument("--max-bytes", type=int, default=64 * 1024 * 1024)
    parser.add_argument("--stream-format", choices=("mpegps", "ts"), default="mpegps")
    parser.add_argument("--skip-snapshot", action="store_true")
    arguments = parser.parse_args()
    token = sys.stdin.readline().strip()
    if len(token) < 32:
        raise SystemExit("API token missing on standard input")
    base = arguments.base_url.rstrip("/")
    image = b""
    if not arguments.skip_snapshot:
        with request(
            f"{base}/v1/cameras/{arguments.camera}/snapshot.jpg", token, 30
        ) as result:
            image = result.read(16 * 1024 * 1024 + 1)
        if len(image) > 16 * 1024 * 1024 or not image.startswith(b"\xff\xd8\xff"):
            raise SystemExit("snapshot validation failed")
    with tempfile.TemporaryDirectory(prefix="ezviz-bridge-canary-") as directory:
        media = Path(directory) / f"live.{arguments.stream_format}"
        deadline = time.monotonic() + arguments.seconds
        total = 0
        with (
            request(
                f"{base}/v1/cameras/{arguments.camera}/live.{arguments.stream_format}",
                token,
                60,
            ) as result,
            media.open("wb") as output,
        ):
            while time.monotonic() < deadline:
                chunk = result.read(64 * 1024)
                if not chunk:
                    break
                total += len(chunk)
                if total > arguments.max_bytes:
                    raise SystemExit("live canary exceeded byte bound")
                output.write(chunk)
        if total == 0:
            raise SystemExit("live canary returned no media")
        probe = subprocess.run(  # noqa: S603 - fixed executable and arguments
            [
                "/usr/bin/ffprobe",
                "-v",
                "error",
                "-show_entries",
                "stream=codec_name,codec_type",
                "-of",
                "json",
                str(media),
            ],
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        )
        streams = json.loads(probe.stdout).get("streams", [])
        if not any(stream.get("codec_type") == "video" for stream in streams):
            raise SystemExit("ffprobe found no video stream")
    sys.stdout.write(json.dumps({
        "status": "ok",
        "snapshot_bytes": len(image),
        "stream_bytes": total,
        "streams": streams,
    }, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
