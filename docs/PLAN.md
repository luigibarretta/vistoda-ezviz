# Delivery plan

## Outcome

Deliver a production bridge that exposes one EZVIZ CP4 cloud media session to
Home Assistant and SceneTrove without forking either consumer, leaking vendor
credentials, continuously draining the camera battery, or transcoding video.

## Phases and acceptance gates

1. **Contracts and safety** — accepted ADRs, threat model, OpenAPI contract,
   secret-redaction tests and bounded defaults.
2. **Transport** — native Rust token refresh, VTM/VTDU, fresh snapshot and a
   cancellable PS or decrypted RTP producer. Offline tests use golden wire fixtures.
3. **Fan-out** — exactly one upstream per camera, bounded subscriber queues,
   slow-consumer eviction, idle shutdown and restart backoff.
4. **Consumer media** — manifest-typed PS/TS for SceneTrove; shared FFmpeg
   copy-remux to MPEG-TS for Home Assistant; no video re-encode.
5. **Finite capture** — idempotent, duration-bounded recordings, atomic publish,
   SHA-256 manifest, authenticated download and durable idempotent ACK.
6. **Packaging** — rootless/read-only container, persistent token/recording
   state only, healthcheck, metrics and immutable CI artifacts.
7. **Canary** — snapshot decode, bounded live stream, multi-consumer fan-out,
   disconnect cleanup, restart recovery and no secret-bearing logs.
8. **Integration** — Home Assistant Generic Camera first; SceneTrove connector
   imports manifest-typed media through its existing remux path. Automatic AI remains off
   until its existing accuracy gates pass.

## Global quality gates

- Rust 1.88+, rustfmt and strict Clippy `all`, `pedantic` and `nursery`;
- property, negative, crash-recovery and HTTP contract tests;
- deterministic tests require no network or secrets;
- every loop, queue, recording and request has an explicit bound;
- no API response or log contains vendor tokens, camera serials or signed URLs;
- graceful SIGTERM and zero abandoned FFmpeg/producer processes;
- configuration is fail-closed and refuses default API credentials;
- rollback is removal of the consumer URL and bridge container only; official
  Home Assistant EZVIZ entities and SceneTrove archives remain untouched.

## Verified delivery state

- Phases 1–6 passed for the native Rust implementation: rustfmt, strict Clippy,
  deterministic unit/contract/property/recovery tests, RustSec audit, frozen
  Rust 1.88 container build and the 300-LOC guard.
- The recording contract is OpenAPI 1.1: SceneTrove commits and verifies local
  media before idempotent `DELETE`; a durable receipt closes the crash window
  and bounded tombstones preserve retry behavior across bridge restarts.
- Phases 7–8 passed against the owned CP4 on the native Rust release: fresh
  JPEG, H.264/AAC MPEG-PS and MPEG-TS, single-upstream fan-out, teardown, Home
  Assistant HLS and a finite SceneTrove import all completed successfully.
- Production runs commit `37d757b055cfe2a7323eee7b4e9c7ec1722b454b`
  at manifest digest `sha256:931a8c4564a9d9c8f2284e936c2b0d2cd35aa6e34820d75cd54cf2e4d2b71688`.
  The deployed container is healthy, rootless, read-only and has zero restarts.
- The final SceneTrove canary proved the ordering invariant: verify and fsync
  the local file, persist the recovery receipt, then issue idempotent `DELETE`.
  The bridge spool and adapter receipt set were both empty afterward; the
  durable ACK tombstone survived the transaction.
- The dedicated EZVIZ enrollment session is encrypted in the infrastructure
  vault; its plaintext enrollment artifact was securely removed after import.
- Home Assistant uses the supported Generic Camera flow as
  `camera.ezviz_cp4_vtm`. Both the proxied JPEG and a real HLS media segment
  were verified, followed by zero upstream and remux activity after teardown.
- SceneTrove imported and validated one bounded 15-second H.264/AAC capture.
  Its immutable pull unit remains disabled and is invoked explicitly, so no
  background schedule drains the doorbell battery.
- The optimized cold-start canary measured 6.72 seconds to first upstream
  MPEG-PS bytes and 10.81 seconds to first remuxed MPEG-TS bytes; both pipelines
  returned to zero active consumers after teardown.
- The production listener is private and source-filtered to its declared
  Home Assistant, SceneTrove and management consumers. No public route exists.
