# ADR-0019: Read-only microSD index and video key resolution

- Status: accepted
- Date: 2026-10-04

## Context

The Home Assistant app forced `decrypt_video: false`, so cameras with video
encryption enabled could not stream unless an operator mounted a private
verification-code file. Owners who plan to uninstall the official app also
need to see whether the camera's microSD card is healthy and recording. The
EZVIZ 7.6.1 analysis in `docs/RESEARCH.md` shows that the account's cloud
`encryptkey` is the device verification code, validated by the app against
`STATUS.encryptPwd` (twice MD5), and identifies the storage-status and
record-index calls the app makes.

## Decision

- Each camera may receive an optional `verification_code` app option. The
  renderer writes it to a `0600` file under `/data/keys` as `media_key_file`;
  it never sets `decrypt_video` and the code never enters `cameras.json`,
  argv or logs.
- Live video follows the cloud `isEncrypt` flag (cached 10 minutes, failures
  60 seconds, waited for at most 3 seconds); `decrypt_video` is only the
  fallback when the flag is unknown. The key is the option code when it
  matches `encryptPwd`, else the cloud copy when it matches. Without a hash
  both are tried in that order; a code rejected by the RTP parameter set or
  by a picture header hash falls through to the next. Alarm pictures use the
  same order. `live.mpegps` answers `409` once MPEG-TS is produced.
- `GET /v1/cameras/{camera}/encryption` reports `video_encrypted` and
  `key_source` (`option`, `cloud`, `none`); results are cached 10 minutes.
- `GET /v1/cameras/{camera}/storage` maps `/api/device/queryStorageStatus`
  to `ok`, `no_card`, `unformatted`, `error` or `unknown`, with
  `capacity_mb`. It is fetched at most every 10 minutes per camera and only on
  demand, so no background poll wakes battery cameras.
- `GET /v1/cameras/{camera}/sd-records?date=YYYY-MM-DD` reads one
  camera-local day through the capability-selected record index (common or
  V2, falling back to the other) and returns at most 500 epoch-second
  intervals. Results are cached per camera and day (60 seconds for recent
  days, 10 minutes for older ones) and at most two lookups run at once.
- Toggling encryption is not implemented: it requires SMS/risk validation.
  Formatting, rebooting and deleting recordings are out of scope.

## Consequences

- Encrypted cameras work without manual secret files when the account can
  read the verification code; a configured code takes precedence.
- Every new call is read-only and bounded. A wrong code is never used silently
  when the cloud provides a hash.
- Record times rely on the camera's reported fixed offset; on daylight-saving
  transition days intervals may shift by one hour until an owned sample proves
  how `timeZone` and `daylightSavingTime` interact.
- ADR-0016's microSD boundary is narrowed to playback, download and
  administration; see ADR-0020.
