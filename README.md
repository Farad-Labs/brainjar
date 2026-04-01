# 🧠 brainjar

> AI agent memory backed by AWS Bedrock Knowledge Bases + S3

brainjar is a CLI tool that manages persistent memory for AI agents using AWS Bedrock Knowledge Bases with S3 Vectors as the vector storage backend. It syncs files to S3, triggers Bedrock ingestion, and exposes semantic search — either directly or via an MCP server.

## Use Cases

1. **Standalone CLI** — sync your agent's memory files and search them
2. **OpenClaw plugin backend** — shell out to brainjar from the memory plugin
3. **Claude Code / MCP integration** — run `brainjar mcp` as an MCP server

## Quick Start

```bash
# Install
cargo install brainjar

# Initialize a new project
cd my-agent-workspace
brainjar init

# Edit brainjar.toml with your KB IDs (or run terraform first)
# Then sync
brainjar sync

# Search
brainjar search "deployment workflow"

# Status
brainjar status
```

## Installation

### From crates.io

```bash
cargo install brainjar
```

### From source

```bash
git clone https://github.com/farad-labs/brainjar
cd brainjar
cargo build --release
cp target/release/brainjar ~/.local/bin/
```

## Configuration

`brainjar.toml` in your project root (or `~/.config/brainjar/config.toml` globally):

```toml
[aws]
profile = "my-aws-profile"  # or use AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY
region = "us-east-1"

[knowledge_bases.memory]
kb_id = "8KAXLVSBPD"
data_source_id = "ZOGCSXXICI"
s3_bucket = "my-memory-source-bucket"
watch_paths = ["memory/", "MEMORY.md", "AGENTS.md"]
auto_sync = true

[knowledge_bases.codebase]
kb_id = "FDQBH29QQL"
data_source_id = "0GSZWQBPAD"
s3_bucket = "my-code-kb-bucket"
watch_paths = ["~/Code/my-project/docs/"]
auto_sync = false  # manual sync only
```

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

Search across knowledge bases.

```bash
brainjar search "deployment workflow"
brainjar search "API keys" --kb memory
brainjar search "database schema" --limit 10
brainjar search "auth flow" --json
```

Example output:
```
🔍 Results for "deployment workflow" (3 matches)

  1. [0.87] memory/operations.md
     ...deploy skill handles all deployment workflows. Four modes:
     homelab-static, homelab-docker, vps-static, vps-docker...

  2. [0.74] MEMORY.md
     ...Deploy skill: ~/.openclaw/homes/glitch/skills/deploy/...

  3. [0.61] memory/infrastructure.md
     ...Homelab (root@192.168.1.3), Public VPS (root@161.97.72.189)...
```

### `brainjar status [kb_name]`

Show KB health, file count, and last sync info.

```bash
brainjar status
brainjar status memory
brainjar status --json
```

### `brainjar init`

Interactive setup wizard — creates `brainjar.toml` and scaffolds Terraform templates.

### `brainjar mcp`

Run as an MCP server over stdio. Add to your Claude Code / Cursor config:

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

Available MCP tools:
- `memory_search` — search across KBs
- `memory_sync` — trigger sync
- `memory_status` — get status

## AWS Infrastructure

Run `brainjar init` to scaffold Terraform, or set up manually:

1. **S3 bucket** — stores source documents
2. **S3 Vectors index** — vector storage (requires `awscc` Terraform provider)
3. **Bedrock Knowledge Base** — Titan Embed V2, NONE chunking
4. **IAM role** — Bedrock service role

See [`terraform/README.md`](terraform/README.md) for full setup instructions.

## Architecture

```
brainjar sync
  ↓
Collect files from watch_paths
  ↓
Compute SHA256 of relative path → stable S3 key
  ↓
Compare with .brainjar/state.json
  ↓
Upload changed files to S3 (with metadata: source path)
  ↓
StartIngestionJob → poll until COMPLETE
  ↓
Update state.json

brainjar search
  ↓
Bedrock Retrieve API
  ↓
Ranked results with source attribution
```

### Key Design Decisions

- **Flat S3 filenames** — `{sha256_of_relative_path}.md` avoids S3 path encoding issues and stays within S3 Vectors' 2KB metadata limit
- **NONE chunking** — each file = one vector; hierarchical chunking hits metadata limits
- **Content-addressed state** — state tracks SHA256 of file content to detect changes
- **Non-filterable metadata** — `AMAZON_BEDROCK_TEXT` / `AMAZON_BEDROCK_METADATA` must be non-filterable in the vector index (handled by the Terraform templates)

## State File

`.brainjar/state.json` tracks sync state:

```json
{
  "version": 1,
  "knowledge_bases": {
    "memory": {
      "last_sync": "2026-03-31T01:37:00Z",
      "last_ingestion_status": "COMPLETE",
      "files": {
        "memory/2026-03-31.md": {
          "content_hash": "abc123...",
          "s3_key": "def456...md",
          "last_modified": "2026-03-31T01:37:00Z"
        }
      }
    }
  }
}
```

## License

MIT — see [LICENSE](LICENSE)
