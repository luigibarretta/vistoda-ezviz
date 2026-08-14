# ADR-0004: Single upstream and bounded fan-out

- Status: Accepted
- Date: 2026-08-13

## Decision

At most one VTM stream may exist per configured camera. A hub distributes
bounded chunks to subscribers. Full queues evict only the slow subscriber.
The MPEG-TS fan-out retains at most one bounded warm segment beginning near the
latest random-access point and replays it before live chunks to late clients.
The upstream stops after a configurable idle grace and restarts with bounded
exponential backoff after failure.

## Consequences

Multiple dashboards do not multiply cloud sessions or battery drain. Clients
must reconnect after eviction or upstream restart; HTTP responses never buffer
an unbounded stream.
