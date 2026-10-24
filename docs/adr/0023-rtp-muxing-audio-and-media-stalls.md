# ADR-0023: In-process RTP muxing, RTP audio and media stalls

- Status: Accepted
- Date: 2026-10-24

## Context

ADR-0017 piped decrypted Annex-B video into FFmpeg with `+genpts`. A raw
elementary stream carries no timestamps, so FFmpeg assumed 25 fps; the CP4
door viewer encodes 15 fps. Every RTP payload type other than the video one
was dropped, so cameras that send AAC on RTP (RFC 3640 `AAC-hbr`, observed by
`Stinocon/ezviz-stream-bridge` as payload type 104) played silent. A clear
camera that answers in RTP rather than MPEG-PS (the same project reports a
CS-C8c) reached the MPEG-PS remux and produced nothing.

The VTM read timeout restarted on every packet, including the relay's
keepalive replies, so a battery camera that stopped sending media mid-session
held the cloud session open until `max_live_session_seconds`. A consumer that
reconnected at once then woke the camera again two seconds later.

## Decision

- RTP media is muxed into MPEG-TS in-process (`ts_mux`, `rtp_ts`): video access
  units are cut on the RTP timestamp and closed early by the marker bit, and
  are stamped from the camera's 90 kHz clock. Output starts at the first
  IDR/IRAP picture. The encrypted path no longer starts FFmpeg.
- A non-video payload type becomes the audio track after eight consecutive
  AAC frames, in packets that read as complete `AAC-hbr` throughout with every
  frame opening with a single channel or channel pair element (a camera sends
  one frame per packet, FFmpeg's packetizer about five); detection gives up
  after 400 packets. ADTS headers are rebuilt as AAC-LC 16 kHz (the family
  convention, also FFmpeg's `config=1408`) with the channel count read from
  the stream.
- The live remux probes only 0.5 s and fixes its streams then, so video is
  held (at most 1 s, 64 units or 16 MiB) until audio is confirmed or ruled
  out; the first PMT names every stream. The confirming packets' frames are
  kept and interleaved by PTS with the held video, anchored at the video PTS
  current when the first of them arrived, minus the earlier frames it held.
  Each NAL held by the encrypted decryptor keeps its own RTP timestamp.
- The first VTM payload of a clear session decides the path: any RTP version 2
  packet goes to the same muxer without a key, which then finds the codec
  (bounded); MPEG-PS, `IMKH` and MPEG-TS can never parse as RTP and keep the
  byte-exact pass-through.
- A VTM session ends as `camera_offline` when no non-empty media payload
  arrived within the upstream timeout, whatever control traffic the relay
  sends; `live.ts` reports that cause too. After a
  `camera_offline` result the hub waits at least 30 seconds before the next
  upstream session and keeps reporting the offline cause meanwhile.

## Consequences

Frame rate and audio follow the camera, and one FFmpeg process per encrypted
session disappears. The muxer supports one program without B-frames (DTS
equals PTS), which matches every observed EZVIZ profile; a camera with
B-frames would need DTS handling. A wrong audio sample-rate assumption would
shift audio against video; the reference stream used in tests and the
reporter's capture both match 16 kHz. Encrypted audio, if a camera ever sends
it, fails the AAC validation and the session stays video-only.

`scripts/verify-rtp-ffmpeg.sh` checks the muxer against real FFmpeg RTP for
H.264 and HEVC with AAC: codecs, 15 fps, duration, full decode, the bridge's
own remux arguments keeping the audio, and one shared timeline. Cameras gain
up to one second of start-up latency only when they send no audio.
