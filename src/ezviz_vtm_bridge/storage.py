"""Durable atomic persistence helpers."""

from __future__ import annotations

import json
import os
import tempfile
from contextlib import suppress
from pathlib import Path
from typing import Any


def atomic_write_json(path: Path, payload: Any, *, mode: int = 0o600) -> None:
    """Replace a JSON file only after its contents and directory are durable."""
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    open_descriptor = descriptor
    try:
        os.fchmod(descriptor, mode)
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            open_descriptor = -1
            json.dump(payload, handle, separators=(",", ":"), sort_keys=True)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary_name, path)
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    except BaseException:
        if open_descriptor >= 0:
            with suppress(OSError):
                os.close(open_descriptor)
        with suppress(FileNotFoundError):
            os.unlink(temporary_name)
        raise
