use anyhow::{Context, Result};
use colored::Colorize;
use rusqlite::Connection;
use std::collections::HashMap;

use crate::config::Config;
use crate::db;
use crate::embed::Embedder;
use zerocopy::IntoBytes;
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
    /// Run FTS + graph + vector (if configured), merge with RRF (fast)
    All,
    /// Run FTS + graph + fuzzy + vector, merge with RRF (slower, more comprehensive)
    Fuzzy,
    /// Local fuzzy only (nucleo)
    Local,
    /// FTS5 BM25 only
    Text,
    /// Graph traversal from matching entities
    Graph,
    /// Vector KNN similarity search only
    Vector,
}

/// A vector KNN search result.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VectorResult {
    pub path: String,
    pub score: f64,
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
    let db_dir = config.effective_db_dir();
    let run_fts = matches!(mode, SearchMode::All | SearchMode::Text | SearchMode::Fuzzy);
    let run_local = matches!(mode, SearchMode::Local);
    let run_graph = matches!(mode, SearchMode::All | SearchMode::Graph | SearchMode::Fuzzy);
    let run_vector = matches!(mode, SearchMode::All | SearchMode::Vector | SearchMode::Fuzzy);

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
                .map(|(n, kb): (&String, _)| (n.as_str(), kb))
                .collect()
        };

        if let Some((first_kb_name, _)) = kbs.first() {
            let conn = db::open_db(first_kb_name, &db_dir)?;
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
                .map(|(n, kb): (&String, _)| (n.as_str(), kb))
                .collect()
        };

        let mut all: Vec<FtsResult> = Vec::new();
        for (name, _kb) in &kbs {
            let conn = db::open_db(name, &db_dir)?;
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
                .map(|(n, kb): (&String, _)| (n.as_str(), kb))
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
            if !crate::graph::KnowledgeGraph::exists(&db_dir, name) {
                continue;
            }
            match crate::graph::KnowledgeGraph::open(&db_dir, name) {
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

    // Vector KNN search
    let vector_results: Vec<VectorResult> = if run_vector {
        if let Some(embed_cfg) = &config.embeddings {
            let api_key = config.resolve_api_key(&embed_cfg.provider, embed_cfg.api_key.as_deref());
            let base_url = config.resolve_base_url(&embed_cfg.provider, embed_cfg.base_url.as_deref());
            let embedder = Embedder::new(embed_cfg, api_key, base_url);
            // Determine task type based on KB file contents
            let all_paths: Vec<String> = if let Some(name) = kb_name {
                let conn = crate::db::open_db(name, &config.effective_db_dir()).ok();
                conn.and_then(|c| crate::db::get_all_paths(&c).ok()).unwrap_or_default()
            } else {
                Vec::new()
            };
            let query_task = crate::embed::task_type_for_query(&all_paths);
            match embedder.embed_batch_with_task(&[search_query], query_task).await {
                Ok(vecs) if !vecs.is_empty() => {
                    let query_vec = &vecs[0];
                    let kbs: Vec<(&str, &crate::config::KnowledgeBaseConfig)> = if let Some(name) = kb_name {
                        let kb = config.knowledge_bases.get(name)
                            .with_context(|| format!("KB '{}' not found", name))?;
                        vec![(name, kb)]
                    } else {
                        config.knowledge_bases.iter().map(|(n, kb): (&String, _)| (n.as_str(), kb)).collect()
                    };
                    let mut all_vec: Vec<VectorResult> = Vec::new();
                    for (name, _kb) in &kbs {
                        let conn = db::open_db(name, &db_dir)?;
                        match search_vector(&conn, query_vec, limit) {
                            Ok(results) => all_vec.extend(results),
                            Err(e) => eprintln!("Vector search error in KB {}: {}", name, e),
                        }
                    }
                    all_vec
                }
                Ok(_) => Vec::new(),
                Err(e) => {
                    eprintln!("Embedding query failed: {}", e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    if json {
        let unified = build_unified_results(&fts_results, &local_results, &graph_results, &vector_results, limit);
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
            &vector_results,
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

/// Vector KNN search using sqlite-vec documents_vec table.
pub fn search_vector(
    conn: &Connection,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<VectorResult>> {
    if !db::vec_table_exists(conn) {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        r#"SELECT dv.document_id, dv.distance, d.path
           FROM documents_vec dv
           JOIN documents d ON d.id = dv.document_id
           WHERE dv.embedding MATCH ?1 AND k = ?2
           ORDER BY dv.distance"#,
    )?;

    let rows = stmt.query_map(
        rusqlite::params![query_embedding.as_bytes(), limit as i64],
        |row| {
            let distance: f64 = row.get(1)?;
            Ok(VectorResult {
                path: row.get(2)?,
                // Convert distance to similarity score (lower distance = better)
                score: 1.0 / (1.0 + distance),
            })
        },
    )?;

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
    vector: &[VectorResult],
    limit: usize,
) -> Vec<UnifiedResult> {
    // Build ranked lists for RRF
    let fts_ranked: Vec<(String, f64)> =
        fts.iter().map(|r| (r.path.clone(), r.score)).collect();
    let local_ranked: Vec<(String, f64)> =
        local.iter().map(|r| (r.file.clone(), r.score)).collect();
    let graph_ranked: Vec<(String, f64)> =
        graph.iter().map(|r| (r.file.clone(), r.score)).collect();
    let vector_ranked: Vec<(String, f64)> =
        vector.iter().map(|r| (r.path.clone(), r.score)).collect();

    let merged = reciprocal_rank_fusion(vec![fts_ranked, local_ranked, graph_ranked, vector_ranked], 60.0);

    // Build lookup maps for excerpts/lines/graph info
    let fts_map: HashMap<&str, &FtsResult> =
        fts.iter().map(|r| (r.path.as_str(), r)).collect();
    let local_map: HashMap<&str, &LocalSearchResult> =
        local.iter().map(|r| (r.file.as_str(), r)).collect();
    let graph_map: HashMap<&str, &crate::graph::GraphSearchResult> =
        graph.iter().map(|r| (r.file.as_str(), r)).collect();
    let vector_set: std::collections::HashSet<&str> =
        vector.iter().map(|r| r.path.as_str()).collect();

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
            if vector_set.contains(file.as_str()) {
                sources.push("vector".to_string());
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
    vector: &[VectorResult],
    mode: SearchMode,
    limit: usize,
) {
    let has_fts = !fts.is_empty();
    let has_local = !local.is_empty();
    let has_graph = !graph.is_empty();
    let has_vector = !vector.is_empty();

    if !has_fts && !has_local && !has_graph && !has_vector {
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
        let unified = build_unified_results(fts, local, graph, vector, limit);
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
    let db_dir = config.effective_db_dir();
    let conn = db::open_db(kb_name, &db_dir)?;
    search_fts(&conn, query, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal in-memory connection with brainjar schema + some documents.
    fn make_conn_with_docs(docs: &[(&str, &str)]) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS documents (
                id           INTEGER PRIMARY KEY,
                path         TEXT UNIQUE NOT NULL,
                content      TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                extracted    INTEGER NOT NULL DEFAULT 0,
                updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
                path,
                content,
                content='documents',
                content_rowid='id'
            );
            CREATE TRIGGER IF NOT EXISTS documents_ai AFTER INSERT ON documents BEGIN
                INSERT INTO documents_fts(rowid, path, content)
                VALUES (new.id, new.path, new.content);
            END;
        "#).unwrap();
        for (path, content) in docs {
            db::upsert_document(&conn, path, content, "hash").unwrap();
        }
        conn
    }

    // ─── SearchMode enum ─────────────────────────────────────────────────────

    #[test]
    fn test_search_mode_equality() {
        assert_eq!(SearchMode::All, SearchMode::All);
        assert_ne!(SearchMode::Text, SearchMode::Fuzzy);
        assert_ne!(SearchMode::Vector, SearchMode::Graph);
        assert_ne!(SearchMode::Local, SearchMode::All);
    }

    // ─── FTS query ───────────────────────────────────────────────────────────

    #[test]
    fn test_search_fts_basic_hit() {
        let conn = make_conn_with_docs(&[
            ("notes/rust.md", "Rust is a systems programming language focused on safety."),
            ("notes/python.md", "Python is great for scripting and data science."),
        ]);
        let results = search_fts(&conn, "Rust", 10).unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.path.contains("rust")));
    }

    #[test]
    fn test_search_fts_no_results_for_missing_term() {
        let conn = make_conn_with_docs(&[
            ("notes/rust.md", "Rust is a systems programming language."),
        ]);
        let results = search_fts(&conn, "python", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_fts_respects_limit() {
        let docs: Vec<(String, String)> = (0..10)
            .map(|i| (format!("doc{i}.md"), format!("brainjar document number {i}")))
            .collect();
        let docs_ref: Vec<(&str, &str)> = docs.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
        let conn = make_conn_with_docs(&docs_ref);
        let results = search_fts(&conn, "brainjar", 3).unwrap();
        assert!(results.len() <= 3);
    }

    #[test]
    fn test_search_fts_score_positive() {
        let conn = make_conn_with_docs(&[
            ("doc.md", "sqlite full text search is powerful"),
        ]);
        let results = search_fts(&conn, "sqlite", 5).unwrap();
        assert!(!results.is_empty());
        // We negate FTS5 rank so score should be positive
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn test_search_fts_empty_table() {
        let conn = make_conn_with_docs(&[]);
        let results = search_fts(&conn, "anything", 10).unwrap();
        assert!(results.is_empty());
    }

    // ─── search_vector ──────────────────────────────────────────────────────

    #[test]
    fn test_search_vector_no_table_returns_empty() {
        let conn = make_conn_with_docs(&[
            ("doc.md", "some content"),
        ]);
        // No vec table — should return empty, not error
        let results = search_vector(&conn, &[0.1, 0.2, 0.3], 5).unwrap();
        assert!(results.is_empty());
    }

    // ─── reciprocal_rank_fusion ─────────────────────────────────────────────

    #[test]
    fn test_rrf_single_set() {
        let set = vec![
            ("doc_a".to_string(), 10.0),
            ("doc_b".to_string(), 5.0),
            ("doc_c".to_string(), 1.0),
        ];
        let merged = reciprocal_rank_fusion(vec![set], 60.0);
        // doc_a is rank 1 (highest score) → RRF score = 1/(60+1+1) = 1/61
        // doc_b is rank 2 → 1/62, doc_c is rank 3 → 1/63
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].0, "doc_a");
        assert_eq!(merged[1].0, "doc_b");
        assert_eq!(merged[2].0, "doc_c");
    }

    #[test]
    fn test_rrf_math_correctness() {
        // Exact score verification
        let set = vec![
            ("a".to_string(), 100.0),
            ("b".to_string(), 50.0),
        ];
        let merged = reciprocal_rank_fusion(vec![set], 60.0);
        // a is rank 0 → 1/(60+0+1) = 1/61 ≈ 0.01639
        let expected_a = 1.0 / (60.0 + 0.0 + 1.0);
        let expected_b = 1.0 / (60.0 + 1.0 + 1.0);
        let score_a = merged.iter().find(|(k, _)| k == "a").unwrap().1;
        let score_b = merged.iter().find(|(k, _)| k == "b").unwrap().1;
        assert!((score_a - expected_a).abs() < 1e-9);
        assert!((score_b - expected_b).abs() < 1e-9);
    }

    #[test]
    fn test_rrf_two_sets_merged_higher_for_overlap() {
        // doc_x appears in both sets at rank 1 — should have higher combined score
        let set_a = vec![
            ("doc_x".to_string(), 10.0),
            ("doc_y".to_string(), 5.0),
        ];
        let set_b = vec![
            ("doc_x".to_string(), 8.0),
            ("doc_z".to_string(), 3.0),
        ];
        let merged = reciprocal_rank_fusion(vec![set_a, set_b], 60.0);
        let score_x = merged.iter().find(|(k, _)| k == "doc_x").unwrap().1;
        let score_y = merged.iter().find(|(k, _)| k == "doc_y").unwrap().1;
        // doc_x appears twice so its score should be roughly 2x a single appearance
        assert!(score_x > score_y);
    }

    #[test]
    fn test_rrf_empty_sets() {
        let merged = reciprocal_rank_fusion(vec![], 60.0);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_rrf_empty_inner_set() {
        let merged = reciprocal_rank_fusion(vec![vec![]], 60.0);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_rrf_result_sorted_descending() {
        let set = vec![
            ("low".to_string(), 1.0),
            ("high".to_string(), 100.0),
            ("mid".to_string(), 50.0),
        ];
        let merged = reciprocal_rank_fusion(vec![set], 60.0);
        // Results should be sorted highest score first
        for i in 0..merged.len() - 1 {
            assert!(merged[i].1 >= merged[i + 1].1);
        }
    }
}
