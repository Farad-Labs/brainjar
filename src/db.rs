use anyhow::{Context, Result};
use rusqlite::ffi::sqlite3_auto_extension;
use rusqlite::Connection;
use sqlite_vec::sqlite3_vec_init;
use std::path::Path;

/// Register the sqlite-vec extension so it is automatically loaded for every
/// new `Connection`. Must be called **once**, before any `open_db()` call.
pub fn init_vec_extension() {
    #[allow(clippy::missing_transmute_annotations)]
    unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite3_vec_init as *const (),
        )));
    }
}

/// Open (or create) the SQLite database for a knowledge base.
/// `db_dir` is the directory that will contain `<kb_name>.db`.
/// (This is typically `config.effective_db_dir()`.)
/// Tables and FTS triggers are created on first open.
///
/// `vec_dimensions` — if > 0 **and** the `documents_vec` virtual table does
/// not yet exist, it will be created with that many float dimensions.
pub fn open_db(kb_name: &str, db_dir: &Path) -> Result<Connection> {
    open_db_with_dims(kb_name, db_dir, 0)
}

/// Like `open_db` but creates the `documents_vec` virtual table when
/// `vec_dimensions > 0` and the table is not present yet.
pub fn open_db_with_dims(kb_name: &str, db_dir: &Path, vec_dimensions: usize) -> Result<Connection> {
    std::fs::create_dir_all(db_dir)
        .with_context(|| format!("Failed to create db directory: {}", db_dir.display()))?;

    let db_path = db_dir.join(format!("{}.db", kb_name));
    let conn = Connection::open(&db_path)
        .with_context(|| format!("Failed to open database: {}", db_path.display()))?;

    // Enable WAL mode for better concurrent performance
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

    create_schema(&conn)?;
    run_migrations(&conn)?;

    if vec_dimensions > 0 {
        ensure_vec_table(&conn, vec_dimensions)?;
    }

    Ok(conn)
}

/// Create the `documents_vec` virtual table if it doesn't already exist.
/// `dimensions` must match the actual embedding dimensionality.
fn ensure_vec_table(conn: &Connection, dimensions: usize) -> Result<()> {
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='documents_vec'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    if !exists {
        let sql = format!(
            "CREATE VIRTUAL TABLE documents_vec USING vec0(\
             document_id INTEGER PRIMARY KEY, \
             embedding float[{}]\
             )",
            dimensions
        );
        conn.execute_batch(&sql)
            .context("Failed to create documents_vec virtual table")?;
    }

    Ok(())
}

fn create_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS documents (
            id           INTEGER PRIMARY KEY,
            path         TEXT UNIQUE NOT NULL,
            content      TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            extracted    INTEGER NOT NULL DEFAULT 0,
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
            path,
            content,
            content='documents',
            content_rowid='id'
        );

        -- Keep FTS in sync with documents table
        CREATE TRIGGER IF NOT EXISTS documents_ai AFTER INSERT ON documents BEGIN
            INSERT INTO documents_fts(rowid, path, content)
            VALUES (new.id, new.path, new.content);
        END;

        CREATE TRIGGER IF NOT EXISTS documents_ad AFTER DELETE ON documents BEGIN
            INSERT INTO documents_fts(documents_fts, rowid, path, content)
            VALUES ('delete', old.id, old.path, old.content);
        END;

        CREATE TRIGGER IF NOT EXISTS documents_au AFTER UPDATE ON documents BEGIN
            INSERT INTO documents_fts(documents_fts, rowid, path, content)
            VALUES ('delete', old.id, old.path, old.content);
            INSERT INTO documents_fts(rowid, path, content)
            VALUES (new.id, new.path, new.content);
        END;

        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT
        );

        CREATE TABLE IF NOT EXISTS vocabulary (
            word      TEXT PRIMARY KEY,
            frequency INTEGER DEFAULT 1
        );
        "#,
    )
    .context("Failed to create database schema")?;

    Ok(())
}

/// Version-based schema migrations. Safe to call on every open — each migration
/// is guarded by the stored `schema_version` and is idempotent.
fn run_migrations(conn: &Connection) -> Result<()> {
    // Get current schema version (0 if never set)
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE((SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'), 0)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    if version < 1 {
        // v0 → v1: track entity extraction completion per-document
        let has_extracted: bool = conn
            .prepare("SELECT COUNT(*) FROM pragma_table_info('documents') WHERE name='extracted'")
            .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
            .map(|count| count > 0)
            .unwrap_or(false);
        if !has_extracted {
            conn.execute_batch("ALTER TABLE documents ADD COLUMN extracted INTEGER NOT NULL DEFAULT 0;")?;
        }
        // Mark all existing docs as extracted — they were synced in v0.1.0
        conn.execute_batch("UPDATE documents SET extracted = 1;")?;
        conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', '1')", [])?;
    }

    // Future migrations go here:
    // if version < 2 { ... }

    Ok(())
}

/// Query the current content hashes for all documents in the DB.
/// Returns a map of path → content_hash.
pub fn get_all_hashes(conn: &Connection) -> Result<std::collections::HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT path, content_hash FROM documents")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (path, hash) = row?;
        map.insert(path, hash);
    }
    Ok(map)
}

/// Upsert a document into the documents table (triggers keep FTS in sync).
/// Always resets `extracted = 0` on update so that re-synced docs are re-extracted.
pub fn upsert_document(conn: &Connection, path: &str, content: &str, hash: &str) -> Result<()> {
    conn.execute(
        r#"INSERT INTO documents (path, content, content_hash, extracted, updated_at)
           VALUES (?1, ?2, ?3, 0, datetime('now'))
           ON CONFLICT(path) DO UPDATE SET
               content      = excluded.content,
               content_hash = excluded.content_hash,
               extracted    = 0,
               updated_at   = excluded.updated_at"#,
        rusqlite::params![path, content, hash],
    )
    .with_context(|| format!("Failed to upsert document: {}", path))?;
    Ok(())
}

/// Mark a document as successfully extracted.
pub fn mark_extracted(conn: &Connection, path: &str) -> Result<()> {
    conn.execute(
        "UPDATE documents SET extracted = 1 WHERE path = ?1",
        rusqlite::params![path],
    )
    .with_context(|| format!("Failed to mark extracted: {}", path))?;
    Ok(())
}

/// Return paths of all documents where `extracted = 0` (synced but not yet extracted).
pub fn get_unextracted_paths(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM documents WHERE extracted = 0")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut paths = Vec::new();
    for row in rows {
        paths.push(row?);
    }
    Ok(paths)
}

/// Delete a document by path (triggers keep FTS in sync).
/// Also removes the vector embedding if the documents_vec table exists.
pub fn delete_document(conn: &Connection, path: &str) -> Result<()> {
    // Grab the id before deleting so we can clean up the vec table.
    let doc_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM documents WHERE path = ?1",
            rusqlite::params![path],
            |row| row.get(0),
        )
        .ok();

    conn.execute("DELETE FROM documents WHERE path = ?1", rusqlite::params![path])
        .with_context(|| format!("Failed to delete document: {}", path))?;

    if let Some(id) = doc_id {
        delete_document_vec(conn, id);
    }

    Ok(())
}

/// Remove a vector embedding by document id (no-op if table doesn't exist).
pub fn delete_document_vec(conn: &Connection, doc_id: i64) {
    // Ignore errors — the table may not exist if embeddings are disabled.
    let _ = conn.execute(
        "DELETE FROM documents_vec WHERE document_id = ?1",
        rusqlite::params![doc_id],
    );
}

/// Upsert a vector embedding for a document id.
/// `embedding` is passed as raw f32 bytes (zerocopy::AsBytes).
pub fn upsert_document_vec(conn: &Connection, doc_id: i64, embedding_bytes: &[u8]) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO documents_vec(document_id, embedding) VALUES (?1, ?2)",
        rusqlite::params![doc_id, embedding_bytes],
    )
    .context("Failed to upsert document vector")?;
    Ok(())
}

/// Look up the document id for a given path.
pub fn get_document_id(conn: &Connection, path: &str) -> Result<Option<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM documents WHERE path = ?1")?;
    let mut rows = stmt.query(rusqlite::params![path])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

/// Check whether the documents_vec table exists.
pub fn vec_table_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='documents_vec'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

/// Set a metadata value.
pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

/// Get a metadata value.
pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
    let mut rows = stmt.query(rusqlite::params![key])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

/// Get document count.
pub fn count_documents(conn: &Connection) -> Result<i64> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM documents", [], |row| row.get(0))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create an in-memory SQLite connection with the brainjar schema.
    fn make_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Apply WAL + schema just like open_db_with_dims does
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;").unwrap();
        // create_schema uses the public create_schema fn
        // (we call it via a temp file round-trip — simpler to inline the DDL)
        conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS documents (
                id           INTEGER PRIMARY KEY,
                path         TEXT UNIQUE NOT NULL,
                content      TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                extracted    INTEGER NOT NULL DEFAULT 0,
                updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
                path,
                content,
                content='documents',
                content_rowid='id'
            );
            CREATE TRIGGER IF NOT EXISTS documents_ai AFTER INSERT ON documents BEGIN
                INSERT INTO documents_fts(rowid, path, content)
                VALUES (new.id, new.path, new.content);
            END;
            CREATE TRIGGER IF NOT EXISTS documents_ad AFTER DELETE ON documents BEGIN
                INSERT INTO documents_fts(documents_fts, rowid, path, content)
                VALUES ('delete', old.id, old.path, old.content);
            END;
            CREATE TRIGGER IF NOT EXISTS documents_au AFTER UPDATE ON documents BEGIN
                INSERT INTO documents_fts(documents_fts, rowid, path, content)
                VALUES ('delete', old.id, old.path, old.content);
                INSERT INTO documents_fts(rowid, path, content)
                VALUES (new.id, new.path, new.content);
            END;
            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT
            );
            CREATE TABLE IF NOT EXISTS vocabulary (
                word      TEXT PRIMARY KEY,
                frequency INTEGER DEFAULT 1
            );
        "#).unwrap();
        conn
    }

    #[test]
    fn test_open_db_creates_tables() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db("testdb", dir.path()).unwrap();

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='documents'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 1);

        let fts_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='documents_fts'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(fts_count, 1);
    }

    #[test]
    fn test_upsert_and_retrieve_document() {
        let conn = make_conn();
        upsert_document(&conn, "notes/hello.md", "Hello world", "hash1").unwrap();
        let count = count_documents(&conn).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_upsert_document_updates_existing() {
        let conn = make_conn();
        upsert_document(&conn, "notes/hello.md", "Original content", "hash1").unwrap();
        upsert_document(&conn, "notes/hello.md", "Updated content", "hash2").unwrap();

        // Still only 1 document
        assert_eq!(count_documents(&conn).unwrap(), 1);

        let hashes = get_all_hashes(&conn).unwrap();
        assert_eq!(hashes.get("notes/hello.md").map(|s| s.as_str()), Some("hash2"));
    }

    #[test]
    fn test_delete_document() {
        let conn = make_conn();
        upsert_document(&conn, "notes/delete_me.md", "Content", "hash1").unwrap();
        assert_eq!(count_documents(&conn).unwrap(), 1);

        delete_document(&conn, "notes/delete_me.md").unwrap();
        assert_eq!(count_documents(&conn).unwrap(), 0);
    }

    #[test]
    fn test_delete_nonexistent_document_is_ok() {
        let conn = make_conn();
        // Deleting something that doesn't exist should not error
        delete_document(&conn, "nonexistent/path.md").unwrap();
    }

    #[test]
    fn test_get_all_hashes_empty() {
        let conn = make_conn();
        let hashes = get_all_hashes(&conn).unwrap();
        assert!(hashes.is_empty());
    }

    #[test]
    fn test_get_all_hashes_multiple_docs() {
        let conn = make_conn();
        upsert_document(&conn, "a.md", "AAA", "hash_a").unwrap();
        upsert_document(&conn, "b.md", "BBB", "hash_b").unwrap();

        let hashes = get_all_hashes(&conn).unwrap();
        assert_eq!(hashes.len(), 2);
        assert_eq!(hashes["a.md"], "hash_a");
        assert_eq!(hashes["b.md"], "hash_b");
    }

    #[test]
    fn test_vec_table_exists_false_by_default() {
        let conn = make_conn();
        assert!(!vec_table_exists(&conn));
    }

    #[test]
    fn test_meta_set_and_get() {
        let conn = make_conn();
        set_meta(&conn, "last_sync", "2024-01-01T00:00:00Z").unwrap();
        let val = get_meta(&conn, "last_sync").unwrap();
        assert_eq!(val.as_deref(), Some("2024-01-01T00:00:00Z"));
    }

    #[test]
    fn test_meta_get_missing_key() {
        let conn = make_conn();
        let val = get_meta(&conn, "nonexistent").unwrap();
        assert!(val.is_none());
    }

    #[test]
    fn test_meta_upsert_overwrites() {
        let conn = make_conn();
        set_meta(&conn, "key", "v1").unwrap();
        set_meta(&conn, "key", "v2").unwrap();
        let val = get_meta(&conn, "key").unwrap();
        assert_eq!(val.as_deref(), Some("v2"));
    }

    #[test]
    fn test_get_document_id() {
        let conn = make_conn();
        upsert_document(&conn, "foo/bar.md", "Content", "h1").unwrap();
        let id = get_document_id(&conn, "foo/bar.md").unwrap();
        assert!(id.is_some());
    }

    #[test]
    fn test_get_document_id_missing() {
        let conn = make_conn();
        let id = get_document_id(&conn, "not/here.md").unwrap();
        assert!(id.is_none());
    }

    // ─── Migration tests ───────────────────────────────────────────────────

    #[test]
    fn test_fresh_db_has_schema_version_1_and_extracted_column() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db("test", dir.path()).unwrap();

        // schema_version should be 1
        let version = get_meta(&conn, "schema_version").unwrap();
        assert_eq!(version.as_deref(), Some("1"));

        // extracted column should exist and default to 0
        upsert_document(&conn, "a.md", "content", "h1").unwrap();
        let unextracted = get_unextracted_paths(&conn).unwrap();
        assert_eq!(unextracted.len(), 1);
    }

    #[test]
    fn test_existing_v0_db_migrated_on_open() {
        let dir = tempfile::tempdir().unwrap();
        // Create a v0 DB: full schema WITHOUT the extracted column and no schema_version
        {
            let db_path = dir.path().join("legacy.db");
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(r#"
                CREATE TABLE documents (
                    id           INTEGER PRIMARY KEY,
                    path         TEXT UNIQUE NOT NULL,
                    content      TEXT NOT NULL,
                    content_hash TEXT NOT NULL,
                    updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE VIRTUAL TABLE documents_fts USING fts5(
                    path, content, content='documents', content_rowid='id'
                );
                CREATE TRIGGER documents_ai AFTER INSERT ON documents BEGIN
                    INSERT INTO documents_fts(rowid, path, content)
                    VALUES (new.id, new.path, new.content);
                END;
                CREATE TRIGGER documents_ad AFTER DELETE ON documents BEGIN
                    INSERT INTO documents_fts(documents_fts, rowid, path, content)
                    VALUES ('delete', old.id, old.path, old.content);
                END;
                CREATE TRIGGER documents_au AFTER UPDATE ON documents BEGIN
                    INSERT INTO documents_fts(documents_fts, rowid, path, content)
                    VALUES ('delete', old.id, old.path, old.content);
                    INSERT INTO documents_fts(rowid, path, content)
                    VALUES (new.id, new.path, new.content);
                END;
                CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
                CREATE TABLE vocabulary (word TEXT PRIMARY KEY, frequency INTEGER DEFAULT 1);
                INSERT INTO documents (path, content, content_hash)
                    VALUES ('old.md', 'old content', 'oldhash');
            "#).unwrap();
        }
        // Re-open via open_db — migration should fire
        let conn = open_db("legacy", dir.path()).unwrap();

        // schema_version bumped to 1
        let version = get_meta(&conn, "schema_version").unwrap();
        assert_eq!(version.as_deref(), Some("1"));

        // existing rows get extracted=1 (migration marks all pre-existing docs as extracted)
        let unextracted = get_unextracted_paths(&conn).unwrap();
        assert_eq!(unextracted.len(), 0);
    }

    #[test]
    fn test_already_migrated_db_reopens_without_error() {
        let dir = tempfile::tempdir().unwrap();
        // First open migrates and sets schema_version = 1
        open_db("test", dir.path()).unwrap();
        // Second open should not error (migration is a no-op at v1)
        let conn = open_db("test", dir.path()).unwrap();
        let version = get_meta(&conn, "schema_version").unwrap();
        assert_eq!(version.as_deref(), Some("1"));
    }

    #[test]
    fn test_hash_content_deterministic() {
        let h1 = crate::sync::hash_content(b"hello world");
        let h2 = crate::sync::hash_content(b"hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_content_different_inputs() {
        let h1 = crate::sync::hash_content(b"hello");
        let h2 = crate::sync::hash_content(b"world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hash_content_is_hex_string() {
        let h = crate::sync::hash_content(b"test");
        // SHA256 produces 32 bytes = 64 hex chars
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
