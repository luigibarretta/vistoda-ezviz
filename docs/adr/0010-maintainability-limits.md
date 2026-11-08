# ADR-0010: Enforced maintainability limits

- Status: Accepted; line limit superseded on 2026-11-07
- Date: 2026-08-13

Update 2026-11-07: the fleet standard (repo-tooling 0.6.0, `fleet-default`
preset) replaces the 250-line rule and `tests/loc_budget.rs`: one 400-line
limit for source files (Markdown, YAML, TOML and JSON are no longer counted),
zero growth and a ratchet, enforced by the `loc` and `nasa` ci-local steps.
The Python ban stays, as the `source-languages` step.

## Decision

Every human-maintained Rust, Markdown, YAML, TOML, JSON and Dockerfile in the
repository is limited to 250 physical lines. A dedicated Rust test enforces the
guard locally and in CI and rejects Python source files, keeping operational
tooling under the same Rust-only boundary as the product.
Generated dependency lock files and tool caches are excluded; production files
do not receive one-off exemptions. Rustfmt and Clippy are deterministic and
checked in CI alongside tests and the RustSec audit.

## Consequences

Raw fan-out, remux and composition are separate modules. New responsibilities
must be introduced as cohesive modules before an existing file reaches the
limit. The cap complements, rather than replaces, type checking, complexity
rules, tests and review.
