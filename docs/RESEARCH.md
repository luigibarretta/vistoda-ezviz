# EZVIZ VTM research log

Reviewed 2026-08-13 against immutable commit IDs.

| Project | Commit | License | Relevant evidence | Adoption |
| --- | --- | --- | --- | --- |
| [Bobsilvio/ezviz_hp7](https://github.com/Bobsilvio/ezviz_hp7) | `a6c038a3bb8ef2395823c3217dd51d132b4da88f` | MIT, with Apache-2.0 vendored directory | VTM cloud relay, retry lockout, bounded queues, watchdog, GOP cache, AAC/PES quirk | Lifecycle safeguards; no copied code |
| [RenierM26/pyEzvizApi](https://github.com/RenierM26/pyEzvizApi) | `c713642fd99c3467efe1285dfc5d085714a00b50` | Apache-2.0 | VTM/VTDU transport, session reuse, snapshots, MPEG-PS and copy-remux | Compatibility oracle; not linked at runtime |
| [albrzmr/ezviz_hp7](https://github.com/albrzmr/ezviz_hp7) | `b3dcd6e4e7bdb3467f8a164fbb5867a2c672475f` | MIT | Independent CPD7 LAN path, single upstream, warm/keyframe buffer, failure diagnostics | Design cross-check only |
| [LethalEthan/LE-EZVIZ-VS](https://github.com/LethalEthan/LE-EZVIZ-VS) | `35a267cf2523034cd4022224a3dec2b7a0f6dbb7` | LGPL-2.1 | VTM/VTDU handoff, protobuf messages, keepalive and MPEG-PS/RTP variants; encryption incomplete | Protocol corroboration only |

The installed CP4 is a different model from HP7/CP7. Therefore its captured
stream is authoritative: repository observations become requirements only
after an offline fixture or owner-run live canary reproduces them. In
particular, the HP7/CP7 AAC track reportedly appears under an MPEG audio PES
identifier; the bridge does not add audio transcoding unless FFprobe/FFmpeg
prove the CP4 needs it. Video is never re-encoded.
