# ADR-0022: EZVIZ session resilience and stale control reads

- Status: Accepted
- Date: 2026-10-21

## Context

On 2026-10-20 at 06:59 and 07:46 CEST Home Assistant logged "EZVIZ account
defence check failed" and "EZVIZ controls check failed". The app log shows
`EZVIZ control read failed` with `error_type` `upstream` for both reads, a few
seconds apart, while the alarm poller logged `EZVIZ message API returned HTTP
500` at the same second. EZVIZ's cloud failed briefly; the session was valid
(no refresh or authentication error). The bridge then dropped the cached
value, so each short EZVIZ outage became an entity outage in Home Assistant.

An audit also found that concurrent requests could refresh with the same
(single-use) refresh token, that a cancelled request could lose a rotated
token, that a startup refresh failure made the process exit and crash-loop,
and that an expired session surfaced as HTTP 401, which Home Assistant reads
as a wrong bridge token.

## Decision

- **Single request helper.** Every session-bearing JSON call uses
  `transport/authed.rs` (including the guarded cloud-code lookup): HTTP 401 or
  `meta.code` 99997 is an expired session; it triggers one refresh and one
  retry. If the endpoint still refuses the freshly refreshed session, only that
  request fails (502); the account is not declared revoked. Only a 2xx with
  `meta.code` 200 (or none) confirms the session. Bodies are size-bounded and
  errors never carry URLs. A lookup refused for its session, or never
  connected, does not latch the 24-hour cloud-code guard.
- **Single-flight refresh.** One lock orders refreshes and token-file writes; a
  generation counter lets waiting callers reuse a refresh that happened
  meanwhile. The refresh runs in a detached task holding that lock, so the new
  token reaches the file (then memory) even if the HTTP caller disconnects.
  A refused refresh is not offered again for 30 minutes; a transient failure
  not for 10 seconds.
- **Refusal versus outage.** HTTP 401/403, or an envelope code that is not a
  5xx, on the refresh means EZVIZ refused the session: `reauth_required`.
  5xx, 429 and unreadable bodies are transient.
- **Degraded start.** The bridge loads the token without network I/O, serves
  at once and refreshes in the background with backoff (15 s doubling to
  10 min). `/healthz` stays 200 and adds `ezviz_session`
  (`ok`, `degraded`, `reauth_required`).
- **403 contract.** 401 is only a wrong bridge token. While the session is
  refused, every EZVIZ-backed `/v1` route answers 403
  `{"error":"reauth_required"}`; local recordings stay readable. The running
  bridge serves `/v1/enrollments`; a completed enrollment is written to
  `token.enrolled.json` and atomically adopted under the same lock.
- **Bounded, stale-tolerant reads.** One control or defence read ends within
  15 s (below Home Assistant's 20 s client timeout). When EZVIZ fails or is
  late, the last good value up to 10 minutes old is served with
  `"stale": true`. Forced reads used by writes and their read-backs never use
  a stale value; an expired session is never masked by one.
- **Pre-signed media.** Snapshot and alarm-picture URLs are fetched without
  session or bearer headers and with a size bound. HTTPS is accepted on any
  host. Since 0.13.2 plain HTTP is also accepted, for the URL and for each of
  at most three redirects, when the host is `ezvizlife.com`, `ezvizru.com` or
  `ezviz.com` or a subdomain (dot boundary): EZVIZ can return `http://` capture
  URLs (pyEzvizApi accepts both schemes), which the 0.11.0 HTTPS-only rule
  rejected.
  Nothing secret travels on that hop; an on-path attacker could read or swap
  one picture, which verification-code cameras additionally AES-encrypt. Any
  other host or scheme is refused, and errors name only scheme and host.
- **Enrollment hosts.** Region redirects are capped at two; API domains must be
  plain `*.ezvizlife.com` hosts, or `*.ezvizru.com` (the Russian region's
  rewrite, evidenced in pyEzvizApi `c713642f`).

## Consequences

Short EZVIZ outages no longer make Home Assistant entities unavailable, and a
revoked session leads to Home Assistant's re-authentication flow instead of a
crash loop or a misleading "bad token". Metrics add
`session_refreshes_total{result}`, `session_auth_failures_total`,
`session_reauth_required`, `api_auth_failures_total`,
`control_read_failures_total{read,category}` and
`control_stale_reads_total{read}` and `session_persist_failures_total` (a token
file that could not be written; the rotated session is kept in memory). A misclassified refusal blocks EZVIZ routes
until a request succeeds again, which clears the state.
