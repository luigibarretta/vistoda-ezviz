# Threat model

## Protected assets

- EZVIZ session and refresh tokens;
- bridge API token;
- camera serial, cloud topology and signed media URLs;
- live household video, snapshots and finite recordings;
- availability and battery charge of the camera.

## Trust boundaries

The EZVIZ cloud and `pyezvizapi` are vendor-facing dependencies. The bridge is
the only component allowed to cross that boundary. Home Assistant and
SceneTrove are authenticated consumers on the management LAN. Browsers never
receive bridge or EZVIZ credentials.

## In-scope threats and controls

| Threat | Control |
| --- | --- |
| Unauthorized viewing | Constant-time Bearer authentication; no public route; network allowlist at deployment |
| Credential disclosure | Token files `0600`; request/exception redaction; no debug bodies, stream URLs or serial labels |
| Slow-client memory exhaustion | Bounded queues; slow subscriber eviction; maximum subscribers |
| Battery denial of service | One upstream per camera; idle grace; stream and snapshot rate limits; bounded recording duration |
| Disk exhaustion | Recording quota, maximum duration, atomic files and explicit retention owner |
| Malformed cloud frames | Bounded chunk sizes, producer restart backoff and no unsafe parsing in the HTTP process |
| FFmpeg hangs | Fixed argv, no shell, process-group termination and bounded shutdown |
| Token race/corruption | Atomic replace with fsync and strict file mode; one token owner process |
| SSRF/path traversal | Camera aliases from static config; no caller-controlled upstream URL or filesystem path |
| Supply-chain compromise | Locked dependencies, CI audit, immutable image digest and no runtime package installation |

## Deliberate exclusions

The bridge is not an Internet-facing NVR, credential manager, continuous
recorder or authorization system for end users. Authentik and the consuming
applications retain user authorization. TLS termination is unnecessary on the
container network; if traffic crosses a shared LAN it must be firewall-limited
and use an authenticated reverse proxy or mTLS in a later deployment phase.
