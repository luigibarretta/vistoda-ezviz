# ADR-0024: microSD intervals as recordings

- Status: Accepted
- Date: 2026-10-24
- Supersedes the open questions of ADR-0020

## Context

ADR-0020 kept microSD playback unshipped until the `/playback?` request was
proven. pyEzvizApi PR #266 (adirgan, validated by its author and by
`renatobasa88/ezviz_sd_card_recordings` on H8c and C6N) supplies it: the
same VTM host, port, `streamBizUrl` and VTDU token as live, the path
`/playback?` with `dev`, `chn`, `stream=1`, `begin`, `end`, `serial`,
`cln=3`, `isp=0`, `auth=1`, `ssn`, `rnd`, `timestamp` and `etp=1`, and a
StreamInfo request with client version `v3.9.0.20251120` and field 8 = 10.
`begin`/`end` are the camera's wall clock as `yyyyMMddTHHmmssZ` despite the
`Z`; UTC values are answered with result 5402. The relay sends the interval
on the usual media channels (0x01, and 0x02 on some relays) and then closes.

## Decision

A microSD interval is saved as an ordinary recording:
`POST /v1/cameras/{camera}/recordings` with `{"source":"sd","start":…,"end":…}`
in epoch seconds, bounded like live recordings. The bridge converts the
bounds with the camera's `STATUS.optionals.timeZone`, opens a playback
session, and writes it through the same spool writer (alignment, byte bound,
SHA-256, atomic rename). The manifest carries `source: "sd"`, `sd_start` and
`sd_end`; media, MP4 playback, NFS backup and SceneTrove acknowledgement are
unchanged. A relay that closes after media ends the interval; closing before
media, a session error or a spool overflow fails the recording and keeps no
partial file. Result 5402 becomes `sd_no_recording` (HTTP 404 on the live
path, `error_code` on recordings); 5404 and 5405 (the device did not answer
the relay, a battery camera that did not wake) become `camera_offline`.

An interval is refused while the camera streams live (429) or has another
active recording (409). Encrypted cameras use the existing RTP decryptor
with the first usable verification code. `scenetrove-pull --sd-start
--sd-end` imports an interval and names the file after the recording time;
`--sd-day YYYY-MM-DD` imports the day's listed clips not yet in the
destination's `.sd-ledger.json`, at most `--sd-max-clips` attempts per run.
A failed bridge recording keeps its idempotency key, so a clip whose camera
did not wake is retried on a later run under a new attempt key, at most three
times; `sd_no_recording` (overwritten on the card) is never retried.

## Consequences

Owners can keep event clips from the card without the vendor app, and
SceneTrove can archive them beside live captures. The camera must wake to
serve playback, like live view. Seeking, speed control and whole-file
downloads are not offered. The request shape is third-party evidence: the
first owner-run CP4 canary is release evidence, and a 5402 for a listed
interval would point at the time conversion first.
