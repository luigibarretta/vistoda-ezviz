#!/bin/sh
set -eu

readonly data_dir=/data
readonly options_file=/data/options.json
readonly token_file=/data/api-token
readonly cameras_file=/data/cameras.json
readonly cameras_tmp_file=/data/cameras.json.new.$$
. /usr/local/lib/vistoda-app-bootstrap

umask 077
vistoda_require_supervisor_token
mkdir -p "${data_dir}/recordings"
vistoda_prepare_data_dir bridge:bridge "${data_dir}"

jq -S -e -f /usr/local/lib/vistoda-cameras.jq "${options_file}" >"${cameras_tmp_file}" || {
    rm -f "${cameras_tmp_file}"
    vistoda_fail 'Camera configuration is invalid: use 1-64 unique safe aliases, valid serials, channels 1-256, and boolean substream values.'
    exit 1
}
chown bridge:bridge "${cameras_tmp_file}"
chmod 0600 "${cameras_tmp_file}"
mv -f "${cameras_tmp_file}" "${cameras_file}"
camera_alias="$(jq -er 'keys[0]' "${cameras_file}")"
camera_alias_json="$(jq -c 'to_entries | map({alias: .key, source_id: "\(.value.serial):\(.value.channel)"})' "${cameras_file}")"
vistoda_ensure_hex_token "${token_file}" bridge:bridge ''
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
    --arg alias "${camera_alias}" \
    --argjson aliases "$(jq -c 'keys' "${cameras_file}")" \
    --argjson devices "${camera_alias_json}" \
    --rawfile api_token "${token_file}" \
    '{service: $service, config: {provider: $provider, url: $url,
      alias: $alias, aliases: $aliases, devices: $devices,
      api_token: ($api_token | gsub("\\s"; "")), managed_app: true}}' |
    vistoda_publish_discovery

vistoda_wait_child
