# lib-auth-modules

Shared Rust authentication building blocks for personal projects.

The repository uses the `lib-` prefix to identify its role in the catalog.
The published Cargo package and Rust crate remain `auth-modules` /
`auth_modules`; the repository rename does not change the consumer API.

The rules for deciding whether cross-repository code belongs here, in the
Klaxond protocol SDK, in CI tooling, or in the application are documented in
[`docs/shared-component-boundaries.md`](docs/shared-component-boundaries.md).
The latest cross-repository adoption and reproducibility review is recorded in
[`docs/consumer-audit-2026-07-31.md`](docs/consumer-audit-2026-07-31.md).

This repo is intentionally a single Rust crate, exposed to apps as
`auth-modules` / `auth_modules`. The public modules are:

- `auth_modules::audit`
- `auth_modules::client_ip`
- `auth_modules::keys`
- `auth_modules::errors`
- `auth_modules::password`
- `auth_modules::methods`
- `auth_modules::secrets` (plus `auth_modules::secrets::files` on Unix)
- `auth_modules::rate_limit`
- `auth_modules::session_policy`
- `auth_modules::one_time_token`
- `auth_modules::recovery_codes`
- `auth_modules::security_profile`
- `auth_modules::testing`
- `auth_modules::step_up`
- `auth_modules::oidc`
- `auth_modules::oidc_pkce`
- `auth_modules::totp`
- `auth_modules::ldap`

`auth_modules::password` owns the common password contract:

- new hashes are Argon2id PHC strings;
- verification accepts Argon2id PHC strings only;
- the baseline password policy starts at 12 characters;
- stricter policy checks can be enabled by callers that already expose those
  knobs.

Apps should disable default features and enable only the modules they need:
`audit`, `client-ip`, `errors`, `keys`, `password`, `secret-files`, `rate-limit`, `session-policy`,
`one-time-token`, `recovery-codes`, `security-profile`, `oidc-async`,
`oidc-blocking`, `oidc-pkce`, `totp`, and/or `ldap` (or `ldap-ring`). The default feature set is
kept broad for local development, but consumer manifests use explicit feature
sets to avoid pulling unused auth code into each app.

`auth_modules::secrets` is always available and owns the dependency-free
constant-time byte comparison plus a small `Redacted<T>` wrapper. Applications
still own secret persistence, serialization, rotation, and protocol-specific
token semantics.

`auth_modules::secrets::files` (feature `secret-files`, Unix only, no extra
dependencies) reads and writes owner-private secret files:

```rust
use auth_modules::secrets::files::{
    read_private_file_with, read_private_string, write_private_atomic, FileOwner,
    PrivateFileOptions,
};

let token = read_private_string("/run/secrets/klaxond_token", 4096)?; // Redacted<String>, trimmed
let key = read_private_file_with(
    "/etc/app/master.key",
    &PrivateFileOptions::new(64).owner(FileOwner::Uid(1000)),
)?; // Redacted<Vec<u8>>
write_private_atomic("/var/lib/app/outbox.json", b"...")?;
```

Reads reject symlinks, non-regular files, any group/other permission bit,
files owned by someone other than the effective UID (resolved through
`/proc/self`, failing closed where that is unavailable; pass
`FileOwner::Uid` explicitly there), empty files and files over the size
limit. The path is checked with `lstat`, then `fstat` on the opened file, then
`lstat` again after the read, and the device/inode must not change, which
gives `O_NOFOLLOW`-equivalent protection without `unsafe`. Buffers are zeroed
on rejection. `write_private_atomic` creates a `create_new` 0600 temporary
file in the same directory, `fsync`s it, renames it over the target (a
symlink at the target is replaced, never followed) and `fsync`s the
directory.

Docker and Swarm secrets under `/run/secrets` are often mounted `0444`
owned by root; such files are rejected on purpose (fail closed: any group or
other bit, or a foreign owner, refuses the read). Mount them for the service
user instead, for example Compose `secrets: [{source: app_token, uid: "1000",
gid: "1000", mode: 0400}]` (Swarm honours `uid`/`gid`/`mode`; plain Compose
file-backed secrets are bind mounts, so set `chmod 0400` and `chown` on the
host file). If the file must stay root-owned and the service runs as root,
`FileOwner::Uid(0)` is explicit; `FileOwner::Any` only skips the owner check,
never the permission check.

`auth_modules::client_ip` (feature `client-ip`, no dependencies) resolves the
real client address behind trusted reverse proxies:

```rust
use auth_modules::client_ip::{ForwardingHeader, ForwardingHeaders, TrustedProxies};

// Startup: invalid CIDRs, host bits or IPv4-mapped networks fail here.
let trusted = TrustedProxies::parse_list("10.0.0.0/8, fd00::/8", ForwardingHeader::XForwardedFor)?;

// Per request (axum example):
let mut forwarded = ForwardingHeaders::new();
for value in headers.get_all("x-forwarded-for") {
    forwarded.push_x_forwarded_for_bytes(value.as_bytes());
}
let client = trusted.resolve(peer_addr.ip(), &forwarded);
```

List only the reverse proxies themselves (for example the Traefik and
Authentik outpost addresses or their container network), never ranges that
also contain clients. Traefik's `forwardedHeaders.trustedIPs` is often set to
the whole LAN or VPN; copying that list here makes every LAN/VPN client look
like a proxy, so their own `X-Forwarded-For` values are trusted (spoofable)
and, when they send none, all of them resolve to whichever address the chain
ends on, collapsing them into one rate-limit bucket.

An untrusted peer is always the client. For a trusted peer only the one
configured header is read (a proxy that maintains `X-Forwarded-For` passes a
client-supplied `Forwarded` header through untouched), from the right,
skipping trusted hops; the first untrusted address is the client. A malformed
hop before that address, non-UTF-8 bytes, more than 32 trusted hops, or a
repeated/listed `X-Real-IP` returns the peer; the client-controlled left-most
entry is never chosen unless every hop up to it is trusted. Entries to the left
of the client are not parsed, so injected garbage cannot collapse every client
onto the proxy address and its rate-limit bucket. IPv4-mapped IPv6 addresses
are normalized to IPv4. `resolve_with_source` also reports whether the address
came from the peer or a header.

`auth_modules::keys` (feature `keys`, reuses `hmac`/`sha2`) derives
domain-separated 32-byte sub-keys from one master secret with HKDF-SHA256
(implemented over `hmac`, checked against the RFC 5869 vectors):

```rust
use auth_modules::keys::{KeyGeneration, KeyLabel, KeyRing, MasterKey};

const SESSION: KeyLabel = KeyLabel::new("klaxond/session-cookie/v1"); // validated at compile time
const ACK: KeyLabel = KeyLabel::new("klaxond/alert-ack/v1");

let master = MasterKey::from_bytes(master_bytes)?; // >= 32 bytes
let session_key = master.derive(SESSION);          // SubKey, Debug redacted
// The old secret was used directly for ack tags only: scope it to ACK.
let ring = KeyRing::new(master).with_legacy_raw(old_session_secret, &[ACK])?;
let tag = ring.sign(ACK, b"alert-17");
match ring.verify(ACK, b"alert-17", &tag) {
    Some(KeyGeneration::Current) => {}
    Some(KeyGeneration::Legacy(_)) => { /* accept, then re-issue */ }
    None => { /* reject */ }
}
```

Labels must have at least three `/`-separated `[a-z0-9._-]` segments ending in
`v<digits>`. A `KeyRing` signs only with the current key and verifies previous
master keys (derived with the same label) and legacy raw secrets that a
service used directly as HMAC keys before adopting derivation. A raw legacy
secret must be scoped to the labels that replaced its raw usages
(`with_legacy_raw(secret, &[ACK, SESSION])`) and never verifies under any
other label, so a legacy tag minted for one purpose cannot be replayed as
another. Scope it as narrowly as the old code allowed. Loading,
storing and rotating the master secret stay in the application; there is no
zeroize dependency, so key bytes are redacted from `Debug` but not wiped.

Rust apps can consume crates from this workspace with a local path dependency
while the API stabilizes. There is intentionally no Go sibling package in this
repo right now.

Current Rust consumers:

- `../blackstart`
- `../klaxond`
- `../coveyos`
- `../scenetrove`
- `../quadro-rust-resume/backend-rs`
- StudioFlow's vendored `backend-rust/vendor/auth-modules`

All tracked Rust apps use Argon2id-only verification. Password hash fallbacks
based on bcrypt, SHA1, or non-Argon2id PHC variants are intentionally outside
the gold standard. Not every consumer uses every module yet; all tracked Rust
apps with OIDC login use the shared `auth_modules::oidc` client instead of
app-local protocol implementations.

`auth_modules::methods` owns the shared public method names returned by
`GET /api/auth/methods`: `password`, `oidc`, `totp`, `passkey`,
`hardware_key`, `trusted_proxy`, `ldap`, `api_token`, and `magic_link`.
Apps can mark unsupported methods disabled; the point is that maintainers see
the same method vocabulary everywhere.

Run `scripts/check-auth-gold-standard.sh` before auth changes that touch more
than one app. The guard checks the local repo set, including Blackstart, for
active bcrypt usage, SHA1 password-hash fallbacks, runtime root `/auth/*` API
call/mount regressions, custom in-memory auth rate buckets outside this crate,
repo-local auth brute-force threshold redeclarations, canonical
`/api/auth/password-policy` and `/api/auth/methods` endpoints, shared
`auth_modules::password` usage, shared `auth_modules::methods` usage, shared
auth brute-force policy usage, shared OIDC protocol-client usage, executable
repo-local guard wrappers, absence of direct `openidconnect` app manifest
dependencies, and absence of app-local OIDC discovery/JWKS/token-validation
code. It also asserts that CoveyOS, SceneTrove, Klaxond, and StudioFlow expose
runtime `magic_link` support and map `hardware_key` to their WebAuthn/passkey
capability, and that CoveyOS, SceneTrove, Klaxond, and Quadro consume the shared LDAP
adapter for their directory-login runtime.

Repository CI runs formatting, locked all-feature check, Clippy, tests, the
RSA/security audit, the standalone gold-standard guard, and the shared LOC and
NASA/JPL-inspired quality profiles, all from `ci-local.json` through the
vendored runner (`node third_party/repo-tooling/ci-local.mjs --quick`, then
the full gate). Repository tooling is vendored from the exact commit in
`dependencies/repo-tooling.sha` and byte-verified in CI. Pass
the current checkout explicitly to run the guard in standalone-library mode:
`scripts/check-auth-gold-standard.sh "$PWD"`.

Repository CI runs formatting, locked all-feature check, Clippy, tests, the
RSA/security audit, the standalone gold-standard guard, and the shared LOC and
NASA/JPL-inspired quality profiles, all from `ci-local.json` through the
vendored runner (`node third_party/repo-tooling/ci-local.mjs --quick`, then
the full gate). Repository tooling is vendored from the exact commit in
`dependencies/repo-tooling.sha` and byte-verified in CI.

`auth_modules::rate_limit` owns shared failed-attempt policy for local login,
second-factor, and reauth throttling. The gold constants are 10 account/user
failures in 5 minutes, 10-minute persistent lockout, 50 auth IP burst attempts
in 60 seconds, and 10 short-burst attempts in 60 seconds. It includes an
in-memory limiter for runtime-only buckets and a persistence-friendly policy API
for apps that store failure records themselves. Apps keep their own key
strategy, storage format, Redis/in-memory/persistent backend, and HTTP response
shape. The bounded in-memory implementation retains each bucket's own window
and fails closed for unknown keys when capacity is exhausted; distributed
deployments should use a shared persistent limiter. Current consumers are
`../coveyos`, `../klaxond`, `../scenetrove`,
`../quadro-rust-resume/backend-rs`, and StudioFlow.

`auth_modules::audit` owns the shared auth event vocabulary and request context
shape: login success/failure, OIDC, LDAP, passkey, magic link, TOTP, recovery
codes, session lifecycle, lockout, rate-limit, brute-force, policy violation,
and trusted-proxy events. It intentionally exposes only event types, builders,
and an `AuditSink` trait. Apps still own persistence, log routing, alerting,
and tenant/user-resource mapping.

`auth_modules::errors` owns canonical machine-readable auth error codes and
basic response metadata. Apps can keep their existing HTTP response envelope,
but frontend locale keys should use these shared lowercase codes so errors like
`magic_link_unavailable`, `passkey_unavailable`, `account_locked`, and
`rate_limited` stay consistent across projects.

`auth_modules::session_policy` owns shared cookie/session policy defaults:
secure and HTTP-only cookies, SameSite guidance, eight-hour max lifetime,
30-minute idle timeout, one-hour rotation, max concurrent sessions, and a
stricter high-security preset. Apps still own session storage, cookie names,
CSRF strategy, and invalidation mechanics.

`auth_modules::step_up` owns the shared policy vocabulary for post-primary
authentication requirements such as "the primary browser login must be followed
by TOTP or a passkey". It intentionally does not implement TOTP/WebAuthn
ceremonies, persistence, redirects, or UI; consumers enforce the returned
requirement with their own session and credential stores. A required policy
returns an unsatisfiable, fail-closed requirement for unauthenticated and API
token contexts rather than an allow-shaped result.

`auth_modules::one_time_token` owns CSPRNG URL-safe token generation, SHA-256
hash-at-rest helpers, TTL policy, one-time consume semantics, and a generic
anti-enumeration delivery acknowledgement. It is intended for magic links,
password reset, email verification, invite, and MFA challenge flows. Apps still
own mail delivery, DB writes, and route handlers.

Issue magic links, resets, verifications and invites with
`issue_for_delivery`, which returns `(StoredTokenHash, DeliverableToken)`.
The record holds only the hash; the `DeliverableToken` has no accessor,
`Clone`, `Display` or `Serialize` (compile-time tests enforce this) and its
plaintext leaves only through `deliver`, which hands it to a `TokenDelivery`
(any `FnOnce(&str) -> Result<(), E>`) and returns `()`:

```rust
let (record, token) = issue_for_delivery(email, OneTimeTokenPurpose::MagicLink,
    OneTimeTokenPolicy::magic_link(), now);
store.insert(record)?;
token.deliver(|token: &str| mailer.send_magic_link(email, token))?;
Ok(Json(generic_delivery_acknowledgement()))
```

`issue_token`, which exposes `GeneratedOneTimeToken::plaintext`, is
deprecated since 0.2.0. Tokens meant to be shown once to their owner (API
tokens) keep using `generate_token` and `hash_token`.

## Migrating to 0.2.0: deprecations break `clippy -D warnings`

0.2.0 marks `one_time_token::issue_token`, the free functions
`oidc::async_client::exchange_code` and `oidc::blocking::exchange_code`, and
the `exchange_code` methods of `PreparedAsyncOidcClient` and
`PreparedBlockingOidcClient` as `#[deprecated]`. Every consumer whose CI runs
`cargo clippy -- -D warnings` (or `RUSTFLAGS=-D warnings`) **fails to build**
as soon as it bumps its pin to 0.2.0 while still calling them. Behaviour is
unchanged; only the lint is new.

Temporary bridge, if the bump must land before the migration: put
`#[allow(deprecated)]` on the single call site (function or `impl` item), not
on the crate, with a comment naming the replacement, and remove it in the
follow-up change. Do not use `#![allow(deprecated)]` crate-wide: it would hide
future deprecations too.

Recommended order per consumer, one consumer at a time
(klaxond, blackstart, coveyos, scenetrove, studioflow, immich-rs):

1. Bump the pin and get a green build, bridging deprecated call sites with
   local `#[allow(deprecated)]`.
2. Replace `exchange_code` with `exchange_callback(&CallbackParams { .. })`,
   passing the stored state as `expected_state`, and delete the local state
   comparison (security fix AM-03, smallest diff).
3. Replace `issue_token` with `issue_for_delivery` and move the plaintext into
   the `TokenDelivery`; add a test that the HTTP response body contains no
   token.
4. Remove every `#[allow(deprecated)]` bridge, then adopt the optional new
   modules (`client_ip`, `keys`, `secrets::files`) as separate changes.

The deprecated functions are removed only after every known consumer has
migrated (`docs/shared-component-boundaries.md`, compatibility rule 3).

`auth_modules::recovery_codes` owns MFA recovery-code generation, normalization,
hashing, constant-time verification, and consume-once behavior. Plaintext codes
are returned only at generation time; consumers should persist only the hashes
and used-at state.

`auth_modules::security_profile` composes the gold password, rate-limit, and
session defaults into `GoldAuthProfile::personal_default()` and
`GoldAuthProfile::high_security()`. This is a reference profile for repo audits
and API policy responses, not a replacement for app-local configuration.

`auth_modules::testing` provides small test helpers for consumer repos: an
environment-variable guard, a fixed epoch clock, and captured event storage.
It is feature-gated so applications do not need to enable it in runtime builds.

`auth_modules::oidc` owns the shared OIDC Authorization Code flow on top of the
`openidconnect` crate: provider discovery, authorization URL construction with
PKCE and nonce, token exchange, ID-token validation, optional `at_hash`
validation, `azp` validation, explicit asymmetric signing policy, typed
ACR/AMR/authentication-freshness policy, retained assurance claims, and optional
or required UserInfo loading. Production issuers and discovered endpoints are
HTTPS-only, HTTP redirects are disabled, and connect/request timeouts are
bounded. Applications own only app-domain
glue such as route handlers, session cookies, role/tenant mapping, and user
provisioning. Current async consumers are `../coveyos`, `../klaxond`,
`../quadro-rust-resume/backend-rs`, and
StudioFlow; `../scenetrove` uses the blocking client.

Long-lived services should discover a `PreparedAsyncOidcClient` or
`PreparedBlockingOidcClient` during startup and reuse it for authorization,
exchange, and back-channel logout. The prepared clients accept per-request
assurance-policy changes for login versus step-up, but rejects issuer, client,
secret, redirect, scope, algorithm, timeout, and UserInfo-policy drift. The
required ACR, required AMR set, and clock skew are also pinned; only maximum
authentication age and forced reauthentication may vary per request. Discovery
fails unless the provider exposes a complete Authorization Code flow,
compatible client authentication, and at least one usable configured signing
key. The compatibility free functions still discover per call. Applications
that need
signing-key rotation without restart should refresh the prepared handle on a
bounded background schedule; untrusted requests must not trigger discovery.

Finish the callback with `exchange_callback(config, &CallbackParams { code,
state, expected_state, nonce, pkce_verifier })` (prepared clients and free
functions, async and blocking). It compares the returned `state` with the
stored one in constant time and rejects a mismatch, an empty expected state or
an empty code before discovery or any token request (audit finding AM-03).
The old `exchange_code(config, code, nonce, pkce_verifier)` signatures are
deprecated since 0.2.0 and will be removed after every consumer migrates.

State storage, browser binding, atomic callback consumption, replay detection
and provider-session revocation are deliberately application responsibilities.
`authorization_url_with_flow` rejects empty nonce and RFC-invalid PKCE
verifiers, but a consumer must still generate, bind, expire and consume pending
flows correctly.

Secret-bearing OIDC flow/configuration values, LDAP bind credentials, one-time
tokens, recovery codes, and in-memory rate-limit keys use redacted `Debug`
implementations. Callers must still avoid serializing or logging their public
plaintext fields directly.

The transitive RustCrypto `rsa` dependency is accepted only for public OIDC
assertion verification. No RSA private-key signing, decryption, or JWE decrypt
operation is allowed in application request paths. Run `npm run security:audit`
to enforce the guard and the documented `RUSTSEC-2023-0071` policy; see
[`docs/security-rsa-threat-model.md`](docs/security-rsa-threat-model.md).

`auth_modules::oidc_pkce` owns small shared helpers that are still useful
outside the protocol client: URL-safe token generation, S256 code challenge
generation, parsed authorization-URL construction, and the common local
redirect sanitizer. PKCE entropy requests are clamped to RFC 7636's 43-128
character verifier range, fragments and duplicate protocol parameters are
rejected, and redirect policy checks only the parsed path. Current
consumers are `../coveyos`, `../klaxond`, `../scenetrove`, and
`../quadro-rust-resume/backend-rs`.

`auth_modules::totp` owns shared TOTP/HOTP primitives: base32 secret generation and
decode/encode helpers, RFC 4226 HOTP code generation, RFC 6238-style TOTP
verification with the common 30-second step and one-step skew window, and
otpauth URI construction. `verify_code_counter` returns the accepted counter so
apps can atomically reject replay; boolean verification remains available for
compatibility. Apps still own replay prevention, encrypted secret
storage, QR rendering, recovery-code format, and recovery-code hashing. Current
consumers are `../klaxond`, `../scenetrove`,
`../quadro-rust-resume/backend-rs`, and StudioFlow;
`../coveyos` keeps its existing `totp-rs` dependency for now to avoid churn.

`auth_modules::ldap` owns the common LDAP/Active Directory bind/search adapter:
direct bind DN templates (`{username}` and `%s`), service-bind search, escaped
DN interpolation, escaped user filters, shared attribute defaults, scope
parsing, and a five-second connection timeout default. It uses `ldap3 0.12.1`
with `sync` + rustls so consumers do not inherit `native-tls` from the shared
adapter. The `ldap` feature keeps the historical `tls-rustls-aws-lc-rs`
provider; `ldap-ring` selects `tls-rustls-ring` instead and avoids the
aws-lc-sys C/cmake build. Enable exactly one of the two, and prefer the provider
the rest of the application already uses: ldap3 builds its TLS client from the
rustls process-default provider, and rustls can only infer that default when a
single provider feature (`ring` or `aws_lc_rs`) is active in the whole build.
An application whose graph enables both (for example `ldap` plus reqwest's
default ring-backed rustls) must call `CryptoProvider::install_default` at
startup, otherwise every `ldaps://` connection panics inside ldap3. Direct-bind templates must contain exactly
one username placeholder, identity enrichment reads only the successfully bound
DN, ambiguous searches fail, and connect/bind/search operations are bounded.
`ldaps://` is required; plaintext `ldap://` needs the explicit development-only
`AUTH_MODULES_ALLOW_INSECURE_LDAP=1` override. Apps still own runtime config storage,
session issuance, user provisioning or mapping rules, and UI. Current consumers
are `../coveyos`, `../klaxond`, `../scenetrove`,
`../quadro-rust-resume/backend-rs`, and StudioFlow.
