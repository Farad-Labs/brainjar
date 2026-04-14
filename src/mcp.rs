/// MCP server over stdio (JSON-RPC 2.0)
/// Implements: initialize, tools/list, tools/call
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, Write};

use crate::config::Config;

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl JsonRpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn err(id: Option<Value>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
        }
    }
}

pub async fn run_mcp(config: Config) -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    eprintln!("[brainjar mcp] Server started on stdio");

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) if l.trim().is_empty() => continue,
            Ok(l) => l,
            Err(e) => {
                eprintln!("[brainjar mcp] Read error: {}", e);
                break;
            }
        };

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::err(None, -32700, format!("Parse error: {}", e));
                writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
                stdout.flush()?;
                continue;
            }
        };

        let id = request.id.clone();
        let response = handle_request(&config, request).await;

        let resp = match response {
            Ok(result) => JsonRpcResponse::ok(id, result),
            Err(e) => JsonRpcResponse::err(id, -32603, e.to_string()),
        };

        writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
        stdout.flush()?;
    }

    Ok(())
}

async fn handle_request(config: &Config, request: JsonRpcRequest) -> Result<Value> {
    match request.method.as_str() {
        "initialize" => handle_initialize(request.params),
        "tools/list" => handle_tools_list(),
        "tools/call" => handle_tools_call(config, request.params).await,
        "notifications/initialized" => Ok(serde_json::json!({})),
        method => anyhow::bail!("Method not found: {}", method),
    }
}

fn handle_initialize(_params: Option<Value>) -> Result<Value> {
    Ok(serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "brainjar",
            "version": env!("CARGO_PKG_VERSION")
        }
    }))
}

fn handle_tools_list() -> Result<Value> {
    Ok(serde_json::json!({
        "tools": [
            {
                "name": "memory_search",
                "description": "Search across knowledge bases for relevant memories and context. Runs both FTS5 text search and local fuzzy file search by default.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query"
                        },
                        "kb": {
                            "type": "string",
                            "description": "Knowledge base name (optional, searches all if omitted)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results (default: 5)",
                            "default": 5
                        },
                        "mode": {
                            "type": "string",
                            "enum": ["all", "fuzzy", "local", "text", "graph", "vector"],
                            "description": "Search mode: 'all' runs FTS5 + graph + vector (default), 'fuzzy' adds typo correction, 'local' runs nucleo fuzzy, 'text' FTS5 only, 'graph' entity traversal, 'vector' semantic similarity",
                            "default": "all"
                        },
                        "exact": {
                            "type": "boolean",
                            "description": "Use exact (case-insensitive substring) matching for local search instead of fuzzy",
                            "default": false
                        },
                        "include_content": {
                            "type": "boolean",
                            "description": "Return full chunk content in results instead of a short preview",
                            "default": false
                        },
                        "doc_score": {
                            "type": "boolean",
                            "description": "Aggregate chunk scores per document and return one result per document",
                            "default": false
                        },
                        "smart": {
                            "type": "boolean",
                            "description": "Use LLM to extract targeted search queries from conversational text before searching. Provide context for even better extraction. Requires [extraction] config.",
                            "default": false
                        },
                        "context": {
                            "type": "string",
                            "description": "Conversation context for smart search (plain text, any format). When provided with smart=true, the LLM uses this context to extract better search terms.",
                            "default": null
                        },
                        "exclude_chunks": {
                            "type": "array",
                            "items": { "type": "integer" },
                            "description": "List of chunk IDs to exclude from results (for deduplication across conversation turns)",
                            "default": null
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "retrieve_chunk",
                "description": "Retrieve a specific chunk by ID with optional context expansion (surrounding lines from the parent document or neighboring chunks)",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "chunk_id": {
                            "type": "integer",
                            "description": "The chunk ID to retrieve"
                        },
                        "lines_before": {
                            "type": "integer",
                            "description": "Number of lines of context before the chunk from the parent document",
                            "default": 0
                        },
                        "lines_after": {
                            "type": "integer",
                            "description": "Number of lines of context after the chunk from the parent document",
                            "default": 0
                        },
                        "chunks_before": {
                            "type": "integer",
                            "description": "Number of preceding chunks to include",
                            "default": 0
                        },
                        "chunks_after": {
                            "type": "integer",
                            "description": "Number of following chunks to include",
                            "default": 0
                        }
                    },
                    "required": ["chunk_id"]
                }
            },
            {
                "name": "memory_sync",
                "description": "Sync files to the local SQLite index",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "kb": {
                            "type": "string",
                            "description": "Knowledge base name (optional, syncs all auto_sync KBs if omitted)"
                        },
                        "force": {
                            "type": "boolean",
                            "description": "Force re-index of all files",
                            "default": false
                        }
                    }
                }
            },
            {
                "name": "memory_status",
                "description": "Get knowledge base status including document count and last sync time",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "kb": {
                            "type": "string",
                            "description": "Knowledge base name (optional, shows all if omitted)"
                        }
                    }
                }
            },
            {
                "name": "memory_list",
                "description": "List all configured knowledge bases with their names, descriptions, watch paths, auto-sync status, document counts, and last sync times",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            }
        ]
    }))
}

async fn handle_tools_call(config: &Config, params: Option<Value>) -> Result<Value> {
    let params = params.unwrap_or_default();
    let tool_name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;
    let args = params.get("arguments").cloned().unwrap_or_default();

    match tool_name {
        "memory_search" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing query argument"))?;
            let kb = args.get("kb").and_then(|v| v.as_str());
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
            let exact = args.get("exact").and_then(|v| v.as_bool()).unwrap_or(false);
            let mode_str = args.get("mode").and_then(|v| v.as_str()).unwrap_or("all");

            let mode = match mode_str {
                "local" => crate::search::SearchMode::from_flags(false, false, false, true, false),
                "text" => crate::search::SearchMode::from_flags(true, false, false, false, false),
                "graph" => crate::search::SearchMode::from_flags(false, true, false, false, false),
                "vector" => crate::search::SearchMode::from_flags(false, false, true, false, false),
                "filename" => crate::search::SearchMode::from_flags(false, false, false, false, true),
                _ => crate::search::SearchMode::default_mode(),
            };

            let include_content = args.get("include_content").and_then(|v| v.as_bool()).unwrap_or(false);
            let doc_score = args.get("doc_score").and_then(|v| v.as_bool()).unwrap_or(false);
            let smart = args.get("smart").and_then(|v| v.as_bool()).unwrap_or(false);
            let context = args.get("context").and_then(|v| v.as_str());
            let exclude_chunks: Option<Vec<i64>> = args.get("exclude_chunks")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect());
            let exclude_chunks_ref: Option<&[i64]> = exclude_chunks.as_deref();

            // Build KB list early (needed for both smart and normal paths)
            let kbs_owned: Vec<(String, crate::config::KnowledgeBaseConfig)> = if let Some(name) = kb {
                if let Some(k) = config.knowledge_bases.get(name) {
                    vec![(name.to_string(), k.clone())]
                } else {
                    return Ok(tool_error(format!("KB '{}' not found", name)));
                }
            } else {
                config.knowledge_bases.iter().map(|(n, k)| (n.clone(), k.clone())).collect()
            };
            let kbs: Vec<(&str, &crate::config::KnowledgeBaseConfig)> =
                kbs_owned.iter().map(|(n, k)| (n.as_str(), k)).collect();

            // Smart mode: fan-out search with LLM-extracted queries
            if smart {
                let queries = match crate::search::extract_queries_pub(config, query, context).await {
                    Ok(q) => q,
                    Err(e) => return Ok(tool_error(format!("Smart search query extraction failed: {}", e))),
                };

                let mut all_fts: Vec<crate::search::FtsResult> = Vec::new();
                let mut all_local: Vec<crate::local_search::LocalSearchResult> = Vec::new();
                let mut all_graph: Vec<crate::graph::GraphSearchResult> = Vec::new();

                let run_fts_inner = mode.run_fts();
                let run_graph_inner = mode.run_graph();

                for sub_query in &queries {
                    if run_fts_inner {
                        for (name, _kb_config) in &kbs {
                            match crate::search::search_fts_for_kb(config, name, sub_query, limit) {
                                Ok(results) => all_fts.extend(results),
                                Err(e) => eprintln!("[brainjar mcp] FTS error for KB {}: {}", name, e),
                            }
                        }
                    }
                    if run_graph_inner {
                        for (name, _kb_config) in &kbs {
                            if !crate::graph::KnowledgeGraph::exists(&config.effective_db_dir(), name) {
                                continue;
                            }
                            match crate::graph::KnowledgeGraph::open(&config.effective_db_dir(), name) {
                                Ok(kg) => match kg.search(sub_query, limit) {
                                    Ok(results) => all_graph.extend(results),
                                    Err(e) => eprintln!("[brainjar mcp] Graph error for KB {}: {}", name, e),
                                },
                                Err(e) => eprintln!("[brainjar mcp] Graph open error for KB {}: {}", name, e),
                            }
                        }
                    }
                    match crate::local_search::run_local_search(config, sub_query, limit, exact) {
                        Ok(r) => all_local.extend(r),
                        Err(e) => eprintln!("[brainjar mcp] Local search error: {}", e),
                    }
                }

                // Deduplicate
                all_fts.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
                {
                    let mut seen_chunks = std::collections::HashSet::new();
                    let mut seen_paths = std::collections::HashSet::new();
                    all_fts.retain(|r| {
                        if let Some(id) = r.chunk_id { seen_chunks.insert(id) }
                        else { seen_paths.insert(r.path.clone()) }
                    });
                }
                all_local.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
                {
                    let mut seen = std::collections::HashSet::new();
                    all_local.retain(|r| seen.insert(r.file.clone()));
                }
                all_graph.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
                {
                    let mut seen = std::collections::HashSet::new();
                    all_graph.retain(|r| seen.insert(r.file.clone()));
                }

                // Filter excluded chunk IDs from content-bearing results (FTS has chunk_id).
                // Graph results are entity-level (no chunk_id) and stay unfiltered.
                // For JSON output, build_unified_results handles FTS + vector exclusion.
                if let Some(excl) = exclude_chunks_ref
                    && !excl.is_empty()
                {
                    all_fts.retain(|r| r.chunk_id.is_none_or(|id| !excl.contains(&id)));
                }

                let mut text = format!("🧠 Smart search extracted {} quer{}: {}\n\n",
                    queries.len(),
                    if queries.len() == 1 { "y" } else { "ies" },
                    queries.iter().map(|q| format!("\"{}\"", q)).collect::<Vec<_>>().join(", ")
                );
                text.push_str(&format_search_text(query, &all_fts, &all_local, &all_graph, mode, include_content, doc_score));
                return Ok(tool_text(text));
            }

            let run_fts = mode.run_fts();
            let run_local = mode.run_local();
            let run_graph = mode.run_graph();

            // FTS results
            let fts_results: Vec<crate::search::FtsResult> = if run_fts {
                let mut all = Vec::new();
                for (name, _kb_config) in &kbs {
                    match crate::search::search_fts_for_kb(config, name, query, limit) {
                        Ok(results) => all.extend(results),
                        Err(e) => eprintln!("[brainjar mcp] FTS error for KB {}: {}", name, e),
                    }
                }
                all.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
                all.truncate(limit);
                all
            } else {
                Vec::new()
            };

            // Local fuzzy results
            let local_results: Vec<crate::local_search::LocalSearchResult> = if run_local {
                match crate::local_search::run_local_search(config, query, limit, exact) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("[brainjar mcp] Local search error: {}", e);
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };

            // Graph results
            let graph_results: Vec<crate::graph::GraphSearchResult> = if run_graph {
                let mut all = Vec::new();
                for (name, _kb_config) in &kbs {
                    if !crate::graph::KnowledgeGraph::exists(&config.effective_db_dir(), name) {
                        continue;
                    }
                    match crate::graph::KnowledgeGraph::open(&config.effective_db_dir(), name) {
                        Ok(kg) => match kg.search(query, limit) {
                            Ok(results) => all.extend(results),
                            Err(e) => eprintln!("[brainjar mcp] Graph search error for KB {}: {}", name, e),
                        },
                        Err(e) => eprintln!("[brainjar mcp] Could not open graph DB for KB {}: {}", name, e),
                    }
                }
                all
            } else {
                Vec::new()
            };

            // Filter excluded chunk IDs from content-bearing results (FTS has chunk_id).
            // Graph results are entity-level (no chunk_id) and stay unfiltered.
            // For JSON output, build_unified_results handles FTS + vector exclusion.
            let fts_results_filtered: Vec<crate::search::FtsResult> = if let Some(excl) = exclude_chunks_ref {
                if excl.is_empty() {
                    fts_results
                } else {
                    fts_results.into_iter().filter(|r| r.chunk_id.is_none_or(|id| !excl.contains(&id))).collect()
                }
            } else {
                fts_results
            };

            let text = format_search_text(query, &fts_results_filtered, &local_results, &graph_results, mode, include_content, doc_score);
            Ok(tool_text(text))
        }

        "memory_sync" => {
            let kb = args.get("kb").and_then(|v| v.as_str());
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

            match crate::sync::run_sync(config, kb, force, false, false, true, false).await {
                Ok(()) => Ok(tool_text("Sync completed successfully".to_string())),
                Err(e) => Ok(tool_error(e.to_string())),
            }
        }

        "memory_status" => {
            let kb = args.get("kb").and_then(|v| v.as_str());

            let kbs: Vec<(&str, &crate::config::KnowledgeBaseConfig)> = if let Some(name) = kb {
                if let Some(k) = config.knowledge_bases.get(name) {
                    vec![(name, k)]
                } else {
                    return Ok(tool_error(format!("KB '{}' not found", name)));
                }
            } else {
                config
                    .knowledge_bases
                    .iter()
                    .map(|(n, k): (&String, _)| (n.as_str(), k))
                    .collect()
            };

            let mut status_lines = Vec::new();
            for (name, _kb_config) in &kbs {
                let conn = match crate::db::open_db(name, &config.effective_db_dir()) {
                    Ok(c) => c,
                    Err(_) => {
                        status_lines.push(format!("{}: DB not initialized (run brainjar sync)", name));
                        continue;
                    }
                };
                let count = crate::db::count_documents(&conn).unwrap_or(0);
                let last_sync = crate::db::get_meta(&conn, "last_sync")
                    .unwrap_or_default()
                    .unwrap_or_else(|| "never".to_string());
                status_lines.push(format!(
                    "{}: {} documents, last sync: {}",
                    name, count, last_sync
                ));
            }

            Ok(tool_text(status_lines.join("\n")))
        }

        "memory_list" => {
            let mut kbs: Vec<(&str, &crate::config::KnowledgeBaseConfig)> = config
                .knowledge_bases
                .iter()
                .map(|(n, k): (&String, _)| (n.as_str(), k))
                .collect();
            kbs.sort_by_key(|(n, _)| *n);

            let mut entries = Vec::new();
            for (name, kb) in &kbs {
                let db_dir = config.effective_db_dir();
                let db_path = db_dir.join(format!("{}.db", name));
                let db_exists = db_path.exists();
                let (doc_count, last_sync) = if db_exists {
                    if let Ok(conn) = crate::db::open_db(name, &config.effective_db_dir()) {
                        let count = crate::db::count_documents(&conn).unwrap_or(0);
                        let sync_time = crate::db::get_meta(&conn, "last_sync")
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

            Ok(tool_text(serde_json::to_string_pretty(&entries)?))
        }

        "retrieve_chunk" => {
            let chunk_id = args
                .get("chunk_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| anyhow::anyhow!("Missing chunk_id argument"))?;
            let lines_before = args.get("lines_before").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let lines_after = args.get("lines_after").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let chunks_before = args.get("chunks_before").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let chunks_after = args.get("chunks_after").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

            // Search all KBs for the chunk
            let mut found_conn: Option<rusqlite::Connection> = None;
            for name in config.knowledge_bases.keys() {
                let conn = match crate::db::open_db(name, &config.effective_db_dir()) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let exists: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM chunks WHERE id = ?1",
                        rusqlite::params![chunk_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                if exists > 0 {
                    found_conn = Some(conn);
                    break;
                }
            }

            let conn = match found_conn {
                Some(c) => c,
                None => return Ok(tool_error(format!("Chunk {} not found", chunk_id))),
            };

            let (_, doc_id, content, line_start, line_end, chunk_type, file_path) =
                match crate::db::get_chunk(&conn, chunk_id) {
                    Ok(r) => r,
                    Err(e) => return Ok(tool_error(e.to_string())),
                };

            // Line context
            let (raw_before, raw_after): (Vec<String>, Vec<String>) =
                if lines_before > 0 || lines_after > 0 {
                    match crate::db::get_document_content(&conn, doc_id) {
                        Ok(doc_content) => {
                            let doc_lines: Vec<&str> = doc_content.lines().collect();
                            let total = doc_lines.len();
                            let before_start = line_start.saturating_sub(lines_before);
                            let rb: Vec<String> = doc_lines[before_start..line_start]
                                .iter().map(|l| l.to_string()).collect();
                            let after_end = (line_end + 1 + lines_after).min(total);
                            let ra: Vec<String> = if line_end + 1 < total {
                                doc_lines[(line_end + 1)..after_end]
                                    .iter().map(|l| l.to_string()).collect()
                            } else {
                                Vec::new()
                            };
                            (rb, ra)
                        }
                        Err(e) => return Ok(tool_error(e.to_string())),
                    }
                } else {
                    (Vec::new(), Vec::new())
                };

            // Neighboring chunks
            let (before_chunks, _, after_chunks) = if chunks_before > 0 || chunks_after > 0 {
                match crate::db::get_neighboring_chunks(&conn, chunk_id, chunks_before, chunks_after) {
                    Ok(r) => r,
                    Err(e) => return Ok(tool_error(e.to_string())),
                }
            } else {
                (Vec::new(), crate::db::ChunkRow {
                    chunk_id,
                    doc_id,
                    content: content.clone(),
                    line_start,
                    line_end,
                    chunk_type: chunk_type.clone(),
                }, Vec::new())
            };

            let mut text = format!("─── {}:{}-{} ({}) ───\n", file_path, line_start, line_end, chunk_type);

            for c in &before_chunks {
                text.push_str(&format!("[prev] lines {}-{}:\n{}\n\n", c.line_start, c.line_end, c.content));
            }
            if !raw_before.is_empty() {
                text.push_str(&format!("[context before]:\n{}\n\n", raw_before.join("\n")));
            }
            text.push_str(&format!("[match]:\n{}\n", content));
            if !raw_after.is_empty() {
                text.push_str(&format!("\n[context after]:\n{}\n", raw_after.join("\n")));
            }
            for c in &after_chunks {
                text.push_str(&format!("\n[next] lines {}-{}:\n{}\n", c.line_start, c.line_end, c.content));
            }

            Ok(tool_text(text))
        }

        name => Ok(tool_error(format!("Unknown tool: {}", name))),
    }
}

fn tool_text(text: String) -> Value {
    serde_json::json!({
        "content": [{"type": "text", "text": text}]
    })
}

fn tool_error(message: String) -> Value {
    serde_json::json!({
        "content": [{"type": "text", "text": message}],
        "isError": true
    })
}

fn format_search_text(
    query: &str,
    fts: &[crate::search::FtsResult],
    local: &[crate::local_search::LocalSearchResult],
    graph: &[crate::graph::GraphSearchResult],
    mode: crate::search::SearchMode,
    include_content: bool,
    doc_score: bool,
) -> String {
    if fts.is_empty() && local.is_empty() && graph.is_empty() {
        return format!("No results found for \"{}\"", query);
    }

    let mut out = format!("Results for \"{}\"\n\n", query);

    if mode.run_fts() {
        out.push_str("=== FTS5 (text search) ===\n");
        if fts.is_empty() {
            out.push_str("No text results\n\n");
        } else {
            // Optionally aggregate by document
            let display_fts: Vec<(usize, &crate::search::FtsResult)> = if doc_score {
                // Keep only the best-scoring chunk per path
                let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
                let mut deduped: Vec<&crate::search::FtsResult> = Vec::new();
                for r in fts {
                    let entry = seen.entry(r.path.as_str()).or_insert(deduped.len());
                    if *entry == deduped.len() {
                        deduped.push(r);
                    } else if r.score > deduped[*entry].score {
                        deduped[*entry] = r;
                    }
                }
                deduped.into_iter().enumerate().collect()
            } else {
                fts.iter().enumerate().collect()
            };
            for (i, r) in display_fts {
                let body = if include_content {
                    r.content.as_deref().unwrap_or(r.excerpt.as_str()).to_string()
                } else {
                    format!("...{}...", r.excerpt.replace('\n', " "))
                };
                let chunk_info = match (r.chunk_id, r.line_start, r.line_end) {
                    (Some(cid), Some(ls), Some(le)) => format!(" [chunk:{} L{}-{}]", cid, ls, le),
                    _ => String::new(),
                };
                out.push_str(&format!(
                    "{}. [{:.4}] {}{}\n   {}\n\n",
                    i + 1,
                    r.score,
                    r.path,
                    chunk_info,
                    body,
                ));
            }
        }
    }

    if mode.run_local() {
        out.push_str("=== Local (fuzzy) ===\n");
        if local.is_empty() {
            out.push_str("No local results\n\n");
        } else {
            for (i, r) in local.iter().enumerate() {
                out.push_str(&format!(
                    "{}. [{:.2}] {}:{}\n   {}\n\n",
                    i + 1,
                    r.score,
                    r.file,
                    r.line,
                    r.matched_text
                ));
            }
        }
    }

    if mode.run_graph() {
        out.push_str("=== Graph (entity search) ===\n");
        if graph.is_empty() {
            out.push_str("No graph results\n\n");
        } else {
            for (i, r) in graph.iter().enumerate() {
                let related = if r.related_entities.is_empty() {
                    String::new()
                } else {
                    format!(" → related: {}", r.related_entities.join(", "))
                };
                out.push_str(&format!(
                    "{}. {} (entity: {} [{}]{})\n\n",
                    i + 1,
                    r.file,
                    r.entity,
                    r.entity_type,
                    related
                ));
            }
        }
    }

    out
}
