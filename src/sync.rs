use anyhow::{Context, Result};
use chrono::Utc;
use colored::Colorize;
use hex;
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use glob::Pattern;
use walkdir::WalkDir;

use crate::chunk;
use crate::config::{Config, KnowledgeBaseConfig};
use crate::db;
use crate::embed::Embedder;
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
    reembed: bool,
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
            .map(|(name, kb): (&String, _)| (name.as_str(), kb))
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
            sync_kb_json(config, name, kb, force, dry_run, reembed).await?;
        } else {
            sync_kb_human(config, name, kb, force, dry_run, reembed).await?;
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
    reembed: bool,
) -> Result<()> {
    println!("\n{} {}", "⟳ Syncing".cyan().bold(), kb_name.bold());

    let db_dir = config.effective_db_dir();
    let vec_dims = config.embeddings.as_ref().map(|e| e.dimensions).unwrap_or(0);
    let conn = db::open_db_with_dims(kb_name, &db_dir, vec_dims)?;
    let watch_paths = config.expand_watch_paths(kb);
    let local_files = collect_files(config, &watch_paths);
    let changes = compute_changes(&conn, &local_files, force)?;

    let total_upsert = changes.to_upsert.len();
    let total_delete = changes.to_delete.len();

    // Docs that were synced before but whose extraction was interrupted.
    // We need their absolute paths too — build from local_files map.
    let unextracted_paths = if !force {
        db::get_unextracted_paths(&conn)?
    } else {
        Vec::new() // force re-extracts everything via to_upsert
    };
    let unextracted: HashMap<String, std::path::PathBuf> = unextracted_paths
        .into_iter()
        .filter(|p| !changes.to_upsert.contains_key(p)) // avoid double-counting
        .filter_map(|p| {
            local_files.get(&p).cloned().map(|abs| (p, abs))
        })
        .collect();

    // Detect if chunks exist but vec table is empty (e.g. after a dimension change)
    let chunk_count: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0)).unwrap_or(0);
    let vec_count: i64 = conn.query_row("SELECT COUNT(*) FROM chunks_vec", [], |r| r.get(0)).unwrap_or(0);
    let needs_reembed = reembed || (chunk_count > 0 && vec_count == 0 && vec_dims > 0);

    if total_upsert == 0 && total_delete == 0 && unextracted.is_empty() && !needs_reembed {
        println!("  {} Nothing to sync", "✓".green());
        return Ok(());
    }

    if !unextracted.is_empty() && total_upsert == 0 && total_delete == 0 {
        println!(
            "  {} {} document(s) pending extraction (interrupted previously)",
            "⚠".yellow(),
            unextracted.len().to_string().yellow()
        );
    } else {

        println!(
            "  {} files to update, {} to delete",
            total_upsert.to_string().cyan(),
            total_delete.to_string().yellow()
        );
    }

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

    let sync_start = std::time::Instant::now();
    let mut new_count = 0usize;
    let mut updated_count = 0usize;

    // Pre-compute which paths are new vs updated
    let db_hashes_before = db::get_all_hashes(&conn)?;

    // Upsert files
    if !changes.to_upsert.is_empty() {
        let pb = ProgressBar::new(total_upsert as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("  Syncing: {pos}/{len} docs [{bar:38.cyan/blue}] {percent}%\n  {msg}")
                .unwrap()
                .progress_chars("\u{2588}\u{2588}\u{2591}"),
        );

        let mut total_chunks = 0usize;
        for (rel_path, abs_path) in &changes.to_upsert {
            // Truncate long filenames for display
            let display_name = if rel_path.len() > 60 {
                format!("...{}", &rel_path[rel_path.len() - 57..])
            } else {
                rel_path.clone()
            };
            pb.set_message(display_name);
            let content = std::fs::read_to_string(abs_path)
                .with_context(|| format!("Failed to read file: {}", abs_path.display()))?;
            let hash = hash_content(content.as_bytes());
            if db_hashes_before.contains_key(rel_path) {
                updated_count += 1;
            } else {
                new_count += 1;
            }
            db::upsert_document(&conn, rel_path, &content, &hash)?;
            // Chunk the document and (re)insert chunks
            if let Ok(Some(doc_id)) = db::get_document_id(&conn, rel_path) {
                let _ = db::delete_chunks_for_doc(&conn, doc_id);
                let file_chunks = chunk::chunk_file(rel_path, &content);
                for c in &file_chunks {
                    let _ = db::insert_chunk(&conn, doc_id, &c.content, c.line_start, c.line_end, &c.chunk_type);
                }
                total_chunks += file_chunks.len();
            }
            pb.inc(1);
        }
        pb.finish_and_clear();

        println!("  {} Chunked {} docs ({} chunks)", "✓".green(), total_upsert, total_chunks);
    }

    // Delete removed files
    for path in &changes.to_delete {
        db::delete_document(&conn, path)?;
    }

    if !changes.to_delete.is_empty() {
        println!("  {} Removed {} files from index", "✓".green(), total_delete);
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
    // Extract: newly upserted docs + previously-interrupted docs
    let docs_to_extract: HashMap<&String, &std::path::PathBuf> = changes
        .to_upsert
        .iter()
        .chain(unextracted.iter())
        .collect();

    if let Some(extraction_cfg) = &config.extraction
        && extraction_cfg.enabled && !docs_to_extract.is_empty() {
            let api_key = config.resolve_api_key(&extraction_cfg.provider, extraction_cfg.api_key.as_deref());
            let base_url = config.resolve_base_url(&extraction_cfg.provider, extraction_cfg.base_url.as_deref());
            let extractor = Extractor::new(extraction_cfg, api_key, base_url);
            match KnowledgeGraph::open(&db_dir, kb_name) {
                Ok(kg) => {
                    let extract_total = docs_to_extract.len() as u64;
                    let epb = ProgressBar::new(extract_total);
                    epb.set_style(
                        ProgressStyle::default_bar()
                            .template("  Entities: {pos}/{len} docs [{bar:38.green/white}] {percent}%\n  {msg}")
                            .unwrap()
                            .progress_chars("\u{2588}\u{2588}\u{2591}"),
                    );

                    let mut total_entities = 0usize;
                    let mut total_rels = 0usize;
                    let mut extraction_errors = 0usize;

                    for (rel_path, abs_path) in &docs_to_extract {
                        let display_name = if rel_path.len() > 60 {
                            format!("...{}", &rel_path[rel_path.len() - 57..])
                        } else {
                            rel_path.to_string()
                        };
                        epb.set_message(display_name);
                        let content = match std::fs::read_to_string(abs_path) {
                            Ok(c) => c,
                            Err(_) => { epb.inc(1); continue; }
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
                                let ingest_ok = kg.ingest_entities(
                                    rel_path,
                                    &result.entities,
                                    &result.relationships,
                                );
                                if let Err(e) = ingest_ok {
                                    eprintln!("  ⚠ Graph ingest failed for {}: {}", rel_path, e);
                                    extraction_errors += 1;
                                } else {
                                    // Mark as extracted only on full success
                                    if let Err(e) = db::mark_extracted(&conn, rel_path) {
                                        eprintln!("  ⚠ mark_extracted failed for {}: {}", rel_path, e);
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("  ⚠ Extraction failed for {}: {}", rel_path, e);
                                extraction_errors += 1;
                            }
                        }
                        epb.inc(1);
                    }
                    epb.finish_and_clear();

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
                            "\u{26a0}".yellow(),
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

    // ── Re-embed all chunks if dimension changed or --reembed flag ───────────
    if needs_reembed && db::chunks_vec_table_exists(&conn) && changes.to_upsert.is_empty()
        && let Some(embed_cfg) = &config.embeddings {
        let api_key = config.resolve_api_key(&embed_cfg.provider, embed_cfg.api_key.as_deref());
        let base_url = config.resolve_base_url(&embed_cfg.provider, embed_cfg.base_url.as_deref());
        let embedder = Embedder::new(embed_cfg, api_key, base_url);

        // Load all chunks from DB
        let mut all_chunks: Vec<(i64, String, String)> = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT c.id, c.content, d.path FROM chunks c JOIN documents d ON c.doc_id = d.id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })?;
            for row in rows {
                let (cid, content, path) = row?;
                let title = std::path::Path::new(&path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                all_chunks.push((cid, content, title));
            }
        }

        let reason = if reembed { "--reembed" } else { "dimension change detected" };
        println!(
            "  {} Re-embedding {} chunks ({})",
            "⟳".cyan(),
            all_chunks.len(),
            reason
        );

        let mut embedded_count = 0usize;
        let mut embed_errors = 0usize;

        for batch in all_chunks.chunks(100) {
            let docs: Vec<(&str, Option<&str>)> = batch
                .iter()
                .map(|(_, content, title)| (content.as_str(), Some(title.as_str())))
                .collect();
            match embedder.embed_documents(&docs).await {
                Ok(embeddings) => {
                    for ((chunk_id, _, _), embedding) in batch.iter().zip(embeddings.iter()) {
                        use zerocopy::IntoBytes;
                        if let Err(e) = db::upsert_chunk_vec(&conn, *chunk_id, embedding.as_bytes()) {
                            eprintln!("  ⚠ Chunk vec upsert failed for chunk {}: {}", chunk_id, e);
                            embed_errors += 1;
                        } else {
                            embedded_count += 1;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("  ⚠ Embedding batch failed: {}", e);
                    embed_errors += batch.len();
                }
            }
        }

        if embed_errors == 0 {
            println!("  {} Re-embedded {} chunks", "✓".green(), embedded_count);
        } else {
            println!(
                "  {} Re-embedded {} chunks ({} errors)",
                "\u{26a0}".yellow(),
                embedded_count,
                embed_errors
            );
        }
        } // end if let Some(embed_cfg)

    // ── Optional: vector embeddings via sqlite-vec (per chunk) ────────────────
    if let Some(embed_cfg) = &config.embeddings
        && !changes.to_upsert.is_empty() && db::chunks_vec_table_exists(&conn) {
            let api_key = config.resolve_api_key(&embed_cfg.provider, embed_cfg.api_key.as_deref());
            let base_url = config.resolve_base_url(&embed_cfg.provider, embed_cfg.base_url.as_deref());
            let embedder = Embedder::new(embed_cfg, api_key, base_url);

            // Collect all (chunk_id, content, title) for newly upserted docs
            let mut chunk_items: Vec<(i64, String, String)> = Vec::new(); // (chunk_id, content, file_stem)
            for rel_path in changes.to_upsert.keys() {
                #[allow(clippy::collapsible_if)]
                if let Ok(Some(doc_id)) = db::get_document_id(&conn, rel_path) {
                    if let Ok(doc_chunks) = db::get_chunks_for_doc(&conn, doc_id) {
                        let title = std::path::Path::new(rel_path)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_string();
                        for (cid, content, _, _, _) in doc_chunks {
                            chunk_items.push((cid, content, title.clone()));
                        }
                    }
                }
            }

            let mut embedded_count = 0usize;
            let mut embed_errors = 0usize;

            // Batch 100 chunks at a time — matches Gemini batchEmbedContents limit
            for batch in chunk_items.chunks(100) {
                let docs: Vec<(&str, Option<&str>)> = batch.iter().map(|(_, content, title)| {
                    (content.as_str(), Some(title.as_str()))
                }).collect();
                match embedder.embed_documents(&docs).await {
                    Ok(embeddings) => {
                        for ((chunk_id, _, _), embedding) in batch.iter().zip(embeddings.iter()) {
                            use zerocopy::IntoBytes;
                            if let Err(e) = db::upsert_chunk_vec(&conn, *chunk_id, embedding.as_bytes()) {
                                eprintln!("  ⚠ Chunk vec upsert failed for chunk {}: {}", chunk_id, e);
                                embed_errors += 1;
                            } else {
                                embedded_count += 1;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("  ⚠ Embedding batch failed: {}", e);
                        embed_errors += batch.len();
                    }
                }
            }

            if embed_errors == 0 {
                println!(
                    "  {} Generated embeddings ({} chunks)",
                    "✓".green(),
                    embedded_count
                );
            } else {
                println!(
                    "  {} Generated embeddings ({} chunks, {} errors)",
                    "\u{26a0}".yellow(),
                    embedded_count,
                    embed_errors
                );
            }
        }

    // ── Final summary ────────────────────────────────────────────────────────
    let elapsed = sync_start.elapsed();
    let elapsed_str = if elapsed.as_secs() >= 60 {
        format!("{}m {}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
    } else {
        format!("{:.1}s", elapsed.as_secs_f64())
    };
    let extracted_resumed = unextracted.len();
    if total_upsert > 0 || extracted_resumed > 0 {
        let mut parts = Vec::new();
        if new_count > 0 {
            parts.push(format!("{} new", new_count.to_string().green()));
        }
        if updated_count > 0 {
            parts.push(format!("{} updated", updated_count.to_string().yellow()));
        }
        if extracted_resumed > 0 && total_upsert == 0 {
            parts.push(format!("{} extracted", extracted_resumed.to_string().cyan()));
        }
        let summary = if parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", parts.join(", "))
        };
        let total = if total_upsert > 0 { total_upsert } else { extracted_resumed };
        println!(
            "\n  {} Synced {} docs{} in {}",
            "✓".green().bold(),
            total.to_string().cyan(),
            summary,
            elapsed_str.bold()
        );
    } else {
        println!(
            "\n  {} Done in {}",
            "✓".green().bold(),
            elapsed_str.bold()
        );
    }

    Ok(())
}

async fn sync_kb_json(
    config: &Config,
    kb_name: &str,
    kb: &KnowledgeBaseConfig,
    force: bool,
    dry_run: bool,
    reembed: bool,
) -> Result<()> {
    let db_dir = config.effective_db_dir();
    let vec_dims = config.embeddings.as_ref().map(|e| e.dimensions).unwrap_or(0);
    let conn = db::open_db_with_dims(kb_name, &db_dir, vec_dims)?;
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
            // Chunk the document
            if let Ok(Some(doc_id)) = db::get_document_id(&conn, rel_path) {
                let _ = db::delete_chunks_for_doc(&conn, doc_id);
                let file_chunks = chunk::chunk_file(rel_path, &content);
                for c in &file_chunks {
                    let _ = db::insert_chunk(&conn, doc_id, &c.content, c.line_start, c.line_end, &c.chunk_type);
                }
            }
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

        // Docs needing extraction: newly upserted + previously interrupted
        let unextracted_paths = if !force {
            db::get_unextracted_paths(&conn)?
        } else {
            Vec::new()
        };
        let unextracted_json: HashMap<String, std::path::PathBuf> = unextracted_paths
            .into_iter()
            .filter(|p| !changes.to_upsert.contains_key(p))
            .filter_map(|p| local_files.get(&p).cloned().map(|abs| (p, abs)))
            .collect();
        let docs_to_extract_json: HashMap<&String, &std::path::PathBuf> = changes
            .to_upsert
            .iter()
            .chain(unextracted_json.iter())
            .collect();

        // Optional entity extraction
        let mut entities_extracted = 0usize;
        let mut rels_extracted = 0usize;
        if let Some(extraction_cfg) = &config.extraction
            && extraction_cfg.enabled && !docs_to_extract_json.is_empty() {
                let api_key = config.resolve_api_key(&extraction_cfg.provider, extraction_cfg.api_key.as_deref());
                let base_url = config.resolve_base_url(&extraction_cfg.provider, extraction_cfg.base_url.as_deref());
                let extractor = Extractor::new(extraction_cfg, api_key, base_url);
                if let Ok(kg) = KnowledgeGraph::open(&db_dir, kb_name) {
                    for (rel_path, abs_path) in &docs_to_extract_json {
                        let content = match std::fs::read_to_string(abs_path) {
                            Ok(c) => c,
                            Err(_) => continue,
                        };
                        let _ = kg.remove_document(rel_path);
                        if let Ok(res) = extractor.extract(&content, rel_path).await {
                            entities_extracted += res.entities.len();
                            rels_extracted += res.relationships.len();
                            if kg.ingest_entities(rel_path, &res.entities, &res.relationships).is_ok() {
                                let _ = db::mark_extracted(&conn, rel_path);
                            }
                        }
                    }
                }
            }

        // Detect dimension change / --reembed for JSON mode
        let chunk_count_json: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0)).unwrap_or(0);
        let vec_count_json: i64 = conn.query_row("SELECT COUNT(*) FROM chunks_vec", [], |r| r.get(0)).unwrap_or(0);
        let needs_reembed_json = reembed || (chunk_count_json > 0 && vec_count_json == 0 && vec_dims > 0);

        // Re-embed all chunks if needed (and no new docs being upserted — avoid double-embedding)
        if needs_reembed_json && db::chunks_vec_table_exists(&conn) && changes.to_upsert.is_empty()
            && let Some(embed_cfg) = &config.embeddings {
            let api_key = config.resolve_api_key(&embed_cfg.provider, embed_cfg.api_key.as_deref());
            let base_url = config.resolve_base_url(&embed_cfg.provider, embed_cfg.base_url.as_deref());
            let embedder = Embedder::new(embed_cfg, api_key, base_url);

            let mut all_chunks_json: Vec<(i64, String, String)> = Vec::new();
            {
                let mut stmt = conn.prepare(
                    "SELECT c.id, c.content, d.path FROM chunks c JOIN documents d ON c.doc_id = d.id",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
                })?;
                for row in rows {
                    let (cid, content, path) = row?;
                    let title = std::path::Path::new(&path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    all_chunks_json.push((cid, content, title));
                }
            }

            let mut reembedded = 0usize;
            for batch in all_chunks_json.chunks(100) {
                let docs: Vec<(&str, Option<&str>)> = batch
                    .iter()
                    .map(|(_, content, title)| (content.as_str(), Some(title.as_str())))
                    .collect();
                if let Ok(embeddings) = embedder.embed_documents(&docs).await {
                    for ((chunk_id, _, _), embedding) in batch.iter().zip(embeddings.iter()) {
                        use zerocopy::IntoBytes;
                        if db::upsert_chunk_vec(&conn, *chunk_id, embedding.as_bytes()).is_ok() {
                            reembedded += 1;
                        }
                    }
                }
            }
            result["vectors_reembedded"] = serde_json::Value::Number(reembedded.into());
            } // end if let Some(embed_cfg)

        // Vector embeddings (JSON mode) — per chunk
        let mut vectors_embedded = 0usize;
        if let Some(embed_cfg) = &config.embeddings
            && !changes.to_upsert.is_empty() && db::chunks_vec_table_exists(&conn) {
                let api_key = config.resolve_api_key(&embed_cfg.provider, embed_cfg.api_key.as_deref());
                let base_url = config.resolve_base_url(&embed_cfg.provider, embed_cfg.base_url.as_deref());
                let embedder = Embedder::new(embed_cfg, api_key, base_url);

                let mut chunk_items: Vec<(i64, String, String)> = Vec::new();
                for rel_path in changes.to_upsert.keys() {
                    #[allow(clippy::collapsible_if)]
                    if let Ok(Some(doc_id)) = db::get_document_id(&conn, rel_path) {
                        if let Ok(doc_chunks) = db::get_chunks_for_doc(&conn, doc_id) {
                            let title = std::path::Path::new(rel_path)
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("")
                                .to_string();
                            for (cid, content, _, _, _) in doc_chunks {
                                chunk_items.push((cid, content, title.clone()));
                            }
                        }
                    }
                }

                for batch in chunk_items.chunks(100) {
                    let docs: Vec<(&str, Option<&str>)> = batch.iter().map(|(_, content, title)| {
                        (content.as_str(), Some(title.as_str()))
                    }).collect();
                    if let Ok(embeddings) = embedder.embed_documents(&docs).await {
                        for ((chunk_id, _, _), embedding) in batch.iter().zip(embeddings.iter()) {
                            use zerocopy::IntoBytes;
                            if db::upsert_chunk_vec(&conn, *chunk_id, embedding.as_bytes()).is_ok() {
                                vectors_embedded += 1;
                            }
                        }
                    }
                }
            }

        result["status"] = serde_json::Value::String("COMPLETE".to_string());
        result["entities_extracted"] = serde_json::Value::Number(entities_extracted.into());
        result["relationships_extracted"] = serde_json::Value::Number(rels_extracted.into());
        result["vectors_embedded"] = serde_json::Value::Number(vectors_embedded.into());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, KnowledgeBaseConfig};
    use std::collections::HashMap;

    /// Build a minimal Config pointing at `config_dir` with one KB watching `watch_path`.
    fn make_config(config_dir: &std::path::Path, watch_path: &std::path::Path) -> Config {
        let mut kbs = HashMap::new();
        kbs.insert(
            "test".to_string(),
            KnowledgeBaseConfig {
                watch_paths: vec![watch_path.to_string_lossy().to_string()],
                auto_sync: true,
                description: None,
            },
        );
        Config {
            providers: HashMap::new(),
            knowledge_bases: kbs,
            embeddings: None,
            extraction: None,
            data_dir: Some(config_dir.to_string_lossy().to_string()),
            config_dir: config_dir.to_path_buf(),
            watch: None,
        }
    }

    /// Create a temp dir with a non-dot subdirectory for testing.
    /// Returns (tempdir_handle, data_dir) where data_dir doesn't start with '.'.
    fn make_test_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("testdata");
        std::fs::create_dir(&data).unwrap();
        (tmp, data)
    }

    // ─── hash_content ───────────────────────────────────────────────────────────

    #[test]
    fn test_hash_content_deterministic() {
        let h1 = hash_content(b"hello");
        let h2 = hash_content(b"hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_content_different_inputs_differ() {
        assert_ne!(hash_content(b"a"), hash_content(b"b"));
    }

    #[test]
    fn test_hash_content_is_64_hex_chars() {
        let h = hash_content(b"test data");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ─── collect_files ───────────────────────────────────────────────────────

    #[test]
    fn test_collect_files_finds_markdown() {
        let (_tmp, data) = make_test_dir();
        std::fs::write(data.join("doc.md"), "# Hello").unwrap();
        std::fs::write(data.join("notes.txt"), "some notes").unwrap();

        let config = make_config(data.as_path(), data.as_path());
        let watch_paths = config.expand_watch_paths(config.knowledge_bases.get("test").unwrap());
        let files = collect_files(&config, &watch_paths);

        assert!(files.keys().any(|k| k.ends_with("doc.md")));
        assert!(files.keys().any(|k| k.ends_with("notes.txt")));
    }

    #[test]
    fn test_collect_files_ignores_binary_extensions() {
        let (_tmp, data) = make_test_dir();
        std::fs::write(data.join("image.png"), b"\x89PNG\r\n").unwrap();
        std::fs::write(data.join("data.bin"), b"\x00\x01\x02").unwrap();
        std::fs::write(data.join("note.md"), "# note").unwrap();

        let config = make_config(data.as_path(), data.as_path());
        let watch_paths = config.expand_watch_paths(config.knowledge_bases.get("test").unwrap());
        let files = collect_files(&config, &watch_paths);

        assert!(!files.keys().any(|k| k.ends_with(".png")));
        assert!(!files.keys().any(|k| k.ends_with(".bin")));
        assert!(files.keys().any(|k| k.ends_with("note.md")));
    }

    #[test]
    fn test_collect_files_skips_git_dir() {
        let (_tmp, data) = make_test_dir();
        let git_dir = data.join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::write(git_dir.join("config"), "[core]").unwrap();
        std::fs::write(data.join("real.md"), "# real").unwrap();

        let config = make_config(data.as_path(), data.as_path());
        let watch_paths = config.expand_watch_paths(config.knowledge_bases.get("test").unwrap());
        let files = collect_files(&config, &watch_paths);

        assert!(!files.keys().any(|k| k.contains(".git")));
        assert!(files.keys().any(|k| k.ends_with("real.md")));
    }

    #[test]
    fn test_collect_files_skips_node_modules() {
        let (_tmp, data) = make_test_dir();
        let nm = data.join("node_modules");
        std::fs::create_dir(&nm).unwrap();
        std::fs::write(nm.join("pkg.js"), "const x = 1;").unwrap();
        std::fs::write(data.join("app.ts"), "const y = 2;").unwrap();

        let config = make_config(data.as_path(), data.as_path());
        let watch_paths = config.expand_watch_paths(config.knowledge_bases.get("test").unwrap());
        let files = collect_files(&config, &watch_paths);

        assert!(!files.keys().any(|k| k.contains("node_modules")));
        assert!(files.keys().any(|k| k.ends_with("app.ts")));
    }

    #[test]
    fn test_collect_files_empty_dir() {
        let (_tmp, data) = make_test_dir();
        let config = make_config(data.as_path(), data.as_path());
        let watch_paths = config.expand_watch_paths(config.knowledge_bases.get("test").unwrap());
        let files = collect_files(&config, &watch_paths);
        assert!(files.is_empty());
    }

    // ─── .brainjarignore ────────────────────────────────────────────────────

    #[test]
    fn test_brainjarignore_excludes_pattern() {
        let (_tmp, data) = make_test_dir();
        // .brainjarignore in config_dir (data dir)
        std::fs::write(data.join(".brainjarignore"), "*.log\nsecret.md").unwrap();
        std::fs::write(data.join("secret.md"), "private").unwrap();
        std::fs::write(data.join("app.log"), "log data").unwrap();
        std::fs::write(data.join("public.md"), "public").unwrap();

        let config = make_config(data.as_path(), data.as_path());
        let watch_paths = config.expand_watch_paths(config.knowledge_bases.get("test").unwrap());
        let files = collect_files(&config, &watch_paths);

        assert!(!files.keys().any(|k| k.ends_with("secret.md")));
        assert!(files.keys().any(|k| k.ends_with("public.md")));
    }

    #[test]
    fn test_brainjarignore_comments_ignored() {
        let (_tmp, data) = make_test_dir();
        std::fs::write(
            data.join(".brainjarignore"),
            "# This is a comment\n*.tmp",
        ).unwrap();
        std::fs::write(data.join("doc.md"), "content").unwrap();

        let config = make_config(data.as_path(), data.as_path());
        let watch_paths = config.expand_watch_paths(config.knowledge_bases.get("test").unwrap());
        let files = collect_files(&config, &watch_paths);
        assert!(files.keys().any(|k| k.ends_with("doc.md")));
    }

    #[test]
    fn test_no_brainjarignore_collects_all_text_files() {
        let (_tmp, data) = make_test_dir();
        std::fs::write(data.join("a.md"), "A").unwrap();
        std::fs::write(data.join("b.rs"), "fn main() {}").unwrap();

        let config = make_config(data.as_path(), data.as_path());
        let watch_paths = config.expand_watch_paths(config.knowledge_bases.get("test").unwrap());
        let files = collect_files(&config, &watch_paths);
        assert!(files.keys().any(|k| k.ends_with("a.md")));
        assert!(files.keys().any(|k| k.ends_with("b.rs")));
    }

    // ─── load_ignore_patterns ──────────────────────────────────────────────

    #[test]
    fn test_load_ignore_patterns_empty_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let patterns = load_ignore_patterns(dir.path());
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_load_ignore_patterns_from_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".brainjarignore"), "*.log\n*.tmp\n").unwrap();
        let patterns = load_ignore_patterns(dir.path());
        assert_eq!(patterns.len(), 2);
    }

    #[test]
    fn test_load_ignore_patterns_skips_empty_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".brainjarignore"), "\n\n*.log\n\n").unwrap();
        let patterns = load_ignore_patterns(dir.path());
        assert_eq!(patterns.len(), 1);
    }
}
