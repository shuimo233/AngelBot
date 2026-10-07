//! Workspace-scoped Main-Agent activity projection.
//!
//! Sessions remain an internal history mechanism. This command is the only
//! user-facing read seam: it resolves the Workspace's selected conversation
//! before adapting the existing durable delegation projection.

use crate::agent::attention::{load_visible_summary, VisibleAttentionSummary};
use crate::agent::supervision::delegation_activity_adapter::{
    get_for_session, DelegationProjection, PendingDecisionProjection,
};
use crate::AppState;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::State;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkspaceActivityProjection {
    #[serde(rename = "workspaceId")]
    pub workspace_id: String,
    pub cursor: i64,
    pub delegations: Vec<DelegationProjection>,
    pub pending_decisions: Vec<PendingDecisionProjection>,
    pub attention: Option<VisibleAttentionSummary>,
}

pub fn get_workspace_activity_projection_impl(
    conn: &Connection,
    workspace_id: &str,
) -> Result<WorkspaceActivityProjection, String> {
    let session_id: String = conn
        .query_row(
            "SELECT active_session_id FROM projects WHERE id = ?1",
            params![workspace_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "工作区不存在。".to_owned())?;
    let projection = get_for_session(conn, &session_id)?;
    let attention = load_visible_summary(conn, &session_id)?;
    Ok(WorkspaceActivityProjection {
        workspace_id: workspace_id.to_owned(),
        cursor: projection.cursor,
        delegations: projection.delegations,
        pending_decisions: projection.pending_decisions,
        attention,
    })
}

#[tauri::command]
pub fn get_workspace_activity_projection(
    state: State<AppState>,
    workspace_id: String,
) -> Result<WorkspaceActivityProjection, String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    get_workspace_activity_projection_impl(&conn, &workspace_id)
}

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};

    use super::*;

    fn seed_attention_read_tables(conn: &Connection, session_ids: &[&str]) {
        conn.execute_batch(
            "CREATE TABLE profile (id INTEGER PRIMARY KEY, updated_at INTEGER);
             CREATE TABLE sessions (id TEXT PRIMARY KEY, work_dir TEXT);
             CREATE TABLE attention_states (
                 id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 goal_ref TEXT NOT NULL,
                 status TEXT NOT NULL,
                 scope_owner_profile_id INTEGER,
                 scope_workspace_key TEXT
             );",
        )
        .unwrap();
        for session_id in session_ids {
            conn.execute(
                "INSERT INTO sessions (id, work_dir) VALUES (?1, NULL)",
                params![session_id],
            )
            .unwrap();
        }
    }

    #[test]
    fn workspace_projection_resolves_its_internal_session() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (id TEXT PRIMARY KEY, active_session_id TEXT NOT NULL);
             CREATE TABLE delegations (id TEXT PRIMARY KEY, session_id TEXT, objective TEXT, status TEXT, brief_json TEXT, updated_at INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO projects VALUES ('workspace-1', 'internal-session')",
            [],
        )
        .unwrap();
        seed_attention_read_tables(&conn, &["internal-session"]);
        conn.execute(
            "INSERT INTO attention_states
             (id, session_id, goal_ref, status, scope_owner_profile_id, scope_workspace_key)
             VALUES ('attention-1', 'internal-session', 'message:reply-1', 'open', NULL, NULL)",
            [],
        )
        .unwrap();
        let projection = get_workspace_activity_projection_impl(&conn, "workspace-1").unwrap();
        assert_eq!(projection.workspace_id, "workspace-1");
        assert!(projection.delegations.is_empty());
        assert_eq!(
            projection
                .attention
                .expect("open Attention state should reach the workbench")
                .open_count,
            1
        );
    }

    #[test]
    fn workspace_projection_never_exposes_another_workspaces_delegation() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"CREATE TABLE projects (id TEXT PRIMARY KEY, active_session_id TEXT NOT NULL);
               CREATE TABLE delegations (
                   id TEXT PRIMARY KEY, session_id TEXT, objective TEXT,
                   status TEXT, brief_json TEXT, updated_at INTEGER
               );
               CREATE TABLE delegation_deliveries (
                   id TEXT, delegation_id TEXT, payload_json TEXT,
                   created_at INTEGER, delivery_version INTEGER
               );
               CREATE TABLE delegation_attempts (id TEXT, delegation_id TEXT);
               CREATE TABLE delegation_attempt_events (
                   attempt_id TEXT, sequence INTEGER, event_type TEXT,
                   payload_json TEXT, created_at INTEGER
               );
               CREATE TABLE delegation_review_jobs (
                   id TEXT, delivery_id TEXT, status TEXT, updated_at INTEGER
               );
               INSERT INTO projects VALUES
                   ('workspace-a', 'session-a'),
                   ('workspace-b', 'session-b');
               INSERT INTO delegations VALUES
                   ('delegation-a', 'session-a', 'inspect project A', 'running', '{}', 10),
                   ('delegation-b', 'session-b', 'inspect project B', 'completed', '{}', 20);"#,
        )
        .unwrap();
        seed_attention_read_tables(&conn, &["session-a", "session-b"]);

        let projection = get_workspace_activity_projection_impl(&conn, "workspace-a").unwrap();

        assert_eq!(projection.workspace_id, "workspace-a");
        assert_eq!(projection.delegations.len(), 1);
        assert_eq!(projection.delegations[0].id, "delegation-a");
        assert_eq!(projection.delegations[0].goal, "inspect project A");
    }

    #[test]
    fn unknown_workspace_is_rejected() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (id TEXT PRIMARY KEY, active_session_id TEXT NOT NULL);",
        )
        .unwrap();
        assert_eq!(
            get_workspace_activity_projection_impl(&conn, "missing").unwrap_err(),
            "工作区不存在。"
        );
    }
}
