from __future__ import annotations

import json
import stat
from pathlib import Path

import pytest

from ezviz_vtm_bridge.storage import atomic_write_json


def test_atomic_json_replaces_content_with_private_mode(tmp_path: Path) -> None:
    target = tmp_path / "state.json"
    target.write_text("old", encoding="utf-8")
    atomic_write_json(target, {"z": 1, "a": [2]})
    assert target.read_text(encoding="utf-8") == '{"a":[2],"z":1}'
    assert stat.S_IMODE(target.stat().st_mode) == 0o600
    assert list(tmp_path.iterdir()) == [target]


def test_atomic_json_preserves_existing_file_on_serialization_error(tmp_path: Path) -> None:
    target = tmp_path / "state.json"
    target.write_text('{"stable":true}', encoding="utf-8")
    with pytest.raises(TypeError):
        atomic_write_json(target, {"invalid": object()})
    assert json.loads(target.read_text(encoding="utf-8")) == {"stable": True}
    assert list(tmp_path.iterdir()) == [target]
