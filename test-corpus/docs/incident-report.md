# Incident Report — INC-2026-042

**Severity:** P1
**Duration:** 2026-03-10 02:15 UTC — 2026-03-10 05:30 UTC (3h 15m)
**On-call:** Marcus Webb

## Summary
The Atlas pipeline stopped processing events for Meridian Corp due to a Redis OOM (out of memory) condition. Approximately 450K events were delayed.

## Timeline
- 02:15 — PagerDuty alert: Redis memory > 90%
- 02:22 — Marcus acknowledged, began investigation
- 02:45 — Root cause identified: a malformed event from Meridian's new API integration caused infinite retry loops
- 03:00 — Priya scaled Redis from 8GB to 16GB as temporary mitigation
- 03:15 — Marcus deployed hotfix to skip malformed events (PR #847)
- 05:30 — Backlog fully processed, pipeline healthy

## Root Cause
Meridian Corp's v3 API integration sends events with nested arrays up to 50 levels deep. Our validation layer didn't enforce depth limits, causing the transformation engine to generate massive intermediate objects that filled Redis.

## Action Items
- [ ] Add event depth validation (max 10 levels) — assigned to Sarah Chen
- [ ] Implement Redis memory circuit breaker — assigned to Marcus Webb
- [ ] Notify Meridian about API payload guidelines — assigned to Diana Ross
- [ ] This incident strengthens the case for NATS migration (see March 15 meeting notes)
