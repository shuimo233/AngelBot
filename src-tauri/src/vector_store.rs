//! Vector store using sqlite-vec for ANN (approximate nearest neighbor) search.
//!
//! Replaces the old O(n) brute-force cosine similarity approach with
//! sqlite-vec's vec0 virtual table, which supports KNN queries natively.
//! Issue #20
//!
//! Uses `sqlite3_auto_extension` to register the vec0 module globally,
//! then creates/manages the `memory_vectors` virtual table via raw SQL.

use rusqlite::{params, Connection};

/// Result from a vector similarity search
#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    pub rowid: i64,
    pub distance: f64,
}

/// Register sqlite-vec extension globally so all new connections have it.
/// Must be called once at startup, before opening any database connection.
///
/// # Safety
/// Calls `sqlite3_auto_extension` which modifies global SQLite state.
pub unsafe fn load_extension_globally() {
    static SQLITE_VEC_REGISTERED: std::sync::Once = std::sync::Once::new();

    SQLITE_VEC_REGISTERED.call_once(|| {
        use rusqlite::ffi::sqlite3_auto_extension;
        sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

/// Initialize the memory_vectors virtual table.
/// Safe to call at startup — uses IF NOT EXISTS.
pub fn init_vector_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS memory_vectors USING vec0(
            embedding float[1536]
        );",
    )
    .map_err(|e| format!("Failed to create memory_vectors table: {}", e))
}

/// Pad embedding to 1536 dimensions and convert to JSON array string for vec0.
fn embedding_to_json(embedding: &[f32]) -> String {
    let mut padded = embedding.to_vec();
    padded.resize(1536, 0.0);
    let list: Vec<String> = padded.iter().map(|x| x.to_string()).collect();
    format!("[{}]", list.join(","))
}

/// Insert or update a vector embedding for a memory.
pub fn upsert_embedding(conn: &Connection, rowid: i64, embedding: &[f32]) -> Result<(), String> {
    let json = embedding_to_json(embedding);

    // Delete existing entry if any
    conn.execute(
        "DELETE FROM memory_vectors WHERE rowid = ?1",
        params![rowid],
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        &format!("INSERT INTO memory_vectors(rowid, embedding) VALUES (?1, ?2)"),
        params![rowid, json],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Find the k nearest neighbors to the given query embedding.
pub fn find_nearest(
    conn: &Connection,
    query_embedding: &[f32],
    k: usize,
) -> Result<Vec<VectorSearchResult>, String> {
    let json = embedding_to_json(query_embedding);

    let mut stmt = conn
        .prepare(
            "SELECT rowid, distance FROM memory_vectors
         WHERE embedding MATCH ?1
         ORDER BY distance
         LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![json, k as i64], |r| {
            Ok(VectorSearchResult {
                rowid: r.get(0)?,
                distance: r.get(1)?,
            })
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// Delete a vector entry by rowid.
pub fn delete_embedding(conn: &Connection, rowid: i64) -> Result<(), String> {
    conn.execute(
        "DELETE FROM memory_vectors WHERE rowid = ?1",
        params![rowid],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Get the number of vectors currently stored.
pub fn vector_count(conn: &Connection) -> Result<usize, String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok(count as usize)
}

/// Ensure the memory_vector_map table exists for ID mapping.
pub fn ensure_rowid_mapping(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memory_vector_map (
            memory_id TEXT PRIMARY KEY,
            rowid INTEGER UNIQUE NOT NULL,
            FOREIGN KEY(memory_id) REFERENCES memories(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_memory_vector_map_rowid ON memory_vector_map(rowid);",
    )
    .map_err(|e| format!("Failed to create memory_vector_map: {}", e))
}

/// Get or create a rowid for a memory.
pub fn get_or_create_rowid(conn: &Connection, memory_id: &str) -> Result<i64, String> {
    // Try existing
    let existing: Option<i64> = conn
        .query_row(
            "SELECT rowid FROM memory_vector_map WHERE memory_id = ?1",
            params![memory_id],
            |r| r.get(0),
        )
        .ok();

    if let Some(rid) = existing {
        return Ok(rid);
    }

    // Create new — find max rowid
    let max_rowid: Option<i64> = conn
        .query_row(
            "SELECT COALESCE(MAX(rowid), 0) FROM memory_vector_map",
            [],
            |r| r.get(0),
        )
        .ok();

    let new_rowid = max_rowid.unwrap_or(0) + 1;

    conn.execute(
        "INSERT INTO memory_vector_map (memory_id, rowid) VALUES (?1, ?2)",
        params![memory_id, new_rowid],
    )
    .map_err(|e| e.to_string())?;

    Ok(new_rowid)
}

/// Look up memory IDs for a list of rowids.
pub fn rowids_to_memory_ids(
    conn: &Connection,
    rowids: &[i64],
) -> Result<Vec<(i64, String)>, String> {
    if rowids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders: Vec<String> = rowids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect();
    let sql = format!(
        "SELECT rowid, memory_id FROM memory_vector_map WHERE rowid IN ({})",
        placeholders.join(",")
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;

    let param_refs: Vec<Box<dyn rusqlite::types::ToSql>> = rowids
        .iter()
        .map(|r| Box::new(*r) as Box<dyn rusqlite::types::ToSql>)
        .collect();

    let rows = stmt
        .query_map(
            rusqlite::params_from_iter(param_refs.iter().map(|p| p.as_ref())),
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Connection {
        unsafe {
            crate::vector_store::load_extension_globally();
        }
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                scope TEXT NOT NULL,
                category TEXT NOT NULL,
                content TEXT NOT NULL
            );
            PRAGMA journal_mode=WAL;
            PRAGMA foreign_keys=OFF;",
        )
        .unwrap();
        init_vector_table(&conn).unwrap();
        ensure_rowid_mapping(&conn).unwrap();
        conn
    }

    #[test]
    fn test_insert_and_search() {
        let conn = setup();
        upsert_embedding(&conn, 1, &[1.0, 0.0, 0.0]).unwrap();
        upsert_embedding(&conn, 2, &[0.0, 1.0, 0.0]).unwrap();
        let results = find_nearest(&conn, &[1.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].rowid, 1);
    }

    #[test]
    fn test_upsert_replaces() {
        let conn = setup();
        upsert_embedding(&conn, 1, &[1.0, 0.0, 0.0]).unwrap();
        upsert_embedding(&conn, 1, &[0.0, 1.0, 0.0]).unwrap();
        let results = find_nearest(&conn, &[0.0, 1.0, 0.0], 1).unwrap();
        assert_eq!(results[0].rowid, 1);
    }

    #[test]
    fn test_rowid_mapping() {
        let conn = setup();
        let rid1 = get_or_create_rowid(&conn, "mem-a").unwrap();
        let rid2 = get_or_create_rowid(&conn, "mem-b").unwrap();
        assert_ne!(rid1, rid2);
        assert_eq!(get_or_create_rowid(&conn, "mem-a").unwrap(), rid1);
        let map = rowids_to_memory_ids(&conn, &[rid1, rid2]).unwrap();
        assert_eq!(map.len(), 2);
    }
}
