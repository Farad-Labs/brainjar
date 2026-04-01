use anyhow::Result;
use clap::{Parser, Subcommand};

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

    /// Path to config file (default: brainjar.toml or ~/.config/brainjar/config.toml)
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
        #[arg(long, conflicts_with = "local", conflicts_with = "text", conflicts_with = "graph")]
        fuzzy: bool,
        /// Use exact (case-insensitive substring) matching for local search
        #[arg(long)]
        exact: bool,
    },
    /// Show knowledge base status
    Status {
        /// Knowledge base name (default: all KBs)
        kb_name: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Initialize a new brainjar project
    Init,
    /// Run as an MCP server (stdio transport)
    Mcp,
}

#[tokio::main]
async fn main() -> Result<()> {
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
            exact,
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
            } else {
                brainjar::search::SearchMode::All
            };
            brainjar::search::run_search(&config, &query, kb.as_deref(), limit, json, mode, exact)
                .await?;
        }
        Commands::Status { kb_name, json } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            brainjar::status::run_status(&config, kb_name.as_deref(), json).await?;
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
