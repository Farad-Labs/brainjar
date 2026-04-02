use anyhow::{Context, Result};
use colored::Colorize;

use crate::config::{Config, KnowledgeBaseConfig};
use crate::db;
use crate::graph::KnowledgeGraph;

pub async fn run_status(config: &Config, kb_name: Option<&str>, json: bool) -> Result<()> {
    let kbs: Vec<(&str, &KnowledgeBaseConfig)> = if let Some(name) = kb_name {
        let kb = config
            .knowledge_bases
            .get(name)
            .with_context(|| format!("Knowledge base '{}' not found in config", name))?;
        vec![(name, kb)]
    } else {
        config
            .knowledge_bases
            .iter()
            .map(|(n, kb): (&String, _)| (n.as_str(), kb))
            .collect()
    };

    let mut all_statuses = Vec::new();

    for (name, kb) in &kbs {
        let db_dir = config.effective_db_dir();
        let db_path = db_dir.join(format!("{}.db", name));
        let db_exists = db_path.exists();

        let (doc_count, extracted_count, last_sync) = if db_exists {
            let conn = db::open_db(name, &db_dir)?;
            let count = db::count_documents(&conn)?;
            let extracted: i64 = conn
                .query_row("SELECT COUNT(*) FROM documents WHERE extracted = 1", [], |r| r.get(0))
                .unwrap_or(0);
            let sync_time = db::get_meta(&conn, "last_sync")?.unwrap_or_else(|| "Never".to_string());
            (count, extracted, sync_time)
        } else {
            (0, 0, "Never (DB not initialized — run brainjar sync)".to_string())
        };

        // Graph stats (optional — only if graph DB exists)
        let graph_stats: Option<crate::graph::GraphStats> = if db_exists
            && KnowledgeGraph::exists(&db_dir, name)
        {
            KnowledgeGraph::open(&db_dir, name)
                .ok()
                .and_then(|kg| kg.stats().ok())
        } else {
            None
        };

        if json {
            let mut entry = serde_json::json!({
                "name": name,
                "description": kb.description,
                "db_path": db_path.display().to_string(),
                "db_exists": db_exists,
                "document_count": doc_count,
                "extracted_count": extracted_count,
                "last_sync": last_sync,
                "auto_sync": kb.auto_sync,
                "watch_paths": kb.watch_paths,
            });
            if let Some(ref gs) = graph_stats {
                entry["graph_nodes"] = serde_json::Value::Number(gs.node_count.into());
                entry["graph_edges"] = serde_json::Value::Number(gs.edge_count.into());
            }
            all_statuses.push(entry);
        } else {
            print_kb_status(name, kb, db_exists, doc_count, extracted_count, &last_sync, graph_stats.as_ref());
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&all_statuses)?);
    }

    Ok(())
}

fn print_kb_status(
    name: &str,
    kb: &KnowledgeBaseConfig,
    db_exists: bool,
    doc_count: i64,
    extracted_count: i64,
    last_sync: &str,
    graph_stats: Option<&crate::graph::GraphStats>,
) {
    println!("\n{} {}", "📦".cyan(), name.bold().white());
    if let Some(desc) = &kb.description {
        println!("  {}", desc.dimmed());
    }
    println!(
        "  {:<20} {}",
        "Backend:".dimmed(),
        "SQLite (local)".cyan()
    );
    println!(
        "  {:<20} {}",
        "DB exists:".dimmed(),
        if db_exists {
            "yes".green().to_string()
        } else {
            "no (run brainjar sync)".yellow().to_string()
        }
    );
    println!(
        "  {:<20} {}",
        "Documents:".dimmed(),
        doc_count.to_string().cyan()
    );
    if doc_count > 0 {
        let unextracted = doc_count - extracted_count;
        if unextracted > 0 {
            println!(
                "  {:<20} {} ({} pending extraction)",
                "Extracted:".dimmed(),
                extracted_count.to_string().cyan(),
                unextracted.to_string().yellow(),
            );
        } else {
            println!(
                "  {:<20} {}",
                "Extracted:".dimmed(),
                format!("{} (all done)", extracted_count).green(),
            );
        }
    }
    println!(
        "  {:<20} {}",
        "Last sync:".dimmed(),
        last_sync.dimmed()
    );
    println!(
        "  {:<20} {}",
        "Auto sync:".dimmed(),
        if kb.auto_sync {
            "yes".green().to_string()
        } else {
            "no".dimmed().to_string()
        }
    );
    println!(
        "  {:<20} {}",
        "Watch paths:".dimmed(),
        kb.watch_paths.join(", ").dimmed()
    );
    if let Some(gs) = graph_stats {
        println!(
            "  {:<20} {} nodes, {} edges",
            "Graph:".dimmed(),
            gs.node_count.to_string().cyan(),
            gs.edge_count.to_string().cyan(),
        );
    } else {
        println!(
            "  {:<20} {}",
            "Graph:".dimmed(),
            "not built (run brainjar sync with extraction enabled)".dimmed()
        );
    }
}
