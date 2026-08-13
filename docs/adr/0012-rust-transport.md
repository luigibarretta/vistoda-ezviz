# ADR-0012: Native Rust transport and controlled migration

- Status: Accepted
- Date: 2026-08-14
- Supersedes: ADR-0002

## Context

The Python implementation proved that the owned CP4 exposes usable JPEG and
MPEG-PS media through VTM/VTDU and established a stable consumer contract. Its
vendor library remained the largest runtime dependency, duplicated process and
thread lifecycle concerns, and limited how precisely disconnect and backpressure
could be controlled.

## Decision

Implement the product in safe, idiomatic Rust 1.88. The bridge owns token
refresh, service discovery, signed VTDU requests, VTM framing, the minimal
protobuf fields, redirects, keepalive, optional media decryption, snapshots and
MPEG-PS extraction. Tokio owns cancellation and bounded fan-out. FFmpeg remains
an external, argument-fixed copy-remux specialist; video is never re-encoded.

The HTTP contract, token JSON and persisted recording manifests stay compatible.
Python and the reviewed protocol projects remain test oracles but are not runtime
dependencies. The migration uses an immutable image, a live bounded canary and
the prior image digest as rollback. One process at a time owns the refresh token.

## Consequences

The runtime has no Python interpreter or vendor package. A stream disconnect is
represented by RAII and cancels the producer after the idle grace. Protocol
changes now require golden wire tests, malformed-input tests and a real-camera
canary. Proprietary protocol maintenance becomes this repository's explicit
responsibility, so observability and rollback evidence are release gates.
