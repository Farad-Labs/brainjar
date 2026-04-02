/// Integration tests for the sync pipeline.
use std::collections::HashMap;
use brainjar::config::{Config, KnowledgeBaseConfig};
use brainjar::db;
use brainjar::sync::{collect_files, hash_content};

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
    }
}

// ─── Full sync cycle ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_full_sync_cycle_documents_populated() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();
    std::fs::write(notes_dir.join("rust.md"), "# Rust\nRust is a systems language.").unwrap();
    std::fs::write(notes_dir.join("python.md"), "# Python\nPython for scripting.").unwrap();

    let config = make_config(dir.path(), &notes_dir);

    brainjar::sync::run_sync(&config, Some("test"), false, false, false, false)
        .await
        .unwrap();

    let conn = db::open_db("test", &config.effective_db_dir()).unwrap();
    let count = db::count_documents(&conn).unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn test_full_sync_cycle_fts_works() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();
    std::fs::write(notes_dir.join("sqlite.md"), "SQLite is an embedded database.").unwrap();

    let config = make_config(dir.path(), &notes_dir);
    brainjar::sync::run_sync(&config, Some("test"), false, false, false, false)
        .await
        .unwrap();

    let conn = db::open_db("test", &config.effective_db_dir()).unwrap();
    let results = brainjar::search::search_fts(&conn, "embedded", 5).unwrap();
    assert!(!results.is_empty());
    assert!(results[0].path.contains("sqlite"));
}

// ─── Search pipeline ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_search_pipeline_fts_ranked() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("doc_a.md"), "brainjar knowledge search").unwrap();
    std::fs::write(notes.join("doc_b.md"), "brainjar is mentioned here").unwrap();

    let config = make_config(dir.path(), &notes);
    brainjar::sync::run_sync(&config, Some("test"), false, false, false, false)
        .await
        .unwrap();

    let conn = db::open_db("test", &config.effective_db_dir()).unwrap();
    let results = brainjar::search::search_fts(&conn, "brainjar", 5).unwrap();
    // Both documents should be found
    assert!(results.len() >= 2);
    assert!(results.iter().any(|r| r.path.contains("doc_a")));
    assert!(results.iter().any(|r| r.path.contains("doc_b")));
    // All scores should be positive (we negate FTS5 rank)
    assert!(results.iter().all(|r| r.score > 0.0));
}

// ─── Incremental sync ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_incremental_sync_only_updates_changed_file() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("stable.md"), "stable content").unwrap();
    std::fs::write(notes.join("changing.md"), "original content").unwrap();

    let config = make_config(dir.path(), &notes);

    // First sync
    brainjar::sync::run_sync(&config, Some("test"), false, false, false, false)
        .await
        .unwrap();

    // Record the hash of stable.md after first sync
    let conn = db::open_db("test", &config.effective_db_dir()).unwrap();
    let hashes_before = db::get_all_hashes(&conn).unwrap();
    let stable_hash_before = hashes_before
        .iter()
        .find(|(k, _)| k.contains("stable"))
        .map(|(_, v)| v.clone())
        .unwrap();

    // Modify only changing.md
    std::fs::write(notes.join("changing.md"), "NEW content after modification").unwrap();

    // Second sync
    brainjar::sync::run_sync(&config, Some("test"), false, false, false, false)
        .await
        .unwrap();

    let conn2 = db::open_db("test", dir.path()).unwrap();
    let hashes_after = db::get_all_hashes(&conn2).unwrap();

    let stable_hash_after = hashes_after
        .iter()
        .find(|(k, _)| k.contains("stable"))
        .map(|(_, v)| v.clone())
        .unwrap();

    // stable.md hash should be unchanged
    assert_eq!(stable_hash_before, stable_hash_after);

    // changing.md should have a new hash
    let changing_hash_after = hashes_after
        .iter()
        .find(|(k, _)| k.contains("changing"))
        .map(|(_, v)| v.clone())
        .unwrap();
    assert_ne!(
        changing_hash_after,
        hash_content(b"original content")
    );
}

// ─── Delete detection ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_delete_detection() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("keep.md"), "I will stay").unwrap();
    std::fs::write(notes.join("delete_me.md"), "I will be deleted").unwrap();

    let config = make_config(dir.path(), &notes);

    // First sync
    brainjar::sync::run_sync(&config, Some("test"), false, false, false, false)
        .await
        .unwrap();

    let conn = db::open_db("test", &config.effective_db_dir()).unwrap();
    assert_eq!(db::count_documents(&conn).unwrap(), 2);

    // Delete one file
    std::fs::remove_file(notes.join("delete_me.md")).unwrap();

    // Second sync
    brainjar::sync::run_sync(&config, Some("test"), false, false, false, false)
        .await
        .unwrap();

    let conn2 = db::open_db("test", dir.path()).unwrap();
    assert_eq!(db::count_documents(&conn2).unwrap(), 1);

    let hashes = db::get_all_hashes(&conn2).unwrap();
    assert!(!hashes.keys().any(|k| k.contains("delete_me")));
    assert!(hashes.keys().any(|k| k.contains("keep")));
}

// ─── .brainjarignore ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_brainjarignore_skips_patterns() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    // Create .brainjarignore in config_dir (same as dir.path())
    std::fs::write(dir.path().join(".brainjarignore"), "secret.md\n").unwrap();
    std::fs::write(notes.join("public.md"), "public content").unwrap();
    std::fs::write(notes.join("secret.md"), "private content").unwrap();

    let config = make_config(dir.path(), &notes);
    brainjar::sync::run_sync(&config, Some("test"), false, false, false, false)
        .await
        .unwrap();

    let conn = db::open_db("test", &config.effective_db_dir()).unwrap();
    let hashes = db::get_all_hashes(&conn).unwrap();
    assert!(hashes.keys().any(|k| k.contains("public")));
    assert!(!hashes.keys().any(|k| k.contains("secret")));
}

#[tokio::test]
async fn test_brainjarignore_with_extension_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(dir.path().join(".brainjarignore"), "*.txt\n").unwrap();
    std::fs::write(notes.join("doc.md"), "markdown doc").unwrap();
    std::fs::write(notes.join("notes.txt"), "plain text notes").unwrap();

    let config = make_config(dir.path(), &notes);
    brainjar::sync::run_sync(&config, Some("test"), false, false, false, false)
        .await
        .unwrap();

    let conn = db::open_db("test", &config.effective_db_dir()).unwrap();
    let hashes = db::get_all_hashes(&conn).unwrap();
    assert!(hashes.keys().any(|k| k.contains("doc.md")));
    assert!(!hashes.keys().any(|k| k.contains("notes.txt")));
}

// ─── collect_files tests ──────────────────────────────────────────────────────

#[test]
fn test_collect_files_single_file_watch_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("single.md");
    std::fs::write(&file, "content").unwrap();

    let config = make_config(dir.path(), &file);
    let watch_paths = config.expand_watch_paths(config.knowledge_bases.get("test").unwrap());
    let files = collect_files(&config, &watch_paths);

    assert_eq!(files.len(), 1);
    assert!(files.keys().any(|k| k.contains("single.md")));
}

// ─── Extraction tracking ──────────────────────────────────────────────────────

/// After a full sync with no extraction config, all docs should have extracted=0
/// (extraction not attempted, but sync completed — we don't auto-mark extracted
/// when extraction is disabled; the field stays 0 and the next sync with extraction
/// enabled will correctly pick them up).
#[tokio::test]
async fn test_sync_without_extraction_extracted_stays_zero() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("doc.md"), "Hello world").unwrap();

    let config = make_config(dir.path(), &notes);
    brainjar::sync::run_sync(&config, Some("test"), false, false, false, false)
        .await
        .unwrap();

    let conn = db::open_db("test", &config.effective_db_dir()).unwrap();
    let unextracted = db::get_unextracted_paths(&conn).unwrap();
    // No extraction config → extracted stays 0, so the doc shows as unextracted
    assert_eq!(unextracted.len(), 1);
}

/// Simulate an interruption: sync files (extracted=0), then re-run sync.
/// The re-run should detect the unextracted docs and include them in extraction.
/// Without extraction config the test just verifies the pending docs are detected.
#[tokio::test]
async fn test_interrupted_extraction_detected_on_resync() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("a.md"), "Rust is great").unwrap();
    std::fs::write(notes.join("b.md"), "Python too").unwrap();

    let config = make_config(dir.path(), &notes);

    // First sync — files are indexed, extracted=0 (no extraction config)
    brainjar::sync::run_sync(&config, Some("test"), false, false, false, false)
        .await
        .unwrap();

    let conn = db::open_db("test", &config.effective_db_dir()).unwrap();
    let unextracted_after_first = db::get_unextracted_paths(&conn).unwrap();
    // Both docs are unextracted (no extraction config ran)
    assert_eq!(unextracted_after_first.len(), 2);

    // Second sync — content unchanged, but unextracted docs should be detected
    // (without extraction config they stay pending — the key test is that
    // "nothing to sync" is NOT printed when there are unextracted docs)
    // We verify the paths are still 0 (not falsely marked as done)
    brainjar::sync::run_sync(&config, Some("test"), false, false, false, false)
        .await
        .unwrap();

    let conn2 = db::open_db("test", &config.effective_db_dir()).unwrap();
    let unextracted_after_second = db::get_unextracted_paths(&conn2).unwrap();
    // Still 2 — not falsely marked complete
    assert_eq!(unextracted_after_second.len(), 2);
}

/// mark_extracted sets extracted=1 for the given path.
#[test]
fn test_mark_extracted_sets_flag() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();
    db::upsert_document(&conn, "notes/a.md", "content", "h1").unwrap();

    // Initially unextracted
    let unextracted = db::get_unextracted_paths(&conn).unwrap();
    assert!(unextracted.contains(&"notes/a.md".to_string()));

    // Mark as extracted
    db::mark_extracted(&conn, "notes/a.md").unwrap();

    let unextracted_after = db::get_unextracted_paths(&conn).unwrap();
    assert!(!unextracted_after.contains(&"notes/a.md".to_string()));
}

/// After content changes (re-upsert), extracted resets to 0.
#[test]
fn test_content_change_resets_extracted_flag() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();
    db::upsert_document(&conn, "notes/a.md", "v1 content", "hash1").unwrap();
    db::mark_extracted(&conn, "notes/a.md").unwrap();

    // Confirm it's marked
    let unextracted = db::get_unextracted_paths(&conn).unwrap();
    assert!(unextracted.is_empty());

    // Simulate content change
    db::upsert_document(&conn, "notes/a.md", "v2 content", "hash2").unwrap();

    // extracted should be reset to 0
    let unextracted_after = db::get_unextracted_paths(&conn).unwrap();
    assert!(unextracted_after.contains(&"notes/a.md".to_string()));
}

/// Multiple docs — partial extraction (only some marked) — unextracted returns only unmarked.
#[test]
fn test_get_unextracted_returns_only_unextracted() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();
    db::upsert_document(&conn, "a.md", "aaa", "h_a").unwrap();
    db::upsert_document(&conn, "b.md", "bbb", "h_b").unwrap();
    db::upsert_document(&conn, "c.md", "ccc", "h_c").unwrap();

    db::mark_extracted(&conn, "a.md").unwrap();
    db::mark_extracted(&conn, "c.md").unwrap();

    let unextracted = db::get_unextracted_paths(&conn).unwrap();
    assert_eq!(unextracted.len(), 1);
    assert!(unextracted.contains(&"b.md".to_string()));
}

/// --force flag re-extracts everything: after marking all extracted, force sync
/// should re-upsert all docs (resetting extracted=0 via the ON CONFLICT clause).
#[tokio::test]
async fn test_force_resets_extracted_flag() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("x.md"), "some content").unwrap();

    let config = make_config(dir.path(), &notes);

    // First sync
    brainjar::sync::run_sync(&config, Some("test"), false, false, false, false)
        .await
        .unwrap();

    // Manually mark as extracted
    let conn = db::open_db("test", &config.effective_db_dir()).unwrap();
    let hashes = db::get_all_hashes(&conn).unwrap();
    let first_path = hashes.keys().next().unwrap().clone();
    db::mark_extracted(&conn, &first_path).unwrap();
    drop(hashes);
    drop(conn);

    // Verify it's marked
    let conn2 = db::open_db("test", &config.effective_db_dir()).unwrap();
    assert!(db::get_unextracted_paths(&conn2).unwrap().is_empty());
    drop(conn2);

    // Force sync — should re-upsert all docs, resetting extracted=0
    brainjar::sync::run_sync(&config, Some("test"), true, false, false, false)
        .await
        .unwrap();

    let conn3 = db::open_db("test", &config.effective_db_dir()).unwrap();
    let unextracted = db::get_unextracted_paths(&conn3).unwrap();
    assert_eq!(unextracted.len(), 1);
}

/// Migration: open an existing v0 DB (no extracted column, no schema_version)
/// — open_db should migrate it to v1 transparently.
#[test]
fn test_migration_adds_extracted_column() {
    let dir = tempfile::tempdir().unwrap();
    // Create a v0 DB without the extracted column and without schema_version
    {
        let db_path = dir.path().join("legacy.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(r#"
            CREATE TABLE documents (
                id INTEGER PRIMARY KEY,
                path TEXT UNIQUE NOT NULL,
                content TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
            INSERT INTO documents (path, content, content_hash) VALUES ('old.md', 'old content', 'oldhash');
        "#).unwrap();
    }
    // Re-open via open_db — run_migrations should fire and add the column
    let conn = db::open_db("legacy", dir.path()).unwrap();
    // schema_version bumped
    let version = db::get_meta(&conn, "schema_version").unwrap();
    assert_eq!(version.as_deref(), Some("1"));
    // Existing row defaults to extracted=0
    let unextracted = db::get_unextracted_paths(&conn).unwrap();
    assert_eq!(unextracted.len(), 1);
    assert!(unextracted.contains(&"old.md".to_string()));
}

#[test]
fn test_collect_files_nested_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("project");
    std::fs::create_dir(&data).unwrap();
    let sub = data.join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("nested.md"), "nested").unwrap();
    std::fs::write(data.join("top.md"), "top").unwrap();

    let config = make_config(&data, &data);
    let watch_paths = config.expand_watch_paths(config.knowledge_bases.get("test").unwrap());
    let files = collect_files(&config, &watch_paths);

    assert!(files.keys().any(|k| k.contains("nested.md")));
    assert!(files.keys().any(|k| k.contains("top.md")));
}
