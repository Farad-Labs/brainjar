# Debug Session Notes — December 15, 2025

**Date:** 2025-12-15
**Participants:** Marcus Webb, Sarah Chen

## Issue

Search ranking returned stale documents above recent ones. A query for "deployment checklist" on 2025-12-15 surfaced a doc from 2025-11-03 over one from 2025-12-08.

## Root Cause

No temporal signal in the scoring function. Pure cosine similarity treats all documents equally regardless of age.

## Proposed Fix

```
score_final = score_semantic * (1.0 + temporal_boost(doc_date, query_date))
```

Target implementation: 2025-12-22
