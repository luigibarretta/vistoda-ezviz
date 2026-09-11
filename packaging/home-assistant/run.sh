#!/bin/sh
set -eu

readonly data_dir=/data
readonly options_file=/data/options.json
readonly token_file=/data/api-token
readonly cameras_file=/data/cameras.json
. /usr/local/lib/vistoda-app-bootstrap

umask 077
vistoda_require_supervisor_token
mkdir -p "${data_dir}/recordings"
vistoda_prepare_data_dir bridge:bridge "${data_dir}"

alias_name="$(jq -er '.alias | strings | select(test("^[A-Za-z0-9_-]+$"))' "${options_file}")"
camera_serial="$(jq -er '.camera_serial | strings | select(test("^[A-Za-z0-9]+$"))' "${options_file}")" || {
    vistoda_fail 'Enter the camera serial from EZVIZ device information in app Configuration, save, then start again. Use the serial, not the verification code.'
    exit 1
}
camera_channel="$(jq -er '.camera_channel // 1 | numbers | select(. >= 1 and . <= 256)' "${options_file}")"
substream="$(jq -er '.substream // false | booleans' "${options_file}")"
vistoda_ensure_hex_token "${token_file}" bridge:bridge ''
jq -n --arg alias "${alias_name}" --arg serial "${camera_serial}" \
    --argjson channel "${camera_channel}" --argjson substream "${substream}" \
    '{($alias): {serial: $serial, channel: $channel, substream: $substream,
      decrypt_video: false}}' >"${cameras_file}"
chown bridge:bridge "${cameras_file}"
chown -R bridge:bridge "${data_dir}/recordings"
chmod 0700 "${data_dir}/recordings"
vistoda_secure_file bridge:bridge "${data_dir}/token.json"
chmod 0600 "${cameras_file}"

export EZVIZ_BRIDGE_API_TOKEN_FILE="${token_file}"
export EZVIZ_BRIDGE_CAMERAS_FILE="${cameras_file}"
export EZVIZ_BRIDGE_EZVIZ_TOKEN_FILE="${data_dir}/token.json"
export EZVIZ_BRIDGE_DATA_DIR="${data_dir}"

vistoda_start_child gosu bridge:bridge ezviz-vtm-bridge serve
vistoda_wait_for_health http://127.0.0.1:8765/healthz 30 1

app_hostname="$(vistoda_supervisor_app_info | jq -er '.data.hostname')"
private_url="http://${app_hostname}:8765"
jq -n \
    --arg service media_bridge \
    --arg provider ezviz \
    --arg url "${private_url}" \
    --arg alias "${alias_name}" \
    --rawfile api_token "${token_file}" \
    '{service: $service, config: {provider: $provider, url: $url,
      alias: $alias, api_token: ($api_token | gsub("\\s"; "")), managed_app: true}}' |
    vistoda_publish_discovery

vistoda_wait_child
