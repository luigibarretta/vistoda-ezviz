# Delivery plan

## Outcome

Deliver a production bridge that exposes one EZVIZ CP4 cloud media session to
Home Assistant and SceneTrove without forking either consumer, leaking vendor
credentials, continuously draining the camera battery, or transcoding video.

## Phases and acceptance gates

1. **Contracts and safety** — accepted ADRs, threat model, OpenAPI contract,
   secret-redaction tests and bounded defaults.
2. **Transport** — token-backed `pyezvizapi` adapter, fresh snapshot and a
   cancellable MPEG-PS producer. Offline tests use a deterministic fake.
3. **Fan-out** — exactly one upstream per camera, bounded subscriber queues,
   slow-consumer eviction, idle shutdown and restart backoff.
4. **Consumer media** — raw MPEG-PS for SceneTrove; a shared FFmpeg copy-remux
   to MPEG-TS for Home Assistant; no video re-encode.
5. **Finite capture** — idempotent, duration-bounded recordings, atomic publish,
   SHA-256 manifest and authenticated download.
6. **Packaging** — rootless/read-only container, persistent token/recording
   state only, healthcheck, metrics and immutable CI artifacts.
7. **Canary** — snapshot decode, bounded live stream, multi-consumer fan-out,
   disconnect cleanup, restart recovery and no secret-bearing logs.
8. **Integration** — Home Assistant Generic Camera first; SceneTrove connector
   imports MPEG-PS through its existing remux path. Automatic AI remains off
   until its existing accuracy gates pass.

## Global quality gates

- Python 3.12+, strict mypy and Ruff;
- branch-aware test coverage at least 90%;
- deterministic tests require no network or secrets;
- every loop, queue, recording and request has an explicit bound;
- no API response or log contains vendor tokens, camera serials or signed URLs;
- graceful SIGTERM and zero abandoned FFmpeg/producer processes;
- configuration is fail-closed and refuses default API credentials;
- rollback is removal of the consumer URL and bridge container only; official
  Home Assistant EZVIZ entities and SceneTrove archives remain untouched.

## Verified delivery state

- Phases 1–6 passed locally and in the immutable container: Ruff, strict mypy,
  59 deterministic tests, 90.64% branch coverage and the 300-LOC guard.
- Phase 7 passed against the owned CP4: fresh JPEG, H.264/AAC MPEG-PS and
  MPEG-TS, one upstream for simultaneous consumers, teardown and an atomic
  finite SceneTrove import.
- Phase 8 is active in production from immutable image
  `b49c6067d1f960af27cccb23c663d46c986602b0` at digest
  `sha256:92d9a77cfa74c19698183fbf34a1eaf53a99b9abb35afd7a1028fbb67a93f685`.
- The dedicated EZVIZ enrollment session is encrypted in the infrastructure
  vault; its plaintext enrollment artifact was securely removed after import.
- Home Assistant uses the supported Generic Camera flow as
  `camera.ezviz_cp4_vtm`. Both the proxied JPEG and a real HLS media segment
  were verified, followed by zero upstream and remux activity after teardown.
- SceneTrove imported and validated one bounded 15-second H.264/AAC capture.
  Its immutable pull unit remains disabled and is invoked explicitly, so no
  background schedule drains the doorbell battery.
- The production listener is private and source-filtered to its declared
  Home Assistant, SceneTrove and management consumers. No public route exists.
