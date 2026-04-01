# Brainjar v2 Design — Local-First GraphRAG

## Principle
Same CLI + MCP + plugin API as v1. Different backend: SQLite replaces AWS S3/Bedrock.

## CLI (unchanged interface)

```bash
brainjar init                              # interactive wizard (no Terraform)
brainjar sync [kb_name]                    # sync files → SQLite (embed + extract entities)
brainjar sync --force                      # re-process all files
brainjar sync --dry-run                    # preview changes
brainjar search "query"                    # unified: vector + FTS + graph + fuzzy
brainjar search --vector "query"           # vector similarity only
brainjar search --text "query"             # FTS5 BM25 only  
brainjar search --graph "entity"           # graph traversal from entity
brainjar search --fuzzy "query"            # nucleo fuzzy (file:line)
brainjar search --exact "SUPABASE_KEY"     # exact substring match
brainjar search --json "query"             # JSON output
brainjar status [kb_name]                  # DB stats, file count, entity count
brainjar mcp                               # MCP server (stdio)
```

## Config (brainjar.toml)

```toml
[embeddings]
provider = "gemini"                        # gemini | openai | ollama
model = "text-embedding-004"               # provider-specific model
api_key = "${GEMINI_API_KEY}"              # env var interpolation
dimensions = 768                           # embedding dimensions
# For ollama:
# provider = "ollama"
# model = "nomic-embed-text"
# base_url = "http://localhost:11434"

[extraction]
provider = "gemini"                        # gemini | openai | ollama
model = "gemini-3.1-flash-lite-preview"
api_key = "${GEMINI_API_KEY}"
# Set enabled = false to skip entity extraction (vector + FTS only)
enabled = true

[knowledge_bases.memory]
watch_paths = ["memory/", "MEMORY.md", "AGENTS.md"]
auto_sync = true

[knowledge_bases.codebase]
watch_paths = ["~/Code/myproject/src/"]
auto_sync = false
```

No AWS config. No S3 buckets. No Bedrock KB IDs.

## Database Schema (SQLite + GraphQLite + sqlite-vec)

Each knowledge base gets its own `.db` file at `.brainjar/<kb_name>.db`.
Uses three SQLite extensions in the same DB:
- **GraphQLite** (`graphqlite` crate) — Cypher graph queries, built-in algorithms (PageRank, Louvain, Dijkstra)
- **sqlite-vec** — HNSW vector search
- **FTS5** — BM25 keyword search (built into SQLite)

```sql
-- Source documents
CREATE TABLE documents (
  id INTEGER PRIMARY KEY,
  path TEXT UNIQUE NOT NULL,
  content TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- FTS5 full-text search index
CREATE VIRTUAL TABLE documents_fts USING fts5(
  path, content, content='documents', content_rowid='id'
);

-- Triggers to keep FTS in sync
CREATE TRIGGER documents_ai AFTER INSERT ON documents BEGIN
  INSERT INTO documents_fts(rowid, path, content) VALUES (new.id, new.path, new.content);
END;
CREATE TRIGGER documents_ad AFTER DELETE ON documents BEGIN
  INSERT INTO documents_fts(documents_fts, rowid, path, content) VALUES('delete', old.id, old.path, old.content);
END;
CREATE TRIGGER documents_au AFTER UPDATE ON documents BEGIN
  INSERT INTO documents_fts(documents_fts, rowid, path, content) VALUES('delete', old.id, old.path, old.content);
  INSERT INTO documents_fts(rowid, path, content) VALUES (new.id, new.path, new.content);
END;

-- Vector embeddings (sqlite-vec)
-- Created via sqlite-vec extension
CREATE VIRTUAL TABLE documents_vec USING vec0(
  document_id INTEGER PRIMARY KEY,
  embedding float[768]
);

-- Graph via GraphQLite (Cypher queries on SQLite)
-- GraphQLite manages its own tables internally.
-- We interact via Cypher:

-- Create entity nodes:
--   CREATE (e:Entity {name: 'Supabase', type: 'service', description: '...'})

-- Create relationships:
--   MATCH (a:Entity {name: 'LocalPage Admin'}), (b:Entity {name: 'Supabase'})
--   CREATE (a)-[:DEPENDS_ON {description: '...', source_doc: 'memory/2026-03-31.md'}]->(b)

-- Link entities to documents:
--   CREATE (d:Document {path: 'memory/2026-03-31.md'})-[:MENTIONS]->(e:Entity {name: 'Supabase'})

-- Query-time graph expansion:
--   MATCH (e:Entity {name: $entity})-[r*1..2]-(related) RETURN related.name, related.type

-- Built-in algorithms: PageRank, Louvain (communities), Dijkstra, BFS/DFS

-- Metadata
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT
);
```

## Sync Pipeline

```
brainjar sync
  │
  ├─ Walk watch_paths, collect files
  ├─ Compare content_hash with documents table
  │
  For each new/changed file:
  │  ├─ Update documents table
  │  ├─ Generate embedding (via configured provider)
  │  │   └─ Batch files for efficiency (e.g., 20 at a time)
  │  ├─ Upsert into documents_vec
  │  ├─ Extract entities + relationships (via configured LLM)
  │  │   └─ Batch files for efficiency
  │  ├─ Upsert into entities, relationships, entity_mentions
  │  └─ Update content_hash
  │
  For deleted files:
  │  ├─ Remove from documents, documents_vec
  │  ├─ Clean up orphaned entity_mentions
  │  └─ Optionally prune orphaned entities
  │
  └─ Print summary
```

## Search: Unified Query Pipeline

Default `brainjar search "query"` runs all four modes in parallel:

```
brainjar search "deployment workflow"
  │
  ├─ Vector: embed query → sqlite-vec KNN → top-K docs by cosine sim
  ├─ FTS: documents_fts MATCH query → top-K docs by BM25 rank
  ├─ Graph: extract key terms → find matching entities → walk 1-2 hops → find connected docs
  ├─ Fuzzy: nucleo fuzzy match against file contents (existing local_search.rs)
  │
  └─ Reciprocal Rank Fusion (RRF)
     Score = Σ 1/(k + rank_i) for each mode where doc appears
     Return top-N merged results
```

### JSON Output

```json
{
  "results": [
    {
      "file": "memory/2026-03-31.md",
      "score": 0.87,
      "sources": ["vector", "fts", "graph"],
      "excerpt": "...",
      "line": null
    }
  ],
  "entities": [
    { "name": "Supabase", "type": "service", "related": ["Vercel", "LocalPage"] }
  ]
}
```

## Entity Extraction Prompt

```
Extract entities and relationships from this document.

Entity types: person, project, service, tool, config, decision, concept
Relationship types: depends_on, decided_by, deployed_to, relates_to, replaces, configures, uses, created_by

Return valid JSON only:
{
  "entities": [
    {"name": "Supabase", "type": "service", "description": "Database and auth backend"}
  ],
  "relationships": [
    {"source": "LocalPage Admin", "target": "Supabase", "relation": "depends_on", "description": "Uses Supabase for auth and database"}
  ]
}

Document:
---
{file_content}
---
```

## Embedding Providers

### Interface
```rust
trait EmbeddingProvider {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
}
```

### Implementations
- **GeminiEmbedder** — `POST https://generativelanguage.googleapis.com/v1/models/text-embedding-004:embedContent`
- **OpenAIEmbedder** — `POST https://api.openai.com/v1/embeddings`
- **OllamaEmbedder** — `POST http://localhost:11434/api/embed`

## Module Structure

```
src/
  main.rs          — CLI (updated search flags)
  lib.rs           — module exports
  config.rs        — updated config (embeddings, extraction, no AWS)
  db.rs            — NEW: SQLite database (create, migrate, query helpers)
  sync.rs          — rewritten: file → SQLite + embed + extract
  search.rs        — rewritten: vector + FTS + graph + fuzzy fusion
  local_search.rs  — kept: nucleo fuzzy search
  graph.rs         — NEW: GraphQLite Cypher queries, entity extraction, graph expansion
  embed.rs         — NEW: pluggable embedding providers (Gemini, OpenAI, Ollama)
  extract.rs       — NEW: pluggable LLM entity extraction (structured JSON output)
  mcp.rs           — updated: new search params
  init.rs          — simplified: no Terraform, just config + DB init
  status.rs        — updated: show DB stats
  state.rs         — REMOVED (SQLite IS the state)
  aws.rs           — REMOVED
```

## Dependencies (Cargo.toml changes)

```toml
# Remove
# aws-config, aws-sdk-s3, aws-sdk-bedrockagent, aws-sdk-bedrockagentruntime, aws-smithy-types

# Add
graphqlite = "0.1"                                    # Cypher graph on SQLite
rusqlite = { version = "0.32", features = ["bundled", "fts5"] }
# sqlite-vec loaded as extension at runtime
reqwest = { version = "0.12", features = ["json"] }  # embedding/LLM API calls

# Keep
clap, tokio, serde, serde_json, sha2, walkdir, nucleo-matcher, etc.
```

## Migration Path

- v1 (AWS backend) continues to work — just don't update
- v2 is a new default backend
- `brainjar init` creates local config (no Terraform wizard)
- AWS sync could become an optional feature later (for multi-machine)
- The OpenClaw plugin (brainjar-openclaw) needs zero changes — it shells out to the CLI
