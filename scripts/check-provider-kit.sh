#!/bin/sh
# Fetches lib-vistoda-provider-kit at the commit pinned in
# dependencies/vistoda-provider-kit.sha from its public source of truth, proves
# the vendored bootstrap is byte-identical and runs the kit's lifecycle test.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
kit="${root}/.ci/vistoda-provider-kit"
url=https://github.com/luigibarretta/lib-vistoda-provider-kit.git
pin=$(tr -d '[:space:]' < "${root}/dependencies/vistoda-provider-kit.sha")
printf '%s' "${pin}" | grep -Eq '^[0-9a-f]{40}$'
if test "$(git -C "${kit}" rev-parse HEAD 2>/dev/null || true)" != "${pin}"; then
    rm -rf "${kit}"
    git init --quiet "${kit}"
    git -C "${kit}" remote add origin "${url}"
    git -C "${kit}" fetch --quiet --depth=1 origin "+${pin}:refs/remotes/origin/pinned"
    git -C "${kit}" checkout --quiet --detach refs/remotes/origin/pinned
fi
# Keep the pin in a remote-tracking ref (also for existing checkouts), so the
# homelab Git inventory does not report it as unpushed work.
git -C "${kit}" update-ref refs/remotes/origin/pinned "${pin}"
test "$(git -C "${kit}" rev-parse HEAD)" = "${pin}"
test -z "$(git -C "${kit}" status --porcelain)"
cmp "${kit}/dist/vistoda-app-bootstrap.sh" \
    "${root}/packaging/home-assistant/vistoda-app-bootstrap.sh"
sh "${kit}/tests/bootstrap-test.sh"
printf 'Vistoda provider kit verified at %s\n' "${pin}"
