use anyhow::{Context, Result};
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
        #[arg(long)]
        kb: Option<String>,
        /// Force re-index of all files
        #[arg(long)]
        force: bool,
        /// Show what would be done without actually doing it
        #[arg(long)]
        dry_run: bool,
        /// (no-op in v2: sync is always instant)
        #[arg(long, hide = true)]
        no_wait: bool,
        /// JSON output (default: human-readable)
        #[arg(long)]
        json: bool,
        /// (deprecated: human-readable is now the default)
        #[arg(short = 'H', long, hide = true)]
        human: bool,
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
        /// JSON output (default: human-readable)
        #[arg(long)]
        json: bool,
        /// (deprecated: human-readable is now the default)
        #[arg(short = 'H', long, hide = true)]
        human: bool,
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
        /// Filename stem search — combinable with --text, --graph, --vector
        #[arg(long, conflicts_with = "local")]
        filename: bool,
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
        /// Conversation context for smart search (plain text)
        #[arg(long)]
        context: Option<String>,
        /// Read conversation context from a file
        #[arg(long, conflicts_with = "context")]
        context_file: Option<String>,
    },
    /// Show knowledge base status
    Status {
        /// Knowledge base name (default: all KBs)
        #[arg(long)]
        kb: Option<String>,
        /// JSON output (default: human-readable)
        #[arg(long)]
        json: bool,
        /// (deprecated: human-readable is now the default)
        #[arg(short = 'H', long, hide = true)]
        human: bool,
    },
    /// List all configured knowledge bases
    List {
        /// JSON output (default: human-readable)
        #[arg(long)]
        json: bool,
        /// (deprecated: human-readable is now the default)
        #[arg(short = 'H', long, hide = true)]
        human: bool,
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
        /// JSON output (default: human-readable)
        #[arg(long)]
        json: bool,
        /// (deprecated: human-readable is now the default)
        #[arg(short = 'H', long, hide = true)]
        human: bool,
    },
    /// Initialize a new brainjar project
    Init,
    /// Add a folder to an existing knowledge base
    AddFolder {
        /// Knowledge base name
        kb_name: String,
        /// Folder path to add
        path: String,
        /// Apply a named decay preset (daily, reference, meetings, ephemeral, sot)
        #[arg(long)]
        preset: Option<String>,
        /// Folder title
        #[arg(long)]
        title: Option<String>,
        /// Weight boost (0.0-1.0)
        #[arg(long)]
        boost: Option<f64>,
        /// Decay horizon in days
        #[arg(long)]
        horizon: Option<u32>,
        /// Decay floor (0.0-1.0)
        #[arg(long)]
        floor: Option<f64>,
        /// Decay shape (> 0)
        #[arg(long)]
        shape: Option<f64>,
    },
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
        /// JSON output (default: human-readable)
        #[arg(long)]
        json: bool,
        /// (deprecated: human-readable is now the default)
        #[arg(short = 'H', long, hide = true)]
        human: bool,
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
                "watch_paths": kb.effective_folders().iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
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
        let paths = kb.effective_folders().iter().map(|f| f.path.as_str()).collect::<Vec<_>>().join(", ");
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
            kb,
            force,
            dry_run,
            no_wait,
            json,
            human: _,
            reembed,
        } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            brainjar::sync::run_sync(&config, kb.as_deref(), force, dry_run, no_wait, json, reembed)
                .await?;
        }
        Commands::Search {
            query,
            kb,
            limit,
            json,
            human: _,
            local,
            text,
            graph,
            vector,
            filename,
            exact,
            chunks,
            doc_score,
            smart,
            context,
            context_file,
        } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            let mode = brainjar::search::SearchMode::from_flags(text, graph, vector, local, filename);
            let resolved_context: Option<String> = if let Some(path) = context_file {
                Some(std::fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read context file: {}", path))?)
            } else {
                context
            };
            brainjar::search::run_search(&config, &query, kb.as_deref(), limit, json, mode, exact, chunks, doc_score, smart, resolved_context.as_deref())
                .await?;
        }
        Commands::Status { kb, json, human: _ } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            brainjar::status::run_status(&config, kb.as_deref(), json).await?;
        }
        Commands::List { json, human: _ } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            run_list(&config, json).await?;
        }
        Commands::Watch {
            interval,
            kb,
            daemon,
            stop,
            json,
            human: _,
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
                brainjar::watch::start_daemon(&config, effective_interval, kb.as_deref(), json)?;
            } else {
                brainjar::watch::run_watch(&config, kb.as_deref(), effective_interval, json)
                    .await?;
            }
        }
        Commands::Init => {
            brainjar::init::run_init(cli.config.as_deref()).await?;
        }
        Commands::AddFolder {
            kb_name,
            path,
            preset,
            title,
            boost,
            horizon,
            floor,
            shape,
        } => {
            run_add_folder(
                cli.config.as_deref(),
                &kb_name,
                &path,
                preset.as_deref(),
                title.as_deref(),
                boost,
                horizon,
                floor,
                shape,
            ).await?;
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
            json,
            human: _,
        } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            run_retrieve(
                &config,
                chunk_id,
                lines_before,
                lines_after,
                chunks_before,
                chunks_after,
                json,
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

#[allow(clippy::too_many_arguments)]
async fn run_add_folder(
    config_path: Option<&str>,
    kb_name: &str,
    folder_path: &str,
    preset: Option<&str>,
    title: Option<&str>,
    boost: Option<f64>,
    horizon: Option<u32>,
    floor: Option<f64>,
    shape: Option<f64>,
) -> Result<()> {
    use brainjar::config::{DecayConfig, FolderConfig, KbType};
    use colored::Colorize;
    use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};

    let mut config = brainjar::config::load_config(config_path)?;

    // Determine config file path for writing back
    let config_file = if let Some(p) = config_path {
        std::path::PathBuf::from(p)
            .canonicalize()
            .unwrap_or(std::path::PathBuf::from(p))
    } else {
        config.config_dir.join("brainjar.toml")
    };

    // Validate KB exists
    if !config.knowledge_bases.contains_key(kb_name) {
        anyhow::bail!(
            "Knowledge base '{}' not found in config.\nAvailable KBs: {}",
            kb_name,
            config.knowledge_bases.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }

    let kb_type = config.knowledge_bases[kb_name].kb_type.clone();

    // Warn if path doesn't exist
    if !std::path::Path::new(folder_path).exists() {
        println!(
            "{}",
            format!("Warning: '{}' does not exist (will be indexed when created).", folder_path).yellow()
        );
    }

    // Resolve folder config from preset or interactive
    let new_folder = if let Some(preset_name) = preset {
        // Apply named preset
        let (title_val, horizon_val, floor_val, shape_val, boost_val): (Option<&str>, Option<u32>, Option<f64>, Option<f64>, f64) =
            match preset_name {
                "daily"     => (Some("Daily Notes"),     Some(180), Some(0.3), Some(1.5), 0.1),
                "reference" => (Some("Reference Docs"),  Some(730), Some(0.6), Some(1.0), 0.2),
                "meetings"  => (Some("Meeting Notes"),   Some(90),  Some(0.2), Some(1.2), 0.1),
                "ephemeral" => (Some("Scratch Files"),   Some(30),  Some(0.0), Some(0.5), 0.0),
                "sot"       => (Some("Source of Truth"), None,      None,      None,      0.3),
                _ => anyhow::bail!(
                    "Unknown preset '{}'. Valid presets: daily, reference, meetings, ephemeral, sot",
                    preset_name
                ),
            };
        FolderConfig {
            path: folder_path.to_string(),
            title: title.map(|s| s.to_string()).or_else(|| title_val.map(|s| s.to_string())),
            weight_boost: boost.unwrap_or(boost_val),
            decay: if let (Some(h), Some(fl), Some(sh)) = (
                horizon.or(horizon_val),
                floor.or(floor_val),
                shape.or(shape_val),
            ) {
                Some(DecayConfig { horizon_days: h, floor: fl, shape: sh })
            } else {
                None
            },
        }
    } else if horizon.is_some() || floor.is_some() || shape.is_some() || boost.is_some() || title.is_some() {
        // Values provided via flags
        let weight_boost = boost.unwrap_or(0.1);
        if weight_boost > 0.5 {
            println!("{}", "Warning: weight_boost > 0.5 may dominate other signals.".yellow());
        }
        FolderConfig {
            path: folder_path.to_string(),
            title: title.map(|s| s.to_string()),
            weight_boost,
            decay: horizon.map(|h| DecayConfig {
                horizon_days: h,
                floor: floor.unwrap_or(0.3),
                shape: shape.unwrap_or(1.5),
            }),
        }
    } else {
        // Interactive mode
        let theme = ColorfulTheme::default();

        let title_str: String = Input::with_theme(&theme)
            .with_prompt("Title (optional)")
            .default(String::new())
            .allow_empty(true)
            .interact_text()?;
        let title_val = if title_str.trim().is_empty() { None } else { Some(title_str.trim().to_string()) };

        if matches!(kb_type, KbType::Code) {
            // Code KB: simple
            let boost_str: String = Input::with_theme(&theme)
                .with_prompt("Weight boost (0.0-0.5) [0.1]")
                .default("0.1".to_string())
                .interact_text()?;
            let weight_boost = boost_str.trim().parse::<f64>().unwrap_or(0.1).clamp(0.0, 1.0);
            FolderConfig {
                path: folder_path.to_string(),
                title: title_val,
                weight_boost,
                decay: None,
            }
        } else {
            // Docs KB: show decay presets
            let preset_idx = Select::with_theme(&theme)
                .with_prompt("What kind of documents are in this folder?")
                .items([
                    "Daily notes, journals, logs        (fade over months)",
                    "Reference docs, wikis              (stay relevant for years)",
                    "Meeting notes, standups            (useful for weeks)",
                    "Scratch files, temp notes          (stale in days)",
                    "Source of truth, specs             (never decay)",
                    "Custom                             (set your own values)",
                ])
                .default(0)
                .interact()?;

            let (horizon_val, floor_val, shape_val, boost_val): (Option<u32>, Option<f64>, Option<f64>, f64) =
                match preset_idx {
                    0 => (Some(180), Some(0.3), Some(1.5), 0.1),
                    1 => (Some(730), Some(0.6), Some(1.0), 0.2),
                    2 => (Some(90),  Some(0.2), Some(1.2), 0.1),
                    3 => (Some(30),  Some(0.0), Some(0.5), 0.0),
                    4 => (None,      None,      None,      0.3),
                    _ => {
                        let enable_decay = Confirm::with_theme(&theme)
                            .with_prompt("Enable temporal decay?")
                            .default(true)
                            .interact()?;
                        let (h, fl, sh) = if enable_decay {
                            let h_str: String = Input::with_theme(&theme)
                                .with_prompt("horizon_days [180]")
                                .default("180".to_string())
                                .interact_text()?;
                            let fl_str: String = Input::with_theme(&theme)
                                .with_prompt("floor (0.0-1.0) [0.3]")
                                .default("0.3".to_string())
                                .interact_text()?;
                            let sh_str: String = Input::with_theme(&theme)
                                .with_prompt("shape (>0, 1.0=linear) [1.5]")
                                .default("1.5".to_string())
                                .interact_text()?;
                            (
                                Some(h_str.trim().parse::<u32>().unwrap_or(180).max(1)),
                                Some(fl_str.trim().parse::<f64>().unwrap_or(0.3).clamp(0.0, 1.0)),
                                Some(sh_str.trim().parse::<f64>().unwrap_or(1.5).max(0.01)),
                            )
                        } else {
                            (None, None, None)
                        };
                        let b_str: String = Input::with_theme(&theme)
                            .with_prompt("weight_boost (0.0-0.5) [0.1]")
                            .default("0.1".to_string())
                            .interact_text()?;
                        let custom_boost = b_str.trim().parse::<f64>().unwrap_or(0.1).clamp(0.0, 1.0);
                        if custom_boost > 0.5 {
                            println!("{}", "Warning: weight_boost > 0.5 may dominate other signals.".yellow());
                        }
                        (h, fl, sh, custom_boost)
                    }
                };

            FolderConfig {
                path: folder_path.to_string(),
                title: title_val,
                weight_boost: boost_val,
                decay: if let (Some(h), Some(fl), Some(sh)) = (horizon_val, floor_val, shape_val) {
                    Some(DecayConfig { horizon_days: h, floor: fl, shape: sh })
                } else {
                    None
                },
            }
        }
    };

    // Validate
    if let Some(d) = &new_folder.decay {
        if d.floor < 0.0 || d.floor > 1.0 {
            anyhow::bail!("floor must be in [0.0, 1.0]");
        }
        if d.shape <= 0.0 {
            anyhow::bail!("shape must be > 0.0");
        }
    }

    let label = new_folder.title.clone().unwrap_or_else(|| folder_path.to_string());
    config.knowledge_bases
        .get_mut(kb_name)
        .unwrap()
        .folders
        .push(new_folder);

    // Serialize config back
    let toml_out = toml::to_string_pretty(&config)?;
    std::fs::write(&config_file, &toml_out)
        .with_context(|| format!("Failed to write config to {}", config_file.display()))?;

    println!(
        "{} Added folder \"{}\" to KB '{}' → {}",
        "✓".green(),
        label.cyan(),
        kb_name.bold(),
        config_file.display().to_string().dimmed()
    );

    Ok(())
}
