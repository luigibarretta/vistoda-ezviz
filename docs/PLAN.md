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
  58 deterministic tests, 90% branch coverage and the 300-LOC guard.
- Phase 7 passed against the owned CP4: fresh JPEG, H.264/AAC MPEG-PS and
  MPEG-TS, one upstream for simultaneous consumers, teardown and an atomic
  finite SceneTrove import.
- Production image, Portainer Compose, source-filtered firewall policy,
  Home Assistant config-flow reconciler and SceneTrove pull unit are prepared.
- Activation remains fail-closed until a dedicated EZVIZ enrollment token is
  created; the existing Home Assistant session is never reused.
