#!/bin/sh
set -eu

readonly data_dir=/data
readonly options_file=/data/options.json
readonly token_file=/data/api-token
readonly cameras_file=/data/cameras.json

umask 077
mkdir -p "${data_dir}/recordings"
chown bridge:bridge "${data_dir}"

alias_name="$(jq -er '.alias | strings | select(test("^[A-Za-z0-9_-]+$"))' "${options_file}")"
camera_serial="$(jq -er '.camera_serial | strings | select(test("^[A-Za-z0-9]+$"))' "${options_file}")"
if ! test -f "${token_file}" || ! grep -Eq '^[0-9a-f]{64}$' "${token_file}"; then
    od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"${token_file}"
fi
jq -n --arg alias "${alias_name}" --arg serial "${camera_serial}" \
    '{($alias): {serial: $serial, decrypt_video: false}}' >"${cameras_file}"
chown bridge:bridge "${token_file}" "${cameras_file}"
chown -R bridge:bridge "${data_dir}/recordings"
chmod 0700 "${data_dir}/recordings"
if test -e "${data_dir}/token.json"; then
    chown bridge:bridge "${data_dir}/token.json"
    chmod 0600 "${data_dir}/token.json"
fi
chmod 0600 "${token_file}" "${cameras_file}"

export EZVIZ_BRIDGE_API_TOKEN_FILE="${token_file}"
export EZVIZ_BRIDGE_CAMERAS_FILE="${cameras_file}"
export EZVIZ_BRIDGE_EZVIZ_TOKEN_FILE="${data_dir}/token.json"
export EZVIZ_BRIDGE_DATA_DIR="${data_dir}"

gosu bridge:bridge ezviz-vtm-bridge serve &
child_pid=$!

stop_child() {
    kill -TERM "${child_pid}" 2>/dev/null || true
    wait "${child_pid}" 2>/dev/null || true
}
trap stop_child INT TERM

attempt=0
until curl -fsS --max-time 2 http://127.0.0.1:8765/healthz >/dev/null 2>&1; do
    if ! kill -0 "${child_pid}" 2>/dev/null; then
        wait "${child_pid}"
    fi
    attempt=$((attempt + 1))
    test "${attempt}" -lt 30 || exit 1
    sleep 1
done

test -n "${SUPERVISOR_TOKEN:-}" || exit 1
app_hostname="$(curl -fsS --retry 5 --retry-all-errors \
    -H "Authorization: Bearer ${SUPERVISOR_TOKEN}" \
    http://supervisor/addons/self/info | jq -er '.data.hostname')"
private_url="http://${app_hostname}:8765"
jq -n \
    --arg service media_bridge \
    --arg provider ezviz \
    --arg url "${private_url}" \
    --arg alias "${alias_name}" \
    --rawfile api_token "${token_file}" \
    '{service: $service, config: {provider: $provider, url: $url,
      alias: $alias, api_token: ($api_token | gsub("\\s"; "")), managed_app: true}}' |
    curl -fsS --retry 5 --retry-all-errors \
        -H "Authorization: Bearer ${SUPERVISOR_TOKEN}" \
        -H 'Content-Type: application/json' \
        --data-binary @- http://supervisor/discovery >/dev/null

wait "${child_pid}"
