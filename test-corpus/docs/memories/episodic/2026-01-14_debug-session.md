# Debug Session Notes — January 14, 2026

**Date:** 2026-01-14
**Participants:** Priya Patel, Sarah Chen

## Issue

Search ranking returned stale documents above recent ones. A query for "deployment checklist" on 2026-01-14 surfaced a doc from 2025-11-03 over one from 2026-01-07.

## Root Cause

No temporal signal in the scoring function. Pure cosine similarity treats all documents equally regardless of age.

## Proposed Fix

```
score_final = score_semantic * (1.0 + temporal_boost(doc_date, query_date))
```

Target implementation: 2026-01-21
