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
        ensure_chunks_vec_table(&conn, vec_dimensions)?;
    }

    Ok(conn)
}

/// Create the `documents_vec` virtual table if it doesn't already exist.
/// `dimensions` must match the actual embedding dimensionality.
/// Extract the embedding dimension from a vec0 virtual table's CREATE sql.
/// Returns None if the table doesn't exist or the SQL can't be parsed.
fn get_vec_table_dimensions(conn: &Connection, table_name: &str) -> Option<usize> {
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
            rusqlite::params![table_name],
            |row| row.get(0),
        )
        .ok()?;
    // Parse "float[1024]" from the CREATE VIRTUAL TABLE sql
    let start = sql.find("float[")? + 6;
    let end = sql[start..].find(']')? + start;
    sql[start..end].parse().ok()
}

fn ensure_vec_table(conn: &Connection, dimensions: usize) -> Result<()> {
    let existing_dims = get_vec_table_dimensions(conn, "documents_vec");

    match existing_dims {
        Some(d) if d == dimensions => return Ok(()), // already correct
        Some(_) => {
            // Dimension mismatch — drop old table so it can be recreated with new dims.
            // All embeddings must be re-indexed anyway when the model changes.
            conn.execute_batch("DROP TABLE IF EXISTS documents_vec")
                .context("Failed to drop documents_vec for dimension change")?;
        }
        None => {} // table doesn't exist yet
    }

    let sql = format!(
        "CREATE VIRTUAL TABLE documents_vec USING vec0(\
         document_id INTEGER PRIMARY KEY, \
         embedding float[{}]\
         )",
        dimensions
    );
    conn.execute_batch(&sql)
        .context("Failed to create documents_vec virtual table")?;

    Ok(())
}

/// Public wrapper: ensure chunks_vec table matches expected dimensions.
/// Drops and recreates if dimensions changed.
pub fn recreate_chunks_vec_if_needed(conn: &Connection, dimensions: usize) -> Result<()> {
    ensure_chunks_vec_table(conn, dimensions)
}

/// Create the `chunks_vec` virtual table if it doesn't already exist.
/// If the table exists with a different dimension count, it is dropped and
/// recreated — the embeddings will be re-generated on the next sync.
fn ensure_chunks_vec_table(conn: &Connection, dimensions: usize) -> Result<()> {
    let existing_dims = get_vec_table_dimensions(conn, "chunks_vec");

    match existing_dims {
        Some(d) if d == dimensions => return Ok(()), // already correct
        Some(_) => {
            // Dimension mismatch — drop and recreate.
            conn.execute_batch("DROP TABLE IF EXISTS chunks_vec")
                .context("Failed to drop chunks_vec for dimension change")?;
        }
        None => {} // table doesn't exist yet
    }

    let sql = format!(
        "CREATE VIRTUAL TABLE chunks_vec USING vec0(\
         chunk_id INTEGER PRIMARY KEY, \
         embedding float[{}]\
         )",
        dimensions
    );
    conn.execute_batch(&sql)
        .context("Failed to create chunks_vec virtual table")?;

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

    if version < 2 {
        // Create chunks table and FTS
        conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS chunks (
                id         INTEGER PRIMARY KEY,
                doc_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
                content    TEXT NOT NULL,
                line_start INTEGER NOT NULL,
                line_end   INTEGER NOT NULL,
                chunk_type TEXT
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                content,
                content='chunks',
                content_rowid='id'
            );

            CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
                INSERT INTO chunks_fts(rowid, content) VALUES (new.id, new.content);
            END;
            CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES ('delete', old.id, old.content);
            END;
            CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES ('delete', old.id, old.content);
                INSERT INTO chunks_fts(rowid, content) VALUES (new.id, new.content);
            END;
        "#)?;

        // Force re-sync to generate chunks
        conn.execute_batch("UPDATE documents SET extracted = 0;")?;
        conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', '2')", [])?;
    }

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

/// Return all document paths in the database.
pub fn get_all_paths(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM documents")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    let mut paths = Vec::new();
    for row in rows {
        paths.push(row?);
    }
    Ok(paths)
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

// ─── Chunk functions ─────────────────────────────────────────────────────────

/// A row from the chunks table, used by neighboring chunk queries.
#[derive(Debug, Clone)]
pub struct ChunkRow {
    pub chunk_id: i64,
    pub doc_id: i64,
    pub content: String,
    pub line_start: usize,
    pub line_end: usize,
    pub chunk_type: String,
}

/// A result from chunks FTS search.
#[derive(Debug, Clone)]
pub struct ChunkFtsResult {
    pub chunk_id: i64,
    pub doc_id: i64,
    pub content: String,
    pub line_start: usize,
    pub line_end: usize,
    pub chunk_type: String,
    pub path: String,
    pub score: f64,
}

/// Insert a chunk and return its rowid.
pub fn insert_chunk(
    conn: &Connection,
    doc_id: i64,
    content: &str,
    line_start: usize,
    line_end: usize,
    chunk_type: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO chunks (doc_id, content, line_start, line_end, chunk_type) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![doc_id, content, line_start as i64, line_end as i64, chunk_type],
    )
    .context("Failed to insert chunk")?;
    Ok(conn.last_insert_rowid())
}

/// Delete all chunks for a document (by doc_id).
pub fn delete_chunks_for_doc(conn: &Connection, doc_id: i64) -> Result<()> {
    // Delete chunk vectors first (if table exists)
    let _ = conn.execute(
        "DELETE FROM chunks_vec WHERE chunk_id IN (SELECT id FROM chunks WHERE doc_id = ?1)",
        rusqlite::params![doc_id],
    );
    conn.execute(
        "DELETE FROM chunks WHERE doc_id = ?1",
        rusqlite::params![doc_id],
    )
    .context("Failed to delete chunks for doc")?;
    Ok(())
}

/// Get a single chunk with its parent document path.
/// Returns (chunk_id, doc_id, content, line_start, line_end, chunk_type, file_path).
pub fn get_chunk(
    conn: &Connection,
    chunk_id: i64,
) -> Result<(i64, i64, String, usize, usize, String, String)> {
    conn.query_row(
        "SELECT c.id, c.doc_id, c.content, c.line_start, c.line_end,
                COALESCE(c.chunk_type, ''), d.path
         FROM chunks c
         JOIN documents d ON d.id = c.doc_id
         WHERE c.id = ?1",
        rusqlite::params![chunk_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? as usize,
                row.get::<_, i64>(4)? as usize,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        },
    )
    .context("Chunk not found")
}

/// (chunk_id, content, line_start, line_end, chunk_type)
type ChunkTuple = (i64, String, usize, usize, String);

/// Get all chunks for a document.
/// Returns vec of (chunk_id, content, line_start, line_end, chunk_type).
pub fn get_chunks_for_doc(
    conn: &Connection,
    doc_id: i64,
) -> Result<Vec<ChunkTuple>> {
    let mut stmt = conn.prepare(
        "SELECT id, content, line_start, line_end, COALESCE(chunk_type, '')
         FROM chunks WHERE doc_id = ?1
         ORDER BY line_start",
    )?;
    let rows = stmt.query_map(rusqlite::params![doc_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)? as usize,
            row.get::<_, i64>(3)? as usize,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Get chunks neighboring a given chunk_id (same document, ordered by line_start).
/// Returns (before_chunks, this_chunk, after_chunks).
pub fn get_neighboring_chunks(
    conn: &Connection,
    chunk_id: i64,
    before: usize,
    after: usize,
) -> Result<(Vec<ChunkRow>, ChunkRow, Vec<ChunkRow>)> {
    // First get the target chunk
    let (_, doc_id, content, line_start, line_end, chunk_type, _) = get_chunk(conn, chunk_id)?;
    let this_chunk = ChunkRow { chunk_id, doc_id, content, line_start, line_end, chunk_type };

    // Get all chunks for this document ordered by line_start
    let all = get_chunks_for_doc(conn, doc_id)?;
    let pos = all.iter().position(|(id, _, _, _, _)| *id == chunk_id);

    let pos = match pos {
        Some(p) => p,
        None => return Ok((vec![], this_chunk, vec![])),
    };

    let before_chunks: Vec<ChunkRow> = all[..pos]
        .iter()
        .rev()
        .take(before)
        .rev()
        .map(|(cid, c, ls, le, ct)| ChunkRow {
            chunk_id: *cid,
            doc_id,
            content: c.clone(),
            line_start: *ls,
            line_end: *le,
            chunk_type: ct.clone(),
        })
        .collect();

    let after_chunks: Vec<ChunkRow> = all[pos + 1..]
        .iter()
        .take(after)
        .map(|(cid, c, ls, le, ct)| ChunkRow {
            chunk_id: *cid,
            doc_id,
            content: c.clone(),
            line_start: *ls,
            line_end: *le,
            chunk_type: ct.clone(),
        })
        .collect();

    Ok((before_chunks, this_chunk, after_chunks))
}

/// Fetch the first chunk of a document by file path.
#[allow(clippy::type_complexity)]
pub fn get_first_chunk_for_file(
    conn: &Connection,
    file_path: &str,
) -> Result<Option<(i64, String, i64, i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.content, c.line_start, c.line_end, COALESCE(c.chunk_type, '')
         FROM chunks c
         JOIN documents d ON d.id = c.doc_id
         WHERE d.path = ?1
         ORDER BY c.line_start ASC
         LIMIT 1",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![file_path], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    match rows.next() {
        Some(Ok(tuple)) => Ok(Some(tuple)),
        _ => Ok(None),
    }
}

/// FTS search over chunks_fts. Returns matches with BM25 score.
pub fn search_chunks_fts(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<ChunkFtsResult>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.doc_id, c.content, c.line_start, c.line_end,
                COALESCE(c.chunk_type, ''), d.path,
                -bm25(chunks_fts) AS score
         FROM chunks_fts
         JOIN chunks c ON c.id = chunks_fts.rowid
         JOIN documents d ON d.id = c.doc_id
         WHERE chunks_fts MATCH ?1
         ORDER BY score DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![query, limit as i64],
        |row| {
            Ok(ChunkFtsResult {
                chunk_id: row.get(0)?,
                doc_id: row.get(1)?,
                content: row.get(2)?,
                line_start: row.get::<_, i64>(3)? as usize,
                line_end: row.get::<_, i64>(4)? as usize,
                chunk_type: row.get(5)?,
                path: row.get(6)?,
                score: row.get(7)?,
            })
        },
    )?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Upsert a chunk vector embedding.
pub fn upsert_chunk_vec(conn: &Connection, chunk_id: i64, embedding: &[u8]) -> Result<()> {
    // vec0 virtual tables don't support INSERT OR REPLACE —
    // delete first, then insert.
    conn.execute(
        "DELETE FROM chunks_vec WHERE chunk_id = ?1",
        rusqlite::params![chunk_id],
    ).ok(); // ignore if row doesn't exist
    conn.execute(
        "INSERT INTO chunks_vec(chunk_id, embedding) VALUES (?1, ?2)",
        rusqlite::params![chunk_id, embedding],
    )
    .with_context(|| format!("Failed to upsert chunk vector (id={}, embed_len={})", chunk_id, embedding.len()))?;
    Ok(())
}

/// Remove a chunk vector embedding by chunk id (no-op if table doesn't exist).
pub fn delete_chunk_vec(conn: &Connection, chunk_id: i64) {
    let _ = conn.execute(
        "DELETE FROM chunks_vec WHERE chunk_id = ?1",
        rusqlite::params![chunk_id],
    );
}

/// Get the raw content of a document by its id.
pub fn get_document_content(conn: &Connection, doc_id: i64) -> Result<String> {
    conn.query_row(
        "SELECT content FROM documents WHERE id = ?1",
        rusqlite::params![doc_id],
        |row| row.get::<_, String>(0),
    )
    .context("Document not found")
}

/// Count total chunks in the database.
pub fn count_chunks(conn: &Connection) -> Result<usize> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
    Ok(count as usize)
}

/// Check whether the chunks_vec table exists.
pub fn chunks_vec_table_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chunks_vec'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
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

        // schema_version should be 2 (latest)
        let version = get_meta(&conn, "schema_version").unwrap();
        assert_eq!(version.as_deref(), Some("2"));

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

        // schema_version bumped to 2 (latest)
        let version = get_meta(&conn, "schema_version").unwrap();
        assert_eq!(version.as_deref(), Some("2"));

        // v2 migration sets extracted=0 for re-chunking; re-open won't have extracted=1 anymore
        // The existing doc should still be present
        let paths = get_all_paths(&conn).unwrap();
        assert!(paths.contains(&"old.md".to_string()));
    }

    #[test]
    fn test_already_migrated_db_reopens_without_error() {
        let dir = tempfile::tempdir().unwrap();
        // First open migrates and sets schema_version = 2
        open_db("test", dir.path()).unwrap();
        // Second open should not error (migration is a no-op at v2)
        let conn = open_db("test", dir.path()).unwrap();
        let version = get_meta(&conn, "schema_version").unwrap();
        assert_eq!(version.as_deref(), Some("2"));
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
