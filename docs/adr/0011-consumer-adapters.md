# ADR-0011: Consumer adapters

- Status: Accepted
- Date: 2026-08-13

## Decision

Home Assistant uses Generic Camera with Basic authentication and the shared
copy-remuxed MPEG-TS endpoint. SceneTrove uses a finite pull adapter that
requires a caller-owned idempotency key, rejects redirects, bounds JSON/media,
verifies MPEG-PS pack start, byte count and SHA-256, and publishes with atomic
rename into a registered `mpeg_ps` device folder.

## Consequences

Neither consumer imports vendor code or owns an EZVIZ session. SceneTrove does
not archive an infinite stream and can safely retry a job after interruption.
The reference pull tool is deployable as a scheduled sidecar until equivalent
logic is promoted into SceneTrove's Rust service.
