#!/bin/sh
# Keep provider Docker and Cargo versions aligned before signing a release.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'
standalone=$(sed -n 's/^ARG VERSION=//p' "$root/Dockerfile")
app=$(sed -n 's/^ARG BUILD_VERSION=//p' "$root/packaging/home-assistant/Dockerfile")
if test "$standalone" != "$version" || test "$app" != "$version"; then
    printf 'Docker versions must match Cargo version %s\n' "$version" >&2
    exit 1
fi
printf 'Provider release identity verified: %s\n' "$version"
