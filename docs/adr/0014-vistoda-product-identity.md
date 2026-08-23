# ADR 0014: Vistoda product identity

- Status: Accepted
- Date: 2026-08-23

## Decision

This connector is published as Vistoda EZVIZ in the canonical
`vistoda-ezviz` repository. The Rust package, binary, HTTP paths and deployed
container service keep their existing `ezviz-vtm-bridge` compatibility names.

Repository identity may change without forcing consumers to migrate runtime
contracts. Any later binary or service rename requires an independently tested
deployment migration with compatibility aliases and rollback evidence.
