#!/usr/bin/env python3
"""Fail when a human-maintained source file exceeds the LOC budget."""

from __future__ import annotations

import sys
from pathlib import Path

MAX_LINES = 300
SUFFIXES = {".py", ".md", ".yaml", ".yml", ".toml", ".json"}
NAMES = {"Dockerfile"}
EXCLUDED_PARTS = {".git", ".mypy_cache", ".pytest_cache", ".ruff_cache", ".venv"}
EXCLUDED_NAMES = {"uv.lock"}


def maintained_files(root: Path) -> list[Path]:
    return sorted(
        path
        for path in root.rglob("*")
        if path.is_file()
        and path.name not in EXCLUDED_NAMES
        and not EXCLUDED_PARTS.intersection(path.parts)
        and (path.suffix in SUFFIXES or path.name in NAMES)
    )


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    violations: list[str] = []
    for path in maintained_files(root):
        lines = sum(1 for _line in path.open(encoding="utf-8"))
        if lines > MAX_LINES:
            violations.append(f"{path.relative_to(root)}: {lines} > {MAX_LINES}")
    if violations:
        sys.stderr.write("LOC budget exceeded:\n" + "\n".join(violations) + "\n")
        return 1
    sys.stdout.write(f"LOC guard passed: every maintained file is <= {MAX_LINES} lines\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
