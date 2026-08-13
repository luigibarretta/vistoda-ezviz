# ADR-0001: Standalone vendor boundary

- Status: Accepted
- Date: 2026-08-13

## Decision

Run EZVIZ protocol handling in a standalone service. Home Assistant and
SceneTrove consume a vendor-neutral HTTP API and never import `pyezvizapi`.

## Consequences

One service owns the cloud session and battery policy. Consumers can evolve
independently and a future Rust transport can replace Python without changing
their contracts. The deployment adds one small internal service.
