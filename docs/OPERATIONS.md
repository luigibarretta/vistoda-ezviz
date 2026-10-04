# Operations runbook

This runbook covers the standalone provider and advanced recovery. Home
Assistant OS users should begin with the shared
[installation guide](https://github.com/luigibarretta/vistoda-addons/blob/main/GETTING_STARTED.md).

## Standalone quick start

Run these commands from the repository root. Requirements: Docker with
Compose, OpenSSL and the camera serial. The example
runs as your local UID/GID so its bind-mounted state remains private and
writable. Enroll from a trusted terminal and keep real secrets outside Git:

```bash
install -d -m 0700 config data secrets
cp deploy/cameras.example.json config/cameras.json
```

Edit `config/cameras.json` and replace `REPLACE_WITH_SERIAL` with the real
camera serial. Then replace the example account value below before continuing:

```bash
openssl rand -hex 32 > secrets/api_token
chmod 600 secrets/api_token
export VISTODA_UID="$(id -u)"
export VISTODA_GID="$(id -g)"
export VISTODA_EZVIZ_ACCOUNT='replace-with-your-account-email'
docker compose -f deploy/compose.example.yaml build
docker compose -f deploy/compose.example.yaml run --rm ezviz-vtm-bridge \
  enroll --account "$VISTODA_EZVIZ_ACCOUNT" --token-file /data/token.json
docker compose -f deploy/compose.example.yaml config --quiet
docker compose -f deploy/compose.example.yaml up -d
```

The example builds the current checkout and binds only to loopback. Enrollment
reads password and MFA without echo. Keep `VISTODA_UID` and `VISTODA_GID` set for
later Compose commands. Configure a reviewed private-network bind only when
another approved host must reach the bridge. The sections below
cover canaries, monitoring, backup and rollback.

## Enrollment

Run enrollment from a trusted interactive terminal. Password and MFA are read
without echo and are not stored:

```bash
ezviz-vtm-bridge enroll \
  --account 'owner@example.com' \
  --token-file ./data/token.json
chmod 600 ./data/token.json
```

Generate the independent bridge API token with `openssl rand -hex 32`, place it
in the secrets backend, and render it as a mode-0600 file at deploy time. Never
copy the official Home Assistant EZVIZ token into production bridge state.

## Home Assistant

Create a Generic Camera using:

- still image URL: `http://BRIDGE:8765/v1/cameras/front-door/snapshot.jpg`;
- stream source: `http://BRIDGE:8765/v1/cameras/front-door/live.ts`;
- authentication: Basic;
- username: `homeassistant`;
- password: the bridge API token;
- verify SSL: enabled whenever HTTPS is used.

Keep the official EZVIZ integration for battery, PIR, alarm and configuration
entities. The bridge camera is media-only. Do not expose the service through a
public Traefik router.

After reconciliation, verify playback through Home Assistant's supported
`camera/stream` WebSocket command. Fetch a bounded HLS playlist and one media
segment, then confirm the bridge metrics return `upstream_active`,
`remux_active`, `raw_subscribers` and `ts_subscribers` to zero after the idle
grace. A successful JPEG proxy alone does not prove live playback.

For cold-start analysis, compare `upstream_startup_seconds` (VTM request to
first MPEG-PS chunk) with `remux_startup_seconds` (MPEG-TS subscriber to first
output chunk). Their difference approximates local FFmpeg startup/probing;
optimize probe parameters only after repeated real-camera measurements.

## Live-session battery guard

`EZVIZ_BRIDGE_MAX_LIVE_SESSION_SECONDS` limits each `live.mpegps` and `live.ts`
HTTP client. It defaults to 90 seconds and accepts 30 through 900 seconds. A
client must reconnect deliberately after the bridge closes the response. This
is a fail-safe for unattended browser tabs and media clients; consumers should
still ask the user whether to continue before reaching the hard limit.

The limit does not apply to finite recordings. Their independently bounded
duration is controlled by `EZVIZ_BRIDGE_MAX_RECORDING_SECONDS`. After the final
live subscriber disconnects, verify subscriber and upstream metrics return to
zero once the idle grace expires.

The MPEG-TS relay repeats transport and H.264 codec headers at keyframes and
retains one keyframe-aligned warm segment bounded to 8 MiB. A canary must attach
a second client after the first has already been streaming long enough to pass
the initial GOP; both clients must decode video and audio.

## Alarm feed

The provider polls EZVIZ alarm summaries every `EZVIZ_BRIDGE_ALARM_POLL_SECONDS`
(default 15; 10–300; `0` disables) and never marks messages read. Watch
`alarm_polls_total`, `alarm_poll_errors_total`, `alarms_received_total`,
`alarm_pictures_stored_total`, `alarm_pictures_failed_total`,
`alarm_pictures_unsupported_total` and `alarm_backlog_truncated_total`. History
and pictures live under `/data/alarms/<alias>/`; deleting that directory while
stopped resets them. Errors back off to five minutes and log no URLs or serials.

## Encryption and microSD

`GET /v1/cameras/{alias}/encryption` reports whether the cloud marks video as
encrypted and whether the option code or the account's cloud copy is usable.
`key_source: none` on an encrypted camera means live view will fail closed:
enter the verification code in the app options or disable encryption in the
official app. The endpoint itself never requests the cloud code.

Requesting the account's cloud copy of the code
(`/api/device/query/encryptkey`) can make EZVIZ email or text the owner a
verification code. Vistoda sends it only for an actual decryption (encrypted
live start, snapshot or alarm picture) without a usable option code, at most
once per camera every 24 hours; the attempt time is kept in
`/data/cloud-key-attempts.json` (hashed serials, no codes) so restarts do not
repeat it. A refused request is logged once and pauses that camera's lookup
for 24 hours: encrypted live starts fail with a clear error and encrypted
alarm pictures are skipped (`alarm_pictures_key_unavailable_total`). Deleting
the file lifts the pause; prefer entering the label code instead. `GET …/storage` and `GET …/sd-records?date=YYYY-MM-DD` are
read-only; storage is fetched from EZVIZ at most once per camera every 10
minutes, so a freshly inserted card may take that long to appear.

## SceneTrove

SceneTrove uses the bounded live endpoint for interactive viewing and the
finite-capture workflow for durable recordings. Its UI confirmation must occur
before the bridge hard limit, and closing or hiding the viewer must release the
live response.

The durable capture workflow is:

1. `POST /v1/cameras/front-door/recordings` with Bearer auth,
   `Idempotency-Key`, and `{"duration_seconds": N}`.
2. Poll the manifest until `ready` or `failed`.
3. Download `/v1/recordings/{id}/media` to a temporary file.
4. Verify byte count, SHA-256 and the MPEG-PS/MPEG-TS prefix declared by the
   recording manifest.
5. Atomically publish it with the matching `.mpegps` or `.ts` extension.
   SceneTrove's media profile performs its existing remux.
6. Persist and `fsync` a local receipt, then send idempotent
   `DELETE /v1/recordings/{id}`. A `204` proves the remote spool is released or
   was already released. Remove and `fsync` the receipt only after that ACK.

The consumer must retain its idempotency key until import succeeds. AI analysis
remains governed by the SceneTrove device policy; the bridge never enables it.

The production image also contains the reference adapter at
`/usr/local/bin/scenetrove-pull`. Run that same immutable image with its
entrypoint overridden, a read-only API-token mount, and only the destination
device folder writable. The adapter rejects redirects and credential-bearing
URLs, bounds all responses, verifies the declared MPEG-PS/MPEG-TS prefix, byte count and
SHA-256, then atomically publishes the finished file. Its durable receipt
closes the crash window after local commit: a restart retries only the ACK and
never creates duplicate media. Active captures return `409 recording_active`;
unknown and previously acknowledged IDs return `204` by design.

## Canary and rollback

Run a short, bounded canary with the API token on standard input:

```bash
printf '%s\n' "$BRIDGE_CANARY_TOKEN" | ezviz-vtm-bridge canary \
  --base-url http://BRIDGE:8765 --camera front-door --seconds 20
```

Then validate two concurrent consumers, stop both, and confirm
`upstream_active` and `remux_active` return to zero after the idle grace.
Rollback removes the Generic Camera URL and SceneTrove capture schedule, then
stops the bridge stack. It does not modify the official EZVIZ integration or
existing SceneTrove archives.

Back up only the encrypted/secret-managed EZVIZ session token when operationally
required. Recording spool data is transient; finished SceneTrove imports own
their retention. Never commit `data/`, `secrets/`, tokens or camera serials.

For an NVR, configure the exact channel (1–256); a standalone camera normally
uses channel 1. `substream` selects the lower-bandwidth profile when supported.
Encrypted RTP is opt-in per camera and requires a mode-0600 verification-code
file. The current decryptor supports compatible H.264/HEVC RTP profiles; it
fails closed for encrypted MPEG-PS/MPEG-TS variants rather than emitting cipher
text. Keep the official app available until a canary proves a device profile.
