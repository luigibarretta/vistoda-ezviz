#!/bin/sh
# Shared Home Assistant app bootstrap primitives for Vistoda providers.
# Consumers own provider configuration, discovery payloads and application
# commands. This file must remain sourceable by both Debian dash and BusyBox ash.
# Every helper returns non-zero as soon as a check fails, so it stays fail-closed
# when called from `if`, `&&` or `||` contexts where `set -e` is suspended.

vistoda_fail() {
    printf 'vistoda bootstrap: %s\n' "$1" >&2
    return 1
}

vistoda_require_supervisor_token() {
    test -n "${SUPERVISOR_TOKEN:-}" ||
        { vistoda_fail 'SUPERVISOR_TOKEN is required'; return 1; }
}

vistoda_prepare_data_dir() {
    test "$#" -eq 2 ||
        { vistoda_fail 'vistoda_prepare_data_dir expects OWNER DIR'; return 1; }
    vistoda_owner=$1
    vistoda_data_dir=$2
    test -d "${vistoda_data_dir}" ||
        { vistoda_fail "data directory does not exist: ${vistoda_data_dir}"; return 1; }
    chown "${vistoda_owner}" "${vistoda_data_dir}" ||
        { vistoda_fail "cannot set owner of ${vistoda_data_dir}"; return 1; }
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
    test "$#" -eq 2 ||
        { vistoda_fail 'vistoda_secure_file expects OWNER PATH'; return 1; }
    vistoda_owner=$1
    vistoda_path=$2
    test -e "${vistoda_path}" || return 0
    chown "${vistoda_owner}" "${vistoda_path}" && chmod 0600 "${vistoda_path}" ||
        { vistoda_fail "cannot secure ${vistoda_path}"; return 1; }
}

vistoda_ensure_hex_token() {
    test "$#" -eq 3 ||
        { vistoda_fail 'vistoda_ensure_hex_token expects PATH OWNER LEGACY'; return 1; }
    vistoda_token_path=$1
    vistoda_owner=$2
    vistoda_legacy_token=$3

    if vistoda_file_is_hex_token "${vistoda_token_path}"; then
        vistoda_secure_file "${vistoda_owner}" "${vistoda_token_path}"
        return
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
    chmod 0600 "${vistoda_token_temp}" &&
        chown "${vistoda_owner}" "${vistoda_token_temp}" &&
        mv "${vistoda_token_temp}" "${vistoda_token_path}" || {
        rm -f "${vistoda_token_temp}"
        vistoda_fail 'cannot persist workload token'
        return 1
    }
    vistoda_secure_file "${vistoda_owner}" "${vistoda_token_path}"
}

vistoda_start_child() {
    test "$#" -gt 0 ||
        { vistoda_fail 'vistoda_start_child expects a command'; return 1; }
    test -z "${VISTODA_CHILD_PID:-}" ||
        { vistoda_fail 'a managed child is already running'; return 1; }
    case "${VISTODA_STOP_GRACE_SECONDS:-8}" in
        ''|*[!0-9]*)
            vistoda_fail 'VISTODA_STOP_GRACE_SECONDS must be a non-negative integer'
            return 1 ;;
    esac
    # Supervisor authority belongs only to this bootstrap, never the provider.
    (unset SUPERVISOR_TOKEN; exec "$@") &
    VISTODA_CHILD_PID=$!
    VISTODA_CHILD_STATUS=
    trap 'vistoda_stop_child; exit 130' INT
    trap 'vistoda_stop_child; vistoda_exit_after_term' TERM
    trap 'vistoda_stop_child' EXIT
}

# Records the child's exit status in VISTODA_CHILD_STATUS (137 when it had to
# be killed after the grace period). The default 8-second grace stays below the
# Supervisor default stop timeout of 10 seconds, so the bootstrap reports the
# provider's own shutdown status before Docker kills the container.
vistoda_stop_child() {
    if test -n "${VISTODA_CHILD_PID:-}"; then
        kill -TERM "${VISTODA_CHILD_PID}" 2>/dev/null || true
        vistoda_stop_attempt=0
        while kill -0 "${VISTODA_CHILD_PID}" 2>/dev/null; do
            test "${vistoda_stop_attempt}" -lt "${VISTODA_STOP_GRACE_SECONDS:-8}" || break
            sleep 1
            vistoda_stop_attempt=$((vistoda_stop_attempt + 1))
        done
        kill -KILL "${VISTODA_CHILD_PID}" 2>/dev/null || true
        VISTODA_CHILD_STATUS=0
        wait "${VISTODA_CHILD_PID}" 2>/dev/null || VISTODA_CHILD_STATUS=$?
        VISTODA_CHILD_PID=
    fi
}

# TERM is Supervisor's normal stop request. A provider that then exits cleanly
# (status 0, or 143 when it died on the forwarded TERM itself) ends the app
# with 0, so Supervisor records it as stopped rather than failed. A provider
# that fails during shutdown or has to be killed keeps its non-zero status.
vistoda_exit_after_term() {
    case "${VISTODA_CHILD_STATUS:-0}" in
        0|143) exit 0 ;;
        *) exit "${VISTODA_CHILD_STATUS}" ;;
    esac
}

vistoda_wait_for_health() {
    test "$#" -ge 1 && test "$#" -le 3 || {
        vistoda_fail 'vistoda_wait_for_health expects URL [ATTEMPTS] [DELAY]'
        return 1
    }
    vistoda_health_url=$1
    vistoda_health_attempts=${2:-30}
    vistoda_health_delay=${3:-1}
    case "${vistoda_health_attempts}" in
        *[!0-9]*) vistoda_fail 'health attempts must be a positive integer'; return 1 ;;
        *[!0]*) : ;;
        *) vistoda_fail 'health attempts must be greater than zero'; return 1 ;;
    esac
    case "${vistoda_health_delay}" in
        *[!0-9]*) vistoda_fail 'health delay must be a non-negative integer'; return 1 ;;
    esac
    test -n "${VISTODA_CHILD_PID:-}" ||
        { vistoda_fail 'no managed child is running'; return 1; }

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
    vistoda_require_supervisor_token || return 1
    curl -fsS --connect-timeout 3 --max-time 10 --retry 2 --retry-max-time 30 --retry-all-errors \
        -H "Authorization: Bearer ${SUPERVISOR_TOKEN}" \
        http://supervisor/addons/self/info
}

vistoda_publish_discovery() {
    vistoda_require_supervisor_token || return 1
    curl -fsS --connect-timeout 3 --max-time 10 --retry 2 --retry-max-time 30 --retry-all-errors \
        -H "Authorization: Bearer ${SUPERVISOR_TOKEN}" \
        -H 'Content-Type: application/json' \
        --data-binary @- http://supervisor/discovery >/dev/null
}

# Prints this app's Supervisor hostname; fails unless it is a non-empty DNS label.
vistoda_app_hostname() {
    vistoda_app_info=$(vistoda_supervisor_app_info) ||
        { vistoda_fail 'cannot read Supervisor app info'; return 1; }
    # Older jq exits 0 on empty input even with -e, so check the result too.
    vistoda_app_host=$(printf '%s' "${vistoda_app_info}" |
        jq -er '.data.hostname | strings | select(test("^[A-Za-z0-9][A-Za-z0-9-]*$"))') &&
        test -n "${vistoda_app_host}" ||
        { vistoda_fail 'Supervisor app info has no valid hostname'; return 1; }
    printf '%s\n' "${vistoda_app_host}"
}

# Reads the provider-owned discovery config object on stdin and publishes
# {service, config: config + {url: http://HOSTNAME:PORT, managed_app: true}}.
vistoda_publish_app_discovery() {
    test "$#" -eq 2 || {
        vistoda_fail 'vistoda_publish_app_discovery expects SERVICE PORT'
        return 1
    }
    case "$1" in
        ''|*[!a-z0-9_]*) vistoda_fail "invalid discovery service: $1"; return 1 ;;
    esac
    case "$2" in
        ''|0*|*[!0-9]*) vistoda_fail "invalid discovery port: $2"; return 1 ;;
    esac
    test "$2" -le 65535 || { vistoda_fail "invalid discovery port: $2"; return 1; }
    vistoda_discovery_host=$(vistoda_app_hostname) || return 1
    vistoda_discovery_payload=$(jq -cse --arg service "$1" \
        --arg url "http://${vistoda_discovery_host}:$2" \
        'select(length == 1 and (.[0] | type) == "object") |
         {service: $service, config: (.[0] + {url: $url, managed_app: true})}') &&
        test -n "${vistoda_discovery_payload}" ||
        { vistoda_fail 'discovery config must be one JSON object'; return 1; }
    printf '%s' "${vistoda_discovery_payload}" | vistoda_publish_discovery ||
        { vistoda_fail 'discovery publication failed'; return 1; }
}

vistoda_wait_child() {
    test -n "${VISTODA_CHILD_PID:-}" ||
        { vistoda_fail 'no managed child is running'; return 1; }
    vistoda_wait_pid=${VISTODA_CHILD_PID}
    vistoda_wait_status=0
    wait "${vistoda_wait_pid}" || vistoda_wait_status=$?
    VISTODA_CHILD_PID=
    return "${vistoda_wait_status}"
}
