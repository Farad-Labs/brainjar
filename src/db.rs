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
/// The DB is stored at `<config_dir>/.brainjar/<kb_name>.db`.
/// Tables and FTS triggers are created on first open.
///
/// `vec_dimensions` — if > 0 **and** the `documents_vec` virtual table does
/// not yet exist, it will be created with that many float dimensions.
pub fn open_db(kb_name: &str, config_dir: &Path) -> Result<Connection> {
    open_db_with_dims(kb_name, config_dir, 0)
}

/// Like `open_db` but creates the `documents_vec` virtual table when
/// `vec_dimensions > 0` and the table is not present yet.
pub fn open_db_with_dims(kb_name: &str, config_dir: &Path, vec_dimensions: usize) -> Result<Connection> {
    let db_dir = config_dir.join(".brainjar");
    std::fs::create_dir_all(&db_dir)
        .with_context(|| format!("Failed to create .brainjar directory: {}", db_dir.display()))?;

    let db_path = db_dir.join(format!("{}.db", kb_name));
    let conn = Connection::open(&db_path)
        .with_context(|| format!("Failed to open database: {}", db_path.display()))?;

    // Enable WAL mode for better concurrent performance
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

    create_schema(&conn)?;

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
pub fn upsert_document(conn: &Connection, path: &str, content: &str, hash: &str) -> Result<()> {
    conn.execute(
        r#"INSERT INTO documents (path, content, content_hash, updated_at)
           VALUES (?1, ?2, ?3, datetime('now'))
           ON CONFLICT(path) DO UPDATE SET
               content      = excluded.content,
               content_hash = excluded.content_hash,
               updated_at   = excluded.updated_at"#,
        rusqlite::params![path, content, hash],
    )
    .with_context(|| format!("Failed to upsert document: {}", path))?;
    Ok(())
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
