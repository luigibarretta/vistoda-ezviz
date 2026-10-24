#!/usr/bin/env bash
# Verifies the in-process RTP to MPEG-TS path against real FFmpeg.
#
# FFmpeg inside the bridge image encodes a 15 fps test pattern and 16 kHz
# mono AAC and sends them as RTP the way EZVIZ cameras do: H.264 (RFC 6184)
# or HEVC (RFC 7798) with in-band parameter sets, and AAC as RFC 3640
# mpeg4-generic AAC-hbr. The ignored Rust test receives the packets on
# 127.0.0.1:5004/5006 and writes the transport stream; the same image's
# ffprobe and a full decode then check codecs, frame rate, duration and
# that audio and video stay on one timeline. Usage: scripts/verify-rtp-ffmpeg.sh [image]
set -Eeuo pipefail

readonly image="${1:-ezviz-vtm-bridge:test}"
readonly work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT
chmod 0777 "${work}"

in_image() {
  docker run --rm --network host --user 10001:10001 \
    --volume "${work}:/work" --entrypoint "$1" "${image}" "${@:2}"
}

# Build once, then run the test binary directly so the receiver is not queued
# behind a build lock while FFmpeg is already sending.
# VISTODA_TEST_BINARY reuses a binary that is already built.
binary="${VISTODA_TEST_BINARY:-$(cargo test --locked --lib --no-run 2>&1 |
  sed -n 's/.*(\(target\/[^)]*ezviz_vtm_bridge-[0-9a-f]*\)).*/\1/p' | head -1)}"
test -x "${binary}"

for codec in h264 hevc; do
  case "${codec}" in
    h264) encoder=(-c:v libx264 -profile:v main -pix_fmt yuv420p) ;;
    hevc) encoder=(-c:v libx265 -pix_fmt yuv420p -x265-params log-level=error) ;;
  esac
  output="${work}/${codec}.ts"
  VISTODA_TS_OUTPUT="${output}" "${binary}" real_ffmpeg_rtp_becomes_mpeg_ts \
    --ignored --nocapture >"${work}/${codec}.log" 2>&1 &
  receiver=$!
  # The receiver prints "listening" once both sockets are bound.
  for _ in $(seq 1 120); do
    grep -q listening "${work}/${codec}.log" && break
    sleep 1
  done
  # -re and the duration are per input: both inputs are paced and finite.
  in_image ffmpeg -hide_banner -loglevel error \
    -re -f lavfi -i testsrc=size=640x360:rate=15:duration=6 \
    -re -f lavfi -i sine=frequency=440:sample_rate=16000:duration=6 \
    -map 0:v "${encoder[@]}" -g 30 -bf 0 -bsf:v dump_extra=freq=keyframe \
    -payload_type 96 -f rtp rtp://127.0.0.1:5004 \
    -map 1:a -c:a aac -ac 1 -ar 16000 -b:a 32k \
    -payload_type 104 -f rtp rtp://127.0.0.1:5006
  if ! wait "${receiver}"; then
    cat "${work}/${codec}.log"
    exit 1
  fi
  grep video_packets "${work}/${codec}.log"

  probe="$(in_image ffprobe -v error -show_entries \
    stream=codec_name,avg_frame_rate,sample_rate,channels:format=duration \
    -of default=noprint_wrappers=1 "/work/${codec}.ts")"
  echo "${probe}" | sed "s/^/${codec}: /"
  grep -qx "codec_name=${codec}" <<<"${probe}"
  grep -qx "codec_name=aac" <<<"${probe}"
  grep -qx "avg_frame_rate=15/1" <<<"${probe}"
  grep -qx "sample_rate=16000" <<<"${probe}"
  grep -qx "channels=1" <<<"${probe}"
  duration="$(sed -n 's/^duration=//p' <<<"${probe}")"
  awk -v d="${duration}" 'BEGIN { exit !(d >= 5.0 && d <= 6.5) }'

  # Every frame of both streams must decode without an error.
  in_image ffmpeg -hide_banner -loglevel error -xerror \
    -i "/work/${codec}.ts" -map 0:v -map 0:a -f null -

  # The bridge's own live remux (same arguments as src/remux_args.rs) probes
  # only half a second: the audio stream must already be declared by then.
  in_image ffmpeg -hide_banner -loglevel error -analyzeduration 500000 \
    -probesize 1000000 -fflags +genpts+nobuffer -i "/work/${codec}.ts" \
    -map '0:v:0?' -map '0:a:0?' -c copy \
    -bsf:v extract_extradata,dump_extra=freq=keyframe -muxdelay 0 \
    -mpegts_flags +resend_headers -f mpegts "/work/${codec}-remux.ts"
  remuxed="$(in_image ffprobe -v error -show_entries stream=codec_name \
    -of csv=p=0 "/work/${codec}-remux.ts")"
  grep -qx aac <<<"${remuxed}"
  grep -qx "${codec}" <<<"${remuxed}"

  # Audio joins after its confirmation window but never before the video,
  # and both streams end together: the clocks did not drift apart.
  # ffprobe lists the streams once per program too: keep the first two.
  timing="$(in_image ffprobe -v error -show_entries stream=start_time,duration \
    -of csv=p=0 "/work/${codec}.ts" | awk 'NF' | head -2)"
  echo "${timing}" | sed "s/^/${codec} start,duration: /"
  awk -F, -v s="$(tr '\n' ' ' <<<"${timing}")" 'BEGIN {
    n = split(s, line, " "); split(line[1], v, ","); split(line[2], a, ",")
    lead = a[1] - v[1]; gap = (a[1] + a[2]) - (v[1] + v[2]); if (gap < 0) gap = -gap
    exit !(n == 2 && lead >= 0 && lead <= 1.5 && gap <= 0.3)
  }'
  echo "${codec}: OK"
done
