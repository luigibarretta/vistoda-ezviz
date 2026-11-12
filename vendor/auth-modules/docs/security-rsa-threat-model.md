# RSA dependency threat model

## Scope

`openidconnect` currently pulls RustCrypto `rsa` into the `auth-modules`
dependency graph. RustSec advisory `RUSTSEC-2023-0071` describes a timing side
channel in RSA private-key operations. No patched advisory-compatible release
is available at the time of this decision.

The accepted invariant is:

> `rsa` is present only for public OIDC assertion verification. No RSA
> private-key operation exists in an application request path.

This crate does not own, load, generate, decrypt with, or sign with an RSA
private key. It also does not decrypt RSA-backed JWE payloads.

## Controls

- `.cargo/audit.toml` ignores only `RUSTSEC-2023-0071`.
- `scripts/check-rsa-private-usage.sh` rejects a direct `rsa` dependency and
  known RSA private-key signing, decryption, and JWE APIs in production source.
- Consumers should configure their identity provider for ES256 or EdDSA instead
  of RS256 when supported.
- Dependency review must monitor RustCrypto/RSA issue 390 and the RustSec
  advisory. The ignore must be removed as soon as a patched version can be
  consumed.

The guard is intentionally conservative. A future use case that genuinely
requires RSA private-key operations needs its own threat model and must not be
silently added to this shared authentication crate.

## Residual risk

The transitive `rsa` code remains present in binaries that enable OIDC even when
the configured provider uses ES256 or EdDSA. The vulnerable operation is not
reachable under the invariant above, but supply-chain monitoring remains
necessary until the dependency graph contains a patched implementation.

`cargo audit` also reports yanked transitive crates separately. Yanked status is
a dependency-maintenance signal, not by itself a vulnerability finding; it
remains visible and is reviewed without suppressing vulnerability, unsound, or
unmaintained advisories.
