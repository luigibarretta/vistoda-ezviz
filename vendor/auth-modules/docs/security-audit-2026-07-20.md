# Security audit - 2026-07-20

Scope: `auth-modules` plus source-level compatibility review of StudioFlow,
SceneTrove, CoveyOS, Quadro, Blackstart and Klaxond.

## Closed in this tranche

- secret-bearing `Debug` output is redacted for OIDC, LDAP, one-time tokens,
  recovery codes and limiter keys;
- OIDC defaults to asymmetric algorithms, validates `azp`, retains validated
  assurance claims, supports exact ACR/required AMR/freshness policy, enforces
  HTTPS and bounded HTTP clients, and can make UserInfo mandatory;
- async OIDC consumers can reuse a prepared, discovery-backed provider handle;
  only authentication age and forced reauthentication may vary per request,
  while provider/client settings and the ACR/AMR/skew assurance floor are
  pinned and unauthenticated logout cannot force provider discovery;
- prepared discovery rejects providers without a complete Authorization Code
  flow, compatible client authentication, and a usable configured signing key;
- back-channel logout JWK selection validates `kid`, `use`, `key_ops`, and
  `alg`, and logout correlation identifiers are never silently normalized;
- explicit OIDC flow values reject empty nonce/state and invalid PKCE
  verifiers;
- the standalone PKCE URL helper parses URLs, rejects fragments and duplicate
  protocol parameters, and clamps verifier entropy to RFC bounds;
- LDAP direct bind requires exactly one username placeholder and resolves only
  the successfully bound DN; searches are bounded and ambiguous results fail;
- LDAP plaintext transport is disabled outside an explicit development
  override;
- the in-memory limiter stores each bucket's window, remains bounded, recovers
  poisoned locks and fails closed at capacity;
- TOTP configuration is bounded and verification can return the accepted
  counter for transactional replay prevention;
- required step-up fails closed for missing browser authentication and API
  tokens;
- session timestamps too far in the future fail closed and expiry boundaries
  are inclusive;
- persistent limiter arithmetic is saturating and failure history is bounded;
- one-time tokens expire at the exact expiration instant.

## Deliberately not guessed

- `hardware_key` is not redefined in the shared policy. A passkey can only be
  classified as a hardware key from WebAuthn evidence such as authenticator
  attachment, backup eligibility/state, user verification and the
  application's attestation policy. Existing consumers currently treat the two
  labels as equivalent; changing that requires a coordinated consumer contract.
- OIDC pending-flow persistence and atomic consumption remain application
  responsibilities. The library cannot provide replay safety without owning a
  consumer's session/store transaction boundary.
- In-memory one-time-token and recovery-code mutation is not advertised as a
  database transaction. Consumers must persist consumption with compare-and-set
  or an equivalent atomic update.
- Best-effort UserInfo remains the behavior of `with_userinfo(true)` for
  compatibility. New high-assurance consumers should use
  `with_required_userinfo(true)` only when UserInfo is an intended authority.
- Password verification cost ceilings and recovery-code policy fields require
  a coordinated API migration to validated opaque policy types. They remain a
  follow-up rather than a silent breaking change.
- Boolean TOTP verification remains a compatibility API, but it cannot prevent
  replay by itself. Consumers must persist the counter returned by
  `verify_code_counter` with an atomic compare-and-set before treating a factor
  as satisfied.
- `hardware_key` still requires consumer-supplied WebAuthn evidence. A generic
  passkey must not be represented as hardware-backed unless attachment,
  backup-state, user-verification, and the deployment attestation policy justify
  that classification.
- LDAP and Argon2 verification are synchronous APIs. Async servers must isolate
  them from executor workers (for example with a bounded blocking pool); adding
  a runtime-specific executor dependency to this shared crate was deliberately
  avoided.
- Multi-bucket in-memory limiter decisions are process-local. Consumers that
  require atomic limits across instances must use one shared persistent
  transaction rather than composing independent memory buckets.

## Dependency exception

`RUSTSEC-2023-0071` remains explicitly ignored only under
`.cargo/audit.toml`. The transitive `rsa` crate is used for public OIDC
signature verification; no RSA private-key operation is permitted. See
`docs/security-rsa-threat-model.md` and run `npm run security:audit`.
