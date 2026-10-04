# ADR-0020: microSD playback over VTM

- Status: proposed
- Date: 2026-10-04

## Context

ADR-0019 exposes the microSD record index. Static analysis of the EZVIZ 7.6.1
app (`docs/RESEARCH.md`) shows that SD playback uses the same cloud VTM/VTDU
client as live view: native `CloudClient::startPlayback` receives
`VideoStreamInfo{beginTime, endTime, seqId}` (`yyyyMMddTHHmmssZ`,
camera-local) and builds a `ysproto://…/playback?` URL with `chn`, `stream`,
`seg`, `begin`, `end`, `serial`, `ssn` and related fields. The library also
handles playback-only control such as seek identifiers and an explicit
end-of-playback notification.

## Proposal

Add a disabled-by-default `GET /v1/cameras/{camera}/sd-playback.ts?start=&end=`
that reuses `VtmSession`, the VTDU token allocation and the existing clear
MPEG-PS or decrypted-RTP pipeline, bounded to one recorded interval and the
live-session lifetime. It would not seek, change speed, download files or run
concurrently with live view for the same camera.

## Open questions before acceptance

1. Exact `/playback?` parameter names, order and the `seg`/`begin`/`end`
   syntax; captured from an owner-authorized session or a decompiled native
   builder, not guessed.
2. Whether StreamInfo `0x13b` and keepalives are unchanged for playback and
   which message ends a finished interval.
3. Whether battery cameras wake for playback and how that interacts with the
   live-session guard.
4. A synthetic fixture and an owner-run read-only CP4 canary.

Until then no playback code ships and the official app remains the playback
tool.
