# ADR-0013: Durable, idempotent recording acknowledgement

- Status: Accepted
- Date: 2026-08-14

## Context

Finite recordings were safe to retry and download, but the producer had no
contractual proof that SceneTrove had committed an artifact. Consequently the
bridge could not distinguish a recoverable download from media whose remote
spool could be released.

## Decision

`DELETE /v1/recordings/{id}` is the acknowledgement operation. It returns `204`
when a completed or failed recording is removed, was already acknowledged, or
is unknown. It returns `409 recording_active` for `pending` or `recording`, so an
ACK cannot truncate an active producer.

For ready media, the bridge removes the artifact, `fsync`s the spool directory,
updates the journal atomically and persists a tombstone. Tombstones preserve the
idempotency-key outcome across restart and are FIFO-bounded to 4096 entries.

SceneTrove must verify the declared MPEG-PS/MPEG-TS prefix, expected byte count and SHA-256, write
the local file, `fsync` it, atomically rename it and `fsync` its destination.
It then writes and `fsync`s a receipt before ACK. Only a successful `204` permits
receipt removal. Recovery finds the receipt and retries ACK without creating a
second recording.

## Consequences

Remote spool ownership has an explicit handoff point and bounded retention.
Network failure before or after ACK is safe because both sides persist enough
state to retry. A consumer that never acknowledges intentionally retains remote
media until the configured spool quota applies backpressure.
