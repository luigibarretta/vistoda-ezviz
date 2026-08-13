"""Low-cardinality Prometheus metrics."""

from __future__ import annotations

import threading
from collections import defaultdict


class Metrics:
    """Small dependency-free metrics store."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._counters: dict[tuple[str, str], int] = defaultdict(int)
        self._gauges: dict[tuple[str, str], int | float] = {}

    def increment(self, name: str, camera: str, value: int = 1) -> None:
        with self._lock:
            self._counters[(name, camera)] += value

    def gauge(self, name: str, camera: str, value: int | float) -> None:
        with self._lock:
            self._gauges[(name, camera)] = value

    def render(self) -> str:
        with self._lock:
            counters = sorted(self._counters.items())
            gauges = sorted(self._gauges.items())
        lines = [
            f'ezviz_bridge_{name}{{camera="{camera}"}} {value}'
            for (name, camera), value in counters + gauges
        ]
        return "\n".join(lines) + ("\n" if lines else "")
