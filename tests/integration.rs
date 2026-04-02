/// Brainjar integration tests.
///
/// These tests run against real (but ephemeral) SQLite databases in temp directories.
/// No API keys are required — any code path that would make a network call is skipped
/// (no embeddings or extraction config is set).
///
/// All types are accessed through brainjar's public API; rusqlite types are inferred.
use std::collections::HashMap;

use brainjar::config::{Config, KnowledgeBaseConfig};
use brainjar::{db, search, sync};

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Build a minimal Config pointing `config_dir` at the given `watch_path`.
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
        data_dir: Some(config_dir.join(".brainjar").to_string_lossy().to_string()),
        config_dir: config_dir.to_path_buf(),
        watch: None,
    }
}

// ─── 1. Config loading ────────────────────────────────────────────────────────

#[test]
fn test_config_loading_minimal() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("brainjar.toml");
    std::fs::write(
        &cfg_path,
        r#"
[knowledge_bases.main]
watch_paths = ["notes"]
auto_sync = true
"#,
    )
    .unwrap();

    let config = brainjar::config::load_config(Some(cfg_path.to_str().unwrap())).unwrap();
    assert!(config.knowledge_bases.contains_key("main"));
    assert_eq!(config.config_dir, dir.path());
}

#[test]
fn test_config_loading_with_providers() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("brainjar.toml");
    std::fs::write(
        &cfg_path,
        r#"
[knowledge_bases.docs]
watch_paths = ["docs"]
auto_sync = false

[providers.openai]
api_key = "sk-test-key"
base_url = "https://api.openai.com"

[embeddings]
provider = "openai"
model = "text-embedding-3-small"
dimensions = 1536
"#,
    )
    .unwrap();

    let config = brainjar::config::load_config(Some(cfg_path.to_str().unwrap())).unwrap();
    let provider = config.providers.get("openai").unwrap();
    assert_eq!(provider.api_key.as_deref(), Some("sk-test-key"));
    let emb = config.embeddings.as_ref().unwrap();
    assert_eq!(emb.model, "text-embedding-3-small");
    assert_eq!(emb.dimensions, 1536);
}

#[test]
fn test_config_loading_env_var_expansion() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("brainjar.toml");
    std::fs::write(
        &cfg_path,
        r#"
[knowledge_bases.main]
watch_paths = ["notes"]
auto_sync = true

[providers.gemini]
api_key = "${BRAINJAR_INTEG_TEST_KEY}"
"#,
    )
    .unwrap();

    unsafe { std::env::set_var("BRAINJAR_INTEG_TEST_KEY", "my-secret-key"); }
    let config = brainjar::config::load_config(Some(cfg_path.to_str().unwrap())).unwrap();
    let resolved = config.resolve_api_key("gemini", None);
    assert_eq!(resolved.as_deref(), Some("my-secret-key"));
    unsafe { std::env::remove_var("BRAINJAR_INTEG_TEST_KEY"); }
}

#[test]
fn test_config_backward_compat_inline_api_key() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("brainjar.toml");
    std::fs::write(
        &cfg_path,
        r#"
[knowledge_bases.main]
watch_paths = ["notes"]
auto_sync = true

[embeddings]
provider = "openai"
model = "text-embedding-3-small"
dimensions = 1536
api_key = "inline-key"
"#,
    )
    .unwrap();

    let config = brainjar::config::load_config(Some(cfg_path.to_str().unwrap())).unwrap();
    let emb = config.embeddings.as_ref().unwrap();
    // resolve_api_key falls back to inline key when no providers section
    let resolved = config.resolve_api_key(&emb.provider, emb.api_key.as_deref());
    assert_eq!(resolved.as_deref(), Some("inline-key"));
}

#[test]
fn test_config_loading_missing_file_is_error() {
    let result = brainjar::config::load_config(Some("/nonexistent/brainjar.toml"));
    assert!(result.is_err());
}

#[test]
fn test_config_loading_invalid_toml_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("brainjar.toml");
    std::fs::write(&cfg_path, "NOT { valid TOML [").unwrap();
    let result = brainjar::config::load_config(Some(cfg_path.to_str().unwrap()));
    assert!(result.is_err());
}

#[test]
fn test_config_resolve_base_url_from_providers() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("brainjar.toml");
    std::fs::write(
        &cfg_path,
        r#"
[knowledge_bases.main]
watch_paths = ["notes"]
auto_sync = true

[providers.ollama]
base_url = "http://gpu-server:11434"
"#,
    )
    .unwrap();

    let config = brainjar::config::load_config(Some(cfg_path.to_str().unwrap())).unwrap();
    let url = config.resolve_base_url("ollama", Some("http://localhost:11434"));
    assert_eq!(url.as_deref(), Some("http://gpu-server:11434"));
}

// ─── 2. DB: open_db creates tables ───────────────────────────────────────────

#[test]
fn test_open_db_creates_expected_tables() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("testdb", dir.path()).unwrap();

    for table in &["documents", "meta", "vocabulary"] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [*table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "table '{table}' should exist after open_db");
    }

    // FTS virtual table
    let fts: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='documents_fts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fts, 1, "documents_fts virtual table should exist");
}

#[test]
fn test_open_db_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let _first = db::open_db("testdb", dir.path()).unwrap();
    // Opening a second time should not fail or duplicate schema
    let conn2 = db::open_db("testdb", dir.path()).unwrap();
    assert_eq!(db::count_documents(&conn2).unwrap(), 0);
}

#[test]
fn test_open_db_creates_separate_files_per_kb() {
    let dir = tempfile::tempdir().unwrap();
    let _conn_a = db::open_db("kb_a", dir.path()).unwrap();
    let _conn_b = db::open_db("kb_b", dir.path()).unwrap();
    assert!(dir.path().join("kb_a.db").exists());
    assert!(dir.path().join("kb_b.db").exists());
}

// ─── 3. Full sync cycle ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_full_sync_documents_indexed() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();
    std::fs::write(notes_dir.join("alpha.md"), "# Alpha\nThis is alpha.").unwrap();
    std::fs::write(notes_dir.join("beta.md"), "# Beta\nThis is beta.").unwrap();

    let config = make_config(dir.path(), &notes_dir);
    sync::run_sync(&config, Some("test"), false, false, false, true)
        .await
        .unwrap();

    let conn = db::open_db("test", &config.effective_db_dir()).unwrap();
    assert_eq!(db::count_documents(&conn).unwrap(), 2);
}

#[tokio::test]
async fn test_full_sync_content_is_searchable() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();
    std::fs::write(
        notes_dir.join("rust.md"),
        "Rust is a systems programming language focused on memory safety.",
    )
    .unwrap();

    let config = make_config(dir.path(), &notes_dir);
    sync::run_sync(&config, Some("test"), false, false, false, true)
        .await
        .unwrap();

    let conn = db::open_db("test", &config.effective_db_dir()).unwrap();
    let results = search::search_fts(&conn, "systems", 10).unwrap();
    assert!(!results.is_empty(), "FTS should return results after sync");
    assert!(results.iter().any(|r| r.path.contains("rust")));
}

#[tokio::test]
async fn test_full_sync_non_text_files_excluded() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();
    std::fs::write(notes_dir.join("doc.md"), "# Markdown").unwrap();
    std::fs::write(notes_dir.join("image.png"), b"\x89PNG\r\n\x1a\n").unwrap();

    let config = make_config(dir.path(), &notes_dir);
    sync::run_sync(&config, Some("test"), false, false, false, true)
        .await
        .unwrap();

    let conn = db::open_db("test", &config.effective_db_dir()).unwrap();
    // Only doc.md should be indexed; image.png ignored
    assert_eq!(db::count_documents(&conn).unwrap(), 1);
}

// ─── 4. Search pipeline ───────────────────────────────────────────────────────

#[test]
fn test_search_fts_hit() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();
    db::upsert_document(
        &conn,
        "notes/sqlite.md",
        "SQLite is an embedded relational database.",
        "h1",
    )
    .unwrap();

    let results = search::search_fts(&conn, "embedded", 5).unwrap();
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.path.contains("sqlite")));
}

#[test]
fn test_search_fts_scores_are_positive() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();
    db::upsert_document(&conn, "doc.md", "brainjar is wonderful", "h1").unwrap();

    let results = search::search_fts(&conn, "brainjar", 5).unwrap();
    assert!(!results.is_empty());
    // FTS5 rank is negated → score is positive
    assert!(results[0].score > 0.0);
}

#[test]
fn test_search_fts_miss_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();
    db::upsert_document(&conn, "doc.md", "Hello world from Rust.", "h1").unwrap();

    let results = search::search_fts(&conn, "python", 5).unwrap();
    assert!(results.is_empty());
}

#[test]
fn test_search_fts_limit_respected() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();
    for i in 0..10u32 {
        db::upsert_document(
            &conn,
            &format!("doc{i}.md"),
            &format!("searchterm document {i}"),
            &format!("h{i}"),
        )
        .unwrap();
    }
    let results = search::search_fts(&conn, "searchterm", 3).unwrap();
    assert!(results.len() <= 3);
}

#[test]
fn test_search_fts_empty_db() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();
    let results = search::search_fts(&conn, "anything", 10).unwrap();
    assert!(results.is_empty());
}

// ─── 5. Incremental sync ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_incremental_sync_changed_file_is_re_indexed() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();
    std::fs::write(notes_dir.join("a.md"), "Original content A").unwrap();
    std::fs::write(notes_dir.join("b.md"), "Original content B").unwrap();

    let config = make_config(dir.path(), &notes_dir);

    // First sync
    sync::run_sync(&config, Some("test"), false, false, false, true)
        .await
        .unwrap();
    let conn1 = db::open_db("test", &config.effective_db_dir()).unwrap();
    let hashes_before = db::get_all_hashes(&conn1).unwrap();

    // Modify only a.md
    std::fs::write(notes_dir.join("a.md"), "Updated content A — this changed!").unwrap();

    // Second sync
    sync::run_sync(&config, Some("test"), false, false, false, true)
        .await
        .unwrap();
    let conn2 = db::open_db("test", &config.effective_db_dir()).unwrap();
    let hashes_after = db::get_all_hashes(&conn2).unwrap();

    assert_eq!(hashes_after.len(), 2, "should still have 2 documents");

    let key_a = hashes_after.keys().find(|k| k.ends_with("a.md")).unwrap().clone();
    let key_b = hashes_after.keys().find(|k| k.ends_with("b.md")).unwrap().clone();

    assert_ne!(
        hashes_before[&key_a], hashes_after[&key_a],
        "a.md hash must differ after modification"
    );
    assert_eq!(
        hashes_before[&key_b], hashes_after[&key_b],
        "b.md hash must not change"
    );
}

#[tokio::test]
async fn test_incremental_sync_new_file_is_added() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();
    std::fs::write(notes_dir.join("existing.md"), "Existing doc").unwrap();

    let config = make_config(dir.path(), &notes_dir);

    // First sync: 1 document
    sync::run_sync(&config, Some("test"), false, false, false, true)
        .await
        .unwrap();
    let conn1 = db::open_db("test", &config.effective_db_dir()).unwrap();
    assert_eq!(db::count_documents(&conn1).unwrap(), 1);

    // Add a new file
    std::fs::write(notes_dir.join("new_file.md"), "Brand new document").unwrap();

    // Second sync: 2 documents
    sync::run_sync(&config, Some("test"), false, false, false, true)
        .await
        .unwrap();
    let conn2 = db::open_db("test", &config.effective_db_dir()).unwrap();
    assert_eq!(db::count_documents(&conn2).unwrap(), 2);
}

#[tokio::test]
async fn test_incremental_sync_unchanged_file_hash_stable() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();
    std::fs::write(notes_dir.join("stable.md"), "This never changes").unwrap();

    let config = make_config(dir.path(), &notes_dir);

    sync::run_sync(&config, Some("test"), false, false, false, true)
        .await
        .unwrap();
    let conn1 = db::open_db("test", &config.effective_db_dir()).unwrap();
    let h1 = db::get_all_hashes(&conn1).unwrap();

    sync::run_sync(&config, Some("test"), false, false, false, true)
        .await
        .unwrap();
    let conn2 = db::open_db("test", &config.effective_db_dir()).unwrap();
    let h2 = db::get_all_hashes(&conn2).unwrap();

    assert_eq!(h1, h2, "unchanged files should have stable hashes across syncs");
}

// ─── 6. Delete detection ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_delete_detection_removes_from_db() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();
    std::fs::write(notes_dir.join("keep.md"), "Keep this file.").unwrap();
    std::fs::write(notes_dir.join("delete_me.md"), "Delete this file.").unwrap();

    let config = make_config(dir.path(), &notes_dir);

    // Sync with both files
    sync::run_sync(&config, Some("test"), false, false, false, true)
        .await
        .unwrap();
    let conn1 = db::open_db("test", &config.effective_db_dir()).unwrap();
    assert_eq!(db::count_documents(&conn1).unwrap(), 2);

    // Delete one file
    std::fs::remove_file(notes_dir.join("delete_me.md")).unwrap();

    // Second sync: deleted file should be removed
    sync::run_sync(&config, Some("test"), false, false, false, true)
        .await
        .unwrap();
    let conn2 = db::open_db("test", &config.effective_db_dir()).unwrap();
    assert_eq!(db::count_documents(&conn2).unwrap(), 1);

    let hashes = db::get_all_hashes(&conn2).unwrap();
    assert!(hashes.keys().any(|k| k.ends_with("keep.md")));
    assert!(!hashes.keys().any(|k| k.ends_with("delete_me.md")));
}

#[tokio::test]
async fn test_delete_detection_removes_from_fts() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();
    std::fs::write(
        notes_dir.join("gone.md"),
        "unique_xyzzy_term_for_testing purposes",
    )
    .unwrap();

    let config = make_config(dir.path(), &notes_dir);

    sync::run_sync(&config, Some("test"), false, false, false, true)
        .await
        .unwrap();

    // Verify the term is searchable
    let conn1 = db::open_db("test", &config.effective_db_dir()).unwrap();
    let before = search::search_fts(&conn1, "unique_xyzzy_term_for_testing", 5).unwrap();
    assert!(!before.is_empty(), "should find the unique term before deletion");

    std::fs::remove_file(notes_dir.join("gone.md")).unwrap();
    sync::run_sync(&config, Some("test"), false, false, false, true)
        .await
        .unwrap();

    let conn2 = db::open_db("test", &config.effective_db_dir()).unwrap();
    let after = search::search_fts(&conn2, "unique_xyzzy_term_for_testing", 5).unwrap();
    assert!(after.is_empty(), "deleted document should not be in FTS");
}

// ─── 7. brainjarignore ───────────────────────────────────────────────────────

#[test]
fn test_brainjarignore_excludes_by_filename() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();

    std::fs::write(dir.path().join(".brainjarignore"), "secret.md").unwrap();
    std::fs::write(notes_dir.join("secret.md"), "private content").unwrap();
    std::fs::write(notes_dir.join("public.md"), "public content").unwrap();

    let config = make_config(dir.path(), &notes_dir);
    let kb = config.knowledge_bases.get("test").unwrap();
    let watch_paths = config.expand_watch_paths(kb);
    let files = sync::collect_files(&config, &watch_paths);

    assert!(
        !files.keys().any(|k| k.ends_with("secret.md")),
        "secret.md should be excluded"
    );
    assert!(
        files.keys().any(|k| k.ends_with("public.md")),
        "public.md should be included"
    );
}

#[test]
fn test_brainjarignore_wildcard_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();

    std::fs::write(dir.path().join(".brainjarignore"), "*.draft.md").unwrap();
    std::fs::write(notes_dir.join("doc.draft.md"), "Draft version").unwrap();
    std::fs::write(notes_dir.join("final.md"), "Final version").unwrap();

    let config = make_config(dir.path(), &notes_dir);
    let kb = config.knowledge_bases.get("test").unwrap();
    let watch_paths = config.expand_watch_paths(kb);
    let files = sync::collect_files(&config, &watch_paths);

    assert!(
        files.keys().any(|k| k.ends_with("final.md")),
        "final.md should be indexed"
    );
    assert!(
        !files.keys().any(|k| k.ends_with("doc.draft.md")),
        "draft file should be excluded by wildcard pattern"
    );
}

#[test]
fn test_brainjarignore_comment_lines_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();

    // A comment line should not become a glob pattern
    std::fs::write(
        dir.path().join(".brainjarignore"),
        "# This is a comment\n*.tmp\n",
    )
    .unwrap();
    std::fs::write(notes_dir.join("doc.md"), "Content").unwrap();

    let config = make_config(dir.path(), &notes_dir);
    let kb = config.knowledge_bases.get("test").unwrap();
    let watch_paths = config.expand_watch_paths(kb);
    let files = sync::collect_files(&config, &watch_paths);

    assert!(
        files.keys().any(|k| k.ends_with("doc.md")),
        "doc.md should not be affected by comment lines"
    );
}

#[test]
fn test_no_brainjarignore_indexes_all_text_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir(&notes_dir).unwrap();

    std::fs::write(notes_dir.join("readme.md"), "# Readme").unwrap();
    std::fs::write(notes_dir.join("main.rs"), "fn main() {}").unwrap();
    std::fs::write(notes_dir.join("config.toml"), "[settings]").unwrap();

    let config = make_config(dir.path(), &notes_dir);
    let kb = config.knowledge_bases.get("test").unwrap();
    let watch_paths = config.expand_watch_paths(kb);
    let files = sync::collect_files(&config, &watch_paths);

    assert!(files.keys().any(|k| k.ends_with("readme.md")));
    assert!(files.keys().any(|k| k.ends_with("main.rs")));
    assert!(files.keys().any(|k| k.ends_with("config.toml")));
}

#[test]
fn test_brainjarignore_path_containing_excluded_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let notes_dir = dir.path().join("notes");
    let private_sub = notes_dir.join("private");
    std::fs::create_dir_all(&private_sub).unwrap();

    std::fs::write(dir.path().join(".brainjarignore"), "*private*").unwrap();
    std::fs::write(private_sub.join("secrets.md"), "secrets").unwrap();
    std::fs::write(notes_dir.join("public.md"), "public").unwrap();

    let config = make_config(dir.path(), &notes_dir);
    let kb = config.knowledge_bases.get("test").unwrap();
    let watch_paths = config.expand_watch_paths(kb);
    let files = sync::collect_files(&config, &watch_paths);

    assert!(files.keys().any(|k| k.ends_with("public.md")));
    // Paths containing "private" should be filtered by the glob
    assert!(!files.keys().any(|k| k.contains("private")));
}

// ─── 8. Hash-based change detection ─────────────────────────────────────────

#[test]
fn test_hash_content_is_deterministic() {
    let data = b"brainjar hash test content";
    assert_eq!(sync::hash_content(data), sync::hash_content(data));
}

#[test]
fn test_hash_content_differs_for_different_inputs() {
    assert_ne!(sync::hash_content(b"v1"), sync::hash_content(b"v2"));
}

#[test]
fn test_hash_content_is_64_hex_chars() {
    let h = sync::hash_content(b"some data");
    assert_eq!(h.len(), 64);
    assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_hash_content_empty_is_valid() {
    let h = sync::hash_content(b"");
    assert_eq!(h.len(), 64);
}

// ─── 9. collect_files edge cases ─────────────────────────────────────────────

#[test]
fn test_collect_files_nonexistent_path_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("does_not_exist");
    let config = make_config(dir.path(), &missing);
    let kb = config.knowledge_bases.get("test").unwrap();
    let watch_paths = config.expand_watch_paths(kb);
    let files = sync::collect_files(&config, &watch_paths);
    assert!(files.is_empty());
}

#[test]
fn test_collect_files_single_file_path() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("single.md");
    std::fs::write(&file_path, "# Single file").unwrap();

    // Point the KB at the file directly (not a dir)
    let config = make_config(dir.path(), &file_path);
    let kb = config.knowledge_bases.get("test").unwrap();
    let watch_paths = config.expand_watch_paths(kb);
    let files = sync::collect_files(&config, &watch_paths);

    assert_eq!(files.len(), 1);
    assert!(files.keys().any(|k| k.contains("single.md")));
}

#[test]
fn test_collect_files_skips_target_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("project");
    std::fs::create_dir(&data).unwrap();
    let target_dir = data.join("target");
    std::fs::create_dir_all(target_dir.join("debug")).unwrap();
    std::fs::write(target_dir.join("debug").join("out.rs"), "fn main(){}").unwrap();
    std::fs::write(data.join("lib.rs"), "pub fn lib(){}").unwrap();

    let config = make_config(&data, &data);
    let kb = config.knowledge_bases.get("test").unwrap();
    let watch_paths = config.expand_watch_paths(kb);
    let files = sync::collect_files(&config, &watch_paths);

    assert!(!files.keys().any(|k| k.contains("target")));
    assert!(files.keys().any(|k| k.ends_with("lib.rs")));
}

#[test]
fn test_collect_files_skips_node_modules() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("project");
    std::fs::create_dir(&data).unwrap();
    let nm = data.join("node_modules");
    std::fs::create_dir_all(&nm).unwrap();
    std::fs::write(nm.join("pkg.js"), "const x=1;").unwrap();
    std::fs::write(data.join("app.ts"), "const y=2;").unwrap();

    let config = make_config(&data, &data);
    let kb = config.knowledge_bases.get("test").unwrap();
    let watch_paths = config.expand_watch_paths(kb);
    let files = sync::collect_files(&config, &watch_paths);

    assert!(!files.keys().any(|k| k.contains("node_modules")));
    assert!(files.keys().any(|k| k.ends_with("app.ts")));
}

// ─── 10. DB operations ───────────────────────────────────────────────────────

#[test]
fn test_db_upsert_makes_content_searchable() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();

    db::upsert_document(
        &conn,
        "notes/test.md",
        "The quick brown fox jumps over the lazy dog",
        "h1",
    )
    .unwrap();

    let results = search::search_fts(&conn, "quick", 5).unwrap();
    assert!(!results.is_empty());
}

#[test]
fn test_db_delete_removes_from_fts() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();

    let unique_term = "xyzzy_unique_word_9876543";
    db::upsert_document(&conn, "del.md", unique_term, "h1").unwrap();

    let before = search::search_fts(&conn, unique_term, 5).unwrap();
    assert!(!before.is_empty());

    db::delete_document(&conn, "del.md").unwrap();
    let after = search::search_fts(&conn, unique_term, 5).unwrap();
    assert!(after.is_empty(), "FTS must not return deleted document");
}

#[test]
fn test_db_upsert_updates_fts_content() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();

    db::upsert_document(&conn, "doc.md", "old_term_abc content here", "h1").unwrap();
    db::upsert_document(&conn, "doc.md", "completely new content now", "h2").unwrap();

    // Old term should not match
    let old = search::search_fts(&conn, "old_term_abc", 5).unwrap();
    assert!(old.is_empty(), "stale FTS entry should be removed on update");

    // New term should match
    let new = search::search_fts(&conn, "new", 5).unwrap();
    assert!(!new.is_empty());
}

#[test]
fn test_db_meta_set_get_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();

    db::set_meta(&conn, "version", "1.0").unwrap();
    assert_eq!(db::get_meta(&conn, "version").unwrap().as_deref(), Some("1.0"));

    db::set_meta(&conn, "version", "2.0").unwrap();
    assert_eq!(db::get_meta(&conn, "version").unwrap().as_deref(), Some("2.0"));
}

#[test]
fn test_db_meta_missing_key_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();
    assert!(db::get_meta(&conn, "nonexistent").unwrap().is_none());
}

#[test]
fn test_db_vec_table_not_created_without_dims() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db("test", dir.path()).unwrap();
    assert!(!db::vec_table_exists(&conn));
}

// ─── 11. RRF math ────────────────────────────────────────────────────────────

#[test]
fn test_rrf_shared_document_outranks_single_source() {
    let fts = vec![
        ("shared".to_string(), 10.0),
        ("fts_only".to_string(), 5.0),
    ];
    let graph = vec![
        ("shared".to_string(), 8.0),
        ("graph_only".to_string(), 3.0),
    ];
    let merged = search::reciprocal_rank_fusion(vec![fts, graph], 60.0);

    let score = |name: &str| merged.iter().find(|(k, _)| k == name).unwrap().1;

    assert!(score("shared") > score("fts_only"));
    assert!(score("shared") > score("graph_only"));
}

#[test]
fn test_rrf_results_sorted_descending() {
    let set = vec![
        ("low".to_string(), 1.0),
        ("high".to_string(), 100.0),
        ("mid".to_string(), 42.0),
    ];
    let merged = search::reciprocal_rank_fusion(vec![set], 60.0);
    for i in 0..merged.len().saturating_sub(1) {
        assert!(merged[i].1 >= merged[i + 1].1);
    }
}

#[test]
fn test_rrf_exact_math() {
    // With k=60: rank-0 score = 1/61, rank-1 score = 1/62
    let set = vec![("a".to_string(), 100.0), ("b".to_string(), 1.0)];
    let merged = search::reciprocal_rank_fusion(vec![set], 60.0);
    let score_a = merged.iter().find(|(k, _)| k == "a").unwrap().1;
    let score_b = merged.iter().find(|(k, _)| k == "b").unwrap().1;
    assert!((score_a - 1.0 / 61.0).abs() < 1e-9);
    assert!((score_b - 1.0 / 62.0).abs() < 1e-9);
}

#[test]
fn test_rrf_empty_input() {
    assert!(search::reciprocal_rank_fusion(vec![], 60.0).is_empty());
}

#[test]
fn test_rrf_empty_inner_set() {
    assert!(search::reciprocal_rank_fusion(vec![vec![]], 60.0).is_empty());
}
