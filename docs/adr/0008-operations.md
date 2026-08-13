# ADR-0008: Deployment, observability and rollback

- Status: Accepted
- Date: 2026-08-13

## Decision

Deploy an immutable, read-only container on `gpu-01`, without a public Traefik
route. Only Home Assistant and SceneTrove may reach it. Persist only the token
and bounded recording state. Export low-cardinality health and Prometheus
metrics using camera aliases. CI owns tests and images; Ansible/Portainer owns
production deployment. The Rust builder and Debian runtime base are pinned by
digest. Cargo dependencies are frozen in `Cargo.lock`, RustSec-audited in CI,
and absent from the runtime image. Only the two stripped binaries, CA roots and
FFmpeg cross the build boundary.

## Consequences

The bridge is independent of HAOS lifecycle and is available to both
consumers. Rollback stops/removes only this stack and restores consumer URLs;
no camera firmware, HA registry or SceneTrove archive mutation is involved.
The published image digest remains the deployment identity. System packages in
the pinned Debian base are resolved when the image is built, so the build does
not claim bit-for-bit reproducibility across time.
