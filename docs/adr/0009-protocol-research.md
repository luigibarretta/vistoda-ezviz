# ADR-0009: Protocol research provenance and adopted safeguards

- Status: Accepted
- Date: 2026-08-13

## Context

Multiple independent projects implement parts of the modern EZVIZ VTM/VTDU
path. They are useful protocol evidence but vary in maturity, licensing and
production safety. The CP4 has already been proven to emit clear MPEG-PS over
VTM using `pyezvizapi` 1.0.5.0.

## Decision

Keep `pyezvizapi` as the only runtime protocol implementation. Consult these
primary-source repositories as independent evidence:

- Bobsilvio `ezviz_hp7` at commit `a6c038a3`: VTM/VTDU bootstrap, bounded
  relays, account-protecting circuit breaker, output-stall watchdog, GOP cache
  and the observed HP7/CP7 AAC-in-PES mismatch;
- RenierM26 `pyEzvizApi` at commit `c713642f`: the Apache-2.0 package used by
  this bridge, including token reuse and VTM MPEG-PS/MPEG-TS helpers;
- albrzmr `ezviz_hp7` at commit `b3dcd6e4`: a separate LAN CPD7 implementation
  showing one-upstream lifecycle, keyframe-aligned warm buffers and explicit
  failure diagnostics;
- LethalEthan `LE-EZVIZ-VS` at commit `35a267cf`: an incomplete Go protocol
  exploration confirming the VTM to VTDU handoff, keepalive and MPEG-PS/RTP
  variants.

Adopt the lifecycle lessons, not source code: one upstream, bounded queues,
idle teardown, stall termination, and a ten-minute circuit break after three
consecutive upstream failures. Validate the CP4's actual codec/container facts
with FFprobe before adding an audio repair path. Never assume HP7/CP7 audio
quirks apply to CP4 without capture evidence.

## Consequences

No third-party code is copied, so the bridge remains Apache-2.0. Research
commits and licenses are auditable in `docs/RESEARCH.md`. A new protocol path
requires a fixture, negative tests, an ADR amendment and a live CP4 canary.
