use anyhow::{Context, Result};
use aws_sdk_bedrockagentruntime::types::{KnowledgeBaseRetrievalConfiguration, KnowledgeBaseVectorSearchConfiguration, KnowledgeBaseRetrievalResult};
use colored::Colorize;

use crate::aws::build_clients;
use crate::config::{Config, KnowledgeBaseConfig};
use crate::state::State;

#[derive(Debug, serde::Serialize)]
pub struct SearchResult {
    pub kb: String,
    pub score: f64,
    pub source_path: String,
    pub excerpt: String,
}

pub async fn run_search(
    config: &Config,
    query: &str,
    kb_name: Option<&str>,
    limit: usize,
    json: bool,
) -> Result<()> {
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

    // Sort by score descending
    all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    all_results.truncate(limit);

    // Reverse-map S3 keys to human-readable paths using state
    for result in &mut all_results {
        let kb_state = state.kb_state(&result.kb);
        let reverse_map = build_s3_key_to_path_map(&kb_state);
        if let Some(original_path) = reverse_map.get(&result.source_path) {
            result.source_path = original_path.clone();
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&all_results)?);
    } else {
        print_results(query, &all_results);
    }

    Ok(())
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

        // Extract source path from location URI or metadata
        let source_path = extract_source_path(item, kb);

        // Trim excerpt
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

/// Extract the original source file path from a Bedrock retrieval result.
/// Strategy:
/// 1. Get S3 key from the location URI
/// 2. Reverse-map S3 key → original path using local state (s3_key → rel_path)
/// 3. Fall back to the raw S3 key
fn extract_source_path(item: &KnowledgeBaseRetrievalResult, kb: &KnowledgeBaseConfig) -> String {
    // Get S3 key from the location URI
    let s3_key: Option<String> = item
        .location()
        .and_then(|loc| loc.s3_location())
        .and_then(|s3| s3.uri().map(|u| u.to_string()))
        .and_then(|uri: String| {
            // URI format: s3://bucket/key.md
            let prefix = format!("s3://{}/", kb.s3_bucket);
            uri.strip_prefix(&prefix).map(|s| s.to_string())
        });

    // Try custom metadata (Bedrock sometimes passes S3 object metadata)
    if let Some(path) = item
        .metadata()
        .and_then(|m| m.get("x-amz-meta-brainjar-source-path"))
        .and_then(|v| v.as_string())
    {
        return path.to_string();
    }

    // Fall back to raw S3 key
    s3_key.unwrap_or_else(|| "unknown".to_string())
}

/// Build a reverse lookup map: s3_key → original relative path
/// from the brainjar state file
pub fn build_s3_key_to_path_map(kb_state: &crate::state::KbState) -> std::collections::HashMap<String, String> {
    kb_state
        .files
        .iter()
        .map(|(rel_path, file_state)| (file_state.s3_key.clone(), rel_path.clone()))
        .collect()
}

fn print_results(query: &str, results: &[SearchResult]) {
    if results.is_empty() {
        println!("{}", "🔍 No results found".yellow());
        return;
    }

    println!(
        "\n{} {} ({} {})\n",
        "🔍 Results for".cyan().bold(),
        format!("\"{}\"", query).white().bold(),
        results.len().to_string().cyan(),
        if results.len() == 1 { "match" } else { "matches" }
    );

    for (i, result) in results.iter().enumerate() {
        println!(
            "  {}. {} {}",
            (i + 1).to_string().bold(),
            format!("[{:.2}]", result.score).green(),
            result.source_path.cyan().bold()
        );

        // Wrap excerpt
        let excerpt = result.excerpt.replace('\n', " ");
        println!("     {}", format!("...{}...", excerpt).dimmed());
        println!();
    }
}
