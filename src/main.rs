use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "brainjar",
    about = "AI agent memory backed by AWS Bedrock Knowledge Bases + S3",
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
    /// Sync files to S3 and trigger Bedrock ingestion
    Sync {
        /// Knowledge base name (default: all auto_sync KBs)
        kb_name: Option<String>,
        /// Force re-upload of all files
        #[arg(long)]
        force: bool,
        /// Show what would be done without actually doing it
        #[arg(long)]
        dry_run: bool,
        /// Don't wait for ingestion to complete
        #[arg(long)]
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
        /// Local search only (no Bedrock)
        #[arg(long, conflicts_with = "remote")]
        local: bool,
        /// Remote (Bedrock) search only
        #[arg(long, conflicts_with = "local")]
        remote: bool,
        /// Use exact (case-insensitive substring) matching instead of fuzzy for local search
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
        Commands::Search { query, kb, limit, json, local, remote, exact } => {
            let config = brainjar::config::load_config(cli.config.as_deref())?;
            let mode = if local {
                brainjar::search::SearchMode::Local
            } else if remote {
                brainjar::search::SearchMode::Remote
            } else {
                brainjar::search::SearchMode::All
            };
            brainjar::search::run_search(&config, &query, kb.as_deref(), limit, json, mode, exact).await?;
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
