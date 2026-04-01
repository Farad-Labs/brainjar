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
                "description": "Search across knowledge bases for relevant memories and context. Runs both remote Bedrock hybrid search and local fuzzy file search by default.",
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
                            "enum": ["all", "local", "remote"],
                            "description": "Search mode: 'all' runs both remote (Bedrock) and local fuzzy search (default), 'local' runs only local file search, 'remote' runs only Bedrock search",
                            "default": "all"
                        },
                        "exact": {
                            "type": "boolean",
                            "description": "Use exact (case-insensitive substring) matching for local search instead of fuzzy matching",
                            "default": false
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "memory_sync",
                "description": "Sync files to S3 and trigger Bedrock ingestion",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "kb": {
                            "type": "string",
                            "description": "Knowledge base name (optional, syncs all auto_sync KBs if omitted)"
                        },
                        "force": {
                            "type": "boolean",
                            "description": "Force re-upload of all files",
                            "default": false
                        }
                    }
                }
            },
            {
                "name": "memory_status",
                "description": "Get knowledge base status including last sync time and file count",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "kb": {
                            "type": "string",
                            "description": "Knowledge base name (optional, shows all if omitted)"
                        }
                    }
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
                "remote" => crate::search::SearchMode::Remote,
                _ => crate::search::SearchMode::All,
            };

            let run_remote = matches!(mode, crate::search::SearchMode::All | crate::search::SearchMode::Remote);
            let run_local = matches!(mode, crate::search::SearchMode::All | crate::search::SearchMode::Local);

            // Run both searches in parallel
            let remote_future = async {
                if !run_remote {
                    return Ok::<Vec<crate::search::SearchResult>, anyhow::Error>(Vec::new());
                }
                let clients = crate::aws::build_clients(&config.aws).await?;
                let state = crate::state::State::load(&config.config_dir)?;
                let mut all_results: Vec<crate::search::SearchResult> = Vec::new();

                let kbs: Vec<(&str, &crate::config::KnowledgeBaseConfig)> = if let Some(name) = kb {
                    if let Some(k) = config.knowledge_bases.get(name) {
                        vec![(name, k)]
                    } else {
                        return Err(anyhow::anyhow!("KB '{}' not found", name));
                    }
                } else {
                    config
                        .knowledge_bases
                        .iter()
                        .map(|(n, k)| (n.as_str(), k))
                        .collect()
                };

                for (name, kb_config) in &kbs {
                    let kb_state = state.kb_state(name);
                    match crate::search::search_kb_raw(&clients, name, kb_config, query, limit, &kb_state).await {
                        Ok(results) => all_results.extend(results),
                        Err(e) => eprintln!("[brainjar mcp] Search error for KB {}: {}", name, e),
                    }
                }

                all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
                all_results.truncate(limit);

                // Reverse-map S3 keys to human-readable paths
                for result in &mut all_results {
                    let kb_state = state.kb_state(&result.kb);
                    let reverse_map = crate::search::build_s3_key_to_path_map(&kb_state);
                    if let Some(original_path) = reverse_map.get(&result.source_path) {
                        result.source_path = original_path.clone();
                    }
                }

                Ok(all_results)
            };

            let local_future = async {
                if run_local {
                    crate::local_search::run_local_search(config, query, limit, exact)
                } else {
                    Ok(Vec::new())
                }
            };

            let (remote_results, local_results) = tokio::join!(remote_future, local_future);
            let remote_results = match remote_results {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[brainjar mcp] Remote search error: {}", e);
                    Vec::new()
                }
            };
            let local_results = match local_results {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[brainjar mcp] Local search error: {}", e);
                    Vec::new()
                }
            };

            let text = format_unified_search_text(query, &remote_results, &local_results, mode);
            Ok(tool_text(text))
        }

        "memory_sync" => {
            let kb = args.get("kb").and_then(|v| v.as_str());
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

            match crate::sync::run_sync(config, kb, force, false, true, false).await {
                Ok(()) => Ok(tool_text("Sync triggered successfully".to_string())),
                Err(e) => Ok(tool_error(e.to_string())),
            }
        }

        "memory_status" => {
            let kb = args.get("kb").and_then(|v| v.as_str());
            let state = crate::state::State::load(&config.config_dir)?;

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
                    .map(|(n, k)| (n.as_str(), k))
                    .collect()
            };

            let mut status_lines = Vec::new();
            for (name, _kb_config) in &kbs {
                let kb_state = state.kb_state(name);
                status_lines.push(format!(
                    "{}: {} files, last sync: {}, last ingestion: {}",
                    name,
                    kb_state.files.len(),
                    kb_state.last_sync.map(|t| t.to_rfc3339()).unwrap_or_else(|| "never".to_string()),
                    kb_state.last_ingestion_status.unwrap_or_else(|| "unknown".to_string()),
                ));
            }

            Ok(tool_text(status_lines.join("\n")))
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

fn format_unified_search_text(
    query: &str,
    remote: &[crate::search::SearchResult],
    local: &[crate::local_search::LocalSearchResult],
    mode: crate::search::SearchMode,
) -> String {
    if remote.is_empty() && local.is_empty() {
        return format!("No results found for \"{}\"", query);
    }

    let mut out = format!("Results for \"{}\"\n\n", query);

    if matches!(mode, crate::search::SearchMode::All | crate::search::SearchMode::Remote) {
        out.push_str("=== Remote (Bedrock) ===\n");
        if remote.is_empty() {
            out.push_str("No remote results\n\n");
        } else {
            for (i, r) in remote.iter().enumerate() {
                out.push_str(&format!(
                    "{}. [{:.2}] {}\n   ...{}...\n\n",
                    i + 1,
                    r.score,
                    r.source_path,
                    r.excerpt.replace('\n', " ")
                ));
            }
        }
    }

    if matches!(mode, crate::search::SearchMode::All | crate::search::SearchMode::Local) {
        out.push_str("=== Local (files) ===\n");
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

    out
}
