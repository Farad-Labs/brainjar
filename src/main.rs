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
        /// Output as JSON
        #[arg(long)]
        json: bool,
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
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Local fuzzy search only (nucleo)
        #[arg(long, conflicts_with = "text", conflicts_with = "graph", conflicts_with = "fuzzy")]
        local: bool,
        /// FTS5 text search only
        #[arg(long, conflicts_with = "local", conflicts_with = "graph", conflicts_with = "fuzzy")]
        text: bool,
        /// Graph entity traversal search
        #[arg(long, conflicts_with = "local", conflicts_with = "text", conflicts_with = "fuzzy")]
        graph: bool,
        /// Include fuzzy matching in search (slower, more comprehensive)
        #[arg(long, conflicts_with = "local", conflicts_with = "text", conflicts_with = "graph", conflicts_with = "vector")]
        fuzzy: bool,
        /// Vector similarity search only (requires embeddings config)
        #[arg(long, conflicts_with = "local", conflicts_with = "text", conflicts_with = "graph", conflicts_with = "fuzzy")]
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
    },
    /// Show knowledge base status
    Status {
        /// Knowledge base name (default: all KBs)
        kb_name: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// List all configured knowledge bases
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Initialize a new brainjar project
    Init,
    /// Run as an MCP server (stdio transport)
    Mcp,
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
            json,
        } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            brainjar::sync::run_sync(&config, kb_name.as_deref(), force, dry_run, no_wait, json)
                .await?;
        }
        Commands::Search {
            query,
            kb,
            limit,
            json,
            local,
            text,
            graph,
            fuzzy,
            vector,
            exact,
            chunks,
            doc_score,
        } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            let mode = if local {
                brainjar::search::SearchMode::Local
            } else if text {
                brainjar::search::SearchMode::Text
            } else if graph {
                brainjar::search::SearchMode::Graph
            } else if fuzzy {
                brainjar::search::SearchMode::Fuzzy
            } else if vector {
                brainjar::search::SearchMode::Vector
            } else {
                brainjar::search::SearchMode::All
            };
            brainjar::search::run_search(&config, &query, kb.as_deref(), limit, json, mode, exact, chunks, doc_score)
                .await?;
        }
        Commands::Status { kb_name, json } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            brainjar::status::run_status(&config, kb_name.as_deref(), json).await?;
        }
        Commands::List { json } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            run_list(&config, json).await?;
        }
        Commands::Init => {
            brainjar::init::run_init().await?;
        }
        Commands::Mcp => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            brainjar::mcp::run_mcp(config).await?;
        }
    }

    Ok(())
}
