# ADR-0002: Python transport with stable HTTP contract

- Status: Superseded by ADR-0012
- Date: 2026-08-13

## Decision

Implement version 1 in Python 3.12 using the pinned Apache-2.0
`pyezvizapi==1.0.5.0`. Isolate it behind a typed `CameraTransport` protocol.

## Rationale

The package already implements the verified CP4 VTM/VTDU, token, snapshot,
encryption and MPEG-PS behavior. A direct Rust rewrite would duplicate the
highest-risk proprietary layer before operational evidence exists.

## Consequences

The HTTP process remains asynchronous while blocking vendor operations run in
owned threads. A Rust implementation is an allowed later replacement after a
sanitized compatibility corpus and soak evidence exist.
