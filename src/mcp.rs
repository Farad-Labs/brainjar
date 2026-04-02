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
                        }
                    },
                    "required": ["query"]
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
                "local" => crate::search::SearchMode::Local,
                "text" => crate::search::SearchMode::Text,
                "graph" => crate::search::SearchMode::Graph,
                "fuzzy" => crate::search::SearchMode::Fuzzy,
                "vector" => crate::search::SearchMode::Vector,
                _ => crate::search::SearchMode::All,
            };

            let run_fts = matches!(mode, crate::search::SearchMode::All | crate::search::SearchMode::Text | crate::search::SearchMode::Fuzzy);
            let run_local = matches!(mode, crate::search::SearchMode::Local | crate::search::SearchMode::Fuzzy);
            let run_graph = matches!(mode, crate::search::SearchMode::All | crate::search::SearchMode::Graph | crate::search::SearchMode::Fuzzy);

            // Determine KBs to search
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

            let text = format_search_text(query, &fts_results, &local_results, &graph_results, mode);
            Ok(tool_text(text))
        }

        "memory_sync" => {
            let kb = args.get("kb").and_then(|v| v.as_str());
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

            match crate::sync::run_sync(config, kb, force, false, false, false).await {
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
                    "watch_paths": kb.watch_paths,
                    "auto_sync": kb.auto_sync,
                    "document_count": doc_count,
                    "last_sync": last_sync,
                    "db_exists": db_exists,
                }));
            }

            Ok(tool_text(serde_json::to_string_pretty(&entries)?))
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
) -> String {
    if fts.is_empty() && local.is_empty() && graph.is_empty() {
        return format!("No results found for \"{}\"", query);
    }

    let mut out = format!("Results for \"{}\"\n\n", query);

    if matches!(mode, crate::search::SearchMode::All | crate::search::SearchMode::Text) {
        out.push_str("=== FTS5 (text search) ===\n");
        if fts.is_empty() {
            out.push_str("No text results\n\n");
        } else {
            for (i, r) in fts.iter().enumerate() {
                out.push_str(&format!(
                    "{}. [{:.4}] {}\n   ...{}...\n\n",
                    i + 1,
                    r.score,
                    r.path,
                    r.excerpt.replace('\n', " ")
                ));
            }
        }
    }

    if matches!(mode, crate::search::SearchMode::All | crate::search::SearchMode::Local) {
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

    if matches!(mode, crate::search::SearchMode::All | crate::search::SearchMode::Graph) {
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
