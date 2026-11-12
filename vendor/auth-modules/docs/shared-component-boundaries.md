# Shared component boundaries

This document records which code is intentionally shared across Luigi's
personal services and which code remains application-owned. The goal is to
remove duplicated security and integration behaviour without creating a
single foundation crate that couples unrelated release cycles.

## Extract now

| Component | Canonical owner | Consumers | Boundary |
| --- | --- | --- | --- |
| Authentication and secret primitives | `auth-modules` | Rust services | Algorithms, policy vocabulary and safe value types only. Storage, HTTP handlers, sessions and application roles remain local. |
| Klaxond application-event contract | `klaxond/sdk` | Rust and Go services | Versioned event schema plus rendering of the Alertmanager-compatible webhook envelope. Delivery, retries, credentials and service-specific message wording remain local. |

The first non-auth primitive in `auth-modules` is
`auth_modules::secrets`: constant-time byte comparison and `Redacted<T>`.
Blackstart and Klaxond consume it instead of carrying private copies.

Release 0.2.0 adds three more primitives with the same boundary (algorithm and
validated types only, no I/O beyond the explicit file operation, no HTTP
handlers or storage):

- `auth_modules::client_ip`: trusted-proxy CIDR validation and right-to-left
  forwarding-header resolution. Applications still extract the peer address
  and header values from their own HTTP stack.
- `auth_modules::secrets::files`: owner-private secret file reads and atomic
  0600 writes. Applications still choose paths, limits and formats.
- `auth_modules::keys`: HKDF-SHA256 sub-key derivation with namespaced labels
  and a verification key ring for rotation. Applications still load, store
  and rotate the master secret and own the token formats built on the keys.

Klaxond owns its notification protocol because it is the receiver and can
validate backward compatibility. The canonical sources are:

- `sdk/rust` (`klaxond-protocol`);
- `sdk/go` (`git.luigibarretta.com/luigibarretta/klaxond/sdk/go`);
- `docs/schemas/application-event-v1.schema.json`.

Consumers pin an immutable Klaxond commit. Vendored copies must name that
commit and stay byte-identical to the canonical source. This keeps container
builds hermetic while preserving provenance.

## Share as tooling, not a runtime library

The following repetition is real, but belongs in CI tooling or reusable
workflow templates rather than in application binaries:

- immutable OCI image promotion;
- Semaphore task submission and terminal-state polling;
- commit-SHA provenance checks;
- release evidence collection;
- common Rust and Go lint, test and audit steps.

These helpers must fail closed and accept explicit inputs. They must not own
repository-specific image names, deployment template IDs, secrets or release
policy. A dedicated `repo-tooling` repository is appropriate only after its
distribution mechanism is proven on the current Gitea Actions version; until
then, small workflow-local implementations are safer than downloading mutable
scripts during CI.

## Do not extract yet

Do not create a generic `service-foundation` runtime package for these areas:

- HTTP server startup and router composition;
- configuration loading;
- database pools and migrations;
- tracing subscriber setup;
- health endpoints;
- application error envelopes;
- domain models or persistence repositories.

Although their shapes look similar, their semantics and dependencies differ
between Blackstart, Klaxond, CoveyOS, StudioFlow, SceneTrove, Quadro and
Lampo. A shared base would make unrelated services upgrade together and could
turn a local change into a fleet-wide regression.

Reconsider extraction only when at least three active consumers have the same
behaviour, tests and lifecycle for two consecutive releases. Extract the
smallest stable contract first; avoid framework-style wrappers.

## Compatibility rules

1. Shared public contracts use semantic versions or an immutable commit pin.
2. A consumer migration includes a compatibility test for the wire or security
   behaviour it replaces.
3. A breaking change is introduced alongside the previous contract and is
   removed only after every known consumer has migrated.
4. Secrets, tokens and runtime credentials never enter shared test fixtures.
5. Shared components do not perform network or deployment side effects unless
   that side effect is their explicit, tested purpose.
