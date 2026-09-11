# ADR-0017: Bounded encrypted RTP compatibility

## Context

Some VTM camera profiles send clear MPEG-PS, while others return AES-obscured
H.264/HEVC NAL units inside RTP. Resource discovery may also span multiple VTM
pages and NVR channels. Treating every response as MPEG-PS either breaks live
view or risks forwarding encrypted bytes under a false content type.

## Decision

Camera configuration explicitly selects serial, channel, stream profile and
whether encrypted video is expected. Discovery scans a bounded complete
inventory and matches the exact serial/channel. One-shot VTDU token allocation
is serialized.

The encrypted path is enabled only with a private verification-code file. It
parses bounded RTP packets and H.264/HEVC aggregation/fragmentation units,
detects the compatible encrypted NAL form and decrypts only the encrypted
prefix. A short ordered buffer classifies ambiguous inter frames; wrong keys,
unsupported framing and oversized state fail closed. FFmpeg is started on
demand only to copy/remux the resulting Annex-B media into MPEG-TS.

Raw MPEG-PS remains available only for clear profiles. Recordings declare their
actual MPEG-PS or MPEG-TS content type and consumers validate that type, prefix,
size and SHA-256 before commit.

## Consequences

This brings the reusable pagination/channel/decryption ideas observed in the
MIT-licensed `Bahrombekk/cloud-cam-viewer` into Vistoda without adopting its
per-camera permanent processes, one-hour token cache, Python monkey patches or
secret-bearing arguments. It is not a universal EZVIZ decryptor: encrypted
program/transport-stream profiles remain unsupported until captured fixtures
justify a separately tested parser.
