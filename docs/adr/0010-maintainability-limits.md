# ADR-0010: Enforced maintainability limits

- Status: Accepted
- Date: 2026-08-13

## Decision

Every human-maintained Python, Markdown, YAML, TOML, JSON and Dockerfile in the
repository is limited to 300 physical lines. The guard runs locally and in CI.
Generated dependency lock files and tool caches are excluded; production files
do not receive one-off exemptions.

## Consequences

Raw fan-out, remux and composition are separate modules. New responsibilities
must be introduced as cohesive modules before an existing file reaches the
limit. The cap complements, rather than replaces, type checking, complexity
rules, tests and review.
