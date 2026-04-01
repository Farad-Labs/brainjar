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
        },
    );
    Config {
        providers: HashMap::new(),
        knowledge_bases: kbs,
        embeddings: None,
        extraction: None,
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

    let conn = db::open_db("test", dir.path()).unwrap();
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

    let conn = db::open_db("test", dir.path()).unwrap();
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

    let conn = db::open_db("test", dir.path()).unwrap();
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
    let conn = db::open_db("test", dir.path()).unwrap();
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

    let conn = db::open_db("test", dir.path()).unwrap();
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

    let conn = db::open_db("test", dir.path()).unwrap();
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

    let conn = db::open_db("test", dir.path()).unwrap();
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
