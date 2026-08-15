#!/usr/bin/env bash
set -Eeuo pipefail

readonly image="${1:-ezviz-vtm-bridge:test}"

docker run \
  --rm \
  --network none \
  --read-only \
  --user 10001:10001 \
  --tmpfs /tmp:rw,noexec,nosuid,size=16m \
  --entrypoint /bin/sh \
  "${image}" \
  -ec '
    ffmpeg -hide_banner -loglevel error \
      -f lavfi -i testsrc=size=320x180:rate=10 \
      -f lavfi -i sine=frequency=1000:sample_rate=8000 \
      -t 2 -c:v libx264 -g 10 -c:a mp2 -f mpeg pipe:1 |
    ffmpeg -hide_banner -loglevel error -f mpeg -i pipe:0 \
      -map 0:v:0 -map 0:a:0 -c copy \
      -bsf:v extract_extradata,dump_extra=freq=keyframe \
      -mpegts_flags +resend_headers -f mpegts pipe:1 >/dev/null
  '
