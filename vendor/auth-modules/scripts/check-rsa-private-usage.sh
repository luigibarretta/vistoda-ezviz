#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly IGNORED_RSA_ADVISORY="RUSTSEC-2023-0071"
# Verification-only EC path; see .cargo/audit.toml and docs/vex/.
readonly IGNORED_EC_ADVISORY="RUSTSEC-2026-0387"
readonly CARGO_AUDIT_VERSION="0.22.2"

if ! cargo audit --version >/dev/null 2>&1; then
  cat >&2 <<EOF
cargo-audit is required for the auth-modules security check.
Install the pinned version:

  cargo install cargo-audit --version ${CARGO_AUDIT_VERSION} --locked
EOF
  exit 1
fi

direct_rsa_dependency="$(grep -nE '^[[:space:]]*rsa[[:space:]]*=' Cargo.toml || true)"
if [[ -n "${direct_rsa_dependency}" ]]; then
  cat >&2 <<EOF
Direct dependency on the rsa crate is forbidden.
The accepted rsa presence is transitive and limited to public OIDC verification.

Matches:
${direct_rsa_dependency}
EOF
  exit 1
fi

forbidden_usage="$(
  grep -RInE \
    'EncodingKey::from_rsa_|RsaPrivateKey|rsa::.*(SigningKey|DecryptingKey)|from_rsa_(pem|der)|RSA-OAEP|RSA1_5|(^|[^A-Za-z])JWE([^A-Za-z]|$)|(^|[^A-Za-z])jwe([^A-Za-z]|$)' \
    src \
    --include '*.rs' || true
)"

if [[ -n "${forbidden_usage}" ]]; then
  cat >&2 <<EOF
Forbidden RSA private-key or JWE operation found.
auth-modules may verify public OIDC assertions, but must not sign, decrypt, or
load RSA private keys in application request paths.

Matches:
${forbidden_usage}
EOF
  exit 1
fi

cargo audit -D unmaintained -D unsound --ignore "${IGNORED_RSA_ADVISORY}" --ignore "${IGNORED_EC_ADVISORY}"
