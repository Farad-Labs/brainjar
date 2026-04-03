use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;
use brainjar::db;
use brainjar::graph::KnowledgeGraph;

#[derive(Parser)]
#[command(
    name = "brainjar",
    about = "AI agent memory with local SQLite backend — FTS5, fuzzy, and vector search",
    version,
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to config file (default: ./brainjar.toml or ~/.brainjar/brainjar.toml)
    #[arg(long, global = true)]
    config: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Sync files to the local SQLite index
    Sync {
        /// Knowledge base name (default: all auto_sync KBs)
        kb_name: Option<String>,
        /// Force re-index of all files
        #[arg(long)]
        force: bool,
        /// Show what would be done without actually doing it
        #[arg(long)]
        dry_run: bool,
        /// (no-op in v2: sync is always instant)
        #[arg(long, hide = true)]
        no_wait: bool,
        /// Human-readable output (default: JSON)
        #[arg(short = 'H', long)]
        human: bool,
        /// (deprecated: JSON is now the default)
        #[arg(long, hide = true)]
        json: bool,
        /// Re-embed all chunks without re-extracting entities (useful when switching embedding models)
        #[arg(long)]
        reembed: bool,
    },
    /// Search the knowledge base
    Search {
        /// Search query
        query: String,
        /// Knowledge base name (default: all KBs)
        #[arg(long)]
        kb: Option<String>,
        /// Maximum number of results
        #[arg(long, default_value = "5")]
        limit: usize,
        /// Human-readable output (default: JSON)
        #[arg(short = 'H', long)]
        human: bool,
        /// (deprecated: JSON is now the default)
        #[arg(long, hide = true)]
        json: bool,
        /// Local fuzzy file search (nucleo) — cannot combine with other modes
        #[arg(long, conflicts_with = "text", conflicts_with = "graph", conflicts_with = "vector")]
        local: bool,
        /// FTS5 text search (no fuzzy correction) — combinable with --graph, --vector
        #[arg(long, conflicts_with = "local")]
        text: bool,
        /// Graph entity traversal — combinable with --text, --vector
        #[arg(long, conflicts_with = "local")]
        graph: bool,
        /// Vector similarity search — combinable with --text, --graph
        #[arg(long, conflicts_with = "local")]
        vector: bool,
        /// Use exact (case-insensitive substring) matching for local search
        #[arg(long)]
        exact: bool,
        /// Return full chunk content instead of preview
        #[arg(long)]
        chunks: bool,
        /// Aggregate chunk scores per document (one result per doc)
        #[arg(long)]
        doc_score: bool,
        /// Use LLM to extract search queries from conversational text
        #[arg(long)]
        smart: bool,
    },
    /// Show knowledge base status
    Status {
        /// Knowledge base name (default: all KBs)
        kb_name: Option<String>,
        /// Human-readable output (default: JSON)
        #[arg(short = 'H', long)]
        human: bool,
        /// (deprecated: JSON is now the default)
        #[arg(long, hide = true)]
        json: bool,
    },
    /// List all configured knowledge bases
    List {
        /// Human-readable output (default: JSON)
        #[arg(short = 'H', long)]
        human: bool,
        /// (deprecated: JSON is now the default)
        #[arg(long, hide = true)]
        json: bool,
    },
    /// Watch for file changes and auto-sync
    Watch {
        /// Polling interval in seconds (default: 300 = 5 minutes)
        #[arg(long, default_value = "300")]
        interval: u64,
        /// Watch specific knowledge base only
        #[arg(long)]
        kb: Option<String>,
        /// Run as background daemon
        #[arg(long)]
        daemon: bool,
        /// Stop running daemon
        #[arg(long)]
        stop: bool,
        /// Human-readable output (default: JSON)
        #[arg(short = 'H', long)]
        human: bool,
        /// (deprecated: JSON is now the default)
        #[arg(long, hide = true)]
        json: bool,
    },
    /// Initialize a new brainjar project
    Init,
    /// Run as an MCP server (stdio transport)
    Mcp,
    /// Retrieve a chunk by ID with optional context expansion
    Retrieve {
        /// Chunk ID to retrieve
        chunk_id: i64,
        /// Lines of context before the chunk (from parent document)
        #[arg(long, default_value = "0")]
        lines_before: usize,
        /// Lines of context after the chunk (from parent document)
        #[arg(long, default_value = "0")]
        lines_after: usize,
        /// Number of preceding chunks to include
        #[arg(long, default_value = "0")]
        chunks_before: usize,
        /// Number of following chunks to include
        #[arg(long, default_value = "0")]
        chunks_after: usize,
        /// Human-readable output (default: JSON)
        #[arg(short = 'H', long)]
        human: bool,
        /// (deprecated: JSON is now the default)
        #[arg(long, hide = true)]
        json: bool,
    },
}

async fn run_list(config: &brainjar::config::Config, json: bool) -> Result<()> {

    let mut kbs: Vec<(&str, &brainjar::config::KnowledgeBaseConfig)> = config
        .knowledge_bases
        .iter()
        .map(|(n, kb)| (n.as_str(), kb))
        .collect();
    kbs.sort_by_key(|(n, _)| *n);

    let db_dir = config.effective_db_dir();

    if json {
        let mut entries = Vec::new();
        for (name, kb) in &kbs {
            let db_path = db_dir.join(format!("{}.db", name));
            let db_exists = db_path.exists();
            let (doc_count, last_sync) = if db_exists {
                if let Ok(conn) = db::open_db(name, &db_dir) {
                    let count = db::count_documents(&conn).unwrap_or(0);
                    let sync_time = db::get_meta(&conn, "last_sync")
                        .unwrap_or_default()
                        .unwrap_or_else(|| "never".to_string());
                    (count, sync_time)
                } else {
                    (0, "never".to_string())
                }
            } else {
                (0, "never".to_string())
            };
            entries.push(serde_json::json!({
                "name": name,
                "description": kb.description,
                "watch_paths": kb.watch_paths,
                "auto_sync": kb.auto_sync,
                "document_count": doc_count,
                "last_sync": last_sync,
                "db_exists": db_exists,
            }));
        }
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if kbs.is_empty() {
        println!("No knowledge bases configured.");
        return Ok(());
    }

    println!("{}\n", "Knowledge Bases:".bold().white());

    for (name, kb) in &kbs {
        let db_path = db_dir.join(format!("{}.db", name));
        let db_exists = db_path.exists();

        let (doc_count, last_sync) = if db_exists {
            if let Ok(conn) = db::open_db(name, &db_dir) {
                let count = db::count_documents(&conn).unwrap_or(0);
                let sync_time = db::get_meta(&conn, "last_sync")
                    .unwrap_or_default()
                    .unwrap_or_else(|| "never".to_string());
                (count, sync_time)
            } else {
                (0, "never (DB error)".to_string())
            }
        } else {
            (0, "never".to_string())
        };

        println!("  {}", name.bold().cyan());
        if let Some(desc) = &kb.description {
            println!("    {}", desc.dimmed());
        }
        let paths = kb.watch_paths.join(", ");
        println!("    {}  {}", "Paths:".dimmed(), paths);

        let graph_exists = KnowledgeGraph::exists(&db_dir, name);
        let graph_info = if graph_exists {
            if let Ok(kg) = KnowledgeGraph::open(&db_dir, name) {
                if let Ok(stats) = kg.stats() {
                    format!(" | Graph: {} nodes", stats.node_count)
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let auto_sync_str = if kb.auto_sync {
            "yes".green().to_string()
        } else {
            "no".dimmed().to_string()
        };

        println!(
            "    Auto-sync: {} | Docs: {} | Last sync: {}{}",
            auto_sync_str,
            doc_count.to_string().cyan(),
            last_sync.dimmed(),
            graph_info.dimmed(),
        );
        println!();
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    brainjar::db::init_vec_extension();
    let cli = Cli::parse();

    match cli.command {
        Commands::Sync {
            kb_name,
            force,
            dry_run,
            no_wait,
            human,
            json: _,
            reembed,
        } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            brainjar::sync::run_sync(&config, kb_name.as_deref(), force, dry_run, no_wait, !human, reembed)
                .await?;
        }
        Commands::Search {
            query,
            kb,
            limit,
            human,
            json: _,
            local,
            text,
            graph,
            vector,
            exact,
            chunks,
            doc_score,
            smart,
        } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            let mode = brainjar::search::SearchMode::from_flags(text, graph, vector, local);
            brainjar::search::run_search(&config, &query, kb.as_deref(), limit, !human, mode, exact, chunks, doc_score, smart)
                .await?;
        }
        Commands::Status { kb_name, human, json: _ } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            brainjar::status::run_status(&config, kb_name.as_deref(), !human).await?;
        }
        Commands::List { human, json: _ } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            run_list(&config, !human).await?;
        }
        Commands::Watch {
            interval,
            kb,
            daemon,
            stop,
            human,
            json: _,
        } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            let effective_interval = config
                .watch
                .as_ref()
                .and_then(|w| w.interval)
                .unwrap_or(interval);
            if stop {
                brainjar::watch::stop_daemon(&config)?;
            } else if daemon {
                brainjar::watch::start_daemon(&config, effective_interval, kb.as_deref(), !human)?;
            } else {
                brainjar::watch::run_watch(&config, kb.as_deref(), effective_interval, !human)
                    .await?;
            }
        }
        Commands::Init => {
            brainjar::init::run_init(cli.config.as_deref()).await?;
        }
        Commands::Mcp => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            brainjar::mcp::run_mcp(config).await?;
        }
        Commands::Retrieve {
            chunk_id,
            lines_before,
            lines_after,
            chunks_before,
            chunks_after,
            human,
            json: _,
        } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            run_retrieve(
                &config,
                chunk_id,
                lines_before,
                lines_after,
                chunks_before,
                chunks_after,
                !human,
            )
            .await?;
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_retrieve(
    config: &brainjar::config::Config,
    chunk_id: i64,
    lines_before: usize,
    lines_after: usize,
    chunks_before: usize,
    chunks_after: usize,
    json: bool,
) -> Result<()> {
    let db_dir = config.effective_db_dir();

    // Search across all KBs for the chunk
    let mut found_kb: Option<String> = None;
    let mut found_conn: Option<rusqlite::Connection> = None;
    for name in config.knowledge_bases.keys() {
        let conn = match db::open_db(name, &db_dir) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE id = ?1",
                rusqlite::params![chunk_id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            > 0
        {
            found_kb = Some(name.clone());
            found_conn = Some(conn);
            break;
        }
    }

    let conn = match found_conn {
        Some(c) => c,
        None => anyhow::bail!("Chunk {} not found in any knowledge base", chunk_id),
    };
    let _kb_name = found_kb.unwrap();

    let (_, doc_id, content, line_start, line_end, chunk_type, file_path) =
        db::get_chunk(&conn, chunk_id)?;

    // --- Optional: raw line context from parent document ---
    let raw_before: Vec<String>;
    let raw_after: Vec<String>;
    if lines_before > 0 || lines_after > 0 {
        let doc_content = db::get_document_content(&conn, doc_id)?;
        let doc_lines: Vec<&str> = doc_content.lines().collect();
        let total = doc_lines.len();

        // line_start/line_end are 0-based; we want the lines BEFORE line_start
        let before_start = line_start.saturating_sub(lines_before);
        raw_before = doc_lines[before_start..line_start]
            .iter()
            .map(|l| l.to_string())
            .collect();

        let after_end = (line_end + 1 + lines_after).min(total);
        raw_after = if line_end + 1 < total {
            doc_lines[(line_end + 1)..after_end]
                .iter()
                .map(|l| l.to_string())
                .collect()
        } else {
            Vec::new()
        };
    } else {
        raw_before = Vec::new();
        raw_after = Vec::new();
    }

    // --- Optional: neighboring chunks ---
    let (before_chunks, _, after_chunks) = if chunks_before > 0 || chunks_after > 0 {
        db::get_neighboring_chunks(&conn, chunk_id, chunks_before, chunks_after)?
    } else {
        (Vec::new(), db::ChunkRow {
            chunk_id,
            doc_id,
            content: content.clone(),
            line_start,
            line_end,
            chunk_type: chunk_type.clone(),
        }, Vec::new())
    };

    let short_path = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(&file_path);

    if json {
        let out = serde_json::json!({
            "chunk_id": chunk_id,
            "file_path": file_path,
            "line_start": line_start,
            "line_end": line_end,
            "chunk_type": chunk_type,
            "content": content,
            "context_before_lines": raw_before,
            "context_after_lines": raw_after,
            "chunks_before": before_chunks.iter().map(|c| serde_json::json!({
                "chunk_id": c.chunk_id,
                "line_start": c.line_start,
                "line_end": c.line_end,
                "chunk_type": c.chunk_type,
                "content": c.content,
            })).collect::<Vec<_>>(),
            "chunks_after": after_chunks.iter().map(|c| serde_json::json!({
                "chunk_id": c.chunk_id,
                "line_start": c.line_start,
                "line_end": c.line_end,
                "chunk_type": c.chunk_type,
                "content": c.content,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    // --- Plain text output ---
    let header = format!("─── {}:{}-{} ({}) ───", short_path, line_start, line_end, chunk_type);

    if chunks_before > 0 || chunks_after > 0 {
        // Show labeled sections
        for c in &before_chunks {
            let c_short = std::path::Path::new(&file_path)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(&file_path);
            println!("{}", format!("[prev] {}:{}-{}", c_short, c.line_start, c.line_end).dimmed());
            println!("{}", c.content.dimmed());
            println!();
        }
        if !raw_before.is_empty() {
            println!("{}", "[context before]".dimmed());
            println!("{}", raw_before.join("\n").dimmed());
            println!();
        }
        println!("{}", header.bold().cyan());
        println!("[match]");
        println!("{}", content);
        if !raw_after.is_empty() {
            println!();
            println!("{}", "[context after]".dimmed());
            println!("{}", raw_after.join("\n").dimmed());
        }
        for c in &after_chunks {
            println!();
            let c_short = std::path::Path::new(&file_path)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(&file_path);
            println!("{}", format!("[next] {}:{}-{}", c_short, c.line_start, c.line_end).dimmed());
            println!("{}", c.content.dimmed());
        }
    } else {
        println!("{}", header.bold().cyan());
        if !raw_before.is_empty() {
            println!("{}", raw_before.join("\n").dimmed());
        }
        println!("{}", content);
        if !raw_after.is_empty() {
            println!("{}", raw_after.join("\n").dimmed());
        }
    }

    Ok(())
}
