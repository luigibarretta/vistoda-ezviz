# ADR-0025: Sealed EZVIZ session and the shared provider kit crate

- Status: Accepted
- Date: 2026-11-11
- Amends ADR-0005

## Context

ADR-0005 stored the EZVIZ session (`session_id`, `rf_session_id`) as plaintext
JSON in a mode-0600 `token.json`. Vistoda Blink seals its provider store with
AES-256-GCM under a key kept in a dedicated `store.key`; the 2026-11 fleet
audit (E-M1) flagged the difference, since any copy of the data directory
exposed the EZVIZ account session. The constant-time API token check and the
private-file helpers were also implemented three times across the bridges.

## Decision

The bridge depends on the `vistoda-provider-kit` crate of
lib-vistoda-provider-kit, vendored with the lib-auth-modules archive it uses
under `vendor/` at the kit pin and verified by `check-vendored-dep`.

- The session lives in `token.sealed` next to the configured token path:
  envelope `{version, nonce, ciphertext}`, AES-256-GCM, key = HKDF sub-key
  `vistoda-ezviz/ezviz-token/v1` of the 32 random bytes in `store.key`
  (mode 0600, created once without ever replacing an existing key).
- `serve` seals a plaintext `token.json` from an earlier release at startup,
  reads it back and only then removes it; a plaintext file wins over an
  existing sealed one (it is newer after a downgrade). If it fails the
  private-file checks it is kept and startup fails, so an enrolled session is
  never lost. A sealed session whose `store.key` is missing fails closed and
  never reads as "not enrolled".
- `auth.rs` and `storage.rs` are thin adapters over the kit's `auth` and
  `private_file`; the router-wide middleware stays in `api_session` because it
  also counts failures and reports `reauth_required`.

## Consequences

The bridge API token no longer guards the EZVIZ session, and a partial copy
of `/data` does not reveal it. `store.key` sits in the same directory, so a
full backup still contains everything needed to open it: backups of `/data`
remain sensitive. Losing `store.key` (or restoring `token.sealed` without it)
requires a new enrollment. Downgrading to a release without sealing finds no
`token.json` and also requires a new enrollment.
