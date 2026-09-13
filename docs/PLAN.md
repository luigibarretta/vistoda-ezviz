# Vistoda EZVIZ release state

This document records the release gates for Vistoda EZVIZ. The supported user
surface is defined by the repository README, OpenAPI contract and Vistoda
compatibility matrix.

## Current 0.7.2 scope

- private account enrollment and rotating session storage;
- complete bounded camera inventory pagination;
- exact serial, channel and optional substream binding for multiple cameras;
- fresh or cached JPEG snapshots;
- one shared, cancellable upstream per camera;
- compatible clear MPEG-PS and decrypted RTP to MPEG-TS media paths;
- finite local recordings with immutable size and SHA-256 manifests;
- paginated recording inventory and browser-compatible playback;
- Home Assistant and SceneTrove consumer contracts;
- health metrics, rootless packaging and signed multi-architecture images.

Owned-camera evidence covered snapshot decoding, H.264/AAC media, concurrent
consumers, finite capture, Home Assistant playback, SceneTrove import and idle
teardown. Deterministic tests use synthetic fixtures and do not contact EZVIZ.

## Release gates

Every release must pass:

1. Rust formatting, strict Clippy, locked tests and dependency audit.
2. OpenAPI, malformed-input, recovery and consumer contract tests.
3. Exact camera binding checks before snapshot, live, recording and archive use.
4. Bounded queues, requests, live duration, recording duration and spool size.
5. Clean teardown with no abandoned producer or FFmpeg process.
6. Logs and responses free of credentials, signed URLs and camera serials.
7. Rootless read-only images for `amd64` and `aarch64`, signed from the tag.
8. Matching Vistoda app metadata and compatibility documentation.

Real-device canaries are opt-in. Prefer a powered camera, stop on provider
throttling or account warnings and verify that upstream and remux activity both
return to zero.

## Known boundary

The release does not provide two-way voice talk or direct access to files on a
camera microSD card. Those features require a usable, authorized EZVIZ Open
Platform path or new owner-authorized protocol evidence. The Android SDK is not
a Linux runtime dependency and is not shipped in the provider image.

Encrypted-stream support covers the implemented RTP/H.264/HEVC profiles, not
every EZVIZ model or firmware. Unsupported encryption fails without emitting
ciphertext as playable media. Keep the official EZVIZ app for account recovery,
talk, microSD administration and unsupported models.

## Future changes

New device profiles or uplink media remain capability-gated until their wire
format, authentication, cancellation, recovery and resource limits are proven.
Research notes record provenance; production code remains Rust-only and does
not vendor proprietary SDK binaries or third-party Python implementations.
