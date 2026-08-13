# Operations runbook

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
- stream source: `http://BRIDGE:8765/v1/cameras/front-door/stream.ts`;
- authentication: Basic;
- username: `homeassistant`;
- password: the bridge API token;
- verify SSL: enabled whenever HTTPS is used.

Keep the official EZVIZ integration for battery, PIR, alarm and configuration
entities. The bridge camera is media-only. Do not expose the service through a
public Traefik router.

## SceneTrove

SceneTrove uses the finite-capture workflow, never the infinite live endpoint:

1. `POST /v1/cameras/front-door/recordings` with Bearer auth,
   `Idempotency-Key`, and `{"duration_seconds": N}`.
2. Poll the manifest until `ready` or `failed`.
3. Download `/v1/recordings/{id}/media` to a temporary file.
4. Verify byte count, SHA-256 and MPEG-PS pack start.
5. Atomically publish it into the registered device archive with an `hiv*.mp4`
   name. SceneTrove's `mpeg_ps` profile performs the existing remux.

The consumer must retain its idempotency key until import succeeds. AI analysis
remains governed by the SceneTrove device policy; the bridge never enables it.

The production image also contains the reference adapter at
`/usr/local/bin/scenetrove-pull`. Run that same immutable image with its
entrypoint overridden, a read-only API-token mount, and only the destination
device folder writable. The adapter rejects redirects and credential-bearing
URLs, bounds all responses, verifies the MPEG-PS pack prefix, byte count and
SHA-256, then atomically publishes the finished file.

## Canary and rollback

Run a short, bounded canary with the API token on standard input:

```bash
printf '%s\n' "$BRIDGE_CANARY_TOKEN" | scripts/canary.py \
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
