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
        let graph_db_path = db_dir.join(format!("{}_graph.db", name));
        let db_exists = db_path.exists();
        let db_size_bytes: u64 = db_path.metadata().map(|m| m.len()).unwrap_or(0);
        let graph_db_size_bytes: Option<u64> = if graph_db_path.exists() {
            graph_db_path.metadata().map(|m| m.len()).ok()
        } else {
            None
        };

        let (doc_count, extracted_count, chunk_count, vocab_count, embedding_count, last_sync) =
            if db_exists {
                let conn = db::open_db(name, &db_dir)?;
                let count = db::count_documents(&conn)?;
                let extracted: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM documents WHERE extracted = 1",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                let chunks: i64 = conn
                    .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
                    .unwrap_or(0);
                let vocab: i64 = conn
                    .query_row("SELECT COUNT(*) FROM vocabulary", [], |r| r.get(0))
                    .unwrap_or(0);
                let embeddings: i64 = if db::chunks_vec_table_exists(&conn) {
                    conn.query_row("SELECT COUNT(*) FROM chunks_vec", [], |r| r.get(0))
                        .unwrap_or(0)
                } else {
                    -1 // sentinel: vec table not configured
                };
                let sync_time = db::get_meta(&conn, "last_sync")?
                    .unwrap_or_else(|| "Never".to_string());
                (count, extracted, chunks, vocab, embeddings, sync_time)
            } else {
                (
                    0,
                    0,
                    0,
                    0,
                    -1,
                    "Never (DB not initialized — run brainjar sync)".to_string(),
                )
            };

        // Graph stats (optional — only if graph DB exists)
        let graph_stats: Option<crate::graph::GraphStats> =
            if db_exists && KnowledgeGraph::exists(&db_dir, name) {
                KnowledgeGraph::open(&db_dir, name)
                    .ok()
                    .and_then(|kg| kg.stats().ok())
            } else {
                None
            };

        if json {
            let vector_db = if embedding_count >= 0 {
                serde_json::json!({
                    "path": db_path.display().to_string(),
                    "size_bytes": db_size_bytes,
                    "embedding_count": embedding_count,
                })
            } else {
                serde_json::Value::Null
            };

            let graph_db = if let Some(ref gs) = graph_stats {
                serde_json::json!({
                    "path": graph_db_path.display().to_string(),
                    "size_bytes": graph_db_size_bytes.unwrap_or(0),
                    "node_count": gs.node_count,
                    "edge_count": gs.edge_count,
                })
            } else {
                serde_json::Value::Null
            };

            let entry = serde_json::json!({
                "name": name,
                "description": kb.description,
                "db_exists": db_exists,
                "document_count": doc_count,
                "extracted_count": extracted_count,
                "chunk_count": chunk_count,
                "vocab_count": vocab_count,
                "last_sync": last_sync,
                "auto_sync": kb.auto_sync,
                "watch_paths": kb.watch_paths,
                "vector_db": vector_db,
                "graph_db": graph_db,
            });
            all_statuses.push(entry);
        } else {
            print_kb_status(
                name,
                kb,
                &db_path,
                &graph_db_path,
                db_exists,
                db_size_bytes,
                graph_db_size_bytes,
                doc_count,
                extracted_count,
                chunk_count,
                vocab_count,
                embedding_count,
                &last_sync,
                graph_stats.as_ref(),
            );
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&all_statuses)?);
    }

    Ok(())
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Abbreviate the home directory in a path to `~`.
fn tilde_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rel) = path.strip_prefix(&home)
    {
        return format!("~/{}", rel.display());
    }
    path.display().to_string()
}

#[allow(clippy::too_many_arguments)]
fn print_kb_status(
    name: &str,
    kb: &KnowledgeBaseConfig,
    db_path: &std::path::Path,
    graph_db_path: &std::path::Path,
    db_exists: bool,
    db_size_bytes: u64,
    graph_db_size_bytes: Option<u64>,
    doc_count: i64,
    extracted_count: i64,
    chunk_count: i64,
    vocab_count: i64,
    embedding_count: i64, // -1 = vec table not configured
    last_sync: &str,
    graph_stats: Option<&crate::graph::GraphStats>,
) {
    // ── Header ──────────────────────────────────────────────────────────────
    println!("\n{} {}", "📦".cyan(), name.bold().white());
    if let Some(desc) = &kb.description {
        println!("  {}", desc.dimmed());
    }

    // ── Watch / sync metadata ────────────────────────────────────────────────
    println!(
        "  {:<20} {}",
        "Watch paths:".dimmed(),
        kb.watch_paths.join(", ").dimmed()
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
        "Last sync:".dimmed(),
        last_sync.dimmed()
    );

    // ── Document / chunk counts ──────────────────────────────────────────────
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
        "Chunks:".dimmed(),
        chunk_count.to_string().cyan()
    );
    if vocab_count > 0 {
        println!(
            "  {:<20} {}",
            "Vocabulary:".dimmed(),
            format!("{} words", vocab_count).cyan()
        );
    }

    // ── Vector DB section ────────────────────────────────────────────────────
    println!();
    if embedding_count < 0 {
        // Vec table not present — embeddings not configured
        println!(
            "  {:<20} {}",
            "Vector DB".bold().white(),
            "not configured (enable embeddings in config)".dimmed()
        );
    } else {
        println!("  {}", "Vector DB".bold().white());
        println!(
            "  {:<20} {}",
            "Path:".dimmed(),
            tilde_path(db_path).dimmed()
        );
        println!(
            "  {:<20} {}",
            "Size:".dimmed(),
            format_size(db_size_bytes).cyan()
        );
        if !db_exists {
            println!(
                "  {:<20} {}",
                "Embeddings:".dimmed(),
                "no DB yet".dimmed()
            );
        } else if chunk_count > 0 && embedding_count < chunk_count {
            let pending = chunk_count - embedding_count;
            println!(
                "  {:<20} {} ({} pending)",
                "Embeddings:".dimmed(),
                embedding_count.to_string().cyan(),
                pending.to_string().yellow(),
            );
        } else {
            println!(
                "  {:<20} {}",
                "Embeddings:".dimmed(),
                format!("{} (all done)", embedding_count).green(),
            );
        }
    }

    // ── Graph DB section ─────────────────────────────────────────────────────
    println!();
    if let Some(gs) = graph_stats {
        println!("  {}", "Graph DB".bold().white());
        println!(
            "  {:<20} {}",
            "Path:".dimmed(),
            tilde_path(graph_db_path).dimmed()
        );
        println!(
            "  {:<20} {}",
            "Size:".dimmed(),
            format_size(graph_db_size_bytes.unwrap_or(0)).cyan()
        );
        println!(
            "  {:<20} {}",
            "Entities:".dimmed(),
            format!("{} nodes", gs.node_count).cyan()
        );
        println!(
            "  {:<20} {}",
            "Relationships:".dimmed(),
            format!("{} edges", gs.edge_count).cyan()
        );
    } else {
        println!(
            "  {:<20} {}",
            "Graph DB".bold().white(),
            "not built (enable extraction in config)".dimmed()
        );
    }
}
