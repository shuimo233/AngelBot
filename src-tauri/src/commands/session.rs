//! Session commands
use crate::AppState;
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    /// A session created by editing/branching another conversation.  Exposing
    /// this existing durable relationship lets the UI render a navigable
    /// conversation index without inventing a second client-side tree.
    #[serde(rename = "parentId", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    #[serde(rename = "contextVersion")]
    pub context_version: i32,
    #[serde(rename = "lastCompressedAt", skip_serializing_if = "Option::is_none")]
    pub last_compressed_at: Option<i64>,
    #[serde(rename = "workDir", skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,
    #[serde(rename = "messageCount", default)]
    pub message_count: Option<i32>,
    #[serde(
        rename = "agentProvider",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub agent_provider: Option<String>,
    #[serde(
        rename = "agentModel",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub agent_model: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HistorySession {
    pub id: String,
    pub title: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    #[serde(rename = "messageCount")]
    pub message_count: i32,
}

/// The conversation portion of the context that will be sent to the model.
/// System instructions and dynamic tool definitions are intentionally excluded:
/// their size depends on the selected model and active integrations.
#[derive(Debug, Serialize)]
pub struct ContextUsage {
    #[serde(rename = "activeMessageCount")]
    pub active_message_count: i32,
    #[serde(rename = "totalMessageCount")]
    pub total_message_count: i32,
    #[serde(rename = "estimatedTokens")]
    pub estimated_tokens: i32,
    #[serde(rename = "contextLimit")]
    pub context_limit: i32,
    #[serde(rename = "isCompressed")]
    pub is_compressed: bool,
    /// `provider_reported` means this is the input-token count returned by
    /// the model for its latest request. `estimated` is a local fallback.
    #[serde(rename = "measurementSource")]
    pub measurement_source: String,
    /// Input tokens reported by the latest completed provider request.
    /// This is an anchor, not an accumulated session total.
    #[serde(
        rename = "reportedPromptTokens",
        skip_serializing_if = "Option::is_none"
    )]
    pub reported_prompt_tokens: Option<i32>,
    /// Locally estimated active conversation content not covered by the
    /// provider anchor. It is exposed so the UI can be honest about a hybrid
    /// measurement without presenting a second, confusing quota counter.
    #[serde(rename = "estimatedTrailingTokens")]
    pub estimated_trailing_tokens: i32,
    #[serde(rename = "measuredAt", skip_serializing_if = "Option::is_none")]
    pub measured_at: Option<i64>,
}

#[tauri::command]
pub fn create_session(
    state: State<AppState>,
    title: String,
    work_dir: Option<String>,
    agent_provider: Option<String>,
    agent_model: Option<String>,
) -> Result<Session, String> {
    create_session_impl(&state, title, work_dir, agent_provider, agent_model)
}

/// Creates a session using an explicit directory or the last remembered one.
/// Shared with the Dev HTTP path to keep desktop and development behavior equal.
pub fn create_session_impl(
    state: &AppState,
    title: String,
    work_dir: Option<String>,
    agent_provider: Option<String>,
    agent_model: Option<String>,
) -> Result<Session, String> {
    let now = Utc::now().timestamp();
    let id = Uuid::new_v4().to_string();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let valid_work_dir = resolve_initial_work_dir(&conn, work_dir);
    conn.execute(
        "INSERT INTO sessions (id, title, created_at, updated_at, context_version, work_dir, agent_provider, agent_model) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7)",
        params![id, title, now, now, valid_work_dir, agent_provider.as_deref(), agent_model.as_deref()],
    )
    .map_err(|e| e.to_string())?;
    Ok(Session {
        id,
        title,
        parent_id: None,
        created_at: now,
        updated_at: now,
        context_version: 0,
        last_compressed_at: None,
        work_dir: valid_work_dir,
        message_count: None,
        agent_provider,
        agent_model,
    })
}

fn resolve_initial_work_dir(
    conn: &rusqlite::Connection,
    explicit: Option<String>,
) -> Option<String> {
    let explicit_work_dir = explicit.filter(|dir| !dir.trim().is_empty());
    let remembered_work_dir = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'work_directory'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .filter(|dir| !dir.trim().is_empty());
    explicit_work_dir.or(remembered_work_dir).and_then(|dir| {
        let path = std::path::Path::new(&dir);
        (path.exists() && path.is_dir())
            .then(|| {
                std::fs::canonicalize(path)
                    .ok()
                    .map(|absolute| crate::commands::file::display_work_dir(&absolute))
            })
            .flatten()
    })
}

#[tauri::command]
pub fn get_sessions(state: State<AppState>) -> Result<Vec<Session>, String> {
    get_sessions_impl(&state)
}

/// Shared by Tauri IPC and the Dev HTTP gateway so both return the same
/// session shape, including message counts and provider bindings.
pub fn get_sessions_impl(state: &AppState) -> Result<Vec<Session>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.title, s.parent_id, s.created_at, s.updated_at, s.context_version,
                    s.last_compressed_at, s.work_dir,
                    (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id) as message_count,
                    s.agent_provider, s.agent_model
             FROM sessions s ORDER BY s.updated_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Session {
                id: r.get(0)?,
                title: r.get(1)?,
                parent_id: r.get(2)?,
                created_at: r.get(3)?,
                updated_at: r.get(4)?,
                context_version: r.get::<_, i32>(5).unwrap_or(0),
                last_compressed_at: r.get(6)?,
                work_dir: r.get(7)?,
                message_count: r.get::<_, Option<i32>>(8).unwrap_or(None),
                agent_provider: r.get(9)?,
                agent_model: r.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_session(state: State<AppState>, id: String) -> Result<(), String> {
    delete_session_impl(&state, &id)
}

/// Delete a session through one transaction. Migration 026 owns dependent-row
/// cleanup at the database boundary, so production IPC and Dev HTTP share the
/// same behavior and a failed delete cannot partially remove messages.
pub fn delete_session_impl(state: &AppState, id: &str) -> Result<(), String> {
    if id.trim().is_empty() {
        return Err("Session ID cannot be empty".to_string());
    }

    let mut conn = state.db.lock().map_err(|e| e.to_string())?;
    delete_session_from_connection(&mut conn, id)
}

/// Update the model binding for a session. Allows switching models at the
/// session level without affecting global API settings.
#[tauri::command]
pub fn update_session_model(
    state: State<AppState>,
    session_id: String,
    agent_provider: String,
    agent_model: String,
) -> Result<(), String> {
    update_session_model_impl(&state, &session_id, &agent_provider, &agent_model)
}

pub fn update_session_model_impl(
    state: &AppState,
    session_id: &str,
    agent_provider: &str,
    agent_model: &str,
) -> Result<(), String> {
    if session_id.trim().is_empty() {
        return Err("Session ID cannot be empty".to_string());
    }
    if agent_provider.trim().is_empty() || agent_model.trim().is_empty() {
        return Err("Provider and model cannot be empty".to_string());
    }
    let now = Utc::now().timestamp();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let updated = conn
        .execute(
            "UPDATE sessions SET agent_provider = ?1, agent_model = ?2, updated_at = ?3 WHERE id = ?4",
            params![agent_provider, agent_model, now, session_id],
        )
        .map_err(|e| e.to_string())?;
    if updated == 0 {
        return Err("Session not found".to_string());
    }
    Ok(())
}

fn delete_session_from_connection(conn: &mut rusqlite::Connection, id: &str) -> Result<(), String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let deleted = tx
        .execute("DELETE FROM sessions WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    if deleted == 0 {
        return Err("Session not found".to_string());
    }
    tx.commit().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_session_title(
    state: State<AppState>,
    id: String,
    title: String,
) -> Result<(), String> {
    update_session_title_impl(&state, id, title)
}

pub fn update_session_title_impl(
    state: &AppState,
    id: String,
    title: String,
) -> Result<(), String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("Session title cannot be empty".to_string());
    }
    if title.chars().count() > 80 {
        return Err("Session title must be 80 characters or fewer".to_string());
    }
    let now = Utc::now().timestamp();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
        params![title, now, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_context_usage(
    state: State<AppState>,
    session_id: String,
) -> Result<ContextUsage, String> {
    get_context_usage_impl(&state, &session_id)
}

fn active_context_characters(
    conn: &rusqlite::Connection,
    session_id: &str,
    after: Option<i64>,
) -> Result<(i32, i32), String> {
    let mut stmt = conn
        .prepare(
            "SELECT role, content, metadata FROM messages WHERE session_id = ?1
         AND (?2 IS NULL OR created_at > ?2)
         AND COALESCE(metadata, '') NOT LIKE '%\"compressed\": true%'",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(params![session_id, after], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    let mut count: i32 = 0;
    let mut characters: i32 = 0;
    for row in rows {
        let (role, content, metadata) = row.map_err(|error| error.to_string())?;
        let rendered = crate::commands::foreground_text_attachments::render_stored(
            &content,
            &role,
            metadata.as_deref(),
        )
        .unwrap_or_else(|_| format!("{content} {}", metadata.as_deref().unwrap_or_default()));
        count = count.saturating_add(1);
        characters =
            characters.saturating_add(rendered.chars().count().try_into().unwrap_or(i32::MAX));
    }
    Ok((count, characters))
}

pub fn get_context_usage_impl(state: &AppState, session_id: &str) -> Result<ContextUsage, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let provider_config =
        crate::commands::api_config::load_api_config_internal().unwrap_or_default();
    let context_limit = crate::llm::context_window_tokens(&provider_config)
        .try_into()
        .unwrap_or(i32::MAX);

    let (active_message_count, active_chars) = active_context_characters(&conn, session_id, None)?;
    let total_message_count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let summary_chars: i32 = conn.query_row(
        "SELECT LENGTH(summary) FROM context_summaries WHERE session_id = ?1 ORDER BY compressed_at DESC LIMIT 1",
        params![session_id],
        |row| row.get(0),
    ).unwrap_or(0);
    let local_estimate = ((active_chars + summary_chars) + 3) / 4;
    // Provider-reported prompt tokens are the only exact count available
    // without submitting another request. Prefer them when the usage pipeline
    // has recorded a call; otherwise retain the clearly-labelled local estimate.
    let latest_provider_usage: Option<(i32, i64)> = conn
        .query_row(
            "SELECT prompt_tokens, created_at FROM usage_stats WHERE session_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
            params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();
    let (
        estimated_tokens,
        measurement_source,
        reported_prompt_tokens,
        estimated_trailing_tokens,
        measured_at,
    ) = match latest_provider_usage {
        Some((tokens, measured_at)) if tokens >= 0 => {
            // Provider usage is the most reliable anchor available. Messages
            // persisted after that request are added locally, following Pi's
            // exact-anchor plus trailing-estimate model.
            let (_, trailing_chars) =
                active_context_characters(&conn, session_id, Some(measured_at))?;
            let trailing_tokens = (trailing_chars + 3) / 4;
            let source = if trailing_tokens > 0 {
                "hybrid_estimate"
            } else {
                "provider_reported"
            };
            (
                tokens.saturating_add(trailing_tokens),
                source.to_string(),
                Some(tokens),
                trailing_tokens,
                Some(measured_at),
            )
        }
        _ => (
            local_estimate,
            "local_estimate".to_string(),
            None,
            local_estimate,
            None,
        ),
    };

    Ok(ContextUsage {
        active_message_count,
        total_message_count,
        estimated_tokens,
        context_limit,
        is_compressed: total_message_count > active_message_count,
        measurement_source,
        reported_prompt_tokens,
        estimated_trailing_tokens,
        measured_at,
    })
}

#[tauri::command]
pub fn get_history_sessions(
    state: State<AppState>,
    limit: Option<i32>,
) -> Result<Vec<HistorySession>, String> {
    let limit = limit.unwrap_or(20).max(1).min(100) as usize;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.title, s.created_at, s.updated_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id) as message_count
             FROM sessions s
             ORDER BY s.updated_at DESC
             LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([limit], |r| {
            Ok(HistorySession {
                id: r.get(0)?,
                title: r.get(1)?,
                created_at: r.get(2)?,
                updated_at: r.get(3)?,
                message_count: r.get::<_, i32>(4).unwrap_or(0),
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn context_estimates_include_active_and_trailing_file_snapshots_but_not_compressed_ones() {
        use crate::commands::foreground_text_attachments::{self, TextAttachment};
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE messages (session_id TEXT, role TEXT, content TEXT, metadata TEXT, created_at INTEGER);
            INSERT INTO messages VALUES ('s', 'assistant', 'reply', NULL, 1);").unwrap();
        let metadata = foreground_text_attachments::metadata(&[TextAttachment {
            name: "notes.txt".into(),
            text: "行,值\n一,二".into(),
        }])
        .unwrap();
        conn.execute(
            "INSERT INTO messages VALUES ('s', 'user', '', ?1, 2)",
            params![metadata],
        )
        .unwrap();
        let rendered =
            foreground_text_attachments::render_stored("", "user", Some(&metadata)).unwrap();
        assert_eq!(
            active_context_characters(&conn, "s", None).unwrap(),
            (2, 5 + rendered.chars().count() as i32)
        );
        assert_eq!(
            active_context_characters(&conn, "s", Some(1)).unwrap(),
            (1, rendered.chars().count() as i32)
        );
        conn.execute(
            "UPDATE messages SET metadata = ?1 WHERE role = 'user'",
            params![foreground_text_attachments::compressed_metadata(Some(
                &metadata
            ))],
        )
        .unwrap();
        assert_eq!(active_context_characters(&conn, "s", None).unwrap(), (1, 5));
        assert_eq!(
            active_context_characters(&conn, "s", Some(1)).unwrap(),
            (0, 0)
        );
    }

    #[test]
    fn uses_the_remembered_directory_when_no_directory_is_provided() {
        let dir = tempdir().unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);").unwrap();
        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES ('work_directory', ?1, 1)",
            params![dir.path().to_string_lossy().to_string()],
        )
        .unwrap();

        assert_eq!(
            resolve_initial_work_dir(&conn, None),
            Some(crate::commands::file::display_work_dir(
                &std::fs::canonicalize(dir.path()).unwrap(),
            )),
        );
    }

    #[test]
    fn deleting_session_cleans_feature_rows_atomically() {
        let mut file = tempfile::NamedTempFile::with_suffix(".db").unwrap();
        file.write_all(b"").unwrap();
        let mut conn = crate::db::init(file.path()).unwrap();
        crate::db::migrate(&conn).unwrap();

        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at) VALUES ('parent', 'Parent', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, parent_id, title, created_at, updated_at) VALUES ('child', 'parent', 'Child', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES ('message', 'parent', 'user', 'hello', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_steps (id, session_id, tool_name, seq, created_at) VALUES ('step', 'parent', 'read_file', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_goals (id, session_id, goal_text, created_at) VALUES ('goal', 'parent', 'test', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO smart_zone_log (id, session_id, token_count, smart_zone_pct, recorded_at) VALUES ('zone', 'parent', 1, 50, 1)",
            [],
        )
        .unwrap();

        delete_session_from_connection(&mut conn, "parent").unwrap();

        for table in ["messages", "agent_steps", "agent_goals", "smart_zone_log"] {
            let count: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE session_id = 'parent'"),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0, "{table} should be cleaned");
        }
        let child_parent: Option<String> = conn
            .query_row(
                "SELECT parent_id FROM sessions WHERE id = 'child'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(child_parent, None);
        assert!(conn
            .execute("DELETE FROM sessions WHERE id = 'missing'", [])
            .is_ok());
    }
}
