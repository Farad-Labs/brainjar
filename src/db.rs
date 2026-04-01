use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

/// Open (or create) the SQLite database for a knowledge base.
/// The DB is stored at `<config_dir>/.brainjar/<kb_name>.db`.
/// Tables and FTS triggers are created on first open.
pub fn open_db(kb_name: &str, config_dir: &Path) -> Result<Connection> {
    let db_dir = config_dir.join(".brainjar");
    std::fs::create_dir_all(&db_dir)
        .with_context(|| format!("Failed to create .brainjar directory: {}", db_dir.display()))?;

    let db_path = db_dir.join(format!("{}.db", kb_name));
    let conn = Connection::open(&db_path)
        .with_context(|| format!("Failed to open database: {}", db_path.display()))?;

    // Enable WAL mode for better concurrent performance
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

    create_schema(&conn)?;

    Ok(conn)
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
pub fn delete_document(conn: &Connection, path: &str) -> Result<()> {
    conn.execute("DELETE FROM documents WHERE path = ?1", rusqlite::params![path])
        .with_context(|| format!("Failed to delete document: {}", path))?;
    Ok(())
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
