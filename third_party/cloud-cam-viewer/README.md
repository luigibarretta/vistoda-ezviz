# cloud-cam-viewer attribution

Upstream: https://github.com/Bahrombekk/cloud-cam-viewer

Research baseline: `6ff4ad280dc18061cf073fcd9eb7931ab28de34a`.
License: MIT, reproduced verbatim in `LICENSE` alongside this file.
License SHA-256: `09c40229c7dd566aa4e53d2710a4b0728ad51e14c858935e4a24858998499dfb`.

Vistoda treats the following as adaptations, not a certified clean-room rewrite:

| Upstream | Vistoda | Relationship |
| --- | --- | --- |
| `cloudcam/decrypt_proxy.py` | `src/transport/video_cipher.rs` | Encrypted-NAL mode classification, AES prefix processing, ordered pending buffer, inter-frame sampling and diversity heuristic |
| `cloudcam/decrypt_proxy.py` | `src/transport/video.rs` | Verification-code key preparation and bounded media classification integration |
| `cloudcam/vtm_cache.py` | `src/transport/http.rs` | Resource pagination/channel discovery approach; exact serial/channel selection in Vistoda |

Vistoda uses Rust, bounded state, per-request resource ownership and on-demand
remuxing. It does not ship the Python application or its persistent per-camera
process model. Those differences do not remove the upstream attribution or
license obligations. The encrypted-RTP tranche entered Vistoda in commit
`2d68c60`; this mapping records the conservative attribution decision following
the 2026-09-12 audit, not evidence of an independently recorded clean-room process.

Both container recipes copy this directory to
`/usr/share/doc/vistoda/third_party/cloud-cam-viewer`.
