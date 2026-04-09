# Debug Session Notes — November 03, 2025

**Date:** 2025-11-03
**Participants:** Sarah Chen, Sarah Chen

## Issue

Search ranking returned stale documents above recent ones. A query for "deployment checklist" on 2025-11-03 surfaced a doc from 2025-11-03 over one from 2025-10-27.

## Root Cause

No temporal signal in the scoring function. Pure cosine similarity treats all documents equally regardless of age.

## Proposed Fix

```
score_final = score_semantic * (1.0 + temporal_boost(doc_date, query_date))
```

Target implementation: 2025-11-10
