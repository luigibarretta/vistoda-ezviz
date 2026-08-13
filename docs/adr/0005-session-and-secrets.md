# ADR-0005: Sessions, secrets and authentication

- Status: Accepted
- Date: 2026-08-13

## Decision

The bridge owns a dedicated EZVIZ session persisted atomically in a mode-0600
token file. Enrollment accepts account password and MFA only through an
interactive one-shot command and never stores the password. Every media and
control endpoint requires a non-default token loaded from a mode-0600 file.
SceneTrove uses it as a Bearer token. Home Assistant Generic Camera uses Basic
authentication with fixed username `homeassistant` and the same high-entropy
token as password. Health is minimal and unauthenticated; metrics require
authentication.

## Consequences

The official Home Assistant session is not shared or raced. Deployment must
provision two secret files outside Git. Logs and metrics use configured camera
aliases, never serials. Basic authentication is permitted only on the isolated
service network (or through TLS) and the bridge has no public route.
