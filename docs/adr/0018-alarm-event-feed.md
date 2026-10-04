# ADR-0018: Read-only alarm event feed

## Context

Home Assistant and other consumers need camera alarms (motion, person,
doorbell and similar) with a picture, without a second EZVIZ session or
the official integration's polling. The official app 7.6.1 reads alarms through
the unified-message API (`summarybydevice/v2`, `list/v2`) and receives pushes
over a proprietary long link and FCM. Neither push channel is practical for a
Linux provider, and the legacy MQTT endpoint is absent from the current app.

## Decision

The provider polls the regional API with its existing session every 15 seconds
(`EZVIZ_BRIDGE_ALARM_POLL_SECONDS`, 10–300, `0` disables), backing off to five
minutes on errors. HTTP 401 and `meta.code` 99997 trigger the existing session
refresh once. Only the cheap summary (`stype=92`) is read every cycle. When a
configured serial's top message or total changes, `list/v2` is paged for that
serial (`limit=20`, `endTime` = last item's epoch-millisecond `time`) until a
known message, `hasNext=false`, a page without progress, or three pages.
Messages are routed to aliases by channel. Messages are never marked read or
deleted.

The `date` parameter is the camera-local day: the offset is learned from the
device's own `timeStr` versus `time`; until known, the process `TZ` is used and
otherwise UTC. When today's pages end before a known alarm and the newest known
alarm predates local midnight, the previous day is consulted within the same
page budget.

The first successful fetch per serial primes history silently. Primed and
persisted alarms are visible as recent history but never returned to a cursor.
Each camera keeps the newest 200 alarms in `/data/alarms/<alias>/history.json`
(atomic, mode 0600) with a SHA-256 serial/channel binding, so a re-pointed
alias starts empty. Sequences are assigned per process; a random `generation`
UUID tells consumers to reset their cursor after a restart.

Pictures are downloaded at arrival with a plain GET (HTTPS only, 4 MiB cap,
15 s timeout, at most ten per camera per cycle), decoded and stored only when
the result starts with a JPEG marker. `picCrypt=0` is clear, `picCrypt=1` uses
the snapshot decryptor with the camera's verification-code file or the
`encryptkey` result, and `picCrypt=2` uses the hex `picChecksum` key with the
48-byte header, `hikpic` and `hiklittlepic` layouts seen in the app.
Unrecognized payloads keep no picture and increment
`alarm_pictures_unsupported_total`. Pictures share a 256 MiB budget split
evenly per camera and are evicted oldest first and together with their alarm.

## Consequences

Alarm latency is one poll interval plus the vendor's own delay. The feed adds
one summary request per interval for the whole account and list requests only
on change. Pre-signed picture URLs, serials, session IDs and keys are never
logged, persisted or exposed; HTTP errors are stripped of URLs. The
`endTime` cursor and the `picCrypt=2` layouts come from static decompilation
without an owned sample and fail closed until a live canary confirms them.
