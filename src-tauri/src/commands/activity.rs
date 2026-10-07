//! User-auditable activity feed.
//!
//! This deliberately exposes task outcomes and explicit memory changes, not
//! individual tool calls, hidden adaptive-learning records, or raw
//! tool inputs/outputs.  Per-tool execution remains available in the relevant
//! conversation, where it has enough context to be useful.

use crate::AppState;
use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::State;

const DEFAULT_LIMIT: i64 = 80;
const MAX_LIMIT: i64 = 200;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ActivityEntry {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub detail: Option<String>,
    pub status: Option<String>,
    pub occurred_at: i64,
}

/// Returns a chronological, typed feed of events a user can inspect.
///
/// Only terminal task outcomes and explicit memory changes are included.
/// Tool payloads, individual reads/writes, and automatic adaptive constraints
/// are intentionally omitted: those are implementation details rather than
/// useful global activity for the user.
pub fn get_activity_impl(
    conn: &Connection,
    limit: Option<i64>,
) -> Result<Vec<ActivityEntry>, String> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, title, detail, status, occurred_at FROM (
                SELECT 'run:' || id AS id,
                       'task_result' AS kind,
                       goal AS title,
                       CASE status
                           WHEN 'completed' THEN 'Task completed'
                           WHEN 'needs_attention' THEN 'Task needs attention'
                           WHEN 'provider_unavailable' THEN 'Task could not reach the model provider'
                           WHEN 'stopped' THEN 'Task stopped'
                       END AS detail,
                       status AS status,
                       updated_at AS occurred_at
                  FROM task_runs
                 WHERE status IN ('completed', 'needs_attention', 'provider_unavailable', 'stopped')
                UNION ALL
                SELECT 'memory:' || h.id AS id,
                       'memory_change' AS kind,
                       CASE h.operation
                           WHEN 'create' THEN 'Memory saved'
                           WHEN 'update' THEN 'Memory updated'
                           WHEN 'delete' THEN 'Memory removed'
                           WHEN 'archive' THEN 'Memory archived'
                           WHEN 'merge' THEN 'Memories merged'
                           ELSE 'Memory changed'
                       END AS title,
                       NULL AS detail,
                       h.operation AS status,
                       h.created_at AS occurred_at
                  FROM memory_history h
                  LEFT JOIN memories m ON m.id = h.memory_id
                 WHERE COALESCE(m.source, '') NOT IN ('auto_evolution', 'adaptive_constraint', 'adaptive_learning')
            ) ORDER BY occurred_at DESC, id DESC LIMIT ?1",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(params![limit], |row| {
            Ok(ActivityEntry {
                id: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
                detail: row.get(3)?,
                status: row.get(4)?,
                occurred_at: row.get(5)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_activity(
    state: State<AppState>,
    limit: Option<i64>,
) -> Result<Vec<ActivityEntry>, String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    get_activity_impl(&conn, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE task_runs (id TEXT, goal TEXT, status TEXT, updated_at INTEGER);
             CREATE TABLE agent_steps (id TEXT, tool_name TEXT, success INTEGER, created_at INTEGER);
             CREATE TABLE memories (id TEXT, source TEXT);
             CREATE TABLE memory_history (id TEXT, memory_id TEXT, operation TEXT, created_at INTEGER);",
        ).unwrap();
        conn
    }

    #[test]
    fn activity_includes_only_task_outcomes_and_explicit_memory_changes() {
        let conn = fixture();
        conn.execute(
            "INSERT INTO task_runs VALUES ('run-1', 'Organize workspace', 'completed', 30)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_steps VALUES ('step-1', 'write_file', 1, 20)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memories VALUES ('visible', 'manual'), ('hidden', 'auto_evolution')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO memory_history VALUES ('history-1', 'visible', 'update', 10), ('history-2', 'hidden', 'create', 40)", []).unwrap();

        let entries = get_activity_impl(&conn, None).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["run:run-1", "memory:history-1"]
        );
        assert!(entries.iter().all(|entry| !entry.title.contains("hidden")));
        assert_eq!(entries[0].kind, "task_result");
        assert_eq!(entries[0].detail.as_deref(), Some("Task completed"));
    }

    #[test]
    fn activity_clamps_requested_limit() {
        let conn = fixture();
        for index in 0..3 {
            conn.execute(
                "INSERT INTO task_runs VALUES (?1, ?2, 'completed', ?3)",
                params![format!("run-{index}"), format!("Task {index}"), index],
            )
            .unwrap();
        }
        assert_eq!(get_activity_impl(&conn, Some(1)).unwrap().len(), 1);
    }

    #[test]
    fn activity_hides_non_terminal_runs_and_all_tool_steps() {
        let conn = fixture();
        conn.execute(
            "INSERT INTO task_runs VALUES ('running', 'Still working', 'running', 30)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO task_runs VALUES ('paused', 'Awaiting approval', 'awaiting_confirmation', 29)", []).unwrap();
        conn.execute(
            "INSERT INTO agent_steps VALUES ('write', 'write_file', 1, 40)",
            [],
        )
        .unwrap();

        assert!(get_activity_impl(&conn, None).unwrap().is_empty());
    }
}
