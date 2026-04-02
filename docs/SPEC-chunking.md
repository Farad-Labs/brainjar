# Spec: Document Chunking & Chunk Retrieval

**Status:** Draft
**Date:** 2026-04-02
**Migration:** schema_version 2

## Overview

Replace file-level storage with chunk-level storage. Each file is split into semantic chunks at sync time, with line numbers tracked per chunk. This enables line-accurate search results, better embedding quality, and a new `retrieve` command for expanding context around search hits.

## Schema Changes

### New: `chunks` table
```sql
CREATE TABLE chunks (
    id         INTEGER PRIMARY KEY,
    doc_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    content    TEXT NOT NULL,
    line_start INTEGER NOT NULL,
    line_end   INTEGER NOT NULL,
    chunk_type TEXT    -- 'heading_section', 'paragraph', 'code_block', 'frontmatter'
);
```

### New: `chunks_fts` (replaces `documents_fts`)
```sql
CREATE VIRTUAL TABLE chunks_fts USING fts5(
    content,
    content='chunks',
    content_rowid='id'
);
```

With triggers for insert/update/delete sync (same pattern as current `documents_fts`).

### `documents` table changes
- `content` column is **kept** (needed for context window expansion in `retrieve`)
- Documents remain the parent record for path, hash, extracted status
- Each document has N chunks as children

### Vector embeddings
- `documents_vec` → `chunks_vec` (one embedding per chunk, not per file)
- Better quality: smaller, focused chunks get more precise embeddings

### Migration (v1 → v2)
- Create `chunks` and `chunks_fts` tables
- Re-chunk all existing documents
- Migrate `documents_fts` → `chunks_fts`
- Create `chunks_vec` if embeddings are configured
- Drop old `documents_fts` triggers/table
- Set `schema_version = 2`
- **Requires re-sync** (`brainjar sync --force`) after upgrade

## Chunking Strategy

### Markdown files (`.md`)
1. **Split on headings** — each `#`/`##`/`###` section becomes a chunk
2. **Code blocks** — fenced code blocks (``` `) stay as one chunk, tagged `code_block`
3. **Frontmatter** — YAML frontmatter (if present) is its own chunk, tagged `frontmatter`
4. **Fallback** — sections longer than ~1000 tokens get split on paragraph boundaries

### Code files (`.rs`, `.py`, `.ts`, etc.)
1. **Split on function/class boundaries** where possible (regex-based, not full AST)
2. **Fallback** — fixed-size chunks (~100 lines) with 10-line overlap

### Other text files
1. **Paragraph-based** — split on double newlines
2. **Fallback** — fixed-size chunks (~500 tokens)

### All strategies
- Minimum chunk size: 50 characters (avoid tiny fragments)
- Maximum chunk size: ~2000 tokens (stays within embedding model context)
- Each chunk records `line_start` and `line_end` (1-indexed, inclusive)
- `chunk_type` is informational (for filtering/debugging)

## CLI Changes

### `brainjar search` — updated output

**Default mode** (compact previews):
```json
{
  "chunk_id": 42,
  "file": "architecture.md",
  "line_start": 45,
  "line_end": 62,
  "chunk_type": "heading_section",
  "preview": "## Storage\nHot storage: ClickHouse (proposed)...",
  "score": 0.87,
  "sources": ["fts", "vector"]
}
```

`preview` = first ~200 chars of chunk content, truncated with `...`

**`--chunks` flag** (full content, for auto-recall):
```json
{
  "chunk_id": 42,
  "file": "architecture.md",
  "line_start": 45,
  "line_end": 62,
  "chunk_type": "heading_section",
  "content": "## Storage\n- **Hot storage:** ClickHouse (proposed, currently PostgreSQL)\n- **Cold storage:** S3 Glacier\n- **Metadata:** PostgreSQL on RDS",
  "score": 0.87,
  "sources": ["fts", "vector"]
}
```

Human-readable output adjusts similarly (shows full chunk text with `--chunks`).

### `brainjar retrieve` — new command

```
brainjar retrieve <chunk_id>                                    # full chunk content
brainjar retrieve <chunk_id> --lines-before 10 --lines-after 20 # chunk + N raw lines of context
brainjar retrieve <chunk_id> --chunks-before 1 --chunks-after 1  # chunk + neighboring chunks
brainjar retrieve <chunk_id> --json                              # JSON output
```

**Line context** (`--lines-before/after`): Raw lines from the parent document, useful for seeing exactly what's around the match.

**Chunk context** (`--chunks-before/after`): Returns the preceding/following chunks as structured objects. Useful for getting the previous/next paragraph or section as coherent units.

**Output (default):**
```
─── architecture.md:45-62 (heading_section) ───
## Storage
- **Hot storage:** ClickHouse (proposed, currently PostgreSQL)
- **Cold storage:** S3 Glacier
- **Metadata:** PostgreSQL on RDS
```

**With `--chunks-before 1 --chunks-after 1`:**
```
─── [prev] architecture.md:30-44 (heading_section) ───
## Transformation Engine
Marcus Webb's transformation engine reads from S3...

─── [match] architecture.md:45-62 (heading_section) ───
## Storage
- **Hot storage:** ClickHouse...

─── [next] architecture.md:63-78 (heading_section) ───
## Monitoring
We use Grafana + Prometheus...
```

**JSON output (with chunk context):**
```json
{
  "chunk_id": 42,
  "file": "architecture.md",
  "line_start": 45,
  "line_end": 62,
  "chunk_type": "heading_section",
  "content": "...",
  "chunks_before": [
    { "chunk_id": 41, "line_start": 30, "line_end": 44, "chunk_type": "heading_section", "content": "..." }
  ],
  "chunks_after": [
    { "chunk_id": 43, "line_start": 63, "line_end": 78, "chunk_type": "heading_section", "content": "..." }
  ],
  "lines_before": null,
  "lines_after": null
}
```

### Document-level score aggregation

When multiple chunks from the same document match, aggregate for a document-level ranking:

```
brainjar search "deployment" --doc-score          # aggregate chunk scores per document
```

Aggregation method: sum of top-3 chunk scores per document (capped). This surfaces documents that are broadly relevant vs. ones with a single incidental mention.

### MCP tools — updated

- `search_memory` — add `include_content` boolean param (default false = previews, true = full chunks)
- `search_memory` — add `doc_score` boolean param (aggregate chunks to document-level ranking)
- `retrieve_chunk` — new tool: `chunk_id`, optional `lines_before`, `lines_after`, `chunks_before`, `chunks_after`

### Plugin (brainjar-openclaw)

- **Auto-recall** uses `--chunks` flag to get full content for injection
- `memory_search` tool passes `include_content` based on agent request
- New `memory_retrieve` tool wraps `retrieve_chunk`

## Sync Changes

1. For each file in `to_upsert`:
   a. Read file content
   b. Upsert document row (path, hash, content)
   c. Delete existing chunks for this doc_id
   d. Run chunker → produces Vec<Chunk> with line ranges
   e. Insert new chunks
   f. Chunks auto-populate `chunks_fts` via triggers
2. Entity extraction operates on chunks (not full docs)
3. Embeddings are per-chunk
4. Vocabulary is still built from full document content (no change)

## Graph Changes

- Entity extraction per-chunk instead of per-document
- Entities link to chunk_id (more precise source attribution)
- Graph search results include chunk_id and line range

## Testing

1. **Chunking unit tests**: markdown splitting, code splitting, line number accuracy
2. **Migration test**: v1 DB → v2 migration, re-chunking
3. **Search integration**: chunk-level FTS returns line numbers
4. **Retrieve**: chunk retrieval with context window
5. **Embedding**: per-chunk vectors, search quality
6. **End-to-end**: sync → search → retrieve flow with test corpus

## Backward Compatibility

- Existing DBs at v1 will auto-migrate to v2 on first open
- Migration creates chunk tables and sets `extracted = 0` on all docs (forces re-extraction)
- First `brainjar sync` after upgrade will re-chunk and re-extract everything
- CLI output format changes (chunk_id added, line numbers always present)
- MCP tool params are additive (new optional params, no breaking changes)

## Cost Estimate

- More chunks = more embedding API calls (roughly 5-10x more chunks than documents)
- More entity extraction calls (but chunks are smaller, so faster per-call)
- DB size increases modestly (chunks table + original content kept)
- Search is faster (FTS on smaller chunks is more precise)
