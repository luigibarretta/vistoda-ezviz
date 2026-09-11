#!/bin/bash
# Exercise the publication preflight without registry access or real secrets.
set -euo pipefail
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
export GITHUB_ACTOR=fixture
export GH_TOKEN=fixture-token

curl() {
    local headers
    headers=$(sed -n '1p')
    case "$*" in *fixture-token*) return 99 ;; esac
    case "$headers" in *Authorization:*) ;; *) return 99 ;; esac
    if [[ "${FIXTURE_NETWORK_FAILURE:-0}" != 0 ]]; then
        return "$FIXTURE_NETWORK_FAILURE"
    fi
    case "$*" in
        *--get*) printf '{"token":"fixture-bearer"}' ;;
        *--head*)
            case "$*" in
                *application/vnd.oci.image.manifest.v1+json*) ;;
                *) return 99 ;;
            esac
            printf '%s' "$FIXTURE_HTTP_STATUS"
            ;;
        *) return 99 ;;
    esac
}
export -f curl

check_case() {
    local expected=$1
    export FIXTURE_HTTP_STATUS=$2
    export FIXTURE_NETWORK_FAILURE=$3
    local result=0
    bash "$root/scripts/assert-image-absent.sh" ghcr.io/example/provider 1.2.3 >/dev/null 2>&1 || result=$?
    if [[ "$expected" == success ]]; then
        test "$result" -eq 0
    else
        test "$result" -ne 0
    fi
}

check_case success 404 0
check_case failure 200 0
check_case failure 401 0
check_case failure 403 0
check_case failure 503 0
check_case failure 000 28
printf 'Immutable image preflight tests passed.\n'
