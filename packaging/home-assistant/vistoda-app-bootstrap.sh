#!/bin/sh
# Shared Home Assistant app bootstrap primitives for Vistoda providers.
# Consumers own provider configuration, discovery payloads and application
# commands. This file must remain sourceable by both Debian dash and BusyBox ash.

vistoda_fail() {
    printf 'vistoda bootstrap: %s\n' "$1" >&2
    return 1
}

vistoda_require_supervisor_token() {
    test -n "${SUPERVISOR_TOKEN:-}" ||
        vistoda_fail 'SUPERVISOR_TOKEN is required'
}

vistoda_prepare_data_dir() {
    test "$#" -eq 2 || vistoda_fail 'vistoda_prepare_data_dir expects OWNER DIR'
    vistoda_owner=$1
    vistoda_data_dir=$2
    test -d "${vistoda_data_dir}" ||
        vistoda_fail "data directory does not exist: ${vistoda_data_dir}"
    chown "${vistoda_owner}" "${vistoda_data_dir}"
}

vistoda_is_hex_token() {
    printf '%s' "$1" | grep -Eq '^[0-9a-f]{64}$'
}

vistoda_file_is_hex_token() {
    test -f "$1" || return 1
    vistoda_token_size=$(wc -c <"$1")
    test "${vistoda_token_size}" -eq 64 &&
        grep -Eq '^[0-9a-f]{64}$' "$1"
}

vistoda_secure_file() {
    test "$#" -eq 2 || vistoda_fail 'vistoda_secure_file expects OWNER PATH'
    vistoda_owner=$1
    vistoda_path=$2
    if test -e "${vistoda_path}"; then
        chown "${vistoda_owner}" "${vistoda_path}"
        chmod 0600 "${vistoda_path}"
    fi
}

vistoda_ensure_hex_token() {
    test "$#" -eq 3 || vistoda_fail 'vistoda_ensure_hex_token expects PATH OWNER LEGACY'
    vistoda_token_path=$1
    vistoda_owner=$2
    vistoda_legacy_token=$3

    if vistoda_file_is_hex_token "${vistoda_token_path}"; then
        vistoda_secure_file "${vistoda_owner}" "${vistoda_token_path}"
        return 0
    fi

    vistoda_token_temp="${vistoda_token_path}.new"
    (
        umask 077
        if vistoda_is_hex_token "${vistoda_legacy_token}"; then
            printf '%s' "${vistoda_legacy_token}" >"${vistoda_token_temp}"
        else
            od -An -N32 -tx1 /dev/urandom |
                tr -d ' \n' >"${vistoda_token_temp}"
        fi
    )
    if ! vistoda_file_is_hex_token "${vistoda_token_temp}"; then
        rm -f "${vistoda_token_temp}"
        vistoda_fail 'generated workload token is invalid'
        return 1
    fi
    chmod 0600 "${vistoda_token_temp}"
    chown "${vistoda_owner}" "${vistoda_token_temp}"
    mv "${vistoda_token_temp}" "${vistoda_token_path}"
    vistoda_secure_file "${vistoda_owner}" "${vistoda_token_path}"
}

vistoda_start_child() {
    test "$#" -gt 0 || vistoda_fail 'vistoda_start_child expects a command'
    test -z "${VISTODA_CHILD_PID:-}" ||
        vistoda_fail 'a managed child is already running'
    "$@" &
    VISTODA_CHILD_PID=$!
    trap 'vistoda_stop_child' INT TERM
}

vistoda_stop_child() {
    if test -n "${VISTODA_CHILD_PID:-}"; then
        kill -TERM "${VISTODA_CHILD_PID}" 2>/dev/null || true
        wait "${VISTODA_CHILD_PID}" 2>/dev/null || true
        VISTODA_CHILD_PID=
    fi
}

vistoda_wait_for_health() {
    test "$#" -ge 1 && test "$#" -le 3 ||
        vistoda_fail 'vistoda_wait_for_health expects URL [ATTEMPTS] [DELAY]'
    vistoda_health_url=$1
    vistoda_health_attempts=${2:-30}
    vistoda_health_delay=${3:-1}
    case "${vistoda_health_attempts}" in
        ''|*[!0-9]*) vistoda_fail 'health attempts must be a positive integer' ;;
        0) vistoda_fail 'health attempts must be greater than zero' ;;
    esac
    test -n "${VISTODA_CHILD_PID:-}" ||
        vistoda_fail 'no managed child is running'

    vistoda_health_attempt=0
    until curl -fsS --max-time 2 "${vistoda_health_url}" >/dev/null 2>&1; do
        if ! kill -0 "${VISTODA_CHILD_PID}" 2>/dev/null; then
            vistoda_child_status=0
            wait "${VISTODA_CHILD_PID}" || vistoda_child_status=$?
            VISTODA_CHILD_PID=
            test "${vistoda_child_status}" -ne 0 || vistoda_child_status=1
            return "${vistoda_child_status}"
        fi
        vistoda_health_attempt=$((vistoda_health_attempt + 1))
        if test "${vistoda_health_attempt}" -ge "${vistoda_health_attempts}"; then
            vistoda_fail "health check timed out: ${vistoda_health_url}"
            return 1
        fi
        sleep "${vistoda_health_delay}"
    done
}

vistoda_supervisor_app_info() {
    vistoda_require_supervisor_token
    curl -fsS --retry 5 --retry-all-errors \
        -H "Authorization: Bearer ${SUPERVISOR_TOKEN}" \
        http://supervisor/addons/self/info
}

vistoda_publish_discovery() {
    vistoda_require_supervisor_token
    curl -fsS --retry 5 --retry-all-errors \
        -H "Authorization: Bearer ${SUPERVISOR_TOKEN}" \
        -H 'Content-Type: application/json' \
        --data-binary @- http://supervisor/discovery >/dev/null
}

vistoda_wait_child() {
    test -n "${VISTODA_CHILD_PID:-}" || vistoda_fail 'no managed child is running'
    vistoda_wait_pid=${VISTODA_CHILD_PID}
    VISTODA_CHILD_PID=
    wait "${vistoda_wait_pid}"
}
