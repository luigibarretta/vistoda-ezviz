# ADR-0003: Media formats and no transcoding

- Status: Accepted
- Date: 2026-08-13

## Decision

MPEG-PS is the canonical upstream and recording representation. SceneTrove
receives it unchanged and uses its existing `mpeg_ps` remux path. Home
Assistant receives a shared FFmpeg copy-remux to MPEG-TS. Snapshots are JPEG.

## Consequences

Video pixels are never re-encoded. CPU use, latency and quality loss remain
low. The TS transform is lazy and exists only while a TS subscriber is active.
