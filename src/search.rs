use anyhow::{Context, Result};
use colored::Colorize;
use rusqlite::Connection;
use std::collections::HashMap;
use std::collections::HashSet;

use crate::config::Config;
use crate::db;
use crate::embed::Embedder;
use zerocopy::IntoBytes;

/// Sanitize a query string for FTS5 MATCH syntax.
/// Removes special characters that FTS5 interprets as operators.
fn sanitize_fts_query(query: &str) -> String {
    query
        .chars()
        .filter(|c| !matches!(c, '?' | '*' | '(' | ')' | '"' | '+' | '-' | '^' | '{' | '}' | '~'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
use crate::fuzzy;
use crate::local_search::{run_local_search, LocalSearchResult};

/// An FTS5 search result.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FtsResult {
    pub path: String,
    pub excerpt: String,
    pub score: f64,
    /// Chunk id (from chunks_fts), if available
    pub chunk_id: Option<i64>,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub chunk_type: Option<String>,
    pub content: Option<String>,
}

/// A unified search result (for JSON output).
#[derive(Debug, serde::Serialize)]
pub struct UnifiedResult {
    pub file: String,
    pub score: f64,
    pub sources: Vec<String>,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_start: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_type: Option<String>,
}

/// Individual search engines that can be combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchEngine {
    /// Fuzzy-corrected FTS5 BM25
    Fuzzy,
    /// Raw FTS5 BM25 (no fuzzy correction)
    Text,
    /// Graph entity traversal
    Graph,
    /// Vector KNN similarity
    Vector,
    /// Local nucleo file scanner
    Local,
}

/// Set of search engines to run. Default: Fuzzy + Graph + Vector.
#[derive(Debug, Clone)]
pub struct SearchMode {
    pub engines: std::collections::HashSet<SearchEngine>,
}

impl SearchMode {
    /// Default: fuzzy + graph + vector
    pub fn default_mode() -> Self {
        Self {
            engines: [SearchEngine::Fuzzy, SearchEngine::Graph, SearchEngine::Vector]
                .into_iter()
                .collect(),
        }
    }

    /// Build from explicit flags. If none set, use default.
    pub fn from_flags(text: bool, graph: bool, vector: bool, local: bool) -> Self {
        if local {
            return Self {
                engines: [SearchEngine::Local].into_iter().collect(),
            };
        }
        let mut engines = std::collections::HashSet::new();
        if text { engines.insert(SearchEngine::Text); }
        if graph { engines.insert(SearchEngine::Graph); }
        if vector { engines.insert(SearchEngine::Vector); }
        if engines.is_empty() {
            return Self::default_mode();
        }
        Self { engines }
    }

    pub fn has(&self, engine: SearchEngine) -> bool {
        self.engines.contains(&engine)
    }

    /// Whether to run fuzzy vocabulary correction before FTS
    pub fn run_fuzzy(&self) -> bool {
        self.has(SearchEngine::Fuzzy)
    }

    /// Whether to run any FTS (fuzzy or raw text)
    pub fn run_fts(&self) -> bool {
        self.has(SearchEngine::Fuzzy) || self.has(SearchEngine::Text)
    }

    pub fn run_graph(&self) -> bool {
        self.has(SearchEngine::Graph)
    }

    pub fn run_vector(&self) -> bool {
        self.has(SearchEngine::Vector)
    }

    pub fn run_local(&self) -> bool {
        self.has(SearchEngine::Local)
    }
}

/// A vector KNN search result.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VectorResult {
    pub path: String,
    pub score: f64,
    pub chunk_id: Option<i64>,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub chunk_type: Option<String>,
    pub content: Option<String>,
}

/// Use a cheap LLM to extract targeted search queries from conversational text.
/// Public alias for use by mcp.rs.
pub async fn extract_queries_pub(config: &Config, raw_text: &str) -> Result<Vec<String>> {
    extract_queries(config, raw_text).await
}

async fn extract_queries(config: &Config, raw_text: &str) -> Result<Vec<String>> {
    let ext_config = config.extraction.as_ref()
        .context("Smart search requires [extraction] config for LLM query extraction")?;

    let api_key = config.resolve_api_key(&ext_config.provider, ext_config.api_key.as_deref())
        .context("No API key for extraction provider")?;

    let prompt = format!(
        "You are a search query extractor. Given conversational text, extract 2-5 short, specific search queries that would find relevant documents in a knowledge base.\n\nRules:\n- Return a JSON array of strings, e.g. [\"query one\", \"query two\"]\n- Each query should be 1-5 words\n- Extract key concepts, entities, and topics\n- Do NOT return the original text as a query\n\nText: {}\n\nJSON array:",
        raw_text
    );

    let client = reqwest::Client::new();

    let result = match ext_config.provider.as_str() {
        "gemini" => {
            let url = format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
                ext_config.model, api_key
            );
            let body = serde_json::json!({
                "contents": [{"parts": [{"text": prompt}]}],
                "generationConfig": {"responseMimeType": "application/json"}
            });
            let resp = client.post(&url).json(&body).send().await
                .context("Smart search: LLM request failed")?;
            let json: serde_json::Value = resp.json().await?;
            json["candidates"][0]["content"]["parts"][0]["text"]
                .as_str()
                .unwrap_or("[]")
                .to_string()
        }
        "openai" => {
            let url = "https://api.openai.com/v1/chat/completions";
            let body = serde_json::json!({
                "model": ext_config.model,
                "messages": [{"role": "user", "content": prompt}],
                "response_format": {"type": "json_object"}
            });
            let resp = client.post(url)
                .header("Authorization", format!("Bearer {}", api_key))
                .json(&body).send().await?;
            let status = resp.status();
            let json: serde_json::Value = resp.json().await?;
            if !status.is_success() {
                let err_msg = json["error"]["message"].as_str().unwrap_or("unknown error");
                anyhow::bail!("Smart search: OpenAI API error ({}): {}", status, err_msg);
            }
            json["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("[]")
                .to_string()
        }
        "ollama" => {
            let base_url = config.resolve_base_url(&ext_config.provider, ext_config.base_url.as_deref())
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            let url = format!("{}/api/generate", base_url);
            let body = serde_json::json!({
                "model": ext_config.model,
                "prompt": prompt,
                "stream": false,
                "format": "json"
            });
            let resp = client.post(&url).json(&body).send().await?;
            let json: serde_json::Value = resp.json().await?;
            json["response"].as_str().unwrap_or("[]").to_string()
        }
        p => anyhow::bail!("Unknown extraction provider for smart search: {}", p),
    };

    // Parse the JSON array of queries
    let queries: Vec<String> = match serde_json::from_str::<Vec<String>>(&result) {
        Ok(q) => q,
        Err(_) => {
            // Try to extract array from wrapper object (LLMs sometimes return {"queries": [...]})
            if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&result) {
                if let Some(arr) = obj.as_object().and_then(|o| o.values().next()).and_then(|v| v.as_array()) {
                    arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()
                } else {
                    vec![raw_text.to_string()] // fallback: use raw text
                }
            } else {
                vec![raw_text.to_string()]
            }
        }
    };

    // Limit to 5 queries max
    Ok(queries.into_iter().take(5).collect())
}

#[allow(clippy::too_many_arguments)]
pub async fn run_search(
    config: &Config,
    query: &str,
    kb_name: Option<&str>,
    limit: usize,
    json: bool,
    mode: SearchMode,
    exact: bool,
    chunks: bool,
    doc_score: bool,
    smart: bool,
) -> Result<()> {
    // Smart mode: use LLM to extract targeted search queries from conversational text
    if smart {
        let queries = extract_queries(config, query).await?;
        if !json {
            eprintln!(
                "{} Extracted {} quer{}: {}",
                "🧠".dimmed(),
                queries.len(),
                if queries.len() == 1 { "y" } else { "ies" },
                queries.iter().map(|q| format!("\"{}\"", q)).collect::<Vec<_>>().join(", ")
            );
        }

        // Fan-out: run search for each extracted query, collect and merge results
        let mut all_fts: Vec<FtsResult> = Vec::new();
        let mut all_local: Vec<crate::local_search::LocalSearchResult> = Vec::new();
        let mut all_graph: Vec<crate::graph::GraphSearchResult> = Vec::new();
        let mut all_vector: Vec<VectorResult> = Vec::new();

        for sub_query in &queries {
            let (fts, local, graph, vector) =
                collect_search_results(config, sub_query, kb_name, limit, &mode, exact).await?;
            all_fts.extend(fts);
            all_local.extend(local);
            all_graph.extend(graph);
            all_vector.extend(vector);
        }

        // Deduplicate by chunk_id (keeping highest score) or by path for path-keyed results
        all_fts = dedup_fts_results(all_fts);
        all_local = dedup_local_results(all_local);
        all_graph = dedup_graph_results(all_graph);
        all_vector = dedup_vector_results(all_vector);

        if json {
            let unified = build_unified_results(&all_fts, &all_local, &all_graph, &all_vector, limit, chunks, doc_score);
            let output = serde_json::json!({ "results": unified, "smart_queries": queries });
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            print_results(
                query,
                query,
                &[],
                &all_fts,
                &all_local,
                &all_graph,
                &all_vector,
                &mode,
                limit,
                chunks,
                doc_score,
            );
        }
        return Ok(());
    }
    let db_dir = config.effective_db_dir();
    let run_fts = mode.run_fts();
    let run_local = mode.run_local();
    let run_graph = mode.run_graph();
    let run_vector = mode.run_vector();

    // Fuzzy mode: correct the query via vocabulary before FTS/graph search
    let (effective_query, query_corrections) = if mode.run_fuzzy() {
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

        // When fuzzy is active: search graph with both original and corrected query terms
        let graph_query = if mode.run_fuzzy() && !query_corrections.is_empty() {
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
        let unified = build_unified_results(&fts_results, &local_results, &graph_results, &vector_results, limit, chunks, doc_score);
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
            &mode,
            limit,
            chunks,
            doc_score,
        );
    }

    Ok(())
}

/// Core search logic — collects raw results without printing or merging.
/// Used by both normal mode and smart fan-out mode.
#[allow(clippy::too_many_arguments)]
async fn collect_search_results(
    config: &Config,
    query: &str,
    kb_name: Option<&str>,
    limit: usize,
    mode: &SearchMode,
    exact: bool,
) -> Result<(
    Vec<FtsResult>,
    Vec<crate::local_search::LocalSearchResult>,
    Vec<crate::graph::GraphSearchResult>,
    Vec<VectorResult>,
)> {
    let db_dir = config.effective_db_dir();
    let run_fts = mode.run_fts();
    let run_local = mode.run_local();
    let run_graph = mode.run_graph();
    let run_vector = mode.run_vector();

    let search_query = query;

    // FTS results
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
        all.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        all.truncate(limit);
        all
    } else {
        Vec::new()
    };

    // Local fuzzy results
    let local_results: Vec<crate::local_search::LocalSearchResult> = if run_local {
        crate::local_search::run_local_search(config, query, limit, exact)?
    } else {
        Vec::new()
    };

    // Graph results
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
        let mut all_graph: Vec<crate::graph::GraphSearchResult> = Vec::new();
        for (name, _kb) in &kbs {
            if !crate::graph::KnowledgeGraph::exists(&db_dir, name) {
                continue;
            }
            match crate::graph::KnowledgeGraph::open(&db_dir, name) {
                Ok(kg) => match kg.search(search_query, limit) {
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
            let embedder = crate::embed::Embedder::new(embed_cfg, api_key, base_url);
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

    Ok((fts_results, local_results, graph_results, vector_results))
}

/// Deduplicate FTS results by chunk_id (or path if no chunk_id), keeping highest score.
fn dedup_fts_results(mut results: Vec<FtsResult>) -> Vec<FtsResult> {
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let mut seen_chunks: HashSet<i64> = HashSet::new();
    let mut seen_paths: HashSet<String> = HashSet::new();
    results.retain(|r| {
        if let Some(id) = r.chunk_id {
            seen_chunks.insert(id)
        } else {
            seen_paths.insert(r.path.clone())
        }
    });
    results
}

/// Deduplicate local search results by file path, keeping highest score.
fn dedup_local_results(mut results: Vec<crate::local_search::LocalSearchResult>) -> Vec<crate::local_search::LocalSearchResult> {
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let mut seen: HashSet<String> = HashSet::new();
    results.retain(|r| seen.insert(r.file.clone()));
    results
}

/// Deduplicate graph results by file path, keeping highest score.
fn dedup_graph_results(mut results: Vec<crate::graph::GraphSearchResult>) -> Vec<crate::graph::GraphSearchResult> {
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let mut seen: HashSet<String> = HashSet::new();
    results.retain(|r| seen.insert(r.file.clone()));
    results
}

/// Deduplicate vector results by chunk_id (or path if no chunk_id), keeping highest score.
fn dedup_vector_results(mut results: Vec<VectorResult>) -> Vec<VectorResult> {
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let mut seen_chunks: HashSet<i64> = HashSet::new();
    let mut seen_paths: HashSet<String> = HashSet::new();
    results.retain(|r| {
        if let Some(id) = r.chunk_id {
            seen_chunks.insert(id)
        } else {
            seen_paths.insert(r.path.clone())
        }
    });
    results
}

/// FTS5 BM25 search — queries `chunks_fts` if available, falls back to `documents_fts`.
pub fn search_fts(conn: &Connection, query: &str, limit: usize) -> Result<Vec<FtsResult>> {
    let query = &sanitize_fts_query(query);
    if query.is_empty() {
        return Ok(vec![]);
    }
    // Check if chunks_fts exists
    let has_chunks_fts: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chunks_fts'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    if has_chunks_fts {
        // Query chunks_fts, join with chunks + documents
        let mut stmt = conn.prepare(
            "SELECT d.path,
                    snippet(chunks_fts, 0, '', '', '...', 32) AS excerpt,
                    -bm25(chunks_fts) AS score,
                    c.id AS chunk_id,
                    c.line_start,
                    c.line_end,
                    COALESCE(c.chunk_type, '') AS chunk_type,
                    c.content
             FROM chunks_fts
             JOIN chunks c ON c.id = chunks_fts.rowid
             JOIN documents d ON d.id = c.doc_id
             WHERE chunks_fts MATCH ?1
             ORDER BY score DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![query, limit as i64], |row| {
            Ok(FtsResult {
                path: row.get(0)?,
                excerpt: row.get(1)?,
                score: row.get(2)?,
                chunk_id: Some(row.get(3)?),
                line_start: Some(row.get::<_, i64>(4)? as u32),
                line_end: Some(row.get::<_, i64>(5)? as u32),
                chunk_type: Some(row.get(6)?),
                content: Some(row.get(7)?),
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        // If chunks_fts is empty, fall back to documents_fts (legacy compatibility)
        if results.is_empty() {
            let mut fallback_stmt = conn.prepare(
                "SELECT d.path,
                        snippet(documents_fts, 1, '', '', '...', 32) AS excerpt,
                        rank AS score
                 FROM documents_fts
                 JOIN documents d ON d.id = documents_fts.rowid
                 WHERE documents_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )?;
            let fallback_rows = fallback_stmt.query_map(rusqlite::params![query, limit as i64], |row| {
                Ok(FtsResult {
                    path: row.get(0)?,
                    excerpt: row.get(1)?,
                    score: -row.get::<_, f64>(2)?,
                    chunk_id: None,
                    line_start: None,
                    line_end: None,
                    chunk_type: None,
                    content: None,
                })
            })?;
            for row in fallback_rows {
                results.push(row?);
            }
        }
        Ok(results)
    } else {
        // Legacy fallback: documents_fts
        let mut stmt = conn.prepare(
            "SELECT d.path,
                    snippet(documents_fts, 1, '', '', '...', 32) AS excerpt,
                    rank AS score
             FROM documents_fts
             JOIN documents d ON d.id = documents_fts.rowid
             WHERE documents_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![query, limit as i64], |row| {
            Ok(FtsResult {
                path: row.get(0)?,
                excerpt: row.get(1)?,
                score: -row.get::<_, f64>(2)?,
                chunk_id: None,
                line_start: None,
                line_end: None,
                chunk_type: None,
                content: None,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }
}

/// Vector KNN search — queries `chunks_vec` if available, falls back to `documents_vec`.
pub fn search_vector(
    conn: &Connection,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<VectorResult>> {
    if db::chunks_vec_table_exists(conn) {
        let mut stmt = conn.prepare(
            "SELECT cv.chunk_id, cv.distance, d.path, c.line_start, c.line_end, COALESCE(c.chunk_type, ''), c.content
             FROM chunks_vec cv
             JOIN chunks c ON c.id = cv.chunk_id
             JOIN documents d ON d.id = c.doc_id
             WHERE cv.embedding MATCH ?1 AND k = ?2
             ORDER BY cv.distance",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![query_embedding.as_bytes(), limit as i64],
            |row| {
                let distance: f64 = row.get(1)?;
                Ok(VectorResult {
                    path: row.get(2)?,
                    score: 1.0 / (1.0 + distance),
                    chunk_id: Some(row.get(0)?),
                    line_start: Some(row.get::<_, i64>(3)? as u32),
                    line_end: Some(row.get::<_, i64>(4)? as u32),
                    chunk_type: Some(row.get(5)?),
                    content: row.get(6)?,
                })
            },
        )?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    } else if db::vec_table_exists(conn) {
        // Legacy fallback: documents_vec
        let mut stmt = conn.prepare(
            "SELECT dv.document_id, dv.distance, d.path
             FROM documents_vec dv
             JOIN documents d ON d.id = dv.document_id
             WHERE dv.embedding MATCH ?1 AND k = ?2
             ORDER BY dv.distance",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![query_embedding.as_bytes(), limit as i64],
            |row| {
                let distance: f64 = row.get(1)?;
                Ok(VectorResult {
                    path: row.get(2)?,
                    score: 1.0 / (1.0 + distance),
                    chunk_id: None,
                    line_start: None,
                    line_end: None,
                    chunk_type: None,
                    content: None,
                })
            },
        )?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    } else {
        Ok(Vec::new())
    }
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
    _include_content: bool, // deprecated: content always included now
    doc_score: bool,
) -> Vec<UnifiedResult> {
    // Key: use chunk-level identity (chunk_id or path) for dedup
    // We use chunk-keyed ranking: each chunk is its own ranked item
    // If doc_score: aggregate top-3 chunk scores per document

    // Build ranked lists for RRF using chunk_id where available, else path
    let fts_ranked: Vec<(String, f64)> = fts.iter().map(|r| {
        let key = r.chunk_id.map(|id| format!("chunk:{}", id))
            .unwrap_or_else(|| r.path.clone());
        (key, r.score)
    }).collect();
    let local_ranked: Vec<(String, f64)> =
        local.iter().map(|r| (r.file.clone(), r.score)).collect();
    let graph_ranked: Vec<(String, f64)> =
        graph.iter().map(|r| (r.file.clone(), r.score)).collect();
    let vector_ranked: Vec<(String, f64)> = vector.iter().map(|r| {
        let key = r.chunk_id.map(|id| format!("chunk:{}", id))
            .unwrap_or_else(|| r.path.clone());
        (key, r.score)
    }).collect();

    let merged = reciprocal_rank_fusion(vec![fts_ranked, local_ranked, graph_ranked, vector_ranked], 60.0);

    // Build lookup maps
    let fts_by_chunk: HashMap<i64, &FtsResult> =
        fts.iter().filter_map(|r| r.chunk_id.map(|id| (id, r))).collect();
    let fts_by_path: HashMap<&str, &FtsResult> =
        fts.iter().map(|r| (r.path.as_str(), r)).collect();
    let local_map: HashMap<&str, &LocalSearchResult> =
        local.iter().map(|r| (r.file.as_str(), r)).collect();
    let graph_map: HashMap<&str, &crate::graph::GraphSearchResult> =
        graph.iter().map(|r| (r.file.as_str(), r)).collect();
    let vector_by_chunk: HashMap<i64, &VectorResult> =
        vector.iter().filter_map(|r| r.chunk_id.map(|id| (id, r))).collect();
    let vector_by_path: HashMap<&str, &VectorResult> =
        vector.iter().map(|r| (r.path.as_str(), r)).collect();

    let mut results: Vec<UnifiedResult> = merged
        .into_iter()
        .map(|(key, score)| {
            let mut sources = Vec::new();
            let mut excerpt = String::new();
            let mut chunk_id_out: Option<i64> = None;
            let mut line_start: Option<u32> = None;
            let mut line_end: Option<u32> = None;
            let mut chunk_type: Option<String> = None;
            let file: String;

            if let Some(id_str) = key.strip_prefix("chunk:") {
                let chunk_id: i64 = id_str.parse().unwrap_or(0);
                chunk_id_out = Some(chunk_id);

                if let Some(f) = fts_by_chunk.get(&chunk_id) {
                    sources.push("fts".to_string());
                    excerpt = f.content.clone().unwrap_or_default();
                    file = f.path.clone();
                    line_start = f.line_start;
                    line_end = f.line_end;
                    chunk_type = f.chunk_type.clone();
                } else if let Some(v) = vector_by_chunk.get(&chunk_id) {
                    sources.push("vector".to_string());
                    file = v.path.clone();
                    line_start = v.line_start;
                    line_end = v.line_end;
                    chunk_type = v.chunk_type.clone();
                    excerpt = v.content.clone().unwrap_or_default();
                } else {
                    file = key.clone();
                }

                #[allow(clippy::collapsible_if)]
                if fts_by_chunk.contains_key(&chunk_id) {
                    if !sources.contains(&"fts".to_string()) {
                        sources.push("fts".to_string());
                    }
                }
                if vector_by_chunk.contains_key(&chunk_id) && !sources.contains(&"vector".to_string()) {
                    sources.push("vector".to_string());
                }
            } else {
                // Path-keyed result (local/graph or legacy)
                file = key.clone();
                if let Some(f) = fts_by_path.get(key.as_str()) {
                    sources.push("fts".to_string());
                    excerpt = f.content.clone().unwrap_or_default();
                }
                if let Some(v) = vector_by_path.get(key.as_str()) {
                    if !sources.contains(&"vector".to_string()) {
                        sources.push("vector".to_string());
                    }
                    if excerpt.is_empty() {
                        excerpt = v.content.clone().unwrap_or_default();
                    }
                }
            }

            if let Some(l) = local_map.get(file.as_str()) {
                if !sources.contains(&"fuzzy".to_string()) {
                    sources.push("fuzzy".to_string());
                }
                if excerpt.is_empty() {
                    excerpt = l.matched_text.clone();
                }
            }
            if graph_map.contains_key(file.as_str()) && !sources.contains(&"graph".to_string()) {
                sources.push("graph".to_string());
            }

            UnifiedResult {
                file,
                score,
                sources,
                content: excerpt.clone(),
                chunk_id: chunk_id_out,
                line_start,
                line_end,
                chunk_type,
            }
        })
        .collect();

    // Collapse path-keyed results into chunk-keyed results for the same file.
    // Graph search returns file paths as keys, vector returns chunk:{id} keys.
    // Without this, the same file can appear twice.
    {
        let mut chunk_files: HashMap<String, Vec<usize>> = HashMap::new();
        let mut path_only: HashMap<String, usize> = HashMap::new();

        for (i, r) in results.iter().enumerate() {
            if r.chunk_id.is_some() {
                chunk_files.entry(r.file.clone()).or_default().push(i);
            } else {
                path_only.insert(r.file.clone(), i);
            }
        }

        let mut indices_to_remove: Vec<usize> = Vec::new();
        for (file, path_idx) in &path_only {
            if let Some(chunk_indices) = chunk_files.get(file) {
                let extra_sources: Vec<String> = results[*path_idx].sources.clone();
                let extra_score = results[*path_idx].score;

                for &ci in chunk_indices {
                    for src in &extra_sources {
                        if !results[ci].sources.contains(src) {
                            results[ci].sources.push(src.clone());
                        }
                    }
                    results[ci].score += extra_score;
                }

                indices_to_remove.push(*path_idx);
            }
        }

        indices_to_remove.sort_unstable();
        for idx in indices_to_remove.into_iter().rev() {
            results.remove(idx);
        }

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    }

    if doc_score {
        // Aggregate: sum top-3 chunk scores per document, return one result per doc
        let mut doc_scores: HashMap<String, (f64, UnifiedResult)> = HashMap::new();
        let mut doc_chunk_counts: HashMap<String, usize> = HashMap::new();
        for result in results {
            let count = doc_chunk_counts.entry(result.file.clone()).or_insert(0);
            if *count < 3 {
                *count += 1;
                doc_scores
                    .entry(result.file.clone())
                    .and_modify(|(s, _)| *s += result.score)
                    .or_insert((result.score, result));
            }
        }
        let mut doc_results: Vec<UnifiedResult> = doc_scores
            .into_values()
            .map(|(agg_score, mut r)| {
                r.score = agg_score;
                r.chunk_id = None;
                r.line_start = None;
                r.line_end = None;
                r.chunk_type = None;
                r
            })
            .collect();
        doc_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        doc_results.truncate(limit);
        results = doc_results;
    } else {
        results.truncate(limit);
    }

    results
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
    mode: &SearchMode,
    limit: usize,
    include_content: bool,
    doc_score: bool,
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

    let single_text = mode.has(SearchEngine::Text) && mode.engines.len() == 1;
    let single_local = mode.run_local();
    if !single_text && !single_local {
        // Merged RRF view (default for any engine combination)
        let unified = build_unified_results(fts, local, graph, vector, limit, include_content, doc_score);
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
            if !result.content.is_empty() {
                let content_preview = result.content.chars().take(200).collect::<String>().replace('\n', " ");
                println!("     {}", format!("...{}...", content_preview).dimmed());
            }
            println!();
        }
        return;
    }

    if single_text {
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
                let content_preview = r.content.clone().unwrap_or_default().chars().take(200).collect::<String>().replace('\n', " ");
                println!("     {}", format!("...{}...", content_preview).dimmed());
                println!();
            }
        } else {
            println!("  {}\n", "No text results".dimmed());
        }
    }

    if mode.run_local() {
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
    fn test_search_mode_defaults() {
        let mode = SearchMode::default_mode();
        assert!(mode.run_fuzzy());
        assert!(mode.run_fts());
        assert!(mode.run_graph());
        assert!(mode.run_vector());
        assert!(!mode.run_local());
    }

    #[test]
    fn test_search_mode_from_flags() {
        // No flags → default
        let mode = SearchMode::from_flags(false, false, false, false);
        assert!(mode.run_fuzzy());
        assert!(mode.run_graph());
        assert!(mode.run_vector());

        // Single flag
        let mode = SearchMode::from_flags(true, false, false, false);
        assert!(mode.has(SearchEngine::Text));
        assert!(!mode.run_graph());

        // Combination
        let mode = SearchMode::from_flags(false, true, true, false);
        assert!(mode.run_graph());
        assert!(mode.run_vector());
        assert!(!mode.run_fts());

        // Local is exclusive
        let mode = SearchMode::from_flags(false, false, false, true);
        assert!(mode.run_local());
        assert!(!mode.run_fts());
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
