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
  the option code is used alone; the guarded cloud copy is the fallback only
  when no option code exists or it fails the hash. Alarm pictures and
  snapshots use the same order. `live.mpegps` answers `409` once MPEG-TS is
  produced.
- `GET /v1/cameras/{camera}/encryption` reports `video_encrypted` and
  `key_source` (`option`, `cloud`, `none`) from local state only; results are
  cached 10 minutes.
- 0.9.1 amendment: the 0.9.0 deployment showed that `/api/device/query/
  encryptkey` from a third-party terminal makes EZVIZ email the owner a
  verification code. The cloud code is now requested only for an actual
  decryption without a usable option code (an option code is never
  second-guessed by a cloud lookup), cached in memory for the process
  lifetime, and limited to one request per camera every 24 hours by a
  persisted attempt time (`/data/cloud-key-attempts.json`). Any refusal,
  including result 120002, pauses that camera for 24 hours.
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

- Encrypted cameras should be configured with their label code; without it,
  the cloud copy may work but its first request can email or text the owner
  a verification code, and a refusal pauses decryption for 24 hours.
- Every new call is read-only and bounded. A wrong code is never used silently
  when the cloud provides a hash.
- Record times rely on the camera's reported fixed offset; on daylight-saving
  transition days intervals may shift by one hour until an owned sample proves
  how `timeZone` and `daylightSavingTime` interact.
- ADR-0016's microSD boundary is narrowed to playback, download and
  administration; see ADR-0020.
