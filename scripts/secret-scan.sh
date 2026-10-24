#!/bin/sh
# Scans the full git history with the pinned gitleaks binary; the archive
# checksum is verified before the binary is unpacked into the local cache.
set -eu
version=8.30.1
sha256=551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb
cache="${GITLEAKS_CACHE:-${XDG_CACHE_HOME:-${HOME}/.cache}/gitleaks-${version}}"
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
if ! test -x "${cache}/gitleaks"; then
    archive="gitleaks_${version}_linux_x64.tar.gz"
    work=$(mktemp -d)
    trap 'rm -rf "${work}"' EXIT
    curl -fsSLo "${work}/${archive}" \
        "https://github.com/gitleaks/gitleaks/releases/download/v${version}/${archive}"
    printf '%s  %s\n' "${sha256}" "${work}/${archive}" | sha256sum -c -
    tar -xzf "${work}/${archive}" -C "${work}" gitleaks
    mkdir -p "${cache}"
    mv "${work}/gitleaks" "${cache}/gitleaks"
fi
test "$(git -C "${root}" rev-parse --is-shallow-repository)" = false
"${cache}/gitleaks" git --redact --no-banner --exit-code 1 "${root}"
