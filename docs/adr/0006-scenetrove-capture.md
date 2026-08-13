# ADR-0006: SceneTrove finite-capture contract

- Status: Accepted
- Date: 2026-08-13

## Decision

SceneTrove requests bounded recordings with an idempotency key. The bridge
publishes an immutable MPEG-PS artifact plus a versioned manifest containing
alias, UTC boundaries, duration, byte count, SHA-256 and media type. SceneTrove
pulls the artifact and owns archival retention after successful import.
After byte, MPEG-PS and SHA-256 validation it atomically publishes and `fsync`s
the local file and a recovery receipt. Only then does it call idempotent
`DELETE /v1/recordings/{id}` to release the remote spool. The bridge answers
`204` for an existing, already acknowledged or unknown ID and `409` while the
recording is active.

## Consequences

Infinite live streams never enter the durable import journal. Retries cannot
duplicate footage. SceneTrove needs a connector for the manifest, but its
existing playback remux and AI pipelines remain unchanged.
The receipt makes a crash after local commit recoverable without a duplicate
capture. ACK tombstones are durable and bounded; after their retention window,
an old idempotency key may be reused intentionally.
