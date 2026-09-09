# ADR-0016: Vistoda archive, microSD and talk boundaries

- Status: accepted
- Date: 2026-09-09

## Context

The bridge already produced finite MPEG-PS captures for SceneTrove, but Home
Assistant had no inventory or direct standalone workflow. The enrolled camera
also contains a vendor-managed microSD card and the official application offers
two-way audio. Neither capability is part of the proven VTM/VTDU downstream
contract implemented by this repository.

The official EZVIZ Android SDK 5.27.3 confirms separate supported surfaces:
`startVoiceTalk`, `stopVoiceTalk`, device-record search and SD playback. The
official demo also distinguishes full-duplex capability from press-to-talk.
That SDK requires an approved EZVIZ Open Platform application and Android native
runtime; it is not a server-side library for the current HAOS x86_64 app.

## Decision

`GET /v1/recordings` exposes the existing redacted immutable manifests, ordered
newest first. Vistoda for Home Assistant may request 15, 30 or 60 seconds,
poll status, download authenticated media and delete a completed bridge-spool
item. This is deliberately independent from SceneTrove: SceneTrove remains a
specialized ingest consumer and Vistoda never scans or mutates its archive.

The HA control plane may copy a ready recording to a quota-bounded NFS dataset.
It verifies the manifest byte count and SHA-256 before atomic publication.
Provider credentials and bridge tokens never reach the browser or NFS metadata.

No microSD UI is exposed until an approved Open Platform credential scope and a
supported server runtime, or an independently proven consumer-account protocol,
can run a current CP4 list/playback canary. Destructive storage SDK calls are not
used. No microphone/talk control is exposed until the same production boundary
proves the uplink codec, session ownership, mute, teardown and recovery sequence.
Downstream AAC in a live view is not evidence of a safe full-duplex uplink.

## Consequences

- Vistoda has a useful standalone recording archive without depending on
  SceneTrove or the vendor microSD card.
- Local deletion is an acknowledgement of the bridge spool; NFS copies and
  SceneTrove media are not deleted.
- The UI reports unavailable protocol surfaces honestly instead of presenting
  controls that cannot be verified or recovered.
- Android SDK binaries and new provider credentials are not introduced into
  HAOS implicitly; doing so requires a separate reviewed architecture decision.
