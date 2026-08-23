# ADR-0015: Home Assistant app packaging

## Status

Accepted — 2026-08-23.

## Context

The standalone EZVIZ runtime serves SceneTrove well, but bridge URLs, workload
tokens and a separate Docker host are implementation details that ordinary Home
Assistant users should not configure.

## Decision

Publish the unchanged Rust core both as a standalone image and as a Home
Assistant app image. The app keeps port 8765 private, generates its workload
token inside `/data` and publishes the connection only through Supervisor
discovery. Store metadata lives in `vistoda-addons`; this repository owns the
binary image and provider enrollment contract.

The Home Assistant path asks for camera identity and EZVIZ credentials, while
the remote path keeps explicit URL/token configuration for advanced consumers.

## Consequences

Home Assistant users do not manage bridge networking or authentication.
Provider releases must publish matching `amd64` and `aarch64` manifests before
the store version advances. Standalone SceneTrove deployments keep their
existing API and data layout.

