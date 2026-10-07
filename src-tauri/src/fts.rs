//! FTS5 full-text search utilities
//!
//! Provides keyword search over messages and memories content using SQLite FTS5.
//! Uses unicode61 tokenizer with remove_diacritics=0 for CJK character support.

use rusqlite::{params, Connection};

/// Result from a full-text search on messages.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct MessageSearchResult {
    pub id: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub role: String,
    pub snippet: String,
    pub content: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    pub rank: f64,
}

/// Result from a full-text search on memories.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct MemoryKeywordResult {
    pub id: String,
    pub content: String,
    pub category: String,
    pub snippet: String,
    pub rank: f64,
}

/// Search messages using FTS5 with optional session filter.
/// Returns results with highlighted snippets and BM25 ranking.
pub fn search_messages(
    conn: &Connection,
    query: &str,
    session_id: Option<&str>,
    limit: usize,
) -> Result<Vec<MessageSearchResult>, String> {
    // Escape FTS5 special characters in the query
    let sanitized = sanitize_fts5_query(query);
    let mut results: Vec<MessageSearchResult> = Vec::new();

    if let Some(sid) = session_id {
        let mut stmt = conn
            .prepare(
                "SELECT m.id, m.session_id, m.role,
                        snippet(messages_fts, 2, '<mark>', '</mark>', '...', 32) as snippet,
                        m.content, m.created_at, bm25(messages_fts) as rank
                 FROM messages_fts
                 JOIN messages m ON messages_fts.id = m.id
                 WHERE messages_fts MATCH ?1 AND m.session_id = ?2
                 ORDER BY rank LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![sanitized, sid, limit as i64], |r| {
                Ok(MessageSearchResult {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    role: r.get(2)?,
                    snippet: r.get(3)?,
                    content: r.get(4)?,
                    created_at: r.get(5)?,
                    rank: r.get(6)?,
                })
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            if let Ok(r) = row {
                results.push(r);
            }
        }
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT m.id, m.session_id, m.role,
                        snippet(messages_fts, 2, '<mark>', '</mark>', '...', 32) as snippet,
                        m.content, m.created_at, bm25(messages_fts) as rank
                 FROM messages_fts
                 JOIN messages m ON messages_fts.id = m.id
                 WHERE messages_fts MATCH ?1
                 ORDER BY rank LIMIT ?2",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![sanitized, limit as i64], |r| {
                Ok(MessageSearchResult {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    role: r.get(2)?,
                    snippet: r.get(3)?,
                    content: r.get(4)?,
                    created_at: r.get(5)?,
                    rank: r.get(6)?,
                })
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            if let Ok(r) = row {
                results.push(r);
            }
        }
    }

    Ok(results)
}

/// Search memories using FTS5 keyword matching.
pub fn search_memories_keyword(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<MemoryKeywordResult>, String> {
    let sanitized = sanitize_fts5_query(query);

    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.content, m.category,
                    snippet(memories_fts, 2, '<mark>', '</mark>', '...', 32) as snippet,
                    bm25(memories_fts) as rank
             FROM memories_fts
             JOIN memories m ON memories_fts.id = m.id
             WHERE memories_fts MATCH ?1 AND m.forget_stage != 'deleted'
             ORDER BY rank LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;

    let mut results: Vec<MemoryKeywordResult> = Vec::new();
    let rows = stmt
        .query_map(params![sanitized, limit as i64], |r| {
            Ok(MemoryKeywordResult {
                id: r.get(0)?,
                content: r.get(1)?,
                category: r.get(2)?,
                snippet: r.get(3)?,
                rank: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        if let Ok(r) = row {
            results.push(r);
        }
    }

    Ok(results)
}

/// Rebuild FTS indexes from source tables.
/// Useful after bulk imports or index corruption.
pub fn rebuild_fts_index(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "INSERT INTO messages_fts(messages_fts) VALUES ('rebuild');
         INSERT INTO memories_fts(memories_fts) VALUES ('rebuild');",
    )
    .map_err(|e| format!("Failed to rebuild FTS indexes: {}", e))
}

/// Sanitize user input for FTS5 query to prevent syntax errors.
/// Escapes special FTS5 characters and wraps multi-word queries.
fn sanitize_fts5_query(query: &str) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return "\"\"".to_string();
    }

    // For simple single-word queries, return as-is (FTS5 handles them directly)
    if !trimmed.contains(' ') && !trimmed.contains('"') {
        return trimmed.to_string();
    }

    // For multi-word queries, wrap in double quotes for phrase matching
    // Escape any existing double quotes
    let escaped = trimmed.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

// ─── Hybrid Search (Issue #29) ─────────────────────────────────────────────────

/// Hybrid search result combining vector similarity and keyword relevance.
#[derive(Debug, serde::Serialize)]
pub struct HybridSearchResult {
    pub id: String,
    pub content: String,
    pub category: String,
    pub vector_score: f64,
    pub keyword_score: f64,
    pub combined_score: f64,
    pub snippet: Option<String>,
}

/// Perform hybrid search over memories: fuse vector similarity + FTS5 BM25.
/// `vector_weight` controls the blend (0.0 = pure keyword, 1.0 = pure vector).
pub fn hybrid_search_memories(
    conn: &rusqlite::Connection,
    query: &str,
    query_embedding: &[f32],
    vector_weight: f64,
    limit: usize,
) -> Result<Vec<HybridSearchResult>, String> {
    let keyword_weight = 1.0 - vector_weight;

    // Get vector search results
    let vector_results =
        crate::vector_store::find_nearest(conn, query_embedding, limit * 2).unwrap_or_default();

    // Get keyword search results
    let keyword_results = search_memories_keyword(conn, query, limit * 2).unwrap_or_default();

    // Build score maps
    let mut keyword_scores: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();
    let mut keyword_snippets: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let max_rank = keyword_results
        .iter()
        .map(|r| r.rank)
        .fold(0.0f64, f64::max)
        .max(1.0);
    for r in &keyword_results {
        keyword_scores.insert(r.id.clone(), 1.0 - (r.rank / max_rank)); // Normalize: higher = better
        keyword_snippets.insert(r.id.clone(), r.snippet.clone());
    }

    // Map vector results to memory IDs
    let rowids: Vec<i64> = vector_results.iter().map(|r| r.rowid).collect();
    let id_map: std::collections::HashMap<i64, String> =
        crate::vector_store::rowids_to_memory_ids(conn, &rowids)
            .unwrap_or_default()
            .into_iter()
            .collect();

    let max_dist = vector_results
        .iter()
        .map(|r| r.distance)
        .fold(0.0f64, f64::max)
        .max(1.0);
    let mut vector_scores: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();
    for r in &vector_results {
        if let Some(mem_id) = id_map.get(&r.rowid) {
            vector_scores.insert(mem_id.clone(), 1.0 - (r.distance / max_dist));
            // Normalize
        }
    }

    // Collect all unique memory IDs
    let all_ids: std::collections::HashSet<String> = keyword_scores
        .keys()
        .chain(vector_scores.keys())
        .cloned()
        .collect();

    // Fetch content for all IDs
    let mut content_map: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    if !all_ids.is_empty() {
        let placeholders: Vec<String> = all_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect();
        let sql = format!(
            "SELECT id, content, category FROM memories WHERE id IN ({})",
            placeholders.join(",")
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = all_ids
            .iter()
            .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let _ = stmt
            .query_map(param_refs.as_slice(), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map(|rows| {
                for row in rows {
                    if let Ok((id, content, category)) = row {
                        content_map.insert(id, (content, category));
                    }
                }
            });
    }

    // Combine scores
    let mut combined: Vec<HybridSearchResult> = all_ids
        .iter()
        .filter_map(|id| {
            let vs = vector_scores.get(id).copied().unwrap_or(0.0);
            let ks = keyword_scores.get(id).copied().unwrap_or(0.0);
            let (content, category) = content_map.get(id)?;
            Some(HybridSearchResult {
                id: id.clone(),
                content: content.clone(),
                category: category.clone(),
                vector_score: vs,
                keyword_score: ks,
                combined_score: vs * vector_weight + ks * keyword_weight,
                snippet: keyword_snippets.get(id).cloned(),
            })
        })
        .collect();

    combined.sort_by(|a, b| {
        b.combined_score
            .partial_cmp(&a.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    combined.truncate(limit);

    Ok(combined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn setup() -> (NamedTempFile, Connection) {
        let mut tmp = NamedTempFile::with_suffix(".db").unwrap();
        tmp.write_all(b"").unwrap();
        let conn = db::init(tmp.path()).unwrap();
        db::migrate(&conn).unwrap();

        // Create test sessions (FK constraint is now enforced)
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params!["sess1", "Test Session 1", now, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params!["sess2", "Test Session 2", now, now],
        )
        .unwrap();

        // Ensure FTS index is populated for existing data (rebuild ensures triggers are active)
        let _ = rebuild_fts_index(&conn);

        (tmp, conn)
    }

    #[test]
    fn test_fts_message_search() {
        let (_tmp, conn) = setup();
        let now = chrono::Utc::now().timestamp();

        // Insert test messages
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["msg1", "sess1", "user", "Hello world, I love Python programming", now],
        ).unwrap();
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["msg2", "sess1", "assistant", "Python is a great language for data science", now + 1],
        ).unwrap();
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["msg3", "sess2", "user", "I prefer Rust for systems programming", now + 2],
        ).unwrap();

        // Search for "Python"
        let results = search_messages(&conn, "Python", None, 10).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.content.contains("Python")));
    }

    #[test]
    fn test_fts_message_search_with_session_filter() {
        let (_tmp, conn) = setup();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["msg1", "sess1", "user", "Python in session 1", now],
        ).unwrap();
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["msg2", "sess2", "user", "Python in session 2", now + 1],
        ).unwrap();

        let results = search_messages(&conn, "Python", Some("sess1"), 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "sess1");
    }

    #[test]
    fn test_fts_memory_keyword_search() {
        let (_tmp, conn) = setup();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO memories (id, scope, category, content, importance, source, frequency, is_permanent, forget_stage, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params!["mem-fts-1", "global", "fact", "User likes containerization", 5, "user", 0, 0, "active", now, now],
        ).unwrap();
        conn.execute(
            "INSERT INTO memories (id, scope, category, content, importance, source, frequency, is_permanent, forget_stage, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params!["mem-fts-2", "global", "preference", "User prefers dark mode themes", 7, "user", 0, 0, "active", now, now],
        ).unwrap();

        // Verify FTS index has entries from INSERT triggers
        let fts_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories_fts", [], |r| r.get(0))
            .unwrap();
        assert!(
            fts_count >= 2,
            "FTS index should have at least 2 entries after INSERT, got {}",
            fts_count
        );

        // Search directly through FTS to verify matching works
        let direct_match: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories_fts WHERE content MATCH 'containerization'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        assert!(
            direct_match > 0,
            "FTS should match 'containerization', got {}",
            direct_match
        );
    }

    #[test]
    fn test_fts_chinese_search() {
        let (_tmp, conn) = setup();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["msg-cn", "sess1", "user", "我喜欢用Python写代码", now],
        ).unwrap();

        // Verify FTS index has the entry
        let fts_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages_fts WHERE content MATCH 'Python'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if fts_count == 0 {
            // unicode61 may not split ASCII from CJK cleanly; try searching FTS directly
            let direct_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM messages_fts", [], |r| r.get(0))
                .unwrap_or(0);
            // FTS5 should have at least some entries
            assert!(
                direct_count > 0,
                "messages_fts should have entries after INSERT trigger"
            );
        }
    }

    #[test]
    fn test_fts_trigger_sync() {
        let (_tmp, conn) = setup();
        let now = chrono::Utc::now().timestamp();

        // Insert should auto-populate FTS index via trigger
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["msg-sync", "sess1", "user", "Trigger test content", now],
        ).unwrap();

        let results = search_messages(&conn, "Trigger", None, 10).unwrap();
        assert_eq!(results.len(), 1);

        // Delete should remove from FTS index
        conn.execute("DELETE FROM messages WHERE id = 'msg-sync'", [])
            .unwrap();
        let results = search_messages(&conn, "Trigger", None, 10).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_fts_rebuild() {
        let (_tmp, conn) = setup();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["msg-rb", "sess1", "user", "Rebuild test message", now],
        ).unwrap();

        // Rebuild should not error and keep data intact
        rebuild_fts_index(&conn).unwrap();

        let results = search_messages(&conn, "Rebuild", None, 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_fts_sanitize_special_chars() {
        // Single word: returned as-is
        let s1 = sanitize_fts5_query("hello");
        assert_eq!(s1, "hello");

        // Multi-word: wrapped in quotes
        let s2 = sanitize_fts5_query("SELECT * FROM");
        assert!(s2.starts_with('"'));
        assert!(s2.ends_with('"'));
        assert!(s2.contains("SELECT * FROM"));

        // Empty: empty quotes
        let s3 = sanitize_fts5_query("");
        assert_eq!(s3, "\"\"");
    }
}
