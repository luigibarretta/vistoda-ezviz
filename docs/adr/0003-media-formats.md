# ADR-0003: Media formats and no transcoding

- Status: Accepted
- Date: 2026-08-13

## Decision

Clear profiles use MPEG-PS upstream; compatible encrypted H.264/HEVC RTP
profiles are decrypted into an MPEG-TS-compatible pipeline. Home Assistant
receives shared copy-remuxed MPEG-TS. Finite recordings retain the actual PS or
TS form and declare it in their manifest. The TS mux repeats PAT/PMT and codec
headers at keyframes so clients may join an already-running upstream. Snapshots
are JPEG.

## Consequences

Video pixels are never re-encoded. CPU use, latency and quality loss remain
low. The TS transform is lazy and exists only while a TS subscriber is active.
A bounded keyframe-aligned warm segment lets a late subscriber initialize its
decoder without opening another camera session or retaining a recording.
