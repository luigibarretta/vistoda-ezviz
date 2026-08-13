# ADR-0007: Home Assistant consumption

- Status: Accepted
- Date: 2026-08-13

## Decision

Home Assistant initially consumes authenticated snapshot and MPEG-TS URLs via
Generic Camera. The official EZVIZ integration remains authoritative for
motion, battery and alarm entities. A thin custom component is deferred unless
Generic Camera cannot satisfy stream lifecycle or credential handling.

## Consequences

No Core dependency override or EZVIZ fork is required. Rollback restores the
existing dashboard entity without touching integration registries.
