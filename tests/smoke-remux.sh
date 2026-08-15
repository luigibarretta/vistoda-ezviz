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
    ffmpeg -hide_banner -loglevel fatal \
      -f lavfi -i testsrc=size=320x180:rate=10 \
      -f lavfi -i sine=frequency=1000:sample_rate=8000 \
      -t 4 -c:v libx264 -g 10 -keyint_min 10 -sc_threshold 0 -bf 0 \
      -c:a mp2 -muxdelay 0 -f mpeg pipe:1 |
    ffmpeg -hide_banner -loglevel error -fflags +genpts -f mpeg -i pipe:0 \
      -map 0:v:0 -map 0:a:0 -c copy \
      -bsf:v extract_extradata,dump_extra=freq=keyframe \
      -f segment -segment_time 1 -reset_timestamps 1 \
      -segment_format mpegts \
      -segment_format_options mpegts_flags=+resend_headers \
      /tmp/late-%02d.ts

    ffmpeg -hide_banner -loglevel error -xerror \
      -i /tmp/late-01.ts -t 0.5 \
      -map 0:v:0 -map 0:a:0 -f null -
  '
