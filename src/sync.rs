use anyhow::{Context, Result};
use chrono::Utc;
use colored::Colorize;
use hex;
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use glob::Pattern;
use walkdir::WalkDir;

use crate::config::{Config, KnowledgeBaseConfig};
use crate::db;
use crate::extract::Extractor;
use crate::fuzzy;
use crate::graph::KnowledgeGraph;

pub async fn run_sync(
    config: &Config,
    kb_name: Option<&str>,
    force: bool,
    dry_run: bool,
    _no_wait: bool, // no-op: everything is local/instant
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

    for (name, kb) in &kbs_to_sync {
        if json {
            sync_kb_json(config, name, kb, force, dry_run).await?;
        } else {
            sync_kb_human(config, name, kb, force, dry_run).await?;
        }
    }

    Ok(())
}

async fn sync_kb_human(
    config: &Config,
    kb_name: &str,
    kb: &KnowledgeBaseConfig,
    force: bool,
    dry_run: bool,
) -> Result<()> {
    println!("\n{} {}", "⟳ Syncing".cyan().bold(), kb_name.bold());

    let conn = db::open_db(kb_name, &config.config_dir)?;
    let watch_paths = config.expand_watch_paths(kb);
    let local_files = collect_files(config, &watch_paths);
    let changes = compute_changes(&conn, &local_files, force)?;

    if changes.to_upsert.is_empty() && changes.to_delete.is_empty() {
        println!("  {} Nothing to sync", "✓".green());
        return Ok(());
    }

    println!(
        "  {} files to update, {} to delete",
        changes.to_upsert.len().to_string().cyan(),
        changes.to_delete.len().to_string().yellow()
    );

    if dry_run {
        println!("  {} (dry run, no changes made)", "DRY RUN".yellow().bold());
        for rel_path in changes.to_upsert.keys() {
            println!("    {} {}", "+".green(), rel_path);
        }
        for path in &changes.to_delete {
            println!("    {} {}", "-".red(), path);
        }
        return Ok(());
    }

    // Upsert files
    if !changes.to_upsert.is_empty() {
        let pb = ProgressBar::new(changes.to_upsert.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("  [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("=>-"),
        );

        for (rel_path, abs_path) in &changes.to_upsert {
            pb.set_message(rel_path.clone());
            let content = std::fs::read_to_string(abs_path)
                .with_context(|| format!("Failed to read file: {}", abs_path.display()))?;
            let hash = hash_content(content.as_bytes());
            db::upsert_document(&conn, rel_path, &content, &hash)?;
            pb.inc(1);
        }
        pb.finish_and_clear();
        println!("  {} Synced {} files", "✓".green(), changes.to_upsert.len());
    }

    // Delete removed files
    for path in &changes.to_delete {
        db::delete_document(&conn, path)?;
    }

    if !changes.to_delete.is_empty() {
        println!("  {} Removed {} files from index", "✓".green(), changes.to_delete.len());
    }

    // Record last sync time
    db::set_meta(&conn, "last_sync", &Utc::now().to_rfc3339())?;

    // ── Build vocabulary for fuzzy search ────────────────────────────────────
    match fuzzy::build_vocabulary(&conn) {
        Ok(word_count) => {
            println!("  {} Built vocabulary ({} words)", "✓".green(), word_count);
        }
        Err(e) => {
            eprintln!("  ⚠ Vocabulary build failed: {}", e);
        }
    }

    // ── Optional: entity extraction via configured LLM ──────────────────────
    if let Some(extraction_cfg) = &config.extraction {
        if extraction_cfg.enabled && !changes.to_upsert.is_empty() {
            let extractor = Extractor::new(extraction_cfg);
            // Open graph DB (or create it)
            match KnowledgeGraph::open(&config.config_dir, kb_name) {
                Ok(kg) => {
                    let mut total_entities = 0usize;
                    let mut total_rels = 0usize;
                    let mut extraction_errors = 0usize;

                    for (rel_path, abs_path) in &changes.to_upsert {
                        let content = match std::fs::read_to_string(abs_path) {
                            Ok(c) => c,
                            Err(_) => continue,
                        };

                        // Remove stale graph data for this document
                        if let Err(e) = kg.remove_document(rel_path) {
                            eprintln!("  ⚠ Graph remove failed for {}: {}", rel_path, e);
                        }

                        // Extract entities
                        match extractor.extract(&content, rel_path).await {
                            Ok(result) => {
                                total_entities += result.entities.len();
                                total_rels += result.relationships.len();
                                if let Err(e) = kg.ingest_entities(
                                    rel_path,
                                    &result.entities,
                                    &result.relationships,
                                ) {
                                    eprintln!("  ⚠ Graph ingest failed for {}: {}", rel_path, e);
                                    extraction_errors += 1;
                                }
                            }
                            Err(e) => {
                                eprintln!("  ⚠ Extraction failed for {}: {}", rel_path, e);
                                extraction_errors += 1;
                            }
                        }
                    }

                    if extraction_errors == 0 {
                        println!(
                            "  {} Extracted entities ({} entities, {} relationships)",
                            "✓".green(),
                            total_entities,
                            total_rels
                        );
                    } else {
                        println!(
                            "  {} Extracted entities ({} entities, {} relationships, {} errors)",
                            "⚠".yellow(),
                            total_entities,
                            total_rels,
                            extraction_errors
                        );
                    }
                }
                Err(e) => {
                    eprintln!("  ⚠ Could not open graph DB: {}", e);
                }
            }
        }
    }

    // ── TODO: embedding support (sqlite-vec integration, Phase 3) ───────────
    // if let Some(_embed_cfg) = &config.embeddings {
    //     let _embedder = crate::embed::Embedder::new(_embed_cfg);
    //     // embed_batch() content chunks, upsert into documents_vec
    // }

    println!("  {} Done", "✓".green().bold());

    Ok(())
}

async fn sync_kb_json(
    config: &Config,
    kb_name: &str,
    kb: &KnowledgeBaseConfig,
    force: bool,
    dry_run: bool,
) -> Result<()> {
    let conn = db::open_db(kb_name, &config.config_dir)?;
    let watch_paths = config.expand_watch_paths(kb);
    let local_files = collect_files(config, &watch_paths);
    let changes = compute_changes(&conn, &local_files, force)?;

    let mut result = serde_json::json!({
        "kb": kb_name,
        "to_update": changes.to_upsert.len(),
        "to_delete": changes.to_delete.len(),
        "dry_run": dry_run,
    });

    if !dry_run {
        for (rel_path, abs_path) in &changes.to_upsert {
            let content = std::fs::read_to_string(abs_path)?;
            let hash = hash_content(content.as_bytes());
            db::upsert_document(&conn, rel_path, &content, &hash)?;
        }
        for path in &changes.to_delete {
            db::delete_document(&conn, path)?;
        }
        if !changes.to_upsert.is_empty() || !changes.to_delete.is_empty() {
            db::set_meta(&conn, "last_sync", &Utc::now().to_rfc3339())?;
        }

        // Rebuild vocabulary for fuzzy search
        let vocab_count = fuzzy::build_vocabulary(&conn).unwrap_or(0);
        result["vocabulary_words"] = serde_json::Value::Number(vocab_count.into());

        // Optional entity extraction
        let mut entities_extracted = 0usize;
        let mut rels_extracted = 0usize;
        if let Some(extraction_cfg) = &config.extraction {
            if extraction_cfg.enabled && !changes.to_upsert.is_empty() {
                let extractor = Extractor::new(extraction_cfg);
                if let Ok(kg) = KnowledgeGraph::open(&config.config_dir, kb_name) {
                    for (rel_path, abs_path) in &changes.to_upsert {
                        let content = match std::fs::read_to_string(abs_path) {
                            Ok(c) => c,
                            Err(_) => continue,
                        };
                        let _ = kg.remove_document(rel_path);
                        if let Ok(res) = extractor.extract(&content, rel_path).await {
                            entities_extracted += res.entities.len();
                            rels_extracted += res.relationships.len();
                            let _ = kg.ingest_entities(rel_path, &res.entities, &res.relationships);
                        }
                    }
                }
            }
        }

        result["status"] = serde_json::Value::String("COMPLETE".to_string());
        result["entities_extracted"] = serde_json::Value::Number(entities_extracted.into());
        result["relationships_extracted"] = serde_json::Value::Number(rels_extracted.into());
    } else {
        result["status"] = serde_json::Value::String("DRY_RUN".to_string());
    }

    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

struct SyncChanges {
    to_upsert: HashMap<String, std::path::PathBuf>, // rel_path → abs_path
    to_delete: HashSet<String>,                      // rel_paths to remove
}

fn compute_changes(
    conn: &rusqlite::Connection,
    local_files: &HashMap<String, std::path::PathBuf>,
    force: bool,
) -> Result<SyncChanges> {
    let db_hashes = db::get_all_hashes(conn)?;

    let mut to_upsert = HashMap::new();
    let mut to_delete = HashSet::new();

    for (rel_path, abs_path) in local_files {
        let needs_update = if force {
            true
        } else if let Some(db_hash) = db_hashes.get(rel_path) {
            // Check if content changed
            if let Ok(content) = std::fs::read(abs_path) {
                hash_content(&content) != *db_hash
            } else {
                true
            }
        } else {
            true // New file
        };

        if needs_update {
            to_upsert.insert(rel_path.clone(), abs_path.clone());
        }
    }

    // Find deleted files (in DB but no longer on disk)
    for db_path in db_hashes.keys() {
        if !local_files.contains_key(db_path) {
            to_delete.insert(db_path.clone());
        }
    }

    Ok(SyncChanges { to_upsert, to_delete })
}

/// Collect all files from watch paths, returning (relative_path, absolute_path) pairs.
/// Default file extensions to include when no .brainjarignore exists
const DEFAULT_TEXT_EXTENSIONS: &[&str] = &[
    "md", "txt", "rs", "toml", "yaml", "yml", "json", "py", "js", "ts", "tsx", "jsx",
    "sh", "css", "html", "xml", "csv", "sql", "tf", "hcl", "conf", "ini", "cfg", "env",
];

/// Default directories to always skip
const DEFAULT_SKIP_DIRS: &[&str] = &[
    ".git", ".venv", "node_modules", "__pycache__", "target", ".brainjar",
    "dist", "build", ".next", ".nuxt", ".idea", ".vscode",
];

/// Load ignore patterns from .brainjarignore in the config directory
fn load_ignore_patterns(config_dir: &std::path::Path) -> Vec<Pattern> {
    let ignore_file = config_dir.join(".brainjarignore");
    if !ignore_file.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(&ignore_file)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| Pattern::new(l.trim().trim_end_matches('/')).ok())
        .collect()
}

pub fn collect_files(
    config: &Config,
    watch_paths: &[std::path::PathBuf],
) -> HashMap<String, std::path::PathBuf> {
    let ignore_patterns = load_ignore_patterns(&config.config_dir);
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
                .filter_entry(|e| {
                    let name = e.file_name().to_string_lossy();
                    if e.file_type().is_dir() {
                        // Skip default excluded directories
                        !DEFAULT_SKIP_DIRS.contains(&name.as_ref())
                            && !name.starts_with('.')
                    } else {
                        true
                    }
                })
                .filter_map(|e| e.ok())
                .filter(|e| {
                    if !e.file_type().is_file() {
                        return false;
                    }
                    let path_str = e.path().to_string_lossy();
                    // Check .brainjarignore patterns
                    for pattern in &ignore_patterns {
                        if pattern.matches(&path_str) || pattern.matches(e.file_name().to_string_lossy().as_ref()) {
                            return false;
                        }
                    }
                    // Only index known text file extensions
                    let ext = e.path().extension()
                        .and_then(|ext| ext.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    DEFAULT_TEXT_EXTENSIONS.contains(&ext.as_str())
                })
            {
                let abs = entry.path().to_path_buf();
                let rel = if let Ok(r) = abs.strip_prefix(&config.config_dir) {
                    r.to_string_lossy().to_string()
                } else {
                    abs.to_string_lossy().to_string()
                };
                files.insert(rel, abs);
            }
        } else {
            // Glob pattern
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

/// SHA256 of file content — used to detect changes.
pub fn hash_content(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}
