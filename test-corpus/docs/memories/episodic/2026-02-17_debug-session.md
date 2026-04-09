# Debug Session Notes — February 17, 2026

**Date:** 2026-02-17
**Participants:** James Liu, Sarah Chen

## Issue

Search ranking returned stale documents above recent ones. A query for "deployment checklist" on 2026-02-17 surfaced a doc from 2025-11-03 over one from 2026-02-10.

## Root Cause

No temporal signal in the scoring function. Pure cosine similarity treats all documents equally regardless of age.

## Proposed Fix

```
score_final = score_semantic * (1.0 + temporal_boost(doc_date, query_date))
```

Target implementation: 2026-02-24
