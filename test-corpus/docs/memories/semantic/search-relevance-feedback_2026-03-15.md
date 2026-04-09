# Search Relevance Feedback — March 15, 2026

## Feedback Captured

On **March 15, 2026**, James Liu noted that search results for "Redis connection pooling" were returning too many unrelated caching docs.

### Expected Results
- synonym-concepts.md (Redis caching layer)
- incident-report.md (Redis OOM event on 2026-03-10)

### Actual Results
- Hidden-connections.md ranked #1 (irrelevant compliance doc)
- Correct results ranked #4 and #7

## Action

Boost filename token matches. Consider date-decay weighting.
