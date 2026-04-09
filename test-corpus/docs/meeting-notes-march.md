# Meeting Notes — March 15, 2026

**Attendees:** Sarah Chen, Marcus Webb, Priya Patel, James Liu (CTO)

## Discussion

James raised concerns about the Redis bottleneck we've been seeing during peak hours. Marcus proposed migrating to NATS for the job queue, estimating 2 weeks of work.

Sarah presented the new schema evolution strategy for Atlas v2. Key change: switching from PostgreSQL to ClickHouse for analytics queries. Priya noted that ClickHouse on EKS requires different resource limits than PostgreSQL.

## Action Items
- Marcus: prototype NATS integration by March 22
- Priya: benchmark ClickHouse memory requirements on staging cluster
- Sarah: write ADR for schema evolution approach
- James: approve budget for ClickHouse license

## Decisions
- Approved: migrate from Redis to NATS (Q2 2026)
- Deferred: ClickHouse migration pending benchmark results
