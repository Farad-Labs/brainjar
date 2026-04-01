use anyhow::{Context, Result};
use colored::Colorize;
use rusqlite::Connection;
use std::collections::HashMap;

use crate::config::Config;
use crate::db;
use crate::fuzzy;
use crate::local_search::{run_local_search, LocalSearchResult};

/// An FTS5 search result.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FtsResult {
    pub path: String,
    pub excerpt: String,
    pub score: f64,
}

/// A unified search result (for JSON output).
#[derive(Debug, serde::Serialize)]
pub struct UnifiedResult {
    pub file: String,
    pub score: f64,
    pub sources: Vec<String>,
    pub excerpt: String,
    pub line: Option<u32>,
}

/// Search mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Run FTS + graph, merge with RRF (fast — no fuzzy by default)
    All,
    /// Run FTS + graph + fuzzy, merge with RRF (slower, more comprehensive)
    Fuzzy,
    /// Local fuzzy only (nucleo)
    Local,
    /// FTS5 BM25 only
    Text,
    /// Graph traversal from matching entities
    Graph,
}

pub async fn run_search(
    config: &Config,
    query: &str,
    kb_name: Option<&str>,
    limit: usize,
    json: bool,
    mode: SearchMode,
    exact: bool,
) -> Result<()> {
    let run_fts = matches!(mode, SearchMode::All | SearchMode::Text | SearchMode::Fuzzy);
    let run_local = matches!(mode, SearchMode::Local);
    let run_graph = matches!(mode, SearchMode::All | SearchMode::Graph | SearchMode::Fuzzy);

    // For fuzzy mode: correct the query via vocabulary before FTS/graph search
    let (effective_query, query_corrections) = if mode == SearchMode::Fuzzy {
        // Use the first available KB's connection for vocabulary lookup
        let kbs: Vec<(&str, &crate::config::KnowledgeBaseConfig)> = if let Some(name) = kb_name {
            let kb = config
                .knowledge_bases
                .get(name)
                .with_context(|| format!("Knowledge base '{}' not found in config", name))?;
            vec![(name, kb)]
        } else {
            config
                .knowledge_bases
                .iter()
                .map(|(n, kb)| (n.as_str(), kb))
                .collect()
        };

        if let Some((first_kb_name, _)) = kbs.first() {
            let conn = db::open_db(first_kb_name, &config.config_dir)?;
            match fuzzy::correct_query(&conn, query) {
                Ok((corrected, corrections)) => (corrected, corrections),
                Err(_) => (query.to_string(), Vec::new()),
            }
        } else {
            (query.to_string(), Vec::new())
        }
    } else {
        (query.to_string(), Vec::new())
    };

    let search_query = effective_query.as_str();

    // Collect FTS results across KBs
    let fts_results: Vec<FtsResult> = if run_fts {
        let kbs: Vec<(&str, &crate::config::KnowledgeBaseConfig)> = if let Some(name) = kb_name {
            let kb = config
                .knowledge_bases
                .get(name)
                .with_context(|| format!("Knowledge base '{}' not found in config", name))?;
            vec![(name, kb)]
        } else {
            config
                .knowledge_bases
                .iter()
                .map(|(n, kb)| (n.as_str(), kb))
                .collect()
        };

        let mut all: Vec<FtsResult> = Vec::new();
        for (name, _kb) in &kbs {
            let conn = db::open_db(name, &config.config_dir)?;
            let results = search_fts(&conn, search_query, limit)?;
            all.extend(results);
        }
        // Sort by score (rank from FTS5 is negative — lower is better, but we've negated it)
        all.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        all.truncate(limit);
        all
    } else {
        Vec::new()
    };

    // Local fuzzy results (nucleo — only for --local mode now)
    let local_results: Vec<LocalSearchResult> = if run_local {
        run_local_search(config, query, limit, exact)?
    } else {
        Vec::new()
    };

    // Graph search results
    let graph_results: Vec<crate::graph::GraphSearchResult> = if run_graph {
        let kbs: Vec<(&str, &crate::config::KnowledgeBaseConfig)> = if let Some(name) = kb_name {
            let kb = config
                .knowledge_bases
                .get(name)
                .with_context(|| format!("Knowledge base '{}' not found in config", name))?;
            vec![(name, kb)]
        } else {
            config
                .knowledge_bases
                .iter()
                .map(|(n, kb)| (n.as_str(), kb))
                .collect()
        };

        // For fuzzy mode: search graph with both original and corrected query terms
        let graph_query = if mode == SearchMode::Fuzzy && !query_corrections.is_empty() {
            // Combine original + corrected unique terms
            let combined: Vec<String> = query
                .split_whitespace()
                .chain(search_query.split_whitespace())
                .map(|s| s.to_string())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            combined.join(" ")
        } else {
            search_query.to_string()
        };

        let mut all_graph: Vec<crate::graph::GraphSearchResult> = Vec::new();
        for (name, _kb) in &kbs {
            if !crate::graph::KnowledgeGraph::exists(&config.config_dir, name) {
                continue;
            }
            match crate::graph::KnowledgeGraph::open(&config.config_dir, name) {
                Ok(kg) => match kg.search(&graph_query, limit) {
                    Ok(results) => all_graph.extend(results),
                    Err(e) => eprintln!("Graph search error in KB {}: {}", name, e),
                },
                Err(e) => eprintln!("Could not open graph DB for KB {}: {}", name, e),
            }
        }
        all_graph
    } else {
        Vec::new()
    };

    if json {
        let unified = build_unified_results(&fts_results, &local_results, &graph_results, limit);
        let mut output = serde_json::json!({ "results": unified });
        if !query_corrections.is_empty() {
            let corrections_json: Vec<serde_json::Value> = query_corrections
                .iter()
                .map(|(from, to)| serde_json::json!({ "from": from, "to": to }))
                .collect();
            output["corrections"] = serde_json::Value::Array(corrections_json);
        }
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print_results(
            query,
            search_query,
            &query_corrections,
            &fts_results,
            &local_results,
            &graph_results,
            mode,
            limit,
        );
    }

    Ok(())
}

/// FTS5 BM25 search using the documents_fts virtual table.
pub fn search_fts(conn: &Connection, query: &str, limit: usize) -> Result<Vec<FtsResult>> {
    let mut stmt = conn.prepare(
        r#"SELECT d.path,
                  snippet(documents_fts, 1, '', '', '...', 32) AS excerpt,
                  rank AS score
           FROM documents_fts
           JOIN documents d ON d.id = documents_fts.rowid
           WHERE documents_fts MATCH ?1
           ORDER BY rank
           LIMIT ?2"#,
    )?;

    let rows = stmt.query_map(rusqlite::params![query, limit as i64], |row| {
        Ok(FtsResult {
            path: row.get(0)?,
            excerpt: row.get(1)?,
            // FTS5 rank is negative (lower = better match). Negate to get a
            // positive score where higher is better.
            score: -row.get::<_, f64>(2)?,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Reciprocal Rank Fusion over multiple ranked result sets.
/// Each set is Vec<(doc_id, score)>. Returns merged Vec<(doc_id, rrf_score)>.
pub fn reciprocal_rank_fusion(
    result_sets: Vec<Vec<(String, f64)>>,
    k: f64,
) -> Vec<(String, f64)> {
    let mut scores: HashMap<String, f64> = HashMap::new();

    for result_set in &result_sets {
        // Sort descending by score
        let mut ranked: Vec<&(String, f64)> = result_set.iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        for (rank, (doc_id, _score)) in ranked.iter().enumerate() {
            let rrf_score = 1.0 / (k + rank as f64 + 1.0);
            *scores.entry(doc_id.clone()).or_insert(0.0) += rrf_score;
        }
    }

    let mut merged: Vec<(String, f64)> = scores.into_iter().collect();
    merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    merged
}

fn build_unified_results(
    fts: &[FtsResult],
    local: &[LocalSearchResult],
    graph: &[crate::graph::GraphSearchResult],
    limit: usize,
) -> Vec<UnifiedResult> {
    // Build ranked lists for RRF
    let fts_ranked: Vec<(String, f64)> =
        fts.iter().map(|r| (r.path.clone(), r.score)).collect();
    let local_ranked: Vec<(String, f64)> =
        local.iter().map(|r| (r.file.clone(), r.score)).collect();
    let graph_ranked: Vec<(String, f64)> =
        graph.iter().map(|r| (r.file.clone(), r.score)).collect();

    let merged = reciprocal_rank_fusion(vec![fts_ranked, local_ranked, graph_ranked], 60.0);

    // Build lookup maps for excerpts/lines/graph info
    let fts_map: HashMap<&str, &FtsResult> =
        fts.iter().map(|r| (r.path.as_str(), r)).collect();
    let local_map: HashMap<&str, &LocalSearchResult> =
        local.iter().map(|r| (r.file.as_str(), r)).collect();
    let graph_map: HashMap<&str, &crate::graph::GraphSearchResult> =
        graph.iter().map(|r| (r.file.as_str(), r)).collect();

    merged
        .into_iter()
        .take(limit)
        .map(|(file, score)| {
            let mut sources = Vec::new();
            let mut excerpt = String::new();
            let mut line = None;

            if let Some(f) = fts_map.get(file.as_str()) {
                sources.push("fts".to_string());
                excerpt = f.excerpt.clone();
            }
            if let Some(l) = local_map.get(file.as_str()) {
                sources.push("fuzzy".to_string());
                if excerpt.is_empty() {
                    excerpt = l.matched_text.clone();
                }
                line = Some(l.line);
            }
            if graph_map.contains_key(file.as_str()) {
                sources.push("graph".to_string());
            }

            UnifiedResult {
                file,
                score,
                sources,
                excerpt,
                line,
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn print_results(
    query: &str,
    effective_query: &str,
    corrections: &[(String, String)],
    fts: &[FtsResult],
    local: &[LocalSearchResult],
    graph: &[crate::graph::GraphSearchResult],
    mode: SearchMode,
    limit: usize,
) {
    let has_fts = !fts.is_empty();
    let has_local = !local.is_empty();
    let has_graph = !graph.is_empty();

    if !has_fts && !has_local && !has_graph {
        println!("{}", "🔍 No results found".yellow());
        return;
    }

    println!(
        "\n{} {}\n",
        "🔍 Results for".cyan().bold(),
        format!("\"{}\"", query).white().bold(),
    );

    // Show corrections if any were made
    if !corrections.is_empty() {
        let correction_strs: Vec<String> = corrections
            .iter()
            .map(|(from, to)| format!("{} → {}", from.yellow(), to.green()))
            .collect();
        println!(
            "  {} corrected: {}\n",
            "✎".cyan(),
            correction_strs.join(", ")
        );
        if effective_query != query {
            println!(
                "  {} searching for: {}\n",
                "→".dimmed(),
                format!("\"{}\"", effective_query).white()
            );
        }
    }

    if matches!(mode, SearchMode::All | SearchMode::Graph | SearchMode::Fuzzy) {
        // Show merged RRF results (or pure graph results)
        let unified = build_unified_results(fts, local, graph, limit);
        println!("{}", "── Merged results ────────────────────────────────".dimmed());
        for (i, result) in unified.iter().enumerate() {
            let sources = result.sources.join(", ");
            println!(
                "  {}. {} {} {}",
                (i + 1).to_string().bold(),
                format!("[{:.4}]", result.score).green(),
                result.file.cyan().bold(),
                format!("({})", sources).dimmed(),
            );
            if !result.excerpt.is_empty() {
                let excerpt = result.excerpt.replace('\n', " ");
                if let Some(ln) = result.line {
                    println!(
                        "     {}:{} {}",
                        result.file.dimmed(),
                        ln,
                        format!("...{}...", excerpt).dimmed()
                    );
                } else {
                    println!("     {}", format!("...{}...", excerpt).dimmed());
                }
            }
            println!();
        }
        return;
    }

    if matches!(mode, SearchMode::Text) {
        println!(
            "{}",
            "── FTS5 (text search) ─────────────────────────────".dimmed()
        );
        if has_fts {
            for (i, r) in fts.iter().enumerate() {
                println!(
                    "  {}. {} {}",
                    (i + 1).to_string().bold(),
                    format!("[{:.4}]", r.score).green(),
                    r.path.cyan().bold(),
                );
                let excerpt = r.excerpt.replace('\n', " ");
                println!("     {}", format!("...{}...", excerpt).dimmed());
                println!();
            }
        } else {
            println!("  {}\n", "No text results".dimmed());
        }
    }

    if matches!(mode, SearchMode::Local) {
        println!(
            "{}",
            "── Local (fuzzy) ─────────────────────────────────".dimmed()
        );
        if has_local {
            for (i, r) in local.iter().enumerate() {
                println!(
                    "  {}. {} {}{}",
                    (i + 1).to_string().bold(),
                    format!("[{:.2}]", r.score).green(),
                    r.file.cyan().bold(),
                    format!(":{}", r.line).dimmed(),
                );
                println!("     {}", r.matched_text.dimmed());
                println!();
            }
        } else {
            println!("  {}\n", "No local results".dimmed());
        }
    }
}

/// Public accessor used by mcp.rs.
pub fn search_fts_for_kb(
    config: &Config,
    kb_name: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<FtsResult>> {
    let conn = db::open_db(kb_name, &config.config_dir)?;
    search_fts(&conn, query, limit)
}
