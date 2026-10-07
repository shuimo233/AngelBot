//! Memory and profile commands with frequency decay system
//!
//! Features:
//!   - Vector similarity for memory activation
//!   - Exponential decay for forgotten memories
//!   - Three-stage forgetting: active -> summarized -> archived -> deleted

use crate::vector;
use crate::AppState;
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use tauri::State;

// ============================================
// Memory Types
// ============================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub scope: String,
    pub category: String,
    pub content: String,
    pub importance: i32,
    pub source: String,
    // 频率遗忘系统字段
    pub frequency: i32,
    pub last_mentioned: Option<i64>,
    pub is_permanent: bool,
    pub embedding: Option<Vec<u8>>,
    pub decay_factor: f64,
    pub forget_stage: String,
    pub created_at: i64,
    pub updated_at: i64,
    // 去重字段
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
}

/// Compute a SHA-256 content hash from scope + category + content.
fn compute_content_hash(scope: &str, category: &str, content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(scope.as_bytes());
    hasher.update(b":");
    hasher.update(category.as_bytes());
    hasher.update(b":");
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryInput {
    pub id: String,
    pub scope: String,
    pub category: String,
    pub content: String,
    pub importance: i32,
    pub source: String,
    pub is_permanent: Option<bool>,
    /// Direct writes are only accepted after a user explicitly confirms the
    /// addition in the knowledge UI. Background learning and agent tools use
    /// their own audited paths and cannot impersonate this source.
    #[serde(default, rename = "userConfirmed")]
    pub user_confirmed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateMemoryRequest {
    pub id: String,
    pub scope: String,
    pub category: String,
    pub content: String,
    pub importance: i32,
    #[serde(rename = "isPermanent")]
    pub is_permanent: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub similarity_threshold: f64,
    pub base_decay_rate: f64,
    pub forget_threshold: f64,
    pub max_active_memories: i32,
    pub enable_vector: bool,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.7,
            base_decay_rate: 0.9,
            forget_threshold: 0.5,
            max_active_memories: 100,
            enable_vector: false,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FrequencyUpdate {
    pub memory_id: String,
    pub similarity_score: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ForgetResult {
    pub memory_id: String,
    pub stage: String,
    pub action: String,
    pub new_content: Option<String>,
}

// ============================================
// Memory Commands
// ============================================

#[tauri::command]
pub fn get_memories(state: State<AppState>) -> Result<Vec<Memory>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, scope, category, content, importance, source,
                    frequency, last_mentioned, is_permanent, embedding,
                    decay_factor, forget_stage, created_at, updated_at
             FROM memories
             WHERE forget_stage != 'deleted' AND superseded_by IS NULL
             ORDER BY is_permanent DESC, importance DESC, updated_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Memory {
                id: r.get(0)?,
                scope: r.get(1)?,
                category: r.get(2)?,
                content: r.get(3)?,
                importance: r.get(4)?,
                source: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                frequency: r.get::<_, Option<i32>>(6)?.unwrap_or(0),
                last_mentioned: r.get(7)?,
                is_permanent: r.get::<_, Option<i32>>(8)?.unwrap_or(0) == 1,
                embedding: r.get(9)?,
                decay_factor: r.get::<_, Option<f64>>(10)?.unwrap_or(1.0),
                forget_stage: r
                    .get::<_, Option<String>>(11)?
                    .unwrap_or_else(|| "active".to_string()),
                created_at: r
                    .get::<_, Option<i64>>(12)?
                    .unwrap_or_else(|| Utc::now().timestamp()),
                updated_at: r
                    .get::<_, Option<i64>>(13)?
                    .unwrap_or_else(|| Utc::now().timestamp()),
                content_hash: None,
                superseded_by: None,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// Internal version for use from handle_dev_command (accepts &AppState).
pub fn get_memories_impl(state: &AppState) -> Result<Vec<serde_json::Value>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, scope, category, content, importance, source,
                    frequency, last_mentioned, is_permanent, embedding,
                    decay_factor, forget_stage, created_at, updated_at
             FROM memories
             WHERE forget_stage != 'deleted' AND superseded_by IS NULL
             ORDER BY is_permanent DESC, importance DESC, updated_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<serde_json::Value> = stmt
        .query_map([], |r| {
            let is_perm: i32 = r.get::<_, Option<i32>>(8)?.unwrap_or(0);
            let forget_stage: String = r
                .get::<_, Option<String>>(11)?
                .unwrap_or_else(|| "active".to_string());
            let created_at: i64 = r
                .get::<_, Option<i64>>(12)?
                .unwrap_or_else(|| Utc::now().timestamp());
            let updated_at: i64 = r
                .get::<_, Option<i64>>(13)?
                .unwrap_or_else(|| Utc::now().timestamp());
            Ok(serde_json::json!({
                "id": r.get::<_, String>(0)?,
                "scope": r.get::<_, String>(1)?,
                "category": r.get::<_, String>(2)?,
                "content": r.get::<_, String>(3)?,
                "importance": r.get::<_, i32>(4)?,
                "source": r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                "frequency": r.get::<_, Option<i32>>(6)?.unwrap_or(0),
                "last_mentioned": r.get::<_, Option<i64>>(7)?,
                "is_permanent": is_perm == 1,
                "embedding": r.get::<_, Option<String>>(9)?,
                "decay_factor": r.get::<_, Option<f64>>(10)?.unwrap_or(1.0),
                "forget_stage": forget_stage,
                "created_at": created_at,
                "updated_at": updated_at,
            }))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Internal version for use from handle_dev_command (accepts &AppState).
pub fn get_memory_stats_impl(state: &AppState) -> Result<serde_json::Value, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
        .unwrap_or(0);
    let active: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE forget_stage = 'active'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let summarized: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE forget_stage = 'summarized'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let archived: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE forget_stage = 'archived'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let deleted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE forget_stage = 'deleted'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let permanent: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE is_permanent = 1",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let with_embeddings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE embedding IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(serde_json::json!({
        "total": total,
        "active": active,
        "summarized": summarized,
        "archived": archived,
        "deleted": deleted,
        "permanent": permanent,
        "withEmbeddings": with_embeddings,
    }))
}

#[tauri::command]
pub fn save_memory(state: State<AppState>, memory: MemoryInput) -> Result<(), String> {
    validate_direct_memory_write(&memory)?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let now = Utc::now().timestamp();
    let is_permanent = memory.is_permanent.unwrap_or(false) as i32;
    let content_hash = compute_content_hash(&memory.scope, &memory.category, &memory.content);

    // Check if this exact content already exists (by hash)
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM memories WHERE content_hash = ?1",
            params![&content_hash],
            |r| r.get(0),
        )
        .ok();

    if let Some(existing_id) = existing {
        // Dedup: update frequency instead of creating duplicate
        conn.execute(
            "UPDATE memories SET frequency = frequency + 1,
             last_mentioned = ?1, updated_at = ?2 WHERE id = ?3",
            params![now, now, existing_id],
        )
        .map_err(|e| e.to_string())?;

        // Log to memory_history
        let history_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO memory_history (id, memory_id, operation, details, created_at)
             VALUES (?1, ?2, 'dedup', ?3, ?4)",
            params![
                history_id,
                existing_id,
                "Duplicate content detected, frequency incremented",
                now
            ],
        )
        .map_err(|e| e.to_string())?;
    } else {
        // New unique memory
        conn.execute(
            r#"INSERT INTO memories (id, scope, category, content, importance, source,
                                    frequency, last_mentioned, is_permanent, decay_factor,
                                    forget_stage, content_hash, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, 1.0, 'active', ?9, ?7, ?10)"#,
            params![
                memory.id,
                memory.scope,
                memory.category,
                memory.content,
                memory.importance,
                memory.source,
                now,
                is_permanent,
                &content_hash,
                now
            ],
        )
        .map_err(|e| e.to_string())?;

        // Log to memory_history
        let history_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO memory_history (id, memory_id, operation, new_content, created_at)
             VALUES (?1, ?2, 'create', ?3, ?4)",
            params![history_id, memory.id, memory.content, now],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Guard the public IPC entry point for durable memories.
///
/// `save_memory` is the path used by the user's Knowledge UI. It must not be
/// repurposed by automatic evolution or a model tool call: those paths either
/// collect bounded adaptive preferences or require proposal acceptance.
fn validate_direct_memory_write(memory: &MemoryInput) -> Result<(), String> {
    if memory.source != "user" {
        return Err(
            "Long-term memories may only be created from an explicit user action".to_string(),
        );
    }
    if !memory.user_confirmed {
        return Err("Explicit user confirmation is required to save long-term memory".to_string());
    }
    if memory.content.trim().is_empty() {
        return Err("Cannot save an empty memory".to_string());
    }
    Ok(())
}

#[tauri::command]
pub fn delete_memory(state: State<AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM memories WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn update_memory_impl(conn: &rusqlite::Connection, req: UpdateMemoryRequest) -> Result<(), String> {
    let now = Utc::now().timestamp();
    let existing = conn
        .query_row(
            "SELECT scope, category, content, importance, is_permanent FROM memories WHERE id = ?1",
            params![&req.id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i32>(3)?,
                    r.get::<_, i32>(4)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?;

    let content_hash = compute_content_hash(&req.scope, &req.category, &req.content);
    let changed = conn
        .execute(
            "UPDATE memories
             SET scope = ?1, category = ?2, content = ?3, importance = ?4,
                 is_permanent = ?5, content_hash = ?6, updated_at = ?7
             WHERE id = ?8",
            params![
                &req.scope,
                &req.category,
                &req.content,
                req.importance,
                req.is_permanent as i32,
                &content_hash,
                now,
                &req.id
            ],
        )
        .map_err(|e| e.to_string())?;

    if changed == 0 {
        return Err("Memory not found".to_string());
    }

    let details = format!(
        "scope:{}->{}, category:{}->{}, importance:{}->{}, permanent:{}->{}",
        existing.0,
        req.scope,
        existing.1,
        req.category,
        existing.3,
        req.importance,
        existing.4,
        req.is_permanent as i32
    );
    let history_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO memory_history (id, memory_id, operation, previous_content, new_content, details, created_at)
         VALUES (?1, ?2, 'update', ?3, ?4, ?5, ?6)",
        params![history_id, &req.id, existing.2, &req.content, details, now],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn update_memory(state: State<AppState>, req: UpdateMemoryRequest) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    update_memory_impl(&conn, req)
}

// ============================================
// Frequency Decay Commands
// ============================================

#[tauri::command]
pub fn update_memory_frequency(
    state: State<AppState>,
    updates: Vec<FrequencyUpdate>,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let now = Utc::now().timestamp();

    for update in updates {
        // Only boost frequency if similarity exceeds threshold
        let config = MemoryConfig::default();
        if update.similarity_score >= config.similarity_threshold {
            conn.execute(
                r#"UPDATE memories SET
                   frequency = frequency + 1,
                   last_mentioned = ?1,
                   decay_factor = 1.0,
                   updated_at = ?1
                   WHERE id = ?2"#,
                params![now, update.memory_id],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn decay_memories(state: State<AppState>) -> Result<i32, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let now = Utc::now().timestamp();
    let config = MemoryConfig::default();

    // Get all non-permanent memories that need decay
    let mut stmt = conn
        .prepare(
            "SELECT id, frequency, last_mentioned, decay_factor
             FROM memories
             WHERE is_permanent = 0 AND forget_stage = 'active'",
        )
        .map_err(|e| e.to_string())?;

    let memories: Vec<(String, i32, Option<i64>, f64)> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get::<_, Option<i32>>(1)?.unwrap_or(0),
                r.get(2)?,
                r.get::<_, Option<f64>>(3)?.unwrap_or(1.0),
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let mut updated_count = 0;
    let seconds_per_day = 86400.0;

    for (id, frequency, last_mentioned, current_decay) in memories {
        if let Some(last_ts) = last_mentioned {
            let days_elapsed = (now - last_ts) as f64 / seconds_per_day;
            // Exponential decay: decay_factor = base_decay ^ days_elapsed
            let new_decay = current_decay * config.base_decay_rate.powf(days_elapsed);

            // Check if should forget (frequency * decay < threshold)
            let memory_score = frequency as f64 * new_decay;
            let new_stage = if memory_score < config.forget_threshold {
                "summarized"
            } else {
                "active"
            };

            conn.execute(
                "UPDATE memories SET decay_factor = ?1, forget_stage = ?2, updated_at = ?3 WHERE id = ?4",
                params![new_decay, new_stage, now, id],
            )
            .map_err(|e| e.to_string())?;
            updated_count += 1;
        }
    }

    Ok(updated_count)
}

#[tauri::command]
pub fn get_memories_to_forget(
    state: State<AppState>,
    config: Option<MemoryConfig>,
) -> Result<Vec<Memory>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let cfg = config.unwrap_or_default();
    let threshold = cfg.forget_threshold;

    let mut stmt = conn
        .prepare(
            "SELECT id, scope, category, content, importance, source,
                    frequency, last_mentioned, is_permanent, embedding,
                    decay_factor, forget_stage, created_at, updated_at
             FROM memories
             WHERE is_permanent = 0
               AND forget_stage IN ('active', 'summarized')
               AND (frequency * decay_factor) < ?1
             ORDER BY (frequency * decay_factor) ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([threshold], |r| {
            Ok(Memory {
                id: r.get(0)?,
                scope: r.get(1)?,
                category: r.get(2)?,
                content: r.get(3)?,
                importance: r.get(4)?,
                source: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                frequency: r.get::<_, Option<i32>>(6)?.unwrap_or(0),
                last_mentioned: r.get(7)?,
                is_permanent: r.get::<_, Option<i32>>(8)?.unwrap_or(0) == 1,
                embedding: r.get(9)?,
                decay_factor: r.get::<_, Option<f64>>(10)?.unwrap_or(1.0),
                forget_stage: r
                    .get::<_, Option<String>>(11)?
                    .unwrap_or_else(|| "active".to_string()),
                created_at: r
                    .get::<_, Option<i64>>(12)?
                    .unwrap_or_else(|| Utc::now().timestamp()),
                updated_at: r
                    .get::<_, Option<i64>>(13)?
                    .unwrap_or_else(|| Utc::now().timestamp()),
                content_hash: None,
                superseded_by: None,
            })
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_memory_permanent(
    state: State<AppState>,
    id: String,
    permanent: bool,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE memories SET is_permanent = ?1, updated_at = ?2 WHERE id = ?3",
        params![permanent as i32, Utc::now().timestamp(), id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn archive_memory(state: State<AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE memories SET forget_stage = 'archived', updated_at = ?1 WHERE id = ?2",
        params![Utc::now().timestamp(), id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn permanently_delete_memory(state: State<AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE memories SET forget_stage = 'deleted', updated_at = ?1 WHERE id = ?2",
        params![Utc::now().timestamp(), id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn cleanup_archived_memories(
    state: State<AppState>,
    older_than_days: i32,
) -> Result<i32, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let cutoff = Utc::now().timestamp() - (older_than_days as i64 * 86400);

    let deleted = conn
        .execute(
            "UPDATE memories SET forget_stage = 'deleted', updated_at = ?1 WHERE forget_stage = 'archived' AND updated_at < ?1",
            params![Utc::now().timestamp(), cutoff],
        )
        .map_err(|e| e.to_string())?;

    Ok(deleted as i32)
}

// ============================================
// Vector-Based Memory Commands
// ============================================

#[derive(Debug, Serialize, Deserialize)]
pub struct SimilarityResult {
    pub memory_id: String,
    pub similarity: f64,
    pub should_activate: bool,
}

#[tauri::command]
pub fn find_relevant_memories(
    state: State<AppState>,
    query_text: String,
    top_k: Option<i32>,
    threshold: Option<f64>,
) -> Result<Vec<SimilarityResult>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let cfg = MemoryConfig::default();
    let sim_threshold = threshold.unwrap_or(cfg.similarity_threshold);
    let k = top_k.unwrap_or(10) as usize;

    // Generate query embedding — use real API if available, fall back to TF
    let provider = crate::llm::create_embedding_provider();
    let (query_f32, _dim): (Vec<f32>, usize) = if let Some(ref p) = provider {
        match p.embed(&query_text) {
            Ok(v) => {
                let dim = v.len();
                let mut f32_vec: Vec<f32> = v.iter().map(|&x| x as f32).collect();
                // Pad to 1536 if needed (for vec0 fixed-dim table)
                f32_vec.resize(1536, 0.0);
                (f32_vec, dim)
            }
            Err(_) => {
                let v = vector::text_to_embedding(&query_text, 128);
                let mut f32_vec: Vec<f32> = v.iter().map(|&x| x as f32).collect();
                f32_vec.resize(1536, 0.0);
                (f32_vec, 128)
            }
        }
    } else {
        let v = vector::text_to_embedding(&query_text, 128);
        let mut f32_vec: Vec<f32> = v.iter().map(|&x| x as f32).collect();
        f32_vec.resize(1536, 0.0);
        (f32_vec, 128)
    };

    // Try KNN via sqlite-vec first
    if let Ok(knn_results) = crate::vector_store::find_nearest(&conn, &query_f32, k) {
        if !knn_results.is_empty() {
            // Map rowids to memory IDs
            let rowids: Vec<i64> = knn_results.iter().map(|r| r.rowid).collect();
            if let Ok(mapping) = crate::vector_store::rowids_to_memory_ids(&conn, &rowids) {
                let results: Vec<SimilarityResult> = knn_results
                    .iter()
                    .filter_map(|r| {
                        mapping
                            .iter()
                            .find(|(rid, _)| *rid == r.rowid)
                            .map(|(_, mem_id)| {
                                // Convert L2 distance to similarity score (closer = higher similarity)
                                // d ∈ [0, ∞), sim = 1 / (1 + d) maps to (0, 1]
                                let similarity = 1.0 / (1.0 + r.distance as f64);
                                SimilarityResult {
                                    memory_id: mem_id.clone(),
                                    similarity,
                                    should_activate: similarity >= sim_threshold,
                                }
                            })
                    })
                    .collect();
                return Ok(results);
            }
        }
    }

    // Fallback to brute-force if vector store is empty or unavailable
    let mut stmt = conn
        .prepare(
            "SELECT id, embedding FROM memories
             WHERE forget_stage = 'active' AND embedding IS NOT NULL
             LIMIT 1000",
        )
        .map_err(|e| e.to_string())?;

    let memories: Vec<(String, Vec<f64>)> = stmt
        .query_map([], |r| {
            let id: String = r.get(0)?;
            let embedding_blob: Option<Vec<u8>> = r.get(1)?;
            let embedding = embedding_blob
                .map(|blob| bincode::deserialize(&blob).unwrap_or_else(|_| vec![0.0; 128]))
                .unwrap_or_else(|| vec![0.0; 128]);
            Ok((id, embedding))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    // Use f64 query for brute-force
    let query_f64: Vec<f64> = query_f32.iter().map(|&x| x as f64).collect();
    let mut results: Vec<SimilarityResult> = memories
        .into_iter()
        .map(|(id, embedding)| {
            let sim = vector::cosine_similarity(&query_f64, &embedding);
            SimilarityResult {
                memory_id: id,
                similarity: sim,
                should_activate: vector::is_relevant(sim, sim_threshold),
            }
        })
        .collect();

    results.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(k);

    Ok(results)
}

#[tauri::command]
pub fn update_memory_embedding(
    state: State<AppState>,
    memory_id: String,
    content: String,
) -> Result<(), String> {
    let embedding = vector::text_to_embedding(&content, 128);
    let embedding_bytes = bincode::serialize(&embedding).map_err(|e| e.to_string())?;

    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE memories SET embedding = ?1, updated_at = ?2 WHERE id = ?3",
        params![embedding_bytes, Utc::now().timestamp(), memory_id],
    )
    .map_err(|e| e.to_string())?;

    // Also sync to vector store for KNN search
    let f32_emb: Vec<f32> = embedding.iter().map(|&x| x as f32).collect();
    let padded: Vec<f32> = {
        let mut v = f32_emb;
        v.resize(1536, 0.0);
        v
    };
    if let Ok(rowid) = crate::vector_store::get_or_create_rowid(&conn, &memory_id) {
        let _ = crate::vector_store::upsert_embedding(&conn, rowid, &padded);
    }

    Ok(())
}

#[tauri::command]
pub fn batch_update_embeddings(state: State<AppState>) -> Result<i32, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    // Get all memories without embeddings
    let mut stmt = conn
        .prepare("SELECT id, content FROM memories WHERE embedding IS NULL")
        .map_err(|e| e.to_string())?;

    let memories: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let mut updated = 0;
    let now = Utc::now().timestamp();

    for (id, content) in memories {
        let embedding = vector::text_to_embedding(&content, 128);
        if let Ok(embedding_bytes) = bincode::serialize(&embedding) {
            let _ = conn.execute(
                "UPDATE memories SET embedding = ?1, updated_at = ?2 WHERE id = ?3",
                params![embedding_bytes, now, &id],
            );
            // Sync to vector store
            let f32_emb: Vec<f32> = embedding.iter().map(|&x| x as f32).collect();
            let mut padded = f32_emb;
            padded.resize(1536, 0.0);
            if let Ok(rowid) = crate::vector_store::get_or_create_rowid(&conn, &id) {
                let _ = crate::vector_store::upsert_embedding(&conn, rowid, &padded);
            }
            updated += 1;
        }
    }

    Ok(updated)
}

/// Update all active memory embeddings using the real embedding API.
/// Falls back to TF-based embeddings if no embedding provider is available.
#[tauri::command]
pub fn update_memory_embeddings_real(state: State<AppState>) -> Result<serde_json::Value, String> {
    // Check if real embedding is available
    let provider = crate::llm::create_embedding_provider();
    let using_real_api = provider.is_some();

    let conn = state.db.lock().map_err(|e| e.to_string())?;

    // Get all active memories
    let mut stmt = conn
        .prepare("SELECT id, content FROM memories WHERE forget_stage = 'active'")
        .map_err(|e| e.to_string())?;

    let memories: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    if memories.is_empty() {
        return Ok(serde_json::json!({
            "updated": 0,
            "using_real_api": using_real_api,
            "message": "No active memories to update"
        }));
    }

    let mut updated = 0;
    let mut errors = 0;
    let now = Utc::now().timestamp();

    if let Some(ref provider) = provider {
        // Real embedding API path
        for (id, content) in &memories {
            match provider.embed(content) {
                Ok(embedding) => {
                    // Convert Vec<f32> to Vec<f64> for bincode serialization
                    let embedding_f64: Vec<f64> = embedding.iter().map(|&v| v as f64).collect();
                    if let Ok(embedding_bytes) = bincode::serialize(&embedding_f64) {
                        let _ = conn.execute(
                            "UPDATE memories SET embedding = ?1, updated_at = ?2 WHERE id = ?3",
                            rusqlite::params![embedding_bytes, now, id],
                        );
                        // Sync to vector store
                        let mut padded: Vec<f32> = embedding.clone();
                        padded.resize(1536, 0.0);
                        if let Ok(rowid) = crate::vector_store::get_or_create_rowid(&conn, id) {
                            let _ = crate::vector_store::upsert_embedding(&conn, rowid, &padded);
                        }
                        updated += 1;
                    } else {
                        errors += 1;
                    }
                }
                Err(e) => {
                    eprintln!("[Embedding] Failed for memory {}: {}", id, e);
                    errors += 1;
                }
            }
        }
    } else {
        // TF fallback path
        for (id, content) in &memories {
            let embedding = vector::text_to_embedding(content, 128);
            if let Ok(embedding_bytes) = bincode::serialize(&embedding) {
                let _ = conn.execute(
                    "UPDATE memories SET embedding = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![embedding_bytes, now, id],
                );
                // Sync to vector store
                let f32_emb: Vec<f32> = embedding.iter().map(|&x| x as f32).collect();
                let mut padded = f32_emb;
                padded.resize(1536, 0.0);
                if let Ok(rowid) = crate::vector_store::get_or_create_rowid(&conn, id) {
                    let _ = crate::vector_store::upsert_embedding(&conn, rowid, &padded);
                }
                updated += 1;
            } else {
                errors += 1;
            }
        }
    }

    Ok(serde_json::json!({
        "updated": updated,
        "errors": errors,
        "total": memories.len(),
        "using_real_api": using_real_api,
        "dimensions": provider.map(|p| p.dimensions()).unwrap_or(128),
    }))
}

// ============================================
// Forget Stage Processing
// ============================================

#[derive(Debug, Serialize, Deserialize)]
pub struct ForgetAction {
    pub memory_id: String,
    pub stage: String,
    pub action: String,
    pub new_content: Option<String>,
}

#[tauri::command]
pub fn summarize_memory_content(
    memory_id: String,
    current_content: String,
) -> Result<ForgetAction, String> {
    // For now, use a simple extractive summarization
    // In production, this would call an LLM API
    let sentences: Vec<&str> = current_content
        .split(|c: char| c == '。' || c == '.' || c == '！' || c == '!' || c == '？' || c == '?')
        .filter(|s| !s.trim().is_empty())
        .collect();

    let summarized = if sentences.is_empty() {
        // Fallback: take first 50 chars
        let chars: Vec<char> = current_content.chars().take(50).collect();
        format!("{}...", chars.into_iter().collect::<String>())
    } else if sentences.len() <= 2 {
        current_content.clone()
    } else {
        // Keep first sentence as summary
        format!("{}...", sentences[0].trim())
    };

    Ok(ForgetAction {
        memory_id,
        stage: "summarized".to_string(),
        action: "summarized".to_string(),
        new_content: Some(summarized),
    })
}

#[tauri::command]
pub fn truncate_memory_content(
    memory_id: String,
    current_content: String,
    keep_ratio: Option<f64>,
) -> Result<ForgetAction, String> {
    let ratio = keep_ratio.unwrap_or(0.5); // Default: keep 50%
    let char_limit = (current_content.len() as f64 * ratio) as usize;

    let truncated = if current_content.len() <= char_limit {
        current_content.clone()
    } else {
        // Find a good truncation point (end of sentence or word)
        let chars: Vec<char> = current_content.chars().take(char_limit).collect();
        let mut end = chars.len();

        // Try to find a sentence boundary
        for (i, c) in chars.iter().enumerate().rev() {
            if *c == '，' || *c == ',' || *c == '、' || *c == ' ' {
                end = i + 1;
                break;
            }
        }

        format!("{}...", chars[..end].into_iter().collect::<String>())
    };

    Ok(ForgetAction {
        memory_id,
        stage: "truncated".to_string(),
        action: "truncated".to_string(),
        new_content: Some(truncated),
    })
}

#[tauri::command]
pub fn process_forgetting_stage(
    state: State<AppState>,
    memory_id: String,
    target_stage: String,
    _use_llm: Option<bool>,
) -> Result<ForgetAction, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    // Get current memory
    let (current_content, _current_stage): (String, String) = conn
        .query_row(
            "SELECT content, forget_stage FROM memories WHERE id = ?1",
            params![memory_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;

    let now = Utc::now().timestamp();

    match target_stage.as_str() {
        "summarized" => {
            // Stage 1: Truncate to 50%
            let truncated = if current_content.len() > 100 {
                let half = current_content.len() / 2;
                format!("{}...", &current_content[..half])
            } else {
                current_content.clone()
            };

            conn.execute(
                "UPDATE memories SET content = ?1, forget_stage = 'summarized', updated_at = ?2 WHERE id = ?3",
                params![truncated, now, memory_id],
            )
            .map_err(|e| e.to_string())?;

            Ok(ForgetAction {
                memory_id,
                stage: "summarized".to_string(),
                action: "truncated".to_string(),
                new_content: Some(truncated),
            })
        }
        "archived" => {
            // Stage 2: Archive the memory
            conn.execute(
                "UPDATE memories SET forget_stage = 'archived', updated_at = ?1 WHERE id = ?2",
                params![now, memory_id],
            )
            .map_err(|e| e.to_string())?;

            Ok(ForgetAction {
                memory_id,
                stage: "archived".to_string(),
                action: "archived".to_string(),
                new_content: None,
            })
        }
        "deleted" => {
            // Stage 3: Mark as deleted (soft delete)
            conn.execute(
                "UPDATE memories SET forget_stage = 'deleted', updated_at = ?1 WHERE id = ?2",
                params![now, memory_id],
            )
            .map_err(|e| e.to_string())?;

            Ok(ForgetAction {
                memory_id,
                stage: "deleted".to_string(),
                action: "deleted".to_string(),
                new_content: None,
            })
        }
        _ => Err(format!("Unknown target stage: {}", target_stage)),
    }
}

// ============================================
// Time-Based Fallback (when vectors disabled)
// ============================================

#[tauri::command]
pub fn find_relevant_memories_by_time(
    state: State<AppState>,
    top_k: Option<i32>,
) -> Result<Vec<(String, f64)>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let k = top_k.unwrap_or(10) as usize;
    let now = Utc::now().timestamp();

    let mut stmt = conn
        .prepare(
            "SELECT id, frequency, last_mentioned, importance
             FROM memories
             WHERE forget_stage = 'active' AND is_permanent = 0",
        )
        .map_err(|e| e.to_string())?;

    let mut results: Vec<(String, f64)> = stmt
        .query_map([], |r| {
            let id: String = r.get(0)?;
            let frequency: i32 = r.get::<_, Option<i32>>(1)?.unwrap_or(0);
            let last_mentioned: Option<i64> = r.get(2)?;
            let importance: i32 = r.get::<_, Option<i32>>(3)?.unwrap_or(0);

            let score = vector::time_based_score(frequency, last_mentioned, importance, now);
            Ok((id, score))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(k);

    Ok(results)
}

// ============================================
// Batch Forgetting Scheduler
// ============================================

#[derive(Debug, Serialize, Deserialize)]
pub struct BatchForgetResult {
    pub decayed: i32,
    pub summarized: i32,
    pub archived: i32,
    pub deleted: i32,
}

#[tauri::command]
pub fn run_batch_forgetting(
    state: State<AppState>,
    archive_threshold: Option<i32>,
    delete_threshold: Option<i32>,
) -> Result<BatchForgetResult, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let now = Utc::now().timestamp();
    let archive_days = archive_threshold.unwrap_or(30); // Archive after 30 days
    let delete_days = delete_threshold.unwrap_or(90); // Delete after 90 days

    let mut result = BatchForgetResult {
        decayed: 0,
        summarized: 0,
        archived: 0,
        deleted: 0,
    };

    // Step 1: Decay all active memories
    // Get all memories that need decay
    let mut stmt = conn
        .prepare(
            "SELECT id, frequency, last_mentioned, decay_factor, content
             FROM memories
             WHERE is_permanent = 0 AND forget_stage = 'active'",
        )
        .map_err(|e| e.to_string())?;

    let memories: Vec<(String, i32, Option<i64>, f64, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get::<_, Option<i32>>(1)?.unwrap_or(0),
                r.get(2)?,
                r.get::<_, Option<f64>>(3)?.unwrap_or(1.0),
                r.get(4)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let seconds_per_day = 86400.0;

    for (id, frequency, last_mentioned, current_decay, _content) in memories {
        if let Some(last_ts) = last_mentioned {
            let days_elapsed = (now - last_ts) as f64 / seconds_per_day;
            let new_decay = current_decay * 0.9_f64.powf(days_elapsed);

            // Calculate score
            let score = frequency as f64 * new_decay;

            // Determine new stage
            let (new_stage, _should_truncate) = if score < 0.5 {
                ("summarized", true)
            } else {
                ("active", false)
            };

            if new_stage == "summarized" {
                // Truncate content
                let current_content = conn
                    .query_row(
                        "SELECT content FROM memories WHERE id = ?1",
                        params![id],
                        |r| r.get::<_, String>(0),
                    )
                    .unwrap_or_default();

                let truncated = if current_content.len() > 100 {
                    format!("{}...", &current_content[..current_content.len() / 2])
                } else {
                    current_content
                };

                conn.execute(
                    "UPDATE memories SET decay_factor = ?1, forget_stage = ?2, content = ?3, updated_at = ?4 WHERE id = ?5",
                    params![new_decay, new_stage, truncated, now, id],
                )
                .map_err(|e| e.to_string())?;
                result.summarized += 1;
            } else {
                conn.execute(
                    "UPDATE memories SET decay_factor = ?1, updated_at = ?2 WHERE id = ?3",
                    params![new_decay, now, id],
                )
                .map_err(|e| e.to_string())?;
            }
            result.decayed += 1;
        }
    }

    // Step 2: Archive old summarized memories
    let archive_cutoff = now - (archive_days as i64 * 86400);
    let archived_count = conn
        .execute(
            "UPDATE memories SET forget_stage = 'archived', updated_at = ?1
             WHERE forget_stage = 'summarized' AND updated_at < ?2",
            params![now, archive_cutoff],
        )
        .map_err(|e| e.to_string())?;
    result.archived = archived_count as i32;

    // Step 3: Delete old archived memories
    let delete_cutoff = now - (delete_days as i64 * 86400);
    let deleted_count = conn
        .execute(
            "UPDATE memories SET forget_stage = 'deleted', updated_at = ?1
             WHERE forget_stage = 'archived' AND updated_at < ?2",
            params![now, delete_cutoff],
        )
        .map_err(|e| e.to_string())?;
    result.deleted = deleted_count as i32;

    Ok(result)
}

#[tauri::command]
pub fn get_memory_stats(state: State<AppState>) -> Result<serde_json::Value, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
        .unwrap_or(0);

    let active: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE forget_stage = 'active'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let summarized: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE forget_stage = 'summarized'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let archived: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE forget_stage = 'archived'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let deleted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE forget_stage = 'deleted'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let permanent: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE is_permanent = 1",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let with_embeddings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE embedding IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    Ok(serde_json::json!({
        "total": total,
        "active": active,
        "summarized": summarized,
        "archived": archived,
        "deleted": deleted,
        "permanent": permanent,
        "withEmbeddings": with_embeddings,
    }))
}

// ============================================
// Evolution Review Commands (Hermes KEPA pattern)
// ============================================

#[derive(Debug, Serialize, Deserialize)]
pub struct EvolutionReviewResult {
    pub new_memories: Vec<crate::agent::evolution::NewMemory>,
    pub preferences_json: Option<String>,
    pub summary: String,
    pub total_stored: i32,
    #[serde(rename = "proposalsCreated")]
    pub proposals_created: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionProposal {
    pub id: String,
    #[serde(rename = "proposalType")]
    pub proposal_type: String,
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    pub category: Option<String>,
    pub content: Option<String>,
    pub importance: Option<i32>,
    #[serde(rename = "preferencesJson")]
    pub preferences_json: Option<String>,
    pub summary: Option<String>,
    pub source: String,
    pub status: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "reviewedAt")]
    pub reviewed_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ReviewEvolutionProposalRequest {
    pub id: String,
    #[serde(rename = "content")]
    pub content_override: Option<String>,
    pub importance: Option<i32>,
    pub permanent: Option<bool>,
}

#[tauri::command]
/// Internal helper: run evolution on a specific session with a given provider.
///
/// This function is `async` because `provider.chat` is async and must be
/// awaited on the existing Tauri tokio runtime. Earlier versions constructed
/// a fresh `tokio::runtime::Runtime` and called `block_on` from inside an
/// async context, which triggered `Runtime::block_on`'s
/// `"failed to park thread"` panic whenever the function was reached from a
/// request that was already running under Tauri's runtime (e.g. `edit_and_resend`
/// or `send_message`). The panic dropped the db lock guard mid-flight and
/// poisoned the global `state.db` mutex, which then made every subsequent
/// IPC fail with `PoisonError` until the process was restarted.
///
/// The function now takes the db as `Arc<Mutex<Connection>>` so it can
/// reacquire the lock itself after the `await`, and is dispatched via
/// `tokio::spawn` from the call sites so a failing evolution never blocks
/// the chat reply.
pub async fn run_evolution_review_impl(
    db: Arc<Mutex<rusqlite::Connection>>,
    session_id: &str,
    provider: Box<dyn crate::llm::LlmProvider>,
) -> Result<EvolutionReviewResult, String> {
    let messages: Vec<(String, String)> = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT role, content FROM messages WHERE session_id = ?1 ORDER BY created_at DESC LIMIT 50",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![session_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };
    if messages.is_empty() {
        return Err("No messages to review".to_string());
    }
    let prompt = crate::agent::evolution::build_extraction_prompt(&messages, None);
    let llm_messages = vec![crate::llm::Message {
        tool_images: Vec::new(),
        protocol_state: None,
        role: "user".to_string(),
        content: prompt,
        tool_calls: None,
        tool_call_id: None,
    }];
    let response = provider
        .chat(&llm_messages, None, None)
        .await
        .map_err(|e| e.to_string())?;
    let response_text = match response.into_parts().0 {
        crate::llm::LlmResponse::Text { text: t, .. } => t,
        crate::llm::LlmResponse::ProtocolViolation { reason, .. } => {
            return Err(format!("Invalid tool-call protocol: {reason}"));
        }
        crate::llm::LlmResponse::ToolCalls { .. } => return Err("Unexpected tool call".to_string()),
        crate::llm::LlmResponse::WithProtocol { .. } => unreachable!("flattened protocol response"),
    };
    let evo = crate::agent::evolution::parse_evolution_response(&response_text)?;
    let proposals_created = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        store_evolution_proposals(&conn, Some(session_id), &evo, "auto_evolution")?
    };
    Ok(EvolutionReviewResult {
        new_memories: evo.new_memories,
        preferences_json: evo.preferences_json,
        summary: evo.summary,
        total_stored: 0,
        proposals_created,
    })
}

#[tauri::command]
pub async fn run_evolution_review(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    session_id: Option<String>,
    message_count: Option<i32>,
) -> Result<EvolutionReviewResult, String> {
    let db = state.db.clone();
    let limit = message_count.unwrap_or(50);

    // Fetch recent messages from all sessions or a specific one. We acquire
    // the lock, copy the rows we need, and drop the guard before the await
    // on the LLM so the global db mutex is never held across an await.
    let (messages, existing_prefs): (Vec<(String, String)>, Option<String>) = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        let (sql, params_list): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(
            ref sid,
        ) = session_id
        {
            ("SELECT role, content FROM messages WHERE session_id = ?1 ORDER BY created_at DESC LIMIT ?2",
             vec![Box::new(sid.clone()), Box::new(limit)])
        } else {
            (
                "SELECT role, content FROM messages ORDER BY created_at DESC LIMIT ?1",
                vec![Box::new(limit)],
            )
        };

        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_list.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return Err("没有可用于分析的对话消息".to_string());
        }
        let existing_prefs: Option<String> = conn
            .query_row("SELECT preferences FROM profile WHERE id = 1", [], |r| {
                r.get::<_, Option<String>>(0)
            })
            .ok()
            .flatten();
        // Reverse to chronological order
        let rows: Vec<(String, String)> = rows.into_iter().rev().collect();
        (rows, existing_prefs)
    };

    let prompt =
        crate::agent::evolution::build_extraction_prompt(&messages, existing_prefs.as_deref());

    let config = crate::commands::foreground_model_factory::resolve_model_config(Some(&app))
        .map_err(|error| error.to_string())?;
    let provider = crate::llm::create_provider_for_app(&config, Some(&app))?;

    let llm_messages = vec![crate::llm::Message {
        tool_images: Vec::new(),
        protocol_state: None,
        role: "user".to_string(),
        content: prompt,
        tool_calls: None,
        tool_call_id: None,
    }];

    // Await directly on the existing Tauri tokio runtime. Earlier versions
    // spawned a fresh `Runtime::block_on` here, which corrupted the thread-
    // local CONTEXT and dropped the conn guard mid-flight, poisoning the
    // global db mutex. See `run_evolution_review_impl` for the full story.
    let response = provider
        .chat(&llm_messages, None, None)
        .await
        .map_err(|e| format!("LLM 调用失败: {}", e))?;

    let response_text = match response.into_parts().0 {
        crate::llm::LlmResponse::Text { text, .. } => text,
        crate::llm::LlmResponse::ProtocolViolation { reason, .. } => {
            return Err(format!("Invalid tool-call protocol: {reason}"));
        }
        crate::llm::LlmResponse::ToolCalls { .. } => {
            return Err("LLM 返回了意外的工具调用".to_string());
        }
        crate::llm::LlmResponse::WithProtocol { .. } => unreachable!("flattened protocol response"),
    };

    let evolution_result = crate::agent::evolution::parse_evolution_response(&response_text)?;

    let now = Utc::now().timestamp();
    let proposals_created = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        let created = store_evolution_proposals(
            &conn,
            session_id.as_deref(),
            &evolution_result,
            "manual_evolution_review",
        )?;
        let _ = conn.execute(
            "UPDATE profile SET updated_at = ?1 WHERE id = 1",
            params![now],
        );
        created
    };

    Ok(EvolutionReviewResult {
        new_memories: evolution_result.new_memories,
        preferences_json: evolution_result.preferences_json,
        summary: evolution_result.summary,
        total_stored: 0,
        proposals_created,
    })
}

fn store_evolution_proposals(
    conn: &rusqlite::Connection,
    session_id: Option<&str>,
    evolution_result: &crate::agent::evolution::EvolutionResult,
    source: &str,
) -> Result<i32, String> {
    let now = Utc::now().timestamp();
    let mut created = 0;

    for memory in &evolution_result.new_memories {
        // Automatic reviews are an internal learning mechanism. The user did
        // not ask to inspect a candidate here, so do not surface a proposal
        // card in the frontend. An explicitly requested manual review keeps
        // the existing proposal/audit flow.
        if source == "auto_evolution" {
            continue;
        }
        if memory.content.trim().is_empty() {
            continue;
        }
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO evolution_proposals
             (id, proposal_type, session_id, category, content, importance, summary, source, status, created_at)
             VALUES (?1, 'memory', ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8)",
            params![
                id,
                session_id,
                memory.category,
                memory.content,
                memory.importance.clamp(1, 10),
                evolution_result.summary,
                source,
                now,
            ],
        )
        .map_err(|e| e.to_string())?;
        created += 1;
    }

    // Background adaptation is intentionally kept out of the visible proposal
    // queue. Only bounded preference statements are eligible, and a single
    // extraction is insufficient to affect future prompts: the same evidence
    // must recur before the reader in `message.rs` activates it.
    crate::agent::adaptive_constraints::record_preferences(
        conn,
        session_id,
        &evolution_result.new_memories,
        source,
    )?;

    if source != "auto_evolution" {
        if let Some(prefs_json) = &evolution_result.preferences_json {
            if !prefs_json.trim().is_empty() {
                let id = uuid::Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO evolution_proposals
                     (id, proposal_type, session_id, preferences_json, summary, source, status, created_at)
                     VALUES (?1, 'profile_preferences', ?2, ?3, ?4, ?5, 'pending', ?6)",
                    params![id, session_id, prefs_json, evolution_result.summary, source, now],
                )
                .map_err(|e| e.to_string())?;
                created += 1;
            }
        }
    }

    Ok(created)
}

fn map_evolution_proposal_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<EvolutionProposal> {
    Ok(EvolutionProposal {
        id: r.get(0)?,
        proposal_type: r.get(1)?,
        session_id: r.get(2)?,
        category: r.get(3)?,
        content: r.get(4)?,
        importance: r.get(5)?,
        preferences_json: r.get(6)?,
        summary: r.get(7)?,
        source: r.get(8)?,
        status: r.get(9)?,
        created_at: r.get(10)?,
        reviewed_at: r.get(11)?,
    })
}

fn get_evolution_proposal_impl(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<EvolutionProposal, String> {
    conn.query_row(
        "SELECT id, proposal_type, session_id, category, content, importance,
                preferences_json, summary, source, status, created_at, reviewed_at
         FROM evolution_proposals WHERE id = ?1",
        params![id],
        map_evolution_proposal_row,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_evolution_proposals(
    state: State<AppState>,
    status: Option<String>,
) -> Result<Vec<EvolutionProposal>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let status_filter = status.unwrap_or_else(|| "pending".to_string());

    let mut stmt = if status_filter == "all" {
        conn.prepare(
            "SELECT id, proposal_type, session_id, category, content, importance,
                    preferences_json, summary, source, status, created_at, reviewed_at
             FROM evolution_proposals ORDER BY created_at DESC",
        )
    } else {
        conn.prepare(
            "SELECT id, proposal_type, session_id, category, content, importance,
                    preferences_json, summary, source, status, created_at, reviewed_at
             FROM evolution_proposals WHERE status = ?1 ORDER BY created_at DESC",
        )
    }
    .map_err(|e| e.to_string())?;

    let rows = if status_filter == "all" {
        stmt.query_map([], map_evolution_proposal_row)
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect()
    } else {
        stmt.query_map(params![status_filter], map_evolution_proposal_row)
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect()
    };
    Ok(rows)
}

fn accept_evolution_proposal_impl(
    conn: &rusqlite::Connection,
    req: ReviewEvolutionProposalRequest,
) -> Result<EvolutionProposal, String> {
    let proposal = get_evolution_proposal_impl(conn, &req.id)?;
    if proposal.status != "pending" {
        return Err(format!("Proposal is already {}", proposal.status));
    }

    let now = Utc::now().timestamp();

    match proposal.proposal_type.as_str() {
        "memory" => {
            let content = req
                .content_override
                .clone()
                .or_else(|| proposal.content.clone())
                .unwrap_or_default();
            if content.trim().is_empty() {
                return Err("Cannot accept an empty memory proposal".to_string());
            }
            let category = proposal
                .category
                .clone()
                .unwrap_or_else(|| "fact".to_string());
            let importance = req
                .importance
                .or(proposal.importance)
                .unwrap_or(5)
                .clamp(1, 10);
            let memory_id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                r#"INSERT INTO memories (id, scope, category, content, importance, source,
                                        frequency, last_mentioned, is_permanent, decay_factor,
                                        forget_stage, created_at, updated_at)
                   VALUES (?1, 'global', ?2, ?3, ?4, 'evolution_review', 0, ?5, ?6, 1.0, 'active', ?5, ?5)"#,
                params![memory_id, category, content, importance, now, req.permanent.unwrap_or(false) as i32],
            )
            .map_err(|e| e.to_string())?;

            let history_id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO memory_history (id, memory_id, operation, new_content, details, created_at)
                 VALUES (?1, ?2, 'evolution_accept', ?3, ?4, ?5)",
                params![
                    history_id,
                    memory_id,
                    content,
                    format!("Accepted evolution proposal {}", proposal.id),
                    now,
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        "profile_preferences" => {
            let prefs_json = proposal
                .preferences_json
                .clone()
                .ok_or_else(|| "Missing preferences JSON".to_string())?;
            conn.execute(
                "INSERT INTO profile (id, name, preferences, habits, background, updated_at)
                 VALUES (1, '', ?1, '', '', ?2)
                 ON CONFLICT(id) DO UPDATE SET preferences = excluded.preferences, updated_at = excluded.updated_at",
                params![prefs_json, now],
            )
            .map_err(|e| e.to_string())?;
        }
        other => return Err(format!("Unknown proposal type: {}", other)),
    }

    conn.execute(
        "UPDATE evolution_proposals SET status = 'accepted', reviewed_at = ?1 WHERE id = ?2",
        params![now, req.id],
    )
    .map_err(|e| e.to_string())?;

    get_evolution_proposal_impl(conn, &proposal.id)
}

#[tauri::command]
pub fn accept_evolution_proposal(
    state: State<AppState>,
    req: ReviewEvolutionProposalRequest,
) -> Result<EvolutionProposal, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    accept_evolution_proposal_impl(&conn, req)
}

fn reject_evolution_proposal_impl(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<EvolutionProposal, String> {
    let proposal = get_evolution_proposal_impl(conn, id)?;
    if proposal.status != "pending" {
        return Err(format!("Proposal is already {}", proposal.status));
    }
    let now = Utc::now().timestamp();
    conn.execute(
        "UPDATE evolution_proposals SET status = 'rejected', reviewed_at = ?1 WHERE id = ?2",
        params![now, id],
    )
    .map_err(|e| e.to_string())?;
    get_evolution_proposal_impl(conn, id)
}

#[tauri::command]
pub fn reject_evolution_proposal(
    state: State<AppState>,
    id: String,
) -> Result<EvolutionProposal, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    reject_evolution_proposal_impl(&conn, &id)
}

#[tauri::command]
pub fn check_evolution_due(state: State<AppState>) -> Result<serde_json::Value, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    let last_reviewed: Option<i64> = conn
        .query_row("SELECT updated_at FROM profile WHERE id = 1", [], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .ok()
        .flatten();

    let message_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
        .unwrap_or(0);

    let now = Utc::now().timestamp();
    let seconds_since_review = last_reviewed.map(|ts| now - ts).unwrap_or(86400 * 7); // Default: due
    let days_since_review = seconds_since_review as f64 / 86400.0;

    // Due if more than 7 days or more than 100 messages since last review
    let due = days_since_review >= 7.0 || (last_reviewed.is_none() && message_count > 20);

    Ok(serde_json::json!({
        "due": due,
        "daysSinceLastReview": (days_since_review * 10.0).round() / 10.0,
        "totalMessageCount": message_count,
    }))
}

// ============================================
// Memory Dedup & Conflict Commands
// ============================================

/// Result of memory conflict detection.
#[derive(Debug, Serialize, Deserialize)]
pub struct ConflictResult {
    pub memory_id: String,
    pub content: String,
    pub category: String,
    pub similarity: f64,
}

/// Record from the memory_history audit log.
#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryHistoryEntry {
    pub id: String,
    #[serde(rename = "memoryId")]
    pub memory_id: String,
    pub operation: String,
    #[serde(rename = "previousContent")]
    pub previous_content: Option<String>,
    #[serde(rename = "newContent")]
    pub new_content: Option<String>,
    pub details: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
}

/// Detect memories that may conflict with a given content string.
/// Uses vector similarity to find semantically similar but not identical memories.
#[tauri::command]
pub fn detect_memory_conflicts(
    state: State<AppState>,
    content: String,
    threshold: Option<f64>,
) -> Result<Vec<ConflictResult>, String> {
    let threshold = threshold.unwrap_or(0.85);
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    // Use the existing find_relevant_memories logic to get similar memories
    let similar = crate::llm::embedding::create_embedding_config_from_env();
    let query_embedding = if similar.enabled {
        // Use real embedding API if available
        let provider = crate::llm::create_embedding_provider();
        if let Some(p) = provider {
            p.embed(&content)
                .map(|v| v.into_iter().map(|f| f as f64).collect())
                .unwrap_or_else(|_| crate::vector::text_to_embedding(&content, 128))
        } else {
            crate::vector::text_to_embedding(&content, 128)
        }
    } else {
        crate::vector::text_to_embedding(&content, 128)
    };

    let mut stmt = conn
        .prepare(
            "SELECT id, content, category, embedding
             FROM memories
             WHERE forget_stage = 'active' AND embedding IS NOT NULL
             LIMIT 100",
        )
        .map_err(|e| e.to_string())?;

    let mut results: Vec<ConflictResult> = Vec::new();
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Vec<u8>>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    for row in rows {
        if let Ok((id, mem_content, category, embedding_blob)) = row {
            if mem_content == content {
                continue; // Skip identical content
            }
            let stored_embedding: Vec<f64> =
                bincode::deserialize(&embedding_blob).unwrap_or_default();
            let similarity = crate::vector::cosine_similarity(&query_embedding, &stored_embedding);
            if similarity >= threshold {
                results.push(ConflictResult {
                    memory_id: id,
                    content: mem_content,
                    category,
                    similarity,
                });
            }
        }
    }

    // Sort by similarity descending
    results.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap());

    Ok(results)
}

/// Get the change history for a specific memory.
#[tauri::command]
pub fn get_memory_history(
    state: State<AppState>,
    memory_id: String,
) -> Result<Vec<MemoryHistoryEntry>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, memory_id, operation, previous_content, new_content, details, created_at
             FROM memory_history WHERE memory_id = ?1 ORDER BY created_at DESC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![memory_id], |r| {
            Ok(MemoryHistoryEntry {
                id: r.get(0)?,
                memory_id: r.get(1)?,
                operation: r.get(2)?,
                previous_content: r.get(3)?,
                new_content: r.get(4)?,
                details: r.get(5)?,
                created_at: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// Merge two related memories by marking source as superseded by target.
#[tauri::command]
pub fn merge_memories(
    state: State<AppState>,
    source_id: String,
    target_id: String,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let now = Utc::now().timestamp();

    // Mark source as superseded
    conn.execute(
        "UPDATE memories SET superseded_by = ?1, updated_at = ?2 WHERE id = ?3",
        params![&target_id, now, &source_id],
    )
    .map_err(|e| e.to_string())?;

    // Log to memory_history for both memories
    let history_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO memory_history (id, memory_id, operation, details, created_at)
         VALUES (?1, ?2, 'merge', ?3, ?4)",
        params![
            history_id,
            source_id,
            format!("Merged into {}", target_id),
            now
        ],
    )
    .map_err(|e| e.to_string())?;

    let history_id2 = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO memory_history (id, memory_id, operation, details, created_at)
         VALUES (?1, ?2, 'merge', ?3, ?4)",
        params![
            history_id2,
            target_id,
            format!("Absorbed {}", source_id),
            now
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

// ============================================
// Profile Commands
// ============================================

#[derive(Debug, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub preferences: String,
    pub habits: String,
    pub background: String,
}

#[tauri::command]
pub fn get_profile(state: State<AppState>) -> Result<Profile, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let result = conn.query_row(
        "SELECT name, preferences, habits, background FROM profile WHERE id = 1",
        [],
        |r| {
            Ok(Profile {
                name: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                preferences: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                habits: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                background: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            })
        },
    );
    match result {
        Ok(profile) => Ok(profile),
        Err(_) => Ok(Profile {
            name: String::new(),
            preferences: String::new(),
            habits: String::new(),
            background: String::new(),
        }),
    }
}

#[tauri::command]
pub fn save_profile(state: State<AppState>, profile: Profile) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let now = Utc::now().timestamp();
    conn.execute(
        r#"INSERT INTO profile (id, name, preferences, habits, background, updated_at)
           VALUES (1, ?1, ?2, ?3, ?4, ?5)
           ON CONFLICT(id) DO UPDATE SET
           name = excluded.name, preferences = excluded.preferences,
           habits = excluded.habits, background = excluded.background,
           updated_at = excluded.updated_at"#,
        params![
            profile.name,
            profile.preferences,
            profile.habits,
            profile.background,
            now
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmProvider, LlmResponse, Message as LlmMessage, ToolSchema};
    use async_trait::async_trait;
    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    struct StaticProvider;

    #[async_trait]
    impl LlmProvider for StaticProvider {
        fn name(&self) -> &str {
            "static-test"
        }

        async fn chat(
            &self,
            _messages: &[LlmMessage],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            Ok(LlmResponse::Text {
                text: r#"{"memories":[{"content":"用户喜欢 Rust","category":"preference","importance":8}],"summary":"学到用户喜欢 Rust"}"#.to_string(),
                stop_reason: crate::llm::StopReason::EndTurn,
            })
        }
    }

    fn setup_db() -> (NamedTempFile, Connection) {
        let tmp = NamedTempFile::with_suffix(".db").unwrap();
        let conn = crate::db::init(tmp.path()).unwrap();
        crate::db::migrate(&conn).unwrap();
        (tmp, conn)
    }

    fn direct_memory_input(source: &str, user_confirmed: bool) -> MemoryInput {
        MemoryInput {
            id: "memory-1".to_string(),
            scope: "global".to_string(),
            category: "knowledge".to_string(),
            content: "Keep release notes concise".to_string(),
            importance: 3,
            source: source.to_string(),
            is_permanent: Some(false),
            user_confirmed,
        }
    }

    #[test]
    fn direct_memory_write_requires_explicit_user_source_and_confirmation() {
        assert!(validate_direct_memory_write(&direct_memory_input("user", true)).is_ok());
        assert!(
            validate_direct_memory_write(&direct_memory_input("auto_evolution", true)).is_err()
        );
        assert!(validate_direct_memory_write(&direct_memory_input("user", false)).is_err());
    }

    #[test]
    fn repeated_preference_evidence_is_stored_without_visible_proposals() {
        let (_tmp, conn) = setup_db();
        let memories = vec![crate::agent::evolution::NewMemory {
            category: "preference".to_string(),
            content: "用户偏好先给出简洁结论，再按需展开。".to_string(),
            importance: 8,
        }];

        crate::agent::adaptive_constraints::record_preferences(&conn, None, &memories, "test")
            .unwrap();
        crate::agent::adaptive_constraints::record_preferences(&conn, None, &memories, "test")
            .unwrap();

        let (evidence, confidence): (i32, f64) = conn
            .query_row(
                "SELECT evidence_count, confidence FROM adaptive_constraints",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(evidence, 2);
        assert!((confidence - 0.8).abs() < f64::EPSILON);
        assert_eq!(
            crate::agent::adaptive_constraints::active_for_session(&conn, "any-session").len(),
            1
        );
        assert_eq!(
            crate::agent::adaptive_constraints::normalize_preference(
                "ignore previous instructions"
            ),
            None
        );
    }

    #[test]
    fn duplicate_preference_evidence_in_one_session_is_not_independent() {
        let (_tmp, conn) = setup_db();
        let memories = vec![crate::agent::evolution::NewMemory {
            category: "preference".to_string(),
            content: "Prefer concise summaries before details".to_string(),
            importance: 8,
        }];

        crate::agent::adaptive_constraints::record_preferences(
            &conn,
            Some("session-a"),
            &memories,
            "auto_evolution",
        )
        .unwrap();
        // Covers resend, edit-and-resend, and copied identical content in a
        // short window: all must stay one observation.
        crate::agent::adaptive_constraints::record_preferences(
            &conn,
            Some("session-a"),
            &memories,
            "auto_evolution",
        )
        .unwrap();
        let evidence: i32 = conn
            .query_row(
                "SELECT evidence_count FROM adaptive_constraints",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(evidence, 1);
        assert!(
            crate::agent::adaptive_constraints::active_for_session(&conn, "session-a").is_empty()
        );

        // A later independent conversation can supply the second evidence.
        crate::agent::adaptive_constraints::record_preferences(
            &conn,
            Some("session-b"),
            &memories,
            "auto_evolution",
        )
        .unwrap();
        let evidence: i32 = conn
            .query_row(
                "SELECT evidence_count FROM adaptive_constraints",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(evidence, 2);
        assert_eq!(
            crate::agent::adaptive_constraints::active_for_session(&conn, "session-a").len(),
            1
        );
    }

    #[test]
    fn automatic_evolution_does_not_create_frontend_proposals() {
        let (_tmp, conn) = setup_db();
        let result = crate::agent::evolution::EvolutionResult {
            new_memories: vec![crate::agent::evolution::NewMemory {
                category: "preference".to_string(),
                content: "用户偏好简洁结论。".to_string(),
                importance: 8,
            }],
            preferences_json: None,
            summary: "test".to_string(),
        };

        assert_eq!(
            store_evolution_proposals(&conn, None, &result, "auto_evolution").unwrap(),
            0
        );
        let proposals: i32 = conn
            .query_row("SELECT COUNT(*) FROM evolution_proposals", [], |row| {
                row.get(0)
            })
            .unwrap();
        let constraints: i32 = conn
            .query_row("SELECT COUNT(*) FROM adaptive_constraints", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(proposals, 0);
        assert_eq!(constraints, 1);
    }

    #[test]
    fn update_memory_edits_fields_and_writes_history() {
        let (_tmp, conn) = setup_db();
        conn.execute(
            "INSERT INTO memories (id, scope, category, content, importance, source, frequency, is_permanent, decay_factor, forget_stage, content_hash, created_at, updated_at)
             VALUES ('m1', 'global', 'fact', 'old content', 3, 'test', 0, 0, 1.0, 'active', 'old-hash', 1, 1)",
            [],
        )
        .unwrap();

        update_memory_impl(
            &conn,
            UpdateMemoryRequest {
                id: "m1".to_string(),
                scope: "global".to_string(),
                category: "preference".to_string(),
                content: "new content".to_string(),
                importance: 8,
                is_permanent: true,
            },
        )
        .unwrap();

        let updated: (String, i32, i32) = conn
            .query_row(
                "SELECT content, importance, is_permanent FROM memories WHERE id = 'm1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(updated, ("new content".to_string(), 8, 1));

        let history_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memory_history WHERE memory_id = 'm1' AND operation = 'update'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(history_count, 1);
    }

    #[tokio::test]
    async fn automatic_evolution_records_hidden_preference_evidence() {
        let (_tmp, conn) = setup_db();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at) VALUES ('s1', 'Test', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES ('m1', 's1', 'user', '我喜欢 Rust', 1)",
            [],
        )
        .unwrap();

        let db = Arc::new(Mutex::new(conn));
        let result = run_evolution_review_impl(db.clone(), "s1", Box::new(StaticProvider))
            .await
            .unwrap();
        assert_eq!(result.total_stored, 0);
        assert_eq!(result.proposals_created, 0);

        let conn = db.lock().unwrap();
        let proposal_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM evolution_proposals WHERE status = 'pending'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(proposal_count, 0);

        let memory_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE source = 'evolution_review'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(memory_count, 0);

        let adaptive_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM adaptive_constraints", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(adaptive_count, 1);
    }

    #[test]
    fn accepting_evolution_proposal_writes_memory_and_audit_history() {
        let (_tmp, conn) = setup_db();
        let evo = crate::agent::evolution::EvolutionResult {
            new_memories: vec![crate::agent::evolution::NewMemory {
                category: "preference".to_string(),
                content: "用户偏好简短回答".to_string(),
                importance: 7,
            }],
            preferences_json: None,
            summary: "偏好简短".to_string(),
        };
        let created = store_evolution_proposals(&conn, None, &evo, "test").unwrap();
        assert_eq!(created, 1);

        let proposal_id: String = conn
            .query_row("SELECT id FROM evolution_proposals LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        let accepted = accept_evolution_proposal_impl(
            &conn,
            ReviewEvolutionProposalRequest {
                id: proposal_id,
                content_override: Some("用户偏好非常简短的回答".to_string()),
                importance: Some(8),
                permanent: Some(true),
            },
        )
        .unwrap();
        assert_eq!(accepted.status, "accepted");

        let (content, importance, is_permanent): (String, i32, i32) = conn
            .query_row(
                "SELECT content, importance, is_permanent FROM memories WHERE source = 'evolution_review'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(content, "用户偏好非常简短的回答");
        assert_eq!(importance, 8);
        assert_eq!(is_permanent, 1);

        let history_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_history WHERE operation = 'evolution_accept'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(history_count, 1);
    }

    #[test]
    fn rejecting_evolution_proposal_does_not_write_memory() {
        let (_tmp, conn) = setup_db();
        let evo = crate::agent::evolution::EvolutionResult {
            new_memories: vec![crate::agent::evolution::NewMemory {
                category: "fact".to_string(),
                content: "用户临时提到一个不应保存的事实".to_string(),
                importance: 3,
            }],
            preferences_json: None,
            summary: "临时事实".to_string(),
        };
        store_evolution_proposals(&conn, None, &evo, "test").unwrap();
        let proposal_id: String = conn
            .query_row("SELECT id FROM evolution_proposals LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();

        let rejected = reject_evolution_proposal_impl(&conn, &proposal_id).unwrap();
        assert_eq!(rejected.status, "rejected");

        let memory_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE content LIKE '%临时%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(memory_count, 0);
    }
}
