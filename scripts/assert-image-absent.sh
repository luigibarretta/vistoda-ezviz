#!/bin/sh
# Fail closed unless GHCR proves the exact release version is absent.
set -eu
test "$#" -eq 2
image=$1
version=$2
case "$image" in ghcr.io/*) ;; *) exit 1 ;; esac
printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'
name=${image#ghcr.io/}
# Send credentials through stdin, never in curl's command arguments.
authorization=$(printf '%s:%s' "$GITHUB_ACTOR" "$GH_TOKEN" | base64 | tr -d '\n')
token=$(
    printf 'header = "Authorization: Basic %s"\n' "$authorization" |
        curl --config - --fail --silent --show-error --connect-timeout 5 --max-time 20 \
            --get --data-urlencode service=ghcr.io \
            --data-urlencode "scope=repository:${name}:pull" https://ghcr.io/token |
        jq -er '.token | strings | select(length > 0)'
)
status=$(
    printf 'header = "Authorization: Bearer %s"\n' "$token" |
        curl --config - --silent --show-error --connect-timeout 5 --max-time 20 \
            --head --output /dev/null --write-out '%{http_code}' \
            --header 'Accept: application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.docker.distribution.manifest.v2+json' \
            "https://ghcr.io/v2/${name}/manifests/${version}"
)
case "$status" in
    404) printf 'Registry confirms %s:%s is absent\n' "$image" "$version" ;;
    200) printf 'Immutable version already exists; create a new version\n' >&2; exit 1 ;;
    *) printf 'Cannot prove version absent: registry HTTP %s\n' "$status" >&2; exit 1 ;;
esac
