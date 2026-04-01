use anyhow::{Context, Result};
use aws_sdk_bedrockagentruntime::types::{KnowledgeBaseRetrievalConfiguration, KnowledgeBaseVectorSearchConfiguration};
use colored::Colorize;

use crate::aws::build_clients;
use crate::config::{Config, KnowledgeBaseConfig};

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
    let mut all_results: Vec<SearchResult> = Vec::new();

    for (name, kb) in &kbs {
        let results = search_kb(&clients, name, kb, query, limit).await?;
        all_results.extend(results);
    }

    // Sort by score descending
    all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    all_results.truncate(limit);

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
) -> Result<Vec<SearchResult>> {
    search_kb(clients, kb_name, kb, query, limit).await
}

async fn search_kb(
    clients: &crate::aws::AwsClients,
    kb_name: &str,
    kb: &KnowledgeBaseConfig,
    query: &str,
    limit: usize,
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

        // Extract source path from metadata
        let source_path = item
            .metadata()
            .and_then(|m| m.get("brainjar-source-path"))
            .and_then(|v| v.as_string())
            .unwrap_or("unknown")
            .to_string();

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
