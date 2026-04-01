use anyhow::{Context, Result};
use aws_sdk_bedrockagentruntime::types::{
    KnowledgeBaseRetrievalConfiguration, KnowledgeBaseRetrievalResult,
    KnowledgeBaseVectorSearchConfiguration,
};
use colored::Colorize;

use crate::aws::build_clients;
use crate::config::{Config, KnowledgeBaseConfig};
use crate::local_search::{run_local_search, LocalSearchResult};
use crate::state::State;

/// A remote (Bedrock) search result.
#[derive(Debug, serde::Serialize)]
pub struct SearchResult {
    pub kb: String,
    pub score: f64,
    pub source_path: String,
    pub excerpt: String,
}

/// Combined result from both remote and local searches.
#[derive(Debug, serde::Serialize)]
pub struct UnifiedSearchResult {
    pub remote: Vec<SearchResult>,
    pub local: Vec<LocalSearchResult>,
}

/// Search mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Run both remote (Bedrock hybrid) and local (fuzzy/exact)
    All,
    /// Local only
    Local,
    /// Remote (Bedrock) only
    Remote,
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
    let run_remote = matches!(mode, SearchMode::All | SearchMode::Remote);
    let run_local = matches!(mode, SearchMode::All | SearchMode::Local);

    // Build remote future
    let remote_future = async {
        if run_remote {
            fetch_remote_results(config, query, kb_name, limit).await
        } else {
            Ok(Vec::new())
        }
    };

    // Build local future
    let local_future = async {
        if run_local {
            run_local_search(config, query, limit, exact)
        } else {
            Ok(Vec::new())
        }
    };

    // Run in parallel
    let (remote_results, local_results) = tokio::join!(remote_future, local_future);

    let remote_results = remote_results?;
    let local_results = local_results?;

    let unified = UnifiedSearchResult {
        remote: remote_results,
        local: local_results,
    };

    if json {
        // JSON output: structured combined object
        println!("{}", serde_json::to_string_pretty(&unified)?);
    } else {
        print_unified_results(query, &unified, mode);
    }

    Ok(())
}

async fn fetch_remote_results(
    config: &Config,
    query: &str,
    kb_name: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchResult>> {
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
            .map(|(n, kb)| (n.as_str(), kb))
            .collect()
    };

    let clients = build_clients(&config.aws).await?;
    let state = State::load(&config.config_dir)?;
    let mut all_results: Vec<SearchResult> = Vec::new();

    for (name, kb) in &kbs {
        let kb_state = state.kb_state(name);
        let results = search_kb(&clients, name, kb, query, limit, &kb_state).await?;
        all_results.extend(results);
    }

    // Sort by score descending and truncate
    all_results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    all_results.truncate(limit);

    // Reverse-map S3 keys to human-readable paths using state
    for result in &mut all_results {
        let kb_state = state.kb_state(&result.kb);
        let reverse_map = build_s3_key_to_path_map(&kb_state);
        if let Some(original_path) = reverse_map.get(&result.source_path) {
            result.source_path = original_path.clone();
        }
    }

    Ok(all_results)
}

pub async fn search_kb_raw(
    clients: &crate::aws::AwsClients,
    kb_name: &str,
    kb: &KnowledgeBaseConfig,
    query: &str,
    limit: usize,
    kb_state: &crate::state::KbState,
) -> Result<Vec<SearchResult>> {
    search_kb(clients, kb_name, kb, query, limit, kb_state).await
}

async fn search_kb(
    clients: &crate::aws::AwsClients,
    kb_name: &str,
    kb: &KnowledgeBaseConfig,
    query: &str,
    limit: usize,
    _kb_state: &crate::state::KbState,
) -> Result<Vec<SearchResult>> {
    let retrieval_config = KnowledgeBaseRetrievalConfiguration::builder()
        .vector_search_configuration(
            KnowledgeBaseVectorSearchConfiguration::builder()
                .number_of_results(limit as i32)
                
                .build(),
        )
        .build();

    let response = clients
        .bedrock_runtime
        .retrieve()
        .knowledge_base_id(&kb.kb_id)
        .retrieval_query(
            aws_sdk_bedrockagentruntime::types::KnowledgeBaseQuery::builder()
                .text(query)
                .build(),
        )
        .retrieval_configuration(retrieval_config)
        .send()
        .await
        .with_context(|| {
            format!(
                "Failed to search KB '{}' ({}). Check KB ID and IAM permissions.",
                kb_name, kb.kb_id
            )
        })?;

    let mut results = Vec::new();
    for item in response.retrieval_results() {
        let score = item.score().unwrap_or(0.0);
        let content = item
            .content()
            .map(|c| c.text().to_string())
            .unwrap_or_default();

        let source_path = extract_source_path(item, kb);

        let excerpt = if content.len() > 200 {
            format!("{}...", &content[..200])
        } else {
            content
        };

        results.push(SearchResult {
            kb: kb_name.to_string(),
            score,
            source_path,
            excerpt,
        });
    }

    Ok(results)
}

fn extract_source_path(item: &KnowledgeBaseRetrievalResult, kb: &KnowledgeBaseConfig) -> String {
    let s3_key: Option<String> = item
        .location()
        .and_then(|loc| loc.s3_location())
        .and_then(|s3| s3.uri().map(|u| u.to_string()))
        .and_then(|uri: String| {
            let prefix = format!("s3://{}/", kb.s3_bucket);
            uri.strip_prefix(&prefix).map(|s| s.to_string())
        });

    if let Some(path) = item
        .metadata()
        .and_then(|m| m.get("x-amz-meta-brainjar-source-path"))
        .and_then(|v| v.as_string())
    {
        return path.to_string();
    }

    s3_key.unwrap_or_else(|| "unknown".to_string())
}

pub fn build_s3_key_to_path_map(
    kb_state: &crate::state::KbState,
) -> std::collections::HashMap<String, String> {
    kb_state
        .files
        .iter()
        .map(|(rel_path, file_state)| (file_state.s3_key.clone(), rel_path.clone()))
        .collect()
}

fn print_unified_results(query: &str, unified: &UnifiedSearchResult, mode: SearchMode) {
    let has_remote = !unified.remote.is_empty();
    let has_local = !unified.local.is_empty();

    if !has_remote && !has_local {
        println!("{}", "🔍 No results found".yellow());
        return;
    }

    println!(
        "\n{} {}\n",
        "🔍 Results for".cyan().bold(),
        format!("\"{}\"", query).white().bold(),
    );

    // Remote results
    if matches!(mode, SearchMode::All | SearchMode::Remote) {
        println!("{}", "── Remote (Bedrock) ──────────────────────".dimmed());
        if has_remote {
            print_remote_results(&unified.remote);
        } else {
            println!("  {}\n", "No remote results".dimmed());
        }
    }

    // Local results
    if matches!(mode, SearchMode::All | SearchMode::Local) {
        println!("{}", "── Local (files) ─────────────────────────".dimmed());
        if has_local {
            print_local_results(&unified.local);
        } else {
            println!("  {}\n", "No local results".dimmed());
        }
    }
}

fn print_remote_results(results: &[SearchResult]) {
    for (i, result) in results.iter().enumerate() {
        println!(
            "  {}. {} {}",
            (i + 1).to_string().bold(),
            format!("[{:.2}]", result.score).green(),
            result.source_path.cyan().bold()
        );
        let excerpt = result.excerpt.replace('\n', " ");
        println!("     {}", format!("...{}...", excerpt).dimmed());
        println!();
    }
}

fn print_local_results(results: &[LocalSearchResult]) {
    for (i, result) in results.iter().enumerate() {
        println!(
            "  {}. {} {}{}",
            (i + 1).to_string().bold(),
            format!("[{:.2}]", result.score).green(),
            result.file.cyan().bold(),
            format!(":{}", result.line).dimmed()
        );
        println!("     {}", result.matched_text.dimmed());
        println!();
    }
}
