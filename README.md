# 🧠 brainjar

> AI agent memory backed by AWS Bedrock Knowledge Bases + S3

brainjar gives AI agents persistent, searchable memory. Sync markdown files to S3, index them with Bedrock Knowledge Bases (Titan Embed V2), and search with three complementary engines — semantic vectors, fuzzy matching, and exact text. Works as a standalone CLI or as an MCP server for Claude Code, Cursor, and any MCP-compatible tool.

## Features

- **Unified search** — runs Bedrock semantic search + local fuzzy search in parallel by default
- **Fuzzy matching** — powered by [nucleo](https://github.com/helix-editor/nucleo) (same engine as Helix editor), tolerates typos and partial matches
- **Exact text search** — case-insensitive substring matching with file:line references
- **Incremental sync** — content-addressed uploads, only syncs changed files (~12s for one file)
- **MCP server** — stdio transport, works with Claude Code, Cursor, Windsurf, and any MCP client
- **Multiple knowledge bases** — search across separate KBs (e.g., personal memory + project docs)
- **Terraform templates** — scaffold your AWS infrastructure with `brainjar init`

## Quick Start

```bash
# Install from source (crates.io publishing coming soon)
git clone https://github.com/Farad-Labs/brainjar
cd brainjar
cargo install --path .

# Initialize a new project
cd my-agent-workspace
brainjar init

# Edit brainjar.toml with your KB IDs
# Then sync your files
brainjar sync

# Search (runs both semantic + fuzzy by default)
brainjar search "deployment workflow"
```

## Search Modes

brainjar combines remote (Bedrock) and local (file) search for comprehensive results.

```bash
# Default: runs BOTH remote + local in parallel
brainjar search "deployment workflow"

# Remote only (Bedrock semantic search)
brainjar search --remote "deployment workflow"

# Local only (fuzzy matching — tolerates typos)
brainjar search --local "branjiar"      # finds "brainjar"

# Local exact (case-insensitive substring)
brainjar search --local --exact "SUPABASE_SECRET_KEY"

# JSON output (for programmatic use)
brainjar search --json "paperclip agent"
```

### Output Format

**Human-readable:**
```
🔍 Results for "deployment workflow"

── Remote (Bedrock) ──────────────────────
  1. [0.87] memory/operations.md
     ...deploy skill handles all deployment workflows...

  2. [0.74] MEMORY.md
     ...Deploy skill: ~/.openclaw/homes/glitch/skills/deploy/...

── Local (files) ─────────────────────────
  1. [1.00] memory/2026-03-31.md:47
     New deployment flow decided:
  
  2. [0.85] memory/infrastructure.md:12
     Homelab (root@192.168.1.3), Public VPS (root@161.97.72.189)
```

**JSON:**
```json
{
  "remote": [
    { "kb": "memory", "score": 0.87, "source_path": "memory/operations.md", "excerpt": "..." }
  ],
  "local": [
    { "file": "memory/2026-03-31.md", "line": 47, "match": "New deployment flow decided:", "score": 1.0 }
  ]
}
```

### When to Use Each Mode

| Mode | Best for | Example |
|------|----------|---------|
| Default (both) | General questions | "how does auth work?" |
| `--remote` | Semantic/conceptual queries | "error handling patterns" |
| `--local` | Finding specific strings, typo-tolerant | "SUPBASE_URL" |
| `--local --exact` | Exact config values, env vars, URLs | "SUPABASE_SECRET_KEY" |

## Commands

### `brainjar sync [kb_name]`

Sync files to S3 and trigger Bedrock ingestion.

```bash
brainjar sync              # sync all auto_sync KBs
brainjar sync memory       # sync specific KB
brainjar sync --force      # re-upload all files
brainjar sync --dry-run    # preview without changes
brainjar sync --no-wait    # don't wait for ingestion
brainjar sync --json       # JSON output
```

### `brainjar search <query>`

Search across knowledge bases. See [Search Modes](#search-modes) above.

```bash
brainjar search "query"                     # both remote + local
brainjar search --local "query"             # local fuzzy only
brainjar search --local --exact "query"     # local exact only
brainjar search --remote "query"            # remote (Bedrock) only
brainjar search --kb memory "query"         # specific KB
brainjar search --limit 10 "query"          # more results
brainjar search --json "query"              # JSON output
```

### `brainjar status [kb_name]`

Show KB health, file count, and last sync info.

```bash
brainjar status            # all KBs
brainjar status memory     # specific KB
brainjar status --json     # JSON output
```

### `brainjar init`

Interactive setup wizard — creates `brainjar.toml` and scaffolds Terraform templates.

### `brainjar mcp`

Run as an MCP server over stdio.

## MCP Integration

Add to your Claude Code or Cursor config:

```json
{
  "mcpServers": {
    "brainjar": {
      "command": "brainjar",
      "args": ["mcp"],
      "cwd": "/path/to/your/project"
    }
  }
}
```

### MCP Tools

| Tool | Description | Parameters |
|------|-------------|------------|
| `memory_search` | Search across KBs | `query` (required), `mode` ("all"/"local"/"remote"), `exact` (bool), `kb`, `limit` |
| `memory_sync` | Trigger file sync + ingestion | `kb`, `force` |
| `memory_status` | Get KB status | `kb` |

## Configuration

`brainjar.toml` in your project root (or `~/.config/brainjar/config.toml` globally):

```toml
[aws]
profile = "my-aws-profile"  # or use AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY
region = "us-east-1"

[knowledge_bases.memory]
kb_id = "YOUR_KB_ID"
data_source_id = "YOUR_DS_ID"
s3_bucket = "your-memory-source-bucket"
watch_paths = ["memory/", "MEMORY.md", "AGENTS.md"]
auto_sync = true

[knowledge_bases.docs]
kb_id = "ANOTHER_KB_ID"
data_source_id = "ANOTHER_DS_ID"
s3_bucket = "your-docs-bucket"
watch_paths = ["~/Code/my-project/docs/"]
auto_sync = false  # manual sync only
```

## AWS Infrastructure

brainjar requires:

1. **S3 bucket** — stores source documents
2. **S3 Vectors index** — vector storage (requires `awscc` Terraform provider)
3. **Bedrock Knowledge Base** — configured with Titan Embed V2
4. **IAM role** — Bedrock service role for S3 + embedding access
5. **IAM user** — agent access for sync + search

Run `brainjar init` to scaffold Terraform templates, or see the [terraform/](terraform/) directory.

### Key Infrastructure Decisions

- **S3 Vectors** (not OpenSearch) — ~$2-3/month vs $70+/month for OpenSearch Serverless
- **NONE chunking** — each file = one vector; hierarchical/fixed chunking hits S3 Vectors' 2KB filterable metadata limit
- **Non-filterable metadata** — `AMAZON_BEDROCK_TEXT` and `AMAZON_BEDROCK_METADATA` must be configured as non-filterable in the vector index
- **Flat S3 keys** — `{sha256_of_relative_path}.md` avoids path encoding issues and metadata size limits

## Architecture

```
brainjar sync                         brainjar search "query"
  │                                     │
  ├─ Walk watch_paths                   ├─ Remote: Bedrock Retrieve API
  ├─ SHA256 content hash                │   └─ Semantic vector search
  ├─ Compare with .brainjar/state.json  │
  ├─ Upload changed files to S3         ├─ Local: nucleo fuzzy / exact
  ├─ StartIngestionJob                  │   └─ Walk watch_paths, search line-by-line
  └─ Update state.json                 │
                                        └─ Merge + rank results
```

## Cost

Running two knowledge bases with ~400 documents total:

| Resource | Monthly Cost |
|----------|-------------|
| S3 Vectors | ~$1-2 |
| Bedrock (Titan Embed V2) | ~$0.50-1.00 |
| S3 storage | ~$0.05 |
| Cost Explorer API | ~$0.01 |
| **Total** | **~$2-3/month** |

## License

MIT — see [LICENSE](LICENSE)
