# AtlasQL Reference

AtlasQL is the domain-specific language for writing data transformations in Atlas. Marcus Webb designed the language, and Sarah Chen contributed the type system.

## Basic Syntax

```atlasql
TRANSFORM events
WHERE source = "meridian"
SELECT
  event_id,
  timestamp,
  payload.user_id AS user,
  COALESCE(payload.amount, 0) AS amount
INTO cleaned_events;
```

## Functions

| Function | Description | Example |
|----------|-------------|---------|
| `COALESCE` | First non-null value | `COALESCE(a, b, 0)` |
| `FLATTEN` | Unnest arrays | `FLATTEN(payload.items)` |
| `HASH` | SHA-256 hash | `HASH(user_id)` |
| `WINDOW` | Time window aggregation | `WINDOW(5m, COUNT(*))` |

## Pipeline Chaining

Transforms can be chained using the `PIPE` operator:

```atlasql
TRANSFORM raw_events
PIPE validate_schema
PIPE enrich_user_data
PIPE aggregate_hourly
INTO final_output;
```

## Known Limitations
- Max 10 PIPE stages per transform (tracked in JIRA ATL-234)
- WINDOW function doesn't support session windows yet (ATL-301)
- Priya noted that transforms with > 5 JOINs cause memory issues on the 4GB worker pods
