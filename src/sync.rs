use anyhow::{Context, Result};
use aws_sdk_bedrockagent::types::IngestionJobStatus;
use chrono::Utc;
use colored::Colorize;
use hex;
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use walkdir::WalkDir;

use crate::aws::build_clients;
use crate::config::{Config, KnowledgeBaseConfig};
use crate::state::{FileState, State};

pub async fn run_sync(
    config: &Config,
    kb_name: Option<&str>,
    force: bool,
    dry_run: bool,
    no_wait: bool,
    json: bool,
) -> Result<()> {
    let kbs_to_sync: Vec<(&str, &KnowledgeBaseConfig)> = if let Some(name) = kb_name {
        let kb = config
            .knowledge_bases
            .get(name)
            .with_context(|| format!("Knowledge base '{}' not found in config", name))?;
        vec![(name, kb)]
    } else {
        config
            .knowledge_bases
            .iter()
            .filter(|(_, kb)| kb.auto_sync)
            .map(|(name, kb)| (name.as_str(), kb))
            .collect()
    };

    if kbs_to_sync.is_empty() {
        if !json {
            println!("{}", "No knowledge bases to sync (none have auto_sync = true)".yellow());
            println!("Specify a KB name explicitly: brainjar sync <kb_name>");
        }
        return Ok(());
    }

    let mut state = State::load(&config.config_dir)?;

    // Pre-check: skip AWS entirely if nothing changed across all KBs
    if !force {
        let mut any_changes = false;
        for (name, kb) in &kbs_to_sync {
            let kb_state = state.kb_state(name);
            let watch_paths = config.expand_watch_paths(kb);
            let local_files = collect_files(config, &watch_paths);
            let changes = compute_changes(&local_files, &kb_state, false);
            if !changes.to_upload.is_empty() || !changes.to_delete.is_empty() {
                any_changes = true;
                break;
            }
        }
        if !any_changes {
            if json {
                for (name, _) in &kbs_to_sync {
                    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                        "kb": name,
                        "to_upload": 0,
                        "to_delete": 0,
                        "status": "NO_CHANGES"
                    }))?);
                }
            } else {
                for (name, _) in &kbs_to_sync {
                    println!("\n{} {}", "⟳ Syncing".cyan().bold(), name.bold());
                    println!("  {} Nothing to sync", "✓".green());
                }
            }
            return Ok(());
        }
    }

    let clients = build_clients(&config.aws).await?;

    for (name, kb) in &kbs_to_sync {
        if json {
            sync_kb_json(config, name, kb, &clients, &mut state, force, dry_run, no_wait).await?;
        } else {
            sync_kb_human(config, name, kb, &clients, &mut state, force, dry_run, no_wait).await?;
        }
    }

    if !dry_run {
        state.save(&config.config_dir)?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn sync_kb_human(
    config: &Config,
    kb_name: &str,
    kb: &KnowledgeBaseConfig,
    clients: &crate::aws::AwsClients,
    state: &mut State,
    force: bool,
    dry_run: bool,
    no_wait: bool,
) -> Result<()> {
    println!("\n{} {}", "⟳ Syncing".cyan().bold(), kb_name.bold());

    let mut kb_state = state.kb_state(kb_name);
    let watch_paths = config.expand_watch_paths(kb);

    // Collect all local files
    let local_files = collect_files(config, &watch_paths);

    // Determine changes
    let changes = compute_changes(&local_files, &kb_state, force);

    if changes.to_upload.is_empty() && changes.to_delete.is_empty() {
        println!("  {} Nothing to sync", "✓".green());
        return Ok(());
    }

    println!(
        "  {} files to upload, {} to delete",
        changes.to_upload.len().to_string().cyan(),
        changes.to_delete.len().to_string().yellow()
    );

    if dry_run {
        println!("  {} (dry run, no changes made)", "DRY RUN".yellow().bold());
        for rel_path in changes.to_upload.keys() {
            println!("    {} {}", "+".green(), rel_path);
        }
        for s3_key in &changes.to_delete {
            println!("    {} {}", "-".red(), s3_key);
        }
        return Ok(());
    }

    // Upload files
    if !changes.to_upload.is_empty() {
        let pb = ProgressBar::new(changes.to_upload.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("  [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("=>-"),
        );

        for (rel_path, (abs_path, s3_key)) in &changes.to_upload {
            pb.set_message(rel_path.clone());
            let content = std::fs::read(abs_path)
                .with_context(|| format!("Failed to read file: {}", abs_path.display()))?;
            let content_hash = hash_file_content(&content);

            clients
                .s3
                .put_object()
                .bucket(&kb.s3_bucket)
                .key(s3_key)
                .body(content.into())
                .metadata("brainjar-source-path", rel_path)
                .content_type("text/markdown")
                .send()
                .await
                .with_context(|| format!("Failed to upload {} to s3://{}/{}", rel_path, kb.s3_bucket, s3_key))?;

            kb_state.files.insert(
                rel_path.clone(),
                FileState {
                    content_hash,
                    s3_key: s3_key.clone(),
                    last_modified: Utc::now(),
                },
            );
            pb.inc(1);
        }
        pb.finish_and_clear();
        println!("  {} Uploaded {} files", "✓".green(), changes.to_upload.len());
    }

    // Delete files
    for s3_key in &changes.to_delete {
        clients
            .s3
            .delete_object()
            .bucket(&kb.s3_bucket)
            .key(s3_key)
            .send()
            .await
            .with_context(|| format!("Failed to delete s3://{}/{}", kb.s3_bucket, s3_key))?;
    }

    // Remove deleted files from state
    kb_state.files.retain(|_, f| !changes.to_delete.contains(&f.s3_key));

    if !changes.to_delete.is_empty() {
        println!("  {} Deleted {} files from S3", "✓".green(), changes.to_delete.len());
    }

    // Trigger ingestion
    println!("  {} Triggering Bedrock ingestion...", "⟳".cyan());
    let job = clients
        .bedrock_agent
        .start_ingestion_job()
        .knowledge_base_id(&kb.kb_id)
        .data_source_id(&kb.data_source_id)
        .send()
        .await
        .with_context(|| format!("Failed to start ingestion job for KB '{}' ({}). Check IAM permissions.", kb_name, kb.kb_id))?;

    let job = job.ingestion_job().context("No ingestion job in response")?;
    let job_id = job.ingestion_job_id().to_string();

    kb_state.last_ingestion_job_id = Some(job_id.clone());
    kb_state.last_sync = Some(Utc::now());

    if no_wait {
        println!("  {} Ingestion started (job: {})", "✓".green(), job_id.dimmed());
        println!("  {} Run `brainjar status {}` to check progress", "ℹ".blue(), kb_name);
        kb_state.last_ingestion_status = Some("IN_PROGRESS".to_string());
        state.set_kb_state(kb_name, kb_state);
        return Ok(());
    }

    // Poll for completion
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("  {spinner:.cyan} {msg}")
            .unwrap(),
    );
    pb.set_message("Waiting for ingestion...");

    let mut delay_ms = 3000u64;
    loop {
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

        let result = clients
            .bedrock_agent
            .get_ingestion_job()
            .knowledge_base_id(&kb.kb_id)
            .data_source_id(&kb.data_source_id)
            .ingestion_job_id(&job_id)
            .send()
            .await
            .context("Failed to poll ingestion job status")?;

        let job = result.ingestion_job().context("No ingestion job in response")?;
        let status = job.status();
        let status_str = format!("{:?}", status);
        pb.set_message(format!("Ingestion status: {}", status_str.cyan()));

        match status {
            IngestionJobStatus::Complete => {
                pb.finish_and_clear();
                println!("  {} Ingestion complete!", "✓".green().bold());
                kb_state.last_ingestion_status = Some("COMPLETE".to_string());
                break;
            }
            IngestionJobStatus::Failed => {
                pb.finish_and_clear();
                let failures = job.failure_reasons();
                eprintln!("  {} Ingestion failed!", "✗".red().bold());
                for reason in failures {
                    eprintln!("    {}", reason.red());
                }
                eprintln!("\n  Check IAM permissions for the Bedrock service role.");
                kb_state.last_ingestion_status = Some("FAILED".to_string());
                break;
            }
            _ => {
                // Still running — back off
                delay_ms = (delay_ms * 2).min(30_000);
            }
        }
    }

    state.set_kb_state(kb_name, kb_state);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn sync_kb_json(
    config: &Config,
    kb_name: &str,
    kb: &KnowledgeBaseConfig,
    clients: &crate::aws::AwsClients,
    state: &mut State,
    force: bool,
    dry_run: bool,
    no_wait: bool,
) -> Result<()> {
    // Simplified: just do the work and emit a JSON result at the end
    // For brevity, we call the same logic with silenced output
    let mut kb_state = state.kb_state(kb_name);
    let watch_paths = config.expand_watch_paths(kb);
    let local_files = collect_files(config, &watch_paths);
    let changes = compute_changes(&local_files, &kb_state, force);

    let mut result = serde_json::json!({
        "kb": kb_name,
        "to_upload": changes.to_upload.len(),
        "to_delete": changes.to_delete.len(),
        "dry_run": dry_run,
    });

    if !dry_run {
        for (rel_path, (abs_path, s3_key)) in &changes.to_upload {
            let content = std::fs::read(abs_path)?;
            let content_hash = hash_file_content(&content);
            clients
                .s3
                .put_object()
                .bucket(&kb.s3_bucket)
                .key(s3_key)
                .body(content.into())
                .metadata("brainjar-source-path", rel_path)
                .content_type("text/markdown")
                .send()
                .await?;
            kb_state.files.insert(
                rel_path.clone(),
                FileState {
                    content_hash,
                    s3_key: s3_key.clone(),
                    last_modified: Utc::now(),
                },
            );
        }
        for s3_key in &changes.to_delete {
            clients.s3.delete_object().bucket(&kb.s3_bucket).key(s3_key).send().await?;
        }
        kb_state.files.retain(|_, f| !changes.to_delete.contains(&f.s3_key));

        let job = clients
            .bedrock_agent
            .start_ingestion_job()
            .knowledge_base_id(&kb.kb_id)
            .data_source_id(&kb.data_source_id)
            .send()
            .await?;
        let job = job.ingestion_job().context("No ingestion job in response")?;
        let job_id = job.ingestion_job_id().to_string();
        kb_state.last_ingestion_job_id = Some(job_id.clone());
        kb_state.last_sync = Some(Utc::now());

        result["job_id"] = serde_json::Value::String(job_id.clone());

        if !no_wait {
            let mut delay_ms = 3000u64;
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                let r = clients
                    .bedrock_agent
                    .get_ingestion_job()
                    .knowledge_base_id(&kb.kb_id)
                    .data_source_id(&kb.data_source_id)
                    .ingestion_job_id(&job_id)
                    .send()
                    .await?;
                let j = r.ingestion_job().context("No ingestion job")?;
                match j.status() {
                    IngestionJobStatus::Complete => {
                        result["status"] = serde_json::Value::String("COMPLETE".to_string());
                        kb_state.last_ingestion_status = Some("COMPLETE".to_string());
                        break;
                    }
                    IngestionJobStatus::Failed => {
                        result["status"] = serde_json::Value::String("FAILED".to_string());
                        kb_state.last_ingestion_status = Some("FAILED".to_string());
                        break;
                    }
                    _ => {
                        delay_ms = (delay_ms * 2).min(30_000);
                    }
                }
            }
        } else {
            result["status"] = serde_json::Value::String("IN_PROGRESS".to_string());
            kb_state.last_ingestion_status = Some("IN_PROGRESS".to_string());
        }

        state.set_kb_state(kb_name, kb_state);
    }

    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

/// Collect all files from watch paths, returning (relative_path, absolute_path) pairs
pub fn collect_files(
    config: &crate::config::Config,
    watch_paths: &[std::path::PathBuf],
) -> HashMap<String, std::path::PathBuf> {
    let mut files = HashMap::new();
    for watch_path in watch_paths {
        if watch_path.is_file() {
            let rel = watch_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            files.insert(rel, watch_path.clone());
        } else if watch_path.is_dir() {
            for entry in WalkDir::new(watch_path)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
            {
                // Try to make relative to config_dir for stable keys
                let abs = entry.path().to_path_buf();
                let rel = if let Ok(r) = abs.strip_prefix(&config.config_dir) {
                    r.to_string_lossy().to_string()
                } else {
                    abs.to_string_lossy().to_string()
                };
                files.insert(rel, abs);
            }
        }
        // Glob patterns
        else {
            let pattern = watch_path.to_string_lossy();
            if let Ok(paths) = glob::glob(&pattern) {
                for path in paths.filter_map(|p| p.ok()) {
                    if path.is_file() {
                        let rel = if let Ok(r) = path.strip_prefix(&config.config_dir) {
                            r.to_string_lossy().to_string()
                        } else {
                            path.to_string_lossy().to_string()
                        };
                        files.insert(rel, path);
                    }
                }
            }
        }
    }
    files
}

struct SyncChanges {
    to_upload: HashMap<String, (std::path::PathBuf, String)>, // rel_path -> (abs_path, s3_key)
    to_delete: HashSet<String>,                                 // s3_keys
}

fn compute_changes(
    local_files: &HashMap<String, std::path::PathBuf>,
    kb_state: &crate::state::KbState,
    force: bool,
) -> SyncChanges {
    let mut to_upload = HashMap::new();
    let mut to_delete = HashSet::new();

    for (rel_path, abs_path) in local_files {
        let s3_key = path_to_s3_key(rel_path);
        let needs_upload = if force {
            true
        } else if let Some(file_state) = kb_state.files.get(rel_path) {
            // Check if content changed
            if let Ok(content) = std::fs::read(abs_path) {
                hash_file_content(&content) != file_state.content_hash
            } else {
                true
            }
        } else {
            true // New file
        };

        if needs_upload {
            to_upload.insert(rel_path.clone(), (abs_path.clone(), s3_key));
        }
    }

    // Find deleted files
    for (rel_path, file_state) in &kb_state.files {
        if !local_files.contains_key(rel_path) {
            to_delete.insert(file_state.s3_key.clone());
        }
    }

    SyncChanges { to_upload, to_delete }
}

/// SHA256 of the relative path → used as stable S3 key
pub fn path_to_s3_key(rel_path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(rel_path.as_bytes());
    let hash = hasher.finalize();
    format!("{}.md", hex::encode(hash))
}

/// SHA256 of file content → used to detect changes
pub fn hash_file_content(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}
