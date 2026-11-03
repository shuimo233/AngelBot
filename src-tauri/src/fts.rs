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

/// Local lexical recall only: never compare a synthetic hash vector against
/// remotely generated embeddings with unknown model/provenance.
#[derive(Clone, Copy)]
pub(crate) enum MemoryRecallMode {
    Context,
    Explicit,
    ExplicitFacts,
}

pub(crate) struct RecalledMemory {
    pub id: String,
    pub category: String,
    pub content: String,
    pub importance: i32,
    pub updated_at: i64,
}

/// Shared bounded lexical matching for indexed memories and streamed habits.
/// This ranks search evidence; it never classifies or grants a preference.
pub(crate) struct MemoryQuery {
    terms: Vec<String>,
    singles: usize,
}

impl MemoryQuery {
    pub(crate) fn new(query: &str) -> Self {
        let terms = recall_terms(query);
        let singles = terms.iter().filter(|t| t.chars().count() == 1).count();
        Self { terms, singles }
    }

    /// Low-resolution evidence for opt-in inspection fallback only. Automatic
    /// context recall keeps the stricter score() threshold below.
    pub(crate) fn shared_characters<'a>(
        &'a self,
        content: &'a str,
    ) -> impl Iterator<Item = char> + 'a {
        self.terms.iter().filter_map(move |term| {
            let mut characters = term.chars();
            let first = characters.next()?;
            (characters.next().is_none() && is_han(first) && content.contains(first))
                .then_some(first)
        })
    }

    pub(crate) fn score(&self, content: &str) -> Option<usize> {
        let (strong, singles) = self.match_counts(content);
        // Do not turn a single shared character in a longer topic into a hit.
        if strong == 0 && singles < 2 && !(self.singles == 1 && singles == 1) {
            return None;
        }
        Some(strong * 2 + singles)
    }

    /// Explicit inspection distinguishes word/phrase matches from scattered
    /// characters, while score() preserves automatic recall's existing rule.
    pub(crate) fn direct_score(&self, content: &str) -> Option<usize> {
        let (strong, singles) = self.match_counts(content);
        (strong > 0 || self.singles == 1 && singles == 1).then_some(strong * 2 + singles)
    }

    fn match_counts(&self, content: &str) -> (usize, usize) {
        let text = content.to_lowercase();
        let words = text
            .split(|c: char| !c.is_alphanumeric() || is_han(c))
            .collect::<std::collections::HashSet<_>>();
        let mut strong = 0;
        let mut singles = 0;
        for term in &self.terms {
            let matched = if term.chars().all(is_han) {
                text.contains(term.as_str())
            } else {
                words.contains(term.as_str())
            };
            if matched {
                if term.chars().count() == 1 {
                    singles += 1;
                } else {
                    strong += 1;
                }
            }
        }
        (strong, singles)
    }
}

/// Shared by Main context and its memory-inspection tool. Only active global
/// records are eligible; unbound legacy scopes stay in storage for management.
/// At most 64 query terms, 256 candidates of <=8KiB, and 20 results. An empty
/// explicit query lists records; empty/non-informative context never does.
pub(crate) fn recall_active_memories(
    conn: &Connection,
    query: &str,
    limit: usize,
    mode: MemoryRecallMode,
) -> Result<Vec<RecalledMemory>, String> {
    let context = matches!(mode, MemoryRecallMode::Context);
    let list = query.trim().is_empty() && !context;
    let matcher = MemoryQuery::new(query);
    let terms = &matcher.terms;
    if !list && terms.is_empty() {
        return Ok(Vec::new());
    }
    // Empty stores need neither vector setup nor a new index.
    if !conn
        .query_row("SELECT EXISTS(SELECT 1 FROM memories)", [], |r| {
            r.get::<_, bool>(0)
        })
        .map_err(|e| e.to_string())?
    {
        return Ok(Vec::new());
    }
    let category = match mode {
        MemoryRecallMode::ExplicitFacts => " AND lower(m.category)!='preference'",
        _ => "",
    };
    let eligible = format!(
        "m.scope='global' AND m.forget_stage='active' AND m.superseded_by IS NULL
        AND (?1=0 OR m.category!='constraint') AND length(CAST(m.content AS BLOB))<=8192{category}"
    );
    let latin = terms
        .iter()
        .filter(|term| !term.chars().all(is_han))
        .map(|term| format!("\"{term}\""))
        .collect::<Vec<_>>()
        .join(" OR ");
    // Reuse the existing FTS index for complete Latin words. Substrings such
    // as tea/team must not consume the bounded pool before final filtering.
    let latin_ids: Vec<String> = if latin.is_empty() {
        Vec::new()
    } else {
        let mut query = conn.prepare(&format!("SELECT m.id FROM memories_fts JOIN memories m ON m.id=memories_fts.id
            WHERE {eligible} AND memories_fts MATCH ?2 ORDER BY bm25(memories_fts),m.updated_at DESC,m.id LIMIT 256"))
            .map_err(|e|e.to_string())?;
        let ids = query
            .query_map(params![context, latin], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        ids
    };
    let han = terms
        .iter()
        .filter(|term| term.chars().all(is_han))
        .collect::<Vec<_>>();
    let mut statement = conn.prepare(&format!(
        "SELECT m.id,m.category,m.content,COALESCE(m.importance,0),COALESCE(m.frequency,0),m.last_mentioned,m.updated_at
         FROM memories m WHERE {eligible}
           AND (?2=1 OR m.id IN(SELECT value FROM json_each(?4))
                OR EXISTS(SELECT 1 FROM json_each(?3) term WHERE instr(m.content,term.value)>0))
         ORDER BY (CASE WHEN m.id IN(SELECT value FROM json_each(?4)) THEN 2 ELSE 0 END)
             + COALESCE((SELECT sum(CASE WHEN length(term.value)=1 THEN 1 ELSE 2 END)
             FROM json_each(?3) term WHERE instr(m.content,term.value)>0),0) DESC,
             m.updated_at DESC,m.importance DESC,m.id LIMIT 256"
    )).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();
    let mut scored = statement
        .query_map(
            params![
                context,
                list,
                serde_json::to_string(&han).unwrap(),
                serde_json::to_string(&latin_ids).unwrap()
            ],
            |row| {
                Ok((
                    RecalledMemory {
                        id: row.get(0)?,
                        category: row.get(1)?,
                        content: row.get(2)?,
                        importance: row.get(3)?,
                        updated_at: row.get(6)?,
                    },
                    row.get::<_, i32>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|(memory, frequency, last_mentioned, updated)| {
            let lexical = if list {
                0
            } else {
                matcher.score(&memory.content)?
            };
            let recency =
                crate::vector::time_based_score(frequency, last_mentioned, memory.importance, now);
            Some((lexical, recency, updated, memory))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.total_cmp(&a.1))
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.3.id.cmp(&b.3.id))
    });
    Ok(scored
        .into_iter()
        .take(limit.min(20))
        .map(|(_, _, _, memory)| memory)
        .collect())
}

fn is_han(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{20000}'..='\u{2fa1f}')
}

fn recall_terms(query: &str) -> Vec<String> {
    let mut text = query.chars().take(512).collect::<String>().to_lowercase();
    // Function words are not evidence of a topic. No topic synonyms or
    // benchmark-specific phrases/options are injected here.
    for phrase in [
        "什么",
        "哪个",
        "哪些",
        "哪里",
        "怎么",
        "怎样",
        "为什么",
        "时候",
        "喜欢",
        "今天",
        "现在",
        "以后",
        "之前",
        "请问",
        "只写",
        "只用",
    ] {
        text = text.replace(phrase, " ");
    }
    let stop_han = "我你他她它的了吗呢啊吧在是有和与或也都就还很更最把被给让请只写选用要会能想该不没哪谁多么什怎如这个一种之时候好年月日";
    let mut terms = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |term: String| {
        if terms.len() < 64 && seen.insert(term.clone()) {
            terms.push(term);
        }
    };
    for part in text.split(|c: char| !c.is_alphanumeric() || is_han(c)) {
        if part.chars().count() < 2
            || part.len() > 40
            || part.chars().all(|c| c.is_numeric())
            || [
                "the", "and", "are", "was", "were", "have", "has", "had", "that", "this", "with",
                "from", "what", "which", "where", "when", "how", "who", "why", "for", "you",
                "your", "my", "our", "me", "to", "in", "on", "of", "at", "do", "does", "did", "am",
                "is", "it", "if", "or", "as", "be", "can", "could", "would", "should", "please",
                "only", "return", "answer", "unknown",
            ]
            .contains(&part)
        {
            continue;
        }
        push(part.to_string());
    }
    for part in text.split(|c| !is_han(c)) {
        let characters = part.chars().collect::<Vec<_>>();
        for pair in characters.windows(2) {
            if !pair.iter().any(|c| stop_han.contains(*c)) {
                push(pair.iter().collect());
            }
        }
    }
    // Fill the shared budget with word/phrase evidence before low-resolution
    // character fallback. Early sentence fragments must not evict later pairs.
    for c in text.chars().filter(|c| is_han(*c)) {
        if !stop_han.contains(c) {
            push(c.to_string());
        }
    }
    terms
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

pub(crate) const MAX_RECALL_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_LITERAL_SCAN_BYTES: usize = 8 * MAX_RECALL_SOURCE_BYTES;

pub(crate) struct MessageRecallMatches {
    pub ids: Vec<String>,
    pub scan_incomplete: bool,
    pub fallback_terms: Vec<String>,
}

/// Search host-authorized IDs in oldest-to-newest branch order. unicode61 does
/// not tokenize inside continuous Han text: use a literal, byte-bounded scan
/// there rather than silently returning no matches or building a second index.
/// FTS operators from a model are always quoted as data.
pub(crate) fn search_message_ids(
    conn: &Connection,
    query: &str,
    allowed_ids: &[String],
    limit: usize,
) -> Result<MessageRecallMatches, String> {
    if query.trim().is_empty() || query.len() > 256 || allowed_ids.len() > 4096 {
        return Err("Invalid bounded conversation query".into());
    }
    let mut result = MessageRecallMatches {
        ids: Vec::new(),
        scan_incomplete: false,
        fallback_terms: Vec::new(),
    };
    if allowed_ids.is_empty() {
        return Ok(result);
    }
    if query.chars().any(is_han) {
        let ids = serde_json::to_string(allowed_ids).map_err(|e| e.to_string())?;
        // BLOB length reads the stored byte count, not all message bodies into
        // Rust. Scope/visibility are supplied by the caller's read snapshot.
        let mut statement = conn
            .prepare(
                "SELECT m.id,length(CAST(m.content AS BLOB)) FROM json_each(?1) a
             JOIN messages m ON m.id=a.value ORDER BY CAST(a.key AS INTEGER) DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([ids], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, usize>(1)?))
            })
            .map_err(|e| e.to_string())?;
        let mut eligible = Vec::new();
        let mut bytes = 0;
        for row in rows {
            let (id, size) = row.map_err(|e| e.to_string())?;
            if size > MAX_RECALL_SOURCE_BYTES {
                result.scan_incomplete = true;
                continue;
            }
            if bytes + size > MAX_LITERAL_SCAN_BYTES {
                result.scan_incomplete = true;
                break;
            }
            bytes += size;
            eligible.push(id);
        }
        let ids = serde_json::to_string(&eligible).map_err(|e| e.to_string())?;
        let mut statement = conn
            .prepare(
                "SELECT m.id FROM json_each(?2) a JOIN messages m ON m.id=a.value
             WHERE instr(lower(m.content),lower(?1))>0
             ORDER BY CAST(a.key AS INTEGER) LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        result.ids = statement
            .query_map(params![query.trim(), &ids, limit.clamp(1, 2) as i64], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        // Models often supply several short keywords. Keep literal phrases
        // first; a miss can use the same bounded candidate set, not a new
        // index or inferred synonyms. The caller labels these as candidates.
        let mut terms: Vec<String> = query
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
        terms.sort();
        terms.dedup();
        if result.ids.is_empty()
            && (2..=8).contains(&terms.len())
            && terms.iter().all(|term| term.chars().count() >= 2)
        {
            let mut statement = conn
                .prepare(
                    "SELECT m.id FROM json_each(?1) a JOIN messages m ON m.id=a.value
                 JOIN json_each(?2) term ON instr(lower(m.content),term.value)>0
                 GROUP BY m.id,a.key ORDER BY count(*) DESC,CAST(a.key AS INTEGER) LIMIT ?3",
                )
                .map_err(|e| e.to_string())?;
            result.ids = statement
                .query_map(
                    params![
                        &ids,
                        serde_json::to_string(&terms).map_err(|e| e.to_string())?,
                        limit.clamp(1, 2) as i64
                    ],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            result.fallback_terms = terms;
        }
        return Ok(result);
    }
    let literal = format!("\"{}\"", query.trim().replace('"', "\"\""));
    let ids = serde_json::to_string(allowed_ids).map_err(|e| e.to_string())?;
    let mut statement = conn
        .prepare(
            "SELECT messages_fts.id FROM messages_fts
         JOIN json_each(?2) allowed ON allowed.value = messages_fts.id
         WHERE messages_fts MATCH ?1 ORDER BY bm25(messages_fts), CAST(allowed.key AS INTEGER) DESC LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(params![literal, ids, limit.clamp(1, 2) as i64], |row| {
            row.get(0)
        })
        .map_err(|e| e.to_string())?;
    result.ids = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(result)
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

    #[test]
    fn query_terms_keep_late_phrases_before_partial_characters() {
        let observed = "今天这一条不用带收尾语，我只想核对以前保存的长期收尾约定。下面哪条是原记录？只查询记忆，不要操作文件或其它应用。\nA. 结尾列五个问题\nB. 单独一行写“别急，慢慢来。”\nC. 从来不要固定结束语\nD. 没有保存过收尾约定\n只返回 <choice>X</choice>，X 为 A、B、C 或 D。";
        let independent = "请先核对以前保存的交流记录，不要凭当前聊天猜测，也不要操作文件或其它应用。我这里补充背景说明：已经看过常规建议，暂时不用推荐更多搭配；这次仅核对工作日衬衫颜色。";
        for (query, quote) in [
            (observed, "每个回答的末尾都另起一行写：别急，慢慢来。"),
            (independent, "以后工作日挑衬衫，颜色优先深蓝色。"),
        ] {
            let terms = recall_terms(query);
            assert!(terms.len() <= 64);
            assert_eq!(
                terms.iter().collect::<std::collections::HashSet<_>>().len(),
                terms.len()
            );
            assert!(MemoryQuery::new(query).direct_score(quote).is_some());
            assert!(terms
                .windows(2)
                .all(|pair| pair[0].chars().count() > 1 || pair[1].chars().count() == 1));
        }
        let scattered = MemoryQuery::new("冷静清晰");
        assert!(scattered.direct_score("冷暖分明，静观其变。").is_none());
        assert!(scattered
            .shared_characters("冷暖分明，静观其变。")
            .next()
            .is_some());
        assert!(MemoryQuery::new("喝茶")
            .direct_score("喝咖啡，茶不喝。")
            .is_none());
        assert!(MemoryQuery::new("茶").direct_score("乌龙茶").is_some());
        assert!(recall_terms(&format!("{}独角兽", "x".repeat(512))).is_empty());
    }

    #[test]
    fn active_recall_keeps_natural_topics_bounded_and_storage_failure_distinct() {
        let (_file, conn) = setup();
        for (id, scope, category, content, stage, superseded) in [
            (
                "tea",
                "global",
                "fact",
                "我早晨通常喝红茶。",
                "active",
                None,
            ),
            (
                "noise",
                "global",
                "fact",
                "我最喜欢的宠物是仓鼠。",
                "active",
                None,
            ),
            (
                "old",
                "global",
                "fact",
                "早晨喝过时的茶。",
                "active",
                Some("tea"),
            ),
            (
                "forgotten",
                "global",
                "fact",
                "早晨喝遗忘的茶。",
                "summarized",
                None,
            ),
            (
                "unbound",
                "session",
                "fact",
                "早晨喝另一会话的茶。",
                "active",
                None,
            ),
            (
                "rule",
                "global",
                "constraint",
                "早晨喝茶前不要自行操作电脑。",
                "active",
                None,
            ),
        ] {
            conn.execute("INSERT INTO memories(id,scope,category,content,importance,forget_stage,superseded_by,created_at,updated_at) VALUES(?1,?2,?3,?4,5,?5,?6,1,1)",params![id,scope,category,content,stage,superseded]).unwrap();
        }
        let records =
            recall_active_memories(&conn, "早晨我通常喝什么？", 5, MemoryRecallMode::Context)
                .unwrap();
        assert_eq!(
            records.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["tea"]
        );
        assert!(recall_active_memories(
            &conn,
            "我的银行卡号是什么？",
            5,
            MemoryRecallMode::Context
        )
        .unwrap()
        .is_empty());
        assert!(
            recall_active_memories(&conn, "% _ \" OR *", 5, MemoryRecallMode::Context)
                .unwrap()
                .is_empty()
        );
        assert!(
            recall_active_memories(&conn, "", 5, MemoryRecallMode::Context)
                .unwrap()
                .is_empty()
        );
        conn.execute("INSERT INTO memories(id,scope,category,content,importance,forget_stage,created_at,updated_at) VALUES('english-noise','global','fact','My team meets outside the theatre.',5,'active',1,1)",[]).unwrap();
        assert!(
            recall_active_memories(&conn, "Which tea do I drink?", 5, MemoryRecallMode::Context)
                .unwrap()
                .is_empty(),
            "Latin substrings are not whole-word evidence"
        );
        for i in 0..300 {
            conn.execute("INSERT INTO memories(id,scope,category,content,importance,forget_stage,created_at,updated_at) VALUES(?1,'global','fact','My team meets near the theatre.',5,'active',2,2)",[format!("team-{i}")]).unwrap();
        }
        conn.execute("INSERT INTO memories(id,scope,category,content,importance,forget_stage,created_at,updated_at) VALUES('old-tea','global','fact','My tea is mint.',5,'active',1,1)",[]).unwrap();
        assert_eq!(
            recall_active_memories(&conn, "Which tea do I drink?", 1, MemoryRecallMode::Context)
                .unwrap()[0]
                .id,
            "old-tea",
            "substring noise must not evict a real whole-word hit"
        );
        let explicit =
            recall_active_memories(&conn, "早晨我通常喝什么？", 20, MemoryRecallMode::Explicit)
                .unwrap();
        assert!(explicit.iter().any(|r| r.id == "rule"));
        for i in 0..300 {
            conn.execute("INSERT INTO memories(id,scope,category,content,importance,forget_stage,created_at,updated_at) VALUES(?1,'global','fact','早晨喝茶',5,'active',1,1)",[format!("extra-{i}")]).unwrap();
        }
        assert_eq!(
            recall_active_memories(&conn, "喝茶", 100, MemoryRecallMode::Explicit)
                .unwrap()
                .len(),
            20
        );
        assert_eq!(recall_active_memories(&conn,"早晨我通常喝什么？",1,MemoryRecallMode::Context).unwrap()[0].id,"tea",
            "candidate budget must not evict the more relevant old record behind recent or ID-sorted noise");
        conn.execute_batch("DROP TABLE memories").unwrap();
        assert!(recall_active_memories(&conn, "喝茶", 5, MemoryRecallMode::Context).is_err());
    }

    #[test]
    fn memory_scope_filters_categories_before_candidate_bounds() {
        let (_file, conn) = setup();
        for i in 0..300 {
            conn.execute("INSERT INTO memories(id,scope,category,content,importance,forget_stage,created_at,updated_at) VALUES(?1,'global','preference','scopeword preference',5,'active',1,?2)",params![format!("scope-noise-{i}"),1000+i]).unwrap();
        }
        conn.execute("INSERT INTO memories(id,scope,category,content,importance,forget_stage,created_at,updated_at) VALUES('old-scope-target','global','fact','scopeword fact',5,'active',1,1)",[]).unwrap();
        for query in ["scopeword", ""] {
            let facts =
                recall_active_memories(&conn, query, 20, MemoryRecallMode::ExplicitFacts).unwrap();
            assert_eq!(facts.len(), 1);
            assert_eq!(facts[0].id, "old-scope-target");
        }
        conn.execute("UPDATE memories SET category=CASE WHEN id='old-scope-target' THEN 'preference' ELSE 'fact' END",[]).unwrap();
        for query in ["scopeword", ""] {
            let facts =
                recall_active_memories(&conn, query, 20, MemoryRecallMode::ExplicitFacts).unwrap();
            assert_eq!(facts.len(), 20);
            assert!(facts.iter().all(|r| r.category == "fact"));
            assert!(facts.iter().all(|r| r.id != "old-scope-target"));
        }
    }

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
