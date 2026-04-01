use anyhow::{Context, Result};
use colored::Colorize;

use crate::aws::build_clients;
use crate::config::{Config, KnowledgeBaseConfig};
use crate::state::State;

pub async fn run_status(config: &Config, kb_name: Option<&str>, json: bool) -> Result<()> {
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

    let mut all_statuses = Vec::new();

    for (name, kb) in &kbs {
        let kb_state = state.kb_state(name);

        // Try to fetch live KB info from Bedrock
        let kb_info = clients
            .bedrock_agent
            .get_knowledge_base()
            .knowledge_base_id(&kb.kb_id)
            .send()
            .await
            .ok();

        let ds_info = clients
            .bedrock_agent
            .get_data_source()
            .knowledge_base_id(&kb.kb_id)
            .data_source_id(&kb.data_source_id)
            .send()
            .await
            .ok();

        let kb_status = kb_info
            .as_ref()
            .and_then(|r| r.knowledge_base())
            .map(|kb| format!("{:?}", kb.status()))
            .unwrap_or_else(|| "UNKNOWN".to_string());

        let ds_status = ds_info
            .as_ref()
            .and_then(|r| r.data_source())
            .map(|ds| format!("{:?}", ds.status()))
            .unwrap_or_else(|| "UNKNOWN".to_string());

        let file_count = kb_state.files.len();
        let last_sync = kb_state
            .last_sync
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "Never".to_string());
        let last_ingestion = kb_state
            .last_ingestion_status
            .clone()
            .unwrap_or_else(|| "Never".to_string());

        if json {
            all_statuses.push(serde_json::json!({
                "name": name,
                "kb_id": kb.kb_id,
                "data_source_id": kb.data_source_id,
                "s3_bucket": kb.s3_bucket,
                "kb_status": kb_status,
                "ds_status": ds_status,
                "tracked_files": file_count,
                "last_sync": last_sync,
                "last_ingestion": last_ingestion,
                "auto_sync": kb.auto_sync,
            }));
        } else {
            print_kb_status(name, kb, &kb_status, &ds_status, file_count, &last_sync, &last_ingestion);
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&all_statuses)?);
    }

    Ok(())
}

fn print_kb_status(
    name: &str,
    kb: &KnowledgeBaseConfig,
    kb_status: &str,
    ds_status: &str,
    file_count: usize,
    last_sync: &str,
    last_ingestion: &str,
) {
    println!("\n{} {}", "📦".cyan(), name.bold().white());
    println!("  {:<20} {}", "KB ID:".dimmed(), kb.kb_id.cyan());
    println!("  {:<20} {}", "Data Source ID:".dimmed(), kb.data_source_id.cyan());
    println!("  {:<20} s3://{}", "S3 Bucket:".dimmed(), kb.s3_bucket.cyan());
    println!(
        "  {:<20} {}",
        "KB Status:".dimmed(),
        colorize_status(kb_status)
    );
    println!(
        "  {:<20} {}",
        "DS Status:".dimmed(),
        colorize_status(ds_status)
    );
    println!(
        "  {:<20} {}",
        "Tracked Files:".dimmed(),
        file_count.to_string().cyan()
    );
    println!(
        "  {:<20} {}",
        "Last Sync:".dimmed(),
        last_sync.dimmed()
    );
    println!(
        "  {:<20} {}",
        "Last Ingestion:".dimmed(),
        colorize_status(last_ingestion)
    );
    println!(
        "  {:<20} {}",
        "Auto Sync:".dimmed(),
        if kb.auto_sync {
            "yes".green().to_string()
        } else {
            "no".dimmed().to_string()
        }
    );
}

fn colorize_status(status: &str) -> colored::ColoredString {
    match status {
        s if s.contains("ACTIVE") || s.contains("COMPLETE") || s.contains("AVAILABLE") => {
            s.green()
        }
        s if s.contains("FAIL") || s.contains("ERROR") || s.contains("DELETE") => s.red(),
        s if s.contains("PROGRESS") || s.contains("STARTING") || s.contains("CREATING") => {
            s.yellow()
        }
        s => s.dimmed(),
    }
}
