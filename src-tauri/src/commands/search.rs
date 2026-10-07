//! Search Tauri commands — FTS5 full-text and hybrid search endpoints.

use crate::fts;
use crate::AppState;
use tauri::State;

/// Full-text search messages with optional session filter.
/// Returns results with highlighted snippets and BM25 ranking.
#[tauri::command]
pub fn search_messages(
    state: State<AppState>,
    query: String,
    session_id: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<fts::MessageSearchResult>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    fts::search_messages(&conn, &query, session_id.as_deref(), limit.unwrap_or(50))
}

/// Full-text search memories by keyword.
#[tauri::command]
pub fn search_memories_by_keyword(
    state: State<AppState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<fts::MemoryKeywordResult>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    fts::search_memories_keyword(&conn, &query, limit.unwrap_or(50))
}

/// Rebuild all FTS5 indexes from source tables.
/// Useful after bulk data imports or index recovery.
#[tauri::command]
pub fn rebuild_fts_index(state: State<AppState>) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    fts::rebuild_fts_index(&conn)
}

/// Hybrid search: fuse vector similarity + FTS5 keyword matching.
#[tauri::command]
pub fn hybrid_search_memories(
    state: State<AppState>,
    query: String,
    vector_weight: Option<f64>,
    limit: Option<usize>,
) -> Result<Vec<fts::HybridSearchResult>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let weight = vector_weight.unwrap_or(0.5);
    // Generate query embedding using TF (real API would be better but adds latency)
    let embedding = crate::vector::text_to_embedding(&query, 128);
    let query_f32: Vec<f32> = {
        let mut v: Vec<f32> = embedding.iter().map(|&x| x as f32).collect();
        v.resize(1536, 0.0);
        v
    };
    fts::hybrid_search_memories(&conn, &query, &query_f32, weight, limit.unwrap_or(20))
}
