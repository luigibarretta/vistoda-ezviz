# ADR-0021: Native-free camera controls

- Status: accepted
- Date: 2026-10-05

## Context

The Vistoda Home Assistant panel read and wrote camera settings through Home
Assistant core's `ezviz` integration (pyezvizapi 1.0.0.7). That integration
needs its own EZVIZ login; when it expired, the panel lost every control even
though Vistoda EZVIZ held a valid session of its own (token refresh, MFA
enrollment, regional API). On 2026-10-05 we also learned that
`/api/device/query/encryptkey` makes EZVIZ email the owner a security code, so
any new vendor call must be checked for verification side effects first.

## Decision

Vistoda EZVIZ exposes the controls itself, over its existing session:

- `GET /v1/cameras/{camera}/controls` returns `online`, `defence_enabled`,
  `alarm_schedule_enabled` (read-only), `detection_mode`, `sensitivity`,
  `switches`, `ptz`, `battery` and `firmware`. Data comes from one
  account-wide resource-list fetch (`STATUS,SWITCH,TIME_PLAN,UPGRADE`), cached
  60 seconds (30 seconds after a failure), single-flight, configured serials
  only, with `encryptPwd` dropped. Sensitivity is the one exception: it is not
  in the resource list, so cameras whose `supportExt["61"]` is `1` (type 0,
  0..=6) or `3` (type 3, 0..=100) get a per-camera `queryAlgorithmConfig`
  read with the same cache lifetimes. Nothing is polled in the background.
- `PUT /v1/cameras/{camera}/controls` takes `{key, value, expected_value}` for
  `defence_enabled`, `detection_mode`, `sensitivity` or `switch.<name>`. It
  forces a re-read, answers `409 conflict` (with `current`) on a stale
  expectation and `409 unsupported_control` when the camera does not report
  the control, sends exactly one write, and reads back up to three times
  (after 1, 2 and 3 seconds). A read-back that still disagrees triggers one
  rollback to the previous value and `502 unconfirmed`; when no read-back
  succeeds there is no rollback, also `502 unconfirmed`. Writes and PTZ steps
  are serialized across the account; there is no recursion and no unbounded
  retry (pyezvizapi's own `504` retry is unbounded; ours makes two attempts).
- `POST /v1/cameras/{camera}/ptz` sends one `START`+`STOP` at speed 5 when
  `supportExt["154"]` is `1`, otherwise `409`. `STOP` is retried once.
- `GET`/`PUT /v1/account/defence` read and change the group defence mode
  (`home` 1, `away` 2, `sleep` 3) with the same expected-value, read-back and
  single-rollback rules. Home Assistant core maps disarm to `home`.

Switch names (EZVIZ type, `supportExt` key required as in Home Assistant):
`status_light` 3, `privacy` 7 (40), `infrared_light` 10 (48), `sleep` 21
(62), `audio` 22 (63), `motion_tracking` 25 (73), `all_day_video_recording`
29 (88), `auto_sleep` 32 (144), `human_detection` 200, `flicker_light_on_movement`
301 (96), `pir_motion_activated_light` 305 (297), `tamper_alarm` 306 (327),
`wdr` 604, `distortion_correction` 617, `follow_movement` 650 (198), `logo`
702. Unknown types (for example remote unlock 458, Wi-Fi or 4G) are never
exposed or writable.

Every endpoint was confirmed in pyezvizapi 1.0.0.7 and in the 7.6.1 app
without a `validateCode`, SMS or risk parameter (evidence in
`docs/RESEARCH.md`). The control source trait has no verification-code method,
the service never holds a `DeviceSource`, and tests assert that the control
routes never request the cloud code and that the control transport does not
reference `encryptkey`.

## Not implemented

- Video encryption toggle (`encryptedInfo/risk`) and encryption-key batch
  queries: they need SMS/risk validation or can trigger a verification email.
- Battery work mode, alarm sound mode, do-not-disturb, offline notification,
  alarm schedule editing, siren, firmware upgrade and reboot: outside the
  requested contract; battery mode is reported read-only.

## Consequences

- The panel no longer needs the separate Home Assistant `ezviz` login.
- Controls can be up to 60 seconds stale; a write always re-reads first, so a
  stale view yields `409` instead of overwriting a change made in the app.
- A `PUT` can take several seconds (up to three read-backs) and, on an
  unconfirmed change, may leave the camera in either state until the next read.
- NVR channels share their device's switches; defence uses the camera channel.
