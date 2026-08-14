# ADR-0010: Enforced maintainability limits

- Status: Accepted
- Date: 2026-08-13

## Decision

Every human-maintained Rust, Markdown, YAML, TOML, JSON and Dockerfile in the
repository is limited to 300 physical lines. A dedicated Rust test enforces the
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
