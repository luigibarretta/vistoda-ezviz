# ADR-0006: SceneTrove finite-capture contract

- Status: Accepted
- Date: 2026-08-13

## Decision

SceneTrove requests bounded recordings with an idempotency key. The bridge
publishes an immutable MPEG-PS artifact plus a versioned manifest containing
alias, UTC boundaries, duration, byte count, SHA-256 and media type. SceneTrove
pulls the artifact and owns archival retention after successful import.

## Consequences

Infinite live streams never enter the durable import journal. Retries cannot
duplicate footage. SceneTrove needs a connector for the manifest, but its
existing playback remux and AI pipelines remain unchanged.
