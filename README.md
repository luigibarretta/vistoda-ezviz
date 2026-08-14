# EZVIZ VTM Bridge

Production-oriented media bridge for modern EZVIZ cameras that expose cloud
VTM/VTDU media but no usable RTSP listener. One authenticated upstream session
is shared across bounded, authenticated outputs for Home Assistant and
SceneTrove.

## Why it exists

Some EZVIZ devices work in the vendor app while offering no accessible LAN
stream. Embedding the evolving vendor protocol separately in every consumer
would spread credentials, failure modes and proprietary details. This bridge
keeps that dependency behind a small, versioned HTTP contract.

It does not transcode video, record continuously, expose an Internet route or
modify camera firmware.

## Capabilities

- canonical MPEG-PS (`video/mpeg`) for SceneTrove ingestion;
- copy-remuxed MPEG-TS (`video/mp2t`) for Home Assistant and media clients;
- fresh JPEG snapshots with short request coalescing;
- finite MPEG-PS recordings with immutable manifests and SHA-256 digests;
- one lazy upstream shared by multiple bounded consumers;
- hard live-client lifetime limits that protect battery-powered cameras;
- health and Prometheus metrics without serials, tokens, URLs or media.

## Architecture

```text
EZVIZ VTM/VTDU
      |
      v
native Rust transport -> bounded raw hub -> MPEG-PS / recordings / snapshots
                              |
                              +-> shared FFmpeg copy-remux -> MPEG-TS
```

The transport and operational tooling implement the required VTM/VTDU wire
subset directly in safe Rust; no Python source or vendor SDK is present.
Consumers depend only on
[`openapi.yaml`](openapi.yaml). Architectural choices and consequences are
indexed in [`docs/adr/README.md`](docs/adr/README.md).

## Quick start

Requirements: Docker with Compose, or Rust 1.88 for local development, plus a
random API token of at least 32 characters and the camera serial. Copy the
examples, enroll from a trusted terminal and keep real secrets outside Git:

```bash
install -d -m 0700 config data secrets
cp deploy/cameras.example.json config/cameras.json
openssl rand -hex 32 > secrets/api_token
chmod 600 secrets/api_token
cargo build --release --locked
./target/release/ezviz-vtm-bridge enroll \
  --account 'owner@example.com' --token-file data/token.json
docker compose -f deploy/compose.example.yaml config --quiet
docker compose -f deploy/compose.example.yaml up -d
```

Replace the immutable image placeholder and example bind address before running
Compose. The example documents all required mounts and environment. Enrollment
reads password and MFA without echo; see [`docs/OPERATIONS.md`](docs/OPERATIONS.md)
for canaries, monitoring, backup and rollback.

## HTTP contract

| Endpoint | Purpose | Authentication |
| --- | --- | --- |
| `GET /healthz` | liveness and version | none |
| `GET /metrics` | low-cardinality metrics | bearer |
| `GET /v1/cameras/{camera}/snapshot.jpg` | JPEG snapshot | bearer or Basic |
| `GET /v1/cameras/{camera}/live.mpegps` | shared MPEG-PS | bearer |
| `GET /v1/cameras/{camera}/live.ts` | shared MPEG-TS | bearer or Basic |
| `POST /v1/cameras/{camera}/recordings` | finite capture | bearer |
| `DELETE /v1/recordings/{id}` | idempotent spool ACK after local commit | bearer |

Basic authentication is reserved for Home Assistant's Generic Camera client:
the fixed username is `homeassistant` and the API token is the password. The
authoritative schemas, bounds and responses are in the OpenAPI document.

## Production model

The canonical deployment is an immutable image pinned by tag and digest,
managed through reviewed Ansible and Portainer configuration. The container
runs as UID/GID `10001`, with a read-only root filesystem, dropped capabilities,
bounded memory/processes and only `/data` writable. It has no public Traefik
route; network access is limited to Home Assistant, SceneTrove and monitoring.

Rust dependencies are frozen in `Cargo.lock` and RustSec-audited. Builder and
runtime base images are pinned by digest; build tooling does not cross into the
runtime. The published image digest is the deployment identity.

## Development and quality gates

Rust 1.88+ and Docker are required:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --test loc_budget
cargo test --locked --all-targets
cargo audit --deny warnings
docker build --tag ezviz-vtm-bridge:test .
```

Tests are deterministic and require neither network nor EZVIZ credentials.
Live canaries are separate and opt-in. CI enforces formatting, strict Clippy,
tests, RustSec audit, image build and a maximum of 300 physical lines for every
maintained source, configuration and documentation file. Split a responsibility
instead of adding a LOC exception. A repository test also rejects any future
Python source so the Rust-only boundary cannot silently regress.

## Security and operations

Never commit credentials, signed URLs, serials, packet captures or video. API
and EZVIZ session tokens belong in mode-0600 files and must not be passed on a
command line. See [`SECURITY.md`](SECURITY.md) for disclosure and rotation
rules, [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md) for trust boundaries and
[`docs/OPERATIONS.md`](docs/OPERATIONS.md) for the runbook.

## Project documentation

- [`docs/PLAN.md`](docs/PLAN.md): implementation and verified delivery state;
- [`docs/RESEARCH.md`](docs/RESEARCH.md): protocol research and provenance;
- [`docs/adr/README.md`](docs/adr/README.md): architecture decision records;
- [`deploy/`](deploy): sanitized Compose and camera examples.

## License and notices

Copyright 2026 Luigi Barretta. Licensed under Apache-2.0; see [`LICENSE`](LICENSE)
and [`NOTICE`](NOTICE). pyEzvizApi, ezviz_hp7 and LE-EZVIZ-VS are credited as
protocol research and compatibility evidence only; no source is copied or
linked from them.
