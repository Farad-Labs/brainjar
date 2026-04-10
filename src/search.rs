use anyhow::{Context, Result};
use colored::Colorize;
use rusqlite::Connection;
use std::collections::HashMap;
use std::collections::HashSet;

use crate::config::Config;
use crate::db;
use crate::embed::Embedder;
use zerocopy::IntoBytes;

/// Compute temporal decay multiplier for a document given its age.
///
/// Returns a value in `[floor, 1.0]`:
/// - Age ≤ 0 → 1.0 (no decay)
/// - Age = horizon → floor
/// - Age > horizon → floor (clamped)
/// - `shape` controls the curve: 1.0 = linear, 2.0 = quadratic, etc.
pub fn calc_decay(age_days: f64, horizon: u32, floor: f64, shape: f64) -> f64 {
    if age_days <= 0.0 {
        return 1.0;
    }
    let raw = 1.0 - (age_days / horizon as f64).powf(shape);
    floor + (1.0 - floor) * raw.max(0.0)
}

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
    /// Files containing near-identical content (collapsed during dedup pass).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub also_in: Vec<String>,
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
    /// Filename stem matching
    Filename,
}

/// Set of search engines to run. Default: Fuzzy + Graph + Vector + Filename.
#[derive(Debug, Clone)]
pub struct SearchMode {
    pub engines: std::collections::HashSet<SearchEngine>,
}

impl SearchMode {
    /// Default: fuzzy + graph + vector + filename
    pub fn default_mode() -> Self {
        Self {
            engines: [SearchEngine::Fuzzy, SearchEngine::Graph, SearchEngine::Vector, SearchEngine::Filename]
                .into_iter()
                .collect(),
        }
    }

    /// Build from explicit flags. If none set, use default.
    pub fn from_flags(text: bool, graph: bool, vector: bool, local: bool, filename: bool) -> Self {
        if local {
            return Self {
                engines: [SearchEngine::Local].into_iter().collect(),
            };
        }
        let mut engines = std::collections::HashSet::new();
        if text { engines.insert(SearchEngine::Text); }
        if graph { engines.insert(SearchEngine::Graph); }
        if vector { engines.insert(SearchEngine::Vector); }
        if filename { engines.insert(SearchEngine::Filename); }
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

    pub fn run_filename(&self) -> bool {
        self.has(SearchEngine::Filename)
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

        // Collect filename results for the original query
        let all_filename: Vec<(String, f64)> = if mode.run_filename() {
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
            let db_dir_smart = config.effective_db_dir();
            let mut fn_results: Vec<(String, f64)> = Vec::new();
            for (name, _kb) in &kbs {
                if let Ok(conn) = db::open_db(name, &db_dir_smart) {
                    fn_results.extend(search_filename(&conn, query));
                }
            }
            fn_results
        } else {
            Vec::new()
        };

        if json {
            let mut unified = build_unified_results(&all_fts, &all_local, &all_graph, &all_vector, &all_filename, limit, chunks, doc_score, query);
            enrich_graph_only_results(config, &mut unified);
            let unified = dedup_unified_results(unified);
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
                &all_filename,
                None,
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
    let run_filename = mode.run_filename();

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

    // Filename search results
    let filename_results: Vec<(String, f64)> = if run_filename {
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
        let mut all_fn: Vec<(String, f64)> = Vec::new();
        for (name, _kb) in &kbs {
            if let Ok(conn) = db::open_db(name, &db_dir) {
                all_fn.extend(search_filename(&conn, search_query));
            }
        }
        all_fn
    } else {
        Vec::new()
    };

    // Open a connection for decay scoring (best-effort; None disables decay)
    let decay_conn = kb_name.and_then(|n| db::open_db(n, &db_dir).ok());

    if json {
        let mut unified = build_unified_results(&fts_results, &local_results, &graph_results, &vector_results, &filename_results, limit, chunks, doc_score, query);
        if let Some(ref conn) = decay_conn {
            apply_folder_scoring(conn, &mut unified);
        }
        enrich_graph_only_results(config, &mut unified);
        let unified = dedup_unified_results(unified);
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
            &filename_results,
            decay_conn.as_ref(),
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

/// Filename stem search engine.
///
/// Scores each document's filename (stem without extension) against the query words:
/// - Exact stem match (query word == stem): score 1.0
/// - Substring match (query word contained in stem): score 0.5
/// - Case-insensitive; multiple words: sum, capped at 1.0
///
/// Returns Vec of (file_path, score) for files with score > 0.
pub fn search_filename(conn: &Connection, query: &str) -> Vec<(String, f64)> {
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect();
    if words.is_empty() {
        return vec![];
    }

    let paths: Vec<String> = {
        let mut stmt = match conn.prepare("SELECT path FROM documents") {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let rows = match stmt.query_map([], |row| row.get::<_, String>(0)) {
            Ok(r) => r,
            Err(_) => return vec![],
        };
        rows.filter_map(|r| r.ok()).collect()
    };

    let mut results = Vec::new();
    for path in &paths {
        let stem = std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        let mut score = 0.0f64;
        for word in &words {
            if stem == *word {
                score += 1.0;
            } else if stem.contains(word.as_str()) {
                score += 0.5;
            }
        }
        if score > 0.0 {
            results.push((path.clone(), score.min(1.0)));
        }
    }

    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    results
}

/// Weighted Normalized Score Fusion over multiple ranked result sets.
/// For each engine's result set, scores are normalized to [0.0, 1.0] via min-max,
/// then combined as: final_score = sum(weight × normalized_score).
/// This preserves score magnitude: a BM25 score of 4.76 outranks 4.41, not just "both rank N".
pub fn weighted_score_fusion(
    result_sets: Vec<(Vec<(String, f64)>, f64)>, // (results, weight)
) -> Vec<(String, f64)> {
    let mut scores: HashMap<String, f64> = HashMap::new();

    for (result_set, weight) in &result_sets {
        if result_set.is_empty() {
            continue;
        }

        // Min-max normalization within this engine's result set
        let min_score = result_set.iter().map(|(_, s)| *s).fold(f64::INFINITY, f64::min);
        let max_score = result_set.iter().map(|(_, s)| *s).fold(f64::NEG_INFINITY, f64::max);
        let range = max_score - min_score;

        for (doc_id, score) in result_set {
            let normalized = if range > 1e-10 {
                (score - min_score) / range
            } else {
                1.0 // All scores equal — treat as max normalized
            };
            *scores.entry(doc_id.clone()).or_insert(0.0) += weight * normalized;
        }
    }

    let mut merged: Vec<(String, f64)> = scores.into_iter().collect();
    merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    merged
}

/// Reciprocal Rank Fusion over multiple ranked result sets.
/// Each set is Vec<(doc_id, score)>. Returns merged Vec<(doc_id, rrf_score)>.
/// Kept for potential future use as a configurable alternative to weighted_score_fusion.
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

#[allow(clippy::too_many_arguments)]
fn build_unified_results(
    fts: &[FtsResult],
    local: &[LocalSearchResult],
    graph: &[crate::graph::GraphSearchResult],
    vector: &[VectorResult],
    filename_results: &[(String, f64)],
    limit: usize,
    _include_content: bool, // deprecated: content always included now
    doc_score: bool,
    _query: &str,
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
    let filename_ranked: Vec<(String, f64)> = filename_results.to_vec();

    // Weighted Normalized Score Fusion:
    // FTS5=0.35, Vector=0.25, Graph=0.2, Filename=0.1, Local/fuzzy=0.1
    // Scores within each engine are min-max normalized before weighting.
    let merged = weighted_score_fusion(vec![
        (fts_ranked, 0.35),
        (vector_ranked, 0.25),
        (graph_ranked, 0.2),
        (filename_ranked, 0.1),
        (local_ranked, 0.1),
    ]);

    // Build lookup maps
    let fts_by_chunk: HashMap<i64, &FtsResult> =
        fts.iter().filter_map(|r| r.chunk_id.map(|id| (id, r))).collect();
    let fts_by_path: HashMap<&str, &FtsResult> =
        fts.iter().map(|r| (r.path.as_str(), r)).collect();
    let local_map: HashMap<&str, &LocalSearchResult> =
        local.iter().map(|r| (r.file.as_str(), r)).collect();
    let graph_map: HashMap<&str, &crate::graph::GraphSearchResult> =
        graph.iter().map(|r| (r.file.as_str(), r)).collect();
    let filename_map: HashMap<&str, f64> =
        filename_results.iter().map(|(p, s)| (p.as_str(), *s)).collect();
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
            if filename_map.contains_key(file.as_str()) && !sources.contains(&"filename".to_string()) {
                sources.push("filename".to_string());
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
                also_in: Vec::new(),
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

/// Compute word-level Jaccard similarity between two strings.
fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let words_a: HashSet<&str> = a.split_whitespace().collect();
    let words_b: HashSet<&str> = b.split_whitespace().collect();
    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Normalize content for comparison: trim and collapse whitespace.
fn normalize_content(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Dedup near-identical chunks in search results.
///
/// Two passes:
/// 1. Exact dedup: group by normalized content, keep highest-scoring entry, add others to `also_in`.
/// 2. Near dedup: Jaccard similarity > 0.85 between remaining results collapses into higher-scoring.
///
/// Empty content strings are never considered duplicates.
fn dedup_unified_results(results: Vec<UnifiedResult>) -> Vec<UnifiedResult> {
    // --- Pass 1: Exact dedup by normalized content ---
    let normalized: Vec<String> = results.iter().map(|r| normalize_content(&r.content)).collect();

    let n = results.len();
    let mut absorbed = vec![false; n];
    // Map from normalized content → index of the winner (highest score)
    let mut content_to_winner: HashMap<String, usize> = HashMap::new();

    // Find winners (highest score per normalized content, skipping empty)
    for (i, norm) in normalized.iter().enumerate() {
        if norm.is_empty() {
            continue;
        }
        content_to_winner
            .entry(norm.clone())
            .and_modify(|winner_idx| {
                if results[i].score > results[*winner_idx].score {
                    *winner_idx = i;
                }
            })
            .or_insert(i);
    }

    // Mark non-winners as absorbed; collect their files into the winner's also_in
    let mut also_in_map: HashMap<usize, Vec<String>> = HashMap::new();
    for (i, norm) in normalized.iter().enumerate() {
        if norm.is_empty() {
            continue;
        }
        if let Some(&winner_idx) = content_to_winner.get(norm.as_str())
            && winner_idx != i
        {
            absorbed[i] = true;
            also_in_map
                .entry(winner_idx)
                .or_default()
                .push(results[i].file.clone());
        }
    }

    // Build the post-exact-dedup results with also_in populated
    let mut deduped: Vec<UnifiedResult> = results
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !absorbed[*i])
        .map(|(i, mut r)| {
            if let Some(extra) = also_in_map.remove(&i) {
                r.also_in.extend(extra);
            }
            r
        })
        .collect();

    // Re-sort in case winners were not already highest-score first
    deduped.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    // --- Pass 2: Near dedup by Jaccard similarity (threshold 0.85) ---
    let m = deduped.len();
    let mut near_absorbed = vec![false; m];

    for i in 0..m {
        if near_absorbed[i] || deduped[i].content.is_empty() {
            continue;
        }
        let norm_i = normalize_content(&deduped[i].content);
        for j in (i + 1)..m {
            if near_absorbed[j] || deduped[j].content.is_empty() {
                continue;
            }
            let norm_j = normalize_content(&deduped[j].content);
            if jaccard_similarity(&norm_i, &norm_j) > 0.85 {
                // i has higher or equal score (list is sorted); absorb j into i
                near_absorbed[j] = true;
                let file_j = deduped[j].file.clone();
                deduped[i].also_in.push(file_j);
            }
        }
    }

    deduped
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !near_absorbed[*i])
        .map(|(_, r)| r)
        .collect()
}

/// Pre-fetch per-document decay parameters and apply folder-based scoring.
///
/// For each result:
/// 1. Look up the document's weight_boost, decay config, and updated_at timestamp.
/// 2. Compute a decay multiplier from the document's age.
/// 3. Adjust: `score = score * decay_multiplier + weight_boost`
///
/// Results are re-sorted by score after adjustment.
fn apply_folder_scoring(conn: &Connection, results: &mut [UnifiedResult]) {
    // Collect unique file paths from results
    let unique_files: HashSet<&str> = results.iter().map(|r| r.file.as_str()).collect();
    if unique_files.is_empty() {
        return;
    }

    // Fetch decay params for all relevant documents in one pass
    struct DocDecayParams {
        weight_boost: f64,
        decay_horizon: Option<u32>,
        decay_floor: Option<f64>,
        decay_shape: Option<f64>,
        updated_at: String,
    }

    let mut params_map: HashMap<String, DocDecayParams> = HashMap::new();

    if let Ok(mut stmt) = conn.prepare(
        "SELECT path, weight_boost, decay_horizon, decay_floor, decay_shape, updated_at
         FROM documents",
    )
        && let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1).unwrap_or(0.0),
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, String>(5).unwrap_or_default(),
            ))
        })
    {
        for row in rows.flatten() {
            let (path, wb, dh, df, ds, ua) = row;
            if unique_files.contains(path.as_str()) {
                params_map.insert(
                    path,
                    DocDecayParams {
                        weight_boost: wb,
                        decay_horizon: dh.map(|v| v as u32),
                        decay_floor: df,
                        decay_shape: ds,
                        updated_at: ua,
                    },
                );
            }
        }
    }

    if params_map.is_empty() {
        return;
    }

    let now = chrono::Utc::now();

    for result in results.iter_mut() {
        if let Some(p) = params_map.get(&result.file) {
            // Compute age in days
            let decay_multiplier = if let (Some(horizon), Some(floor), Some(shape)) =
                (p.decay_horizon, p.decay_floor, p.decay_shape)
            {
                let age_days = if let Ok(dt) =
                    chrono::DateTime::parse_from_rfc3339(&p.updated_at)
                {
                    (now - dt.with_timezone(&chrono::Utc))
                        .num_seconds()
                        .max(0) as f64
                        / 86400.0
                } else {
                    0.0
                };
                calc_decay(age_days, horizon, floor, shape)
            } else {
                1.0
            };

            result.score = result.score * decay_multiplier + p.weight_boost;
        }
    }

    // Re-sort after score adjustment
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
}

/// Enrich graph-only results (no FTS/vector match) with content from the chunks table.
fn enrich_graph_only_results(config: &Config, results: &mut [UnifiedResult]) {
    let db_dir = config.effective_db_dir();
    for result in results.iter_mut() {
        if result.content.is_empty()
            && result.chunk_id.is_none()
            && result.sources.contains(&"graph".to_string())
        {
            for kb_name in config.knowledge_bases.keys() {
                if let Ok(conn) = db::open_db(kb_name, &db_dir)
                    && let Ok(Some((id, content, ls, le, ct))) =
                        db::get_first_chunk_for_file(&conn, &result.file)
                {
                    result.content = content;
                    result.chunk_id = Some(id);
                    result.line_start = Some(ls as u32);
                    result.line_end = Some(le as u32);
                    result.chunk_type = Some(ct);
                    break;
                }
            }
        }
    }
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
    filename_results: &[(String, f64)],
    conn: Option<&Connection>,
    mode: &SearchMode,
    limit: usize,
    include_content: bool,
    doc_score: bool,
) {
    let has_fts = !fts.is_empty();
    let has_local = !local.is_empty();
    let has_graph = !graph.is_empty();
    let has_vector = !vector.is_empty();
    let has_filename = !filename_results.is_empty();

    if !has_fts && !has_local && !has_graph && !has_vector && !has_filename {
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
        let mut unified = build_unified_results(fts, local, graph, vector, filename_results, limit, include_content, doc_score, query);
        if let Some(db_conn) = conn {
            apply_folder_scoring(db_conn, &mut unified);
        }
        let unified = dedup_unified_results(unified);
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
            if !result.also_in.is_empty() {
                println!("     {}", format!("Also in: {}", result.also_in.join(", ")).dimmed());
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
                id            INTEGER PRIMARY KEY,
                path          TEXT UNIQUE NOT NULL,
                content       TEXT NOT NULL,
                content_hash  TEXT NOT NULL,
                extracted     INTEGER NOT NULL DEFAULT 0,
                updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
                weight_boost  REAL NOT NULL DEFAULT 0.0,
                decay_horizon INTEGER,
                decay_floor   REAL,
                decay_shape   REAL,
                folder_path   TEXT
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
            db::upsert_document(&conn, path, content, "hash", 0.0, None, None, None, None).unwrap();
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
        let mode = SearchMode::from_flags(false, false, false, false, false);
        assert!(mode.run_fuzzy());
        assert!(mode.run_graph());
        assert!(mode.run_vector());
        assert!(mode.run_filename());

        // Single flag
        let mode = SearchMode::from_flags(true, false, false, false, false);
        assert!(mode.has(SearchEngine::Text));
        assert!(!mode.run_graph());

        // Combination
        let mode = SearchMode::from_flags(false, true, true, false, false);
        assert!(mode.run_graph());
        assert!(mode.run_vector());
        assert!(!mode.run_fts());

        // Local is exclusive
        let mode = SearchMode::from_flags(false, false, false, true, false);
        assert!(mode.run_local());
        assert!(!mode.run_fts());

        // Filename flag alone
        let mode = SearchMode::from_flags(false, false, false, false, true);
        assert!(mode.run_filename());
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

    // ─── weighted_score_fusion ─────────────────────────────────────────────

    #[test]
    fn test_wsf_single_set_preserves_magnitude() {
        // Higher raw scores should produce higher normalized+weighted scores
        let set = vec![
            ("doc_a".to_string(), 4.76),
            ("doc_b".to_string(), 4.41),
            ("doc_c".to_string(), 1.0),
        ];
        let merged = weighted_score_fusion(vec![(set, 1.0)]);
        assert_eq!(merged.len(), 3);
        // doc_a has highest raw score → should rank first
        assert_eq!(merged[0].0, "doc_a");
        // doc_a normalized = 1.0, doc_b = (4.41-1.0)/(4.76-1.0) ≈ 0.906, doc_c = 0.0
        let score_a = merged.iter().find(|(k, _)| k == "doc_a").unwrap().1;
        let score_b = merged.iter().find(|(k, _)| k == "doc_b").unwrap().1;
        let score_c = merged.iter().find(|(k, _)| k == "doc_c").unwrap().1;
        assert!(score_a > score_b);
        assert!(score_b > score_c);
        assert!((score_a - 1.0).abs() < 1e-9); // max normalized = 1.0 × weight
        assert!((score_c - 0.0).abs() < 1e-9); // min normalized = 0.0 × weight
    }

    #[test]
    fn test_wsf_two_sets_combined() {
        // doc_x appears in both sets, should get contributions from both
        let set_fts = vec![
            ("doc_x".to_string(), 10.0),
            ("doc_y".to_string(), 5.0),
        ];
        let set_graph = vec![
            ("doc_x".to_string(), 8.0),
            ("doc_z".to_string(), 3.0),
        ];
        let merged = weighted_score_fusion(vec![(set_fts, 0.4), (set_graph, 0.2)]);
        let score_x = merged.iter().find(|(k, _)| k == "doc_x").unwrap().1;
        let score_y = merged.iter().find(|(k, _)| k == "doc_y").unwrap().1;
        // doc_x appears in both, should score higher than doc_y (only in one)
        assert!(score_x > score_y);
    }

    #[test]
    fn test_wsf_all_equal_scores() {
        // All equal scores → all normalize to 1.0 (range=0 edge case)
        let set = vec![
            ("a".to_string(), 5.0),
            ("b".to_string(), 5.0),
        ];
        let merged = weighted_score_fusion(vec![(set, 0.5)]);
        assert_eq!(merged.len(), 2);
        // Both should have score = 0.5 × 1.0
        for (_, score) in &merged {
            assert!((score - 0.5).abs() < 1e-9);
        }
    }

    #[test]
    fn test_wsf_empty_sets() {
        let merged = weighted_score_fusion(vec![]);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_wsf_result_sorted_descending() {
        let set = vec![
            ("low".to_string(), 1.0),
            ("high".to_string(), 100.0),
            ("mid".to_string(), 50.0),
        ];
        let merged = weighted_score_fusion(vec![(set, 1.0)]);
        for i in 0..merged.len() - 1 {
            assert!(merged[i].1 >= merged[i + 1].1);
        }
    }

    // ─── search_filename unit tests ──────────────────────────────────────────

    fn make_test_db_with_docs(paths: &[&str]) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE documents (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                content TEXT,
                content_hash TEXT,
                extracted INTEGER DEFAULT 0,
                updated_at TEXT
            );",
        )
        .unwrap();
        for path in paths {
            conn.execute(
                "INSERT INTO documents (path) VALUES (?1)",
                rusqlite::params![path],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn test_filename_search_exact_match() {
        let conn = make_test_db_with_docs(&["docs/architecture.md", "docs/other.md"]);
        let results = search_filename(&conn, "architecture");
        let arch = results.iter().find(|(p, _)| p.contains("architecture.md"));
        assert!(arch.is_some(), "architecture.md should match");
        assert!(
            (arch.unwrap().1 - 1.0).abs() < 1e-9,
            "exact match should score 1.0, got {}",
            arch.unwrap().1
        );
        // other.md should not appear
        let other = results.iter().find(|(p, _)| p.contains("other.md"));
        assert!(other.is_none(), "other.md should not match query 'architecture'");
    }

    #[test]
    fn test_filename_search_substring_match() {
        let conn = make_test_db_with_docs(&["docs/architecture.md", "docs/arch-notes.md"]);
        let results = search_filename(&conn, "arch");
        let arch_md = results.iter().find(|(p, _)| p.ends_with("architecture.md"));
        let arch_notes = results.iter().find(|(p, _)| p.ends_with("arch-notes.md"));
        assert!(arch_md.is_some(), "architecture.md should match 'arch'");
        assert_eq!(arch_md.unwrap().1, 0.5, "substring match should score 0.5");
        assert!(arch_notes.is_some(), "arch-notes.md should match 'arch'");
        // arch-notes has stem "arch-notes"; "arch" is a substring → 0.5
        assert_eq!(arch_notes.unwrap().1, 0.5);
    }

    #[test]
    fn test_filename_search_no_match() {
        let conn = make_test_db_with_docs(&["docs/architecture.md", "docs/overview.md"]);
        let results = search_filename(&conn, "zebra");
        assert!(results.is_empty(), "no files should match 'zebra'");
    }

    #[test]
    fn test_filename_search_multiple_words() {
        let conn = make_test_db_with_docs(&[
            "docs/architecture-overview.md",
            "docs/architecture.md",
            "docs/overview.md",
        ]);
        let results = search_filename(&conn, "architecture overview");
        let arch_ov = results.iter().find(|(p, _)| p.ends_with("architecture-overview.md"));
        let arch = results.iter().find(|(p, _)| p.ends_with("architecture.md") && !p.ends_with("architecture-overview.md"));
        assert!(arch_ov.is_some(), "architecture-overview.md should match");
        assert!(arch.is_some(), "architecture.md should match");
        // architecture-overview.md: "architecture" is substring → 0.5, "overview" is substring → 0.5, sum = 1.0 (capped)
        assert!(
            (arch_ov.unwrap().1 - 1.0).abs() < 1e-9,
            "architecture-overview.md should score 1.0 (capped), got {}",
            arch_ov.unwrap().1
        );
        // architecture.md: "architecture" exact → 1.0, "overview" no match → 0, capped at 1.0
        assert!(
            (arch.unwrap().1 - 1.0).abs() < 1e-9,
            "architecture.md should score 1.0 for exact 'architecture' match, got {}",
            arch.unwrap().1
        );
        // architecture-overview.md should rank >= architecture.md (both capped at 1.0)
        assert!(arch_ov.unwrap().1 >= arch.unwrap().1);
    }

    // ─── filename engine in fusion ────────────────────────────────────────────

    #[test]
    fn test_filename_in_fusion_sources() {
        // pricing.md and old-blog.md have identical raw FTS scores.
        // Provide filename results: pricing.md scores 1.0, old-blog.md has no filename score.
        let fts_results = vec![
            FtsResult {
                path: "docs/pricing.md".to_string(),
                excerpt: "Our pricing plans start at $10/mo.".to_string(),
                score: 3.0,
                chunk_id: None,
                line_start: None,
                line_end: None,
                chunk_type: None,
                content: Some("Our pricing plans start at $10/mo.".to_string()),
            },
            FtsResult {
                path: "archive/old-blog.md".to_string(),
                excerpt: "We changed our pricing last year.".to_string(),
                score: 3.0,
                chunk_id: None,
                line_start: None,
                line_end: None,
                chunk_type: None,
                content: Some("We changed our pricing last year.".to_string()),
            },
        ];
        let filename_results = vec![
            ("docs/pricing.md".to_string(), 1.0),
        ];

        let unified = build_unified_results(
            &fts_results,
            &[],
            &[],
            &[],
            &filename_results,
            10,
            true,
            false,
            "pricing",
        );

        let pricing_result = unified.iter().find(|r| r.file.contains("pricing.md"));
        let archive_result = unified.iter().find(|r| r.file.contains("old-blog.md"));

        assert!(pricing_result.is_some(), "pricing.md should be in results");
        assert!(archive_result.is_some(), "old-blog.md should be in results");

        // pricing.md should outrank old-blog.md (it has filename engine contribution)
        let pricing_score = pricing_result.unwrap().score;
        let archive_score = archive_result.unwrap().score;
        assert!(
            pricing_score > archive_score,
            "pricing.md (score {:.4}) should outrank archive/old-blog.md (score {:.4}) via filename engine",
            pricing_score,
            archive_score
        );

        // pricing.md should have "filename" in sources
        assert!(
            pricing_result.unwrap().sources.contains(&"filename".to_string()),
            "pricing.md should have 'filename' in sources: {:?}",
            pricing_result.unwrap().sources
        );
        // old-blog.md should NOT have "filename" in sources
        assert!(
            !archive_result.unwrap().sources.contains(&"filename".to_string()),
            "old-blog.md should NOT have 'filename' in sources"
        );
    }

    #[test]
    fn test_filename_boost_case_insensitive() {
        // PRICING.md (uppercase) should still get filename engine contribution for query "pricing"
        let fts_results = vec![
            FtsResult {
                path: "docs/PRICING.md".to_string(),
                excerpt: "Pricing information.".to_string(),
                score: 2.0,
                chunk_id: None,
                line_start: None,
                line_end: None,
                chunk_type: None,
                content: Some("Pricing information.".to_string()),
            },
            FtsResult {
                path: "docs/other.md".to_string(),
                excerpt: "Also mentions pricing.".to_string(),
                score: 2.0, // same raw FTS score
                chunk_id: None,
                line_start: None,
                line_end: None,
                chunk_type: None,
                content: Some("Also mentions pricing.".to_string()),
            },
        ];
        // Provide filename results simulating case-insensitive match
        let filename_results = vec![
            ("docs/PRICING.md".to_string(), 1.0),
        ];

        let unified = build_unified_results(
            &fts_results,
            &[],
            &[],
            &[],
            &filename_results,
            10,
            true,
            false,
            "pricing",
        );

        let pricing_result = unified.iter().find(|r| r.file.contains("PRICING.md"));
        let other_result = unified.iter().find(|r| r.file.contains("other.md"));

        assert!(pricing_result.is_some());
        assert!(other_result.is_some());

        // PRICING.md should score higher (filename engine contribution)
        assert!(
            pricing_result.unwrap().score > other_result.unwrap().score,
            "PRICING.md should rank higher via filename engine for query 'pricing'"
        );
    }

    #[test]
    fn test_filename_boost_no_match_no_boost() {
        // A file whose name doesn't match the query should not gain filename engine score
        let fts_results = vec![
            FtsResult {
                path: "docs/readme.md".to_string(),
                excerpt: "This readme covers pricing details.".to_string(),
                score: 4.0,
                chunk_id: None,
                line_start: None,
                line_end: None,
                chunk_type: None,
                content: Some("Pricing details.".to_string()),
            },
        ];

        let unified = build_unified_results(
            &fts_results,
            &[],
            &[],
            &[],
            &[],
            10,
            true,
            false,
            "pricing",
        );

        assert_eq!(unified.len(), 1);
        // Score should be the normalized FTS score (1.0 × 0.35 weight) = 0.35
        // No filename contribution since filename_results is empty
        assert!(
            (unified[0].score - 0.35).abs() < 1e-9,
            "readme.md should have score 0.35 (FTS weight only); expected 0.35, got {:.4}",
            unified[0].score
        );
    }

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

    // ─── calc_decay ──────────────────────────────────────────────────────────

    #[test]
    fn test_calc_decay_age_zero_returns_one() {
        assert!((calc_decay(0.0, 30, 0.3, 1.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_calc_decay_negative_age_returns_one() {
        assert!((calc_decay(-5.0, 30, 0.3, 1.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_calc_decay_at_horizon_returns_floor() {
        let floor = 0.3;
        let result = calc_decay(30.0, 30, floor, 1.0);
        assert!((result - floor).abs() < 1e-9, "Expected floor {floor}, got {result}");
    }

    #[test]
    fn test_calc_decay_beyond_horizon_returns_floor() {
        let floor = 0.2;
        let result = calc_decay(100.0, 30, floor, 1.0);
        assert!((result - floor).abs() < 1e-9, "Expected floor {floor}, got {result}");
    }

    #[test]
    fn test_calc_decay_shape_2_gives_quadratic_curve() {
        // At half the horizon with shape=2: raw = 1 - (0.5)^2 = 0.75
        // decay = floor + (1-floor) * 0.75
        let floor = 0.0;
        let shape = 2.0;
        let horizon = 100u32;
        let age = 50.0; // half horizon
        let result = calc_decay(age, horizon, floor, shape);
        let expected = 0.0 + (1.0 - 0.0) * 0.75;
        assert!((result - expected).abs() < 1e-9, "Expected {expected}, got {result}");
    }

    #[test]
    fn test_calc_decay_halfway_linear() {
        // Linear: age=15, horizon=30, floor=0 → raw = 1 - 0.5 = 0.5
        let result = calc_decay(15.0, 30, 0.0, 1.0);
        assert!((result - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_calc_decay_default_floor_and_shape() {
        // Default floor=0.3, shape=1.0
        // At horizon, should return floor=0.3
        let result = calc_decay(30.0, 30, 0.3, 1.0);
        assert!((result - 0.3).abs() < 1e-9);
    }

    // ─── dedup_unified_results ───────────────────────────────────────────────

    fn make_unified(file: &str, score: f64, content: &str) -> UnifiedResult {
        UnifiedResult {
            file: file.to_string(),
            score,
            sources: vec!["fts".to_string()],
            content: content.to_string(),
            chunk_id: None,
            line_start: None,
            line_end: None,
            chunk_type: None,
            also_in: Vec::new(),
        }
    }

    #[test]
    fn test_dedup_exact_duplicates_collapsed() {
        let content = "This is a shared codebase preference snippet.";
        let results = vec![
            make_unified("pref_2025-11-03.md", 0.9, content),
            make_unified("pref_2026-01-10.md", 0.7, content),
            make_unified("pref_2026-02-05.md", 0.5, content),
        ];
        let deduped = dedup_unified_results(results);
        // Only one result should remain (highest score)
        assert_eq!(deduped.len(), 1, "Exact duplicates should collapse to 1");
        assert_eq!(deduped[0].file, "pref_2025-11-03.md");
        assert_eq!(deduped[0].score, 0.9);
        // The other two files should appear in also_in
        assert_eq!(deduped[0].also_in.len(), 2);
        assert!(deduped[0].also_in.contains(&"pref_2026-01-10.md".to_string()));
        assert!(deduped[0].also_in.contains(&"pref_2026-02-05.md".to_string()));
    }

    #[test]
    fn test_dedup_distinct_content_not_collapsed() {
        let results = vec![
            make_unified("doc_a.md", 0.9, "Rust is a systems language with memory safety."),
            make_unified("doc_b.md", 0.7, "Python is great for scripting and data science."),
        ];
        let deduped = dedup_unified_results(results);
        assert_eq!(deduped.len(), 2, "Distinct content should not be collapsed");
        assert!(deduped[0].also_in.is_empty());
        assert!(deduped[1].also_in.is_empty());
    }

    #[test]
    fn test_dedup_empty_content_not_deduplicated() {
        // Two results with empty content should NOT be collapsed
        let results = vec![
            make_unified("doc_a.md", 0.9, ""),
            make_unified("doc_b.md", 0.7, ""),
        ];
        let deduped = dedup_unified_results(results);
        assert_eq!(deduped.len(), 2, "Empty content should not be treated as duplicates");
    }

    #[test]
    fn test_dedup_near_duplicate_jaccard() {
        // Two chunks with >0.85 Jaccard similarity.
        // base has 15 unique words; near has the same 15 words plus one extra.
        // Jaccard = 15 / 16 = 0.9375 > 0.85, so they should collapse.
        let base = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron";
        let near = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron rho";
        let results = vec![
            make_unified("doc_a.md", 0.8, base),
            make_unified("doc_b.md", 0.6, near),
        ];
        let deduped = dedup_unified_results(results);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].file, "doc_a.md");
        assert!(deduped[0].also_in.contains(&"doc_b.md".to_string()));
    }

    #[test]
    fn test_jaccard_similarity_identical() {
        let s = "hello world foo bar";
        assert!((jaccard_similarity(s, s) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_jaccard_similarity_disjoint() {
        assert!(jaccard_similarity("hello world", "foo bar").abs() < 1e-9);
    }

    #[test]
    fn test_jaccard_similarity_empty() {
        assert!(jaccard_similarity("", "").abs() < 1e-9);
    }

    #[test]
    fn test_dedup_preserves_highest_score() {
        // Even if the highest-score item is not first in input, it should win
        let content = "identical content here";
        let results = vec![
            make_unified("low.md", 0.3, content),
            make_unified("high.md", 0.95, content),
            make_unified("mid.md", 0.6, content),
        ];
        let deduped = dedup_unified_results(results);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].file, "high.md");
        assert_eq!(deduped[0].score, 0.95);
        assert_eq!(deduped[0].also_in.len(), 2);
    }
}
