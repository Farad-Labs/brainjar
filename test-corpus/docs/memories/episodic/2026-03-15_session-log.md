# Session Log — March 15, 2026

**Session date:** 2026-03-15
**Duration:** 45 minutes

## Topics Discussed

1. Refactoring the embedding pipeline — Diana Ross wants to split the monolithic `process_document()` into smaller stages
2. Reviewed test coverage: currently at 72%, target 85%
3. Discussed temporal weighting for search results

## Decisions

- Use cosine similarity with a time-decay factor (half-life: 90 days from 2026-03-15)
- Prioritize filename-date extraction in the next sprint

## Follow-up

- Schedule deep-dive on scoring algorithm for 2026-03-22
