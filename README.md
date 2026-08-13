# EZVIZ VTM Bridge

Vendor-isolated bridge for modern EZVIZ cameras that expose cloud VTM/VTDU
media but no usable RTSP listener. It turns one authenticated upstream camera
session into bounded, authenticated outputs for Home Assistant and SceneTrove.

## Outputs

- original MPEG-PS (`video/mpeg`) for SceneTrove ingestion;
- remux-only MPEG-TS (`video/mp2t`) for Home Assistant and generic players;
- fresh JPEG snapshots;
- finite MPEG-PS recordings with immutable manifests and SHA-256 digests;
- health and Prometheus metrics without camera serials, tokens, URLs or media.

The bridge does not transcode video, record continuously, or expose an Internet
route. `pyezvizapi` is the replaceable vendor transport. Consumers depend only
on the versioned HTTP contract.

## Development

```bash
uv sync --all-groups
uv run ruff check .
uv run python scripts/check_loc.py
uv run mypy src
uv run pytest
```

No live EZVIZ credentials are required by the test suite. Live canaries are
separate, opt-in and consume token JSON through standard input.

Every human-maintained source, configuration and documentation file has an
enforced maximum of 300 physical lines. Split responsibilities instead of
adding exceptions; generated dependency locks are the only excluded content.

See [the implementation plan](docs/PLAN.md), [the threat model](docs/THREAT_MODEL.md)
and [the ADR index](docs/adr/README.md).

## License

Apache-2.0. The project depends on the Apache-2.0 `pyezvizapi` package. The
LGPL-2.1 LE-EZVIZ-VS project is credited as protocol research only; no source
is copied from it.
