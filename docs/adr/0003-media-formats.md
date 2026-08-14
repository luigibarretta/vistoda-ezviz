# ADR-0003: Media formats and no transcoding

- Status: Accepted
- Date: 2026-08-13

## Decision

MPEG-PS is the canonical upstream and recording representation. Interactive
SceneTrove and Home Assistant clients receive a shared FFmpeg copy-remux to
MPEG-TS; finite recordings remain MPEG-PS. The TS mux repeats PAT/PMT and H.264
codec headers at keyframes so clients may join an already-running upstream.
Snapshots are JPEG.

## Consequences

Video pixels are never re-encoded. CPU use, latency and quality loss remain
low. The TS transform is lazy and exists only while a TS subscriber is active.
A bounded keyframe-aligned warm segment lets a late subscriber initialize its
decoder without opening another camera session or retaining a recording.
