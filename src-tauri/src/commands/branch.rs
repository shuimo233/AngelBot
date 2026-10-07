//! Session branching commands
//!
//! Issue #41: Session branches with tree structure
//!
//! Provides commands for creating, listing, switching, and merging session branches.

use crate::AppState;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

/// Branch information
#[derive(Debug, Serialize, Deserialize)]
pub struct BranchInfo {
    pub id: String,
    pub session_id: String,
    pub parent_branch_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub is_active: bool,
    pub message_count: i32,
}

/// Session with branch info
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionWithBranch {
    pub id: String,
    pub title: String,
    pub parent_id: Option<String>,
    pub branch_name: Option<String>,
    pub branch_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: i32,
    pub children: Vec<SessionWithBranch>,
}

/// Create a new branch from an existing session
#[tauri::command]
pub fn create_session_branch(
    state: State<'_, AppState>,
    session_id: String,
    branch_name: String,
    description: Option<String>,
) -> Result<BranchInfo, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();
    let branch_id = Uuid::new_v4().to_string();

    // Get the current active branch for this session (if any)
    let current_branch_id: Option<String> = conn
        .query_row(
            "SELECT id FROM session_branches WHERE session_id = ?1 AND is_active = 1 LIMIT 1",
            params![session_id],
            |r| r.get(0),
        )
        .ok();

    // Insert the new branch
    conn.execute(
        "INSERT INTO session_branches (id, session_id, parent_branch_id, name, description, created_at, updated_at, is_active)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
        params![branch_id, session_id, current_branch_id, branch_name, description, now, now],
    )
    .map_err(|e| e.to_string())?;

    // Create a new session as the branch (for message isolation)
    let new_session_id = Uuid::new_v4().to_string();
    let (project_id, work_dir, agent_provider, agent_model): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT project_id, work_dir, agent_provider, agent_model FROM sessions WHERE id = ?1",
            params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|error| error.to_string())?;
    conn.execute(
        "INSERT INTO sessions (id, parent_id, title, created_at, updated_at, branch_name, branch_created_at, project_id, work_dir, agent_provider, agent_model)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            new_session_id,
            session_id,
            format!("{} (分支)", branch_name),
            now,
            now,
            branch_name,
            now,
            project_id,
            work_dir,
            agent_provider,
            agent_model,
        ],
    )
    .map_err(|e| e.to_string())?;

    // Update message count
    let message_count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
            params![new_session_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    Ok(BranchInfo {
        id: branch_id,
        session_id: session_id,
        parent_branch_id: current_branch_id,
        name: branch_name,
        description,
        created_at: now,
        updated_at: now,
        is_active: false,
        message_count,
    })
}

/// Get all branches for a session
#[tauri::command]
pub fn get_session_branches(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<BranchInfo>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare(
            "SELECT id, session_id, parent_branch_id, name, description, created_at, updated_at, is_active, message_count
             FROM session_branches
             WHERE session_id = ?1
             ORDER BY created_at DESC",
        )
        .map_err(|e| e.to_string())?;

    let branches = stmt
        .query_map(params![session_id], |r| {
            Ok(BranchInfo {
                id: r.get(0)?,
                session_id: r.get(1)?,
                parent_branch_id: r.get(2)?,
                name: r.get(3)?,
                description: r.get(4)?,
                created_at: r.get(5)?,
                updated_at: r.get(6)?,
                is_active: r.get::<_, i32>(7)? == 1,
                message_count: r.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(branches)
}

/// Get the branch tree for a session
#[tauri::command]
pub fn get_session_branch_tree(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionWithBranch, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    fn build_tree(
        conn: &rusqlite::Connection,
        session_id: &str,
    ) -> Result<SessionWithBranch, rusqlite::Error> {
        let mut stmt = conn.prepare(
            "SELECT id, title, parent_id, branch_name, created_at, updated_at
             FROM sessions WHERE id = ?1",
        )?;

        let result = stmt.query_row(params![session_id], |r| {
            let session_id: String = r.get(0)?;
            let message_count: i32 = conn
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
                    params![session_id],
                    |r| r.get(0),
                )
                .unwrap_or(0);

            // Get children IDs first, then build trees
            let children_ids: Vec<String> = {
                let mut child_stmt =
                    conn.prepare("SELECT id FROM sessions WHERE parent_id = ?1")?;
                let rows = child_stmt
                    .query_map(params![session_id], |r| r.get::<_, String>(0))?
                    .filter_map(|r| r.ok())
                    .collect();
                rows
            };

            let children: Vec<SessionWithBranch> = children_ids
                .iter()
                .filter_map(|id| build_tree(conn, id).ok())
                .collect();

            Ok(SessionWithBranch {
                id: session_id,
                title: r.get(1)?,
                parent_id: r.get(2)?,
                branch_name: r.get(3)?,
                branch_id: None, // Will be set by caller
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
                message_count,
                children,
            })
        })?;

        // Set branch_id from session_branches if exists
        let branch_id: Option<String> = conn
            .query_row(
                "SELECT id FROM session_branches WHERE session_id = ?1 AND is_active = 1",
                params![session_id],
                |r| r.get(0),
            )
            .ok();

        Ok(SessionWithBranch {
            branch_id,
            ..result
        })
    }

    build_tree(&conn, &session_id).map_err(|e| e.to_string())
}

/// Switch to a different branch
#[tauri::command]
pub fn switch_to_branch(state: State<'_, AppState>, branch_id: String) -> Result<String, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    // Get the session_id for this branch
    let session_id: String = conn
        .query_row(
            "SELECT session_id FROM session_branches WHERE id = ?1",
            params![branch_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    // Deactivate all branches for this session
    conn.execute(
        "UPDATE session_branches SET is_active = 0 WHERE session_id = (SELECT session_id FROM session_branches WHERE id = ?1)",
        params![branch_id],
    )
    .map_err(|e| e.to_string())?;

    // Activate the target branch
    conn.execute(
        "UPDATE session_branches SET is_active = 1, updated_at = ?1 WHERE id = ?2",
        params![chrono::Utc::now().timestamp(), branch_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(session_id)
}

/// Delete a branch (soft delete - keeps messages)
#[tauri::command]
pub fn delete_branch(state: State<'_, AppState>, branch_id: String) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    // Get the session_id for this branch
    let session_id: String = conn
        .query_row(
            "SELECT session_id FROM session_branches WHERE id = ?1",
            params![branch_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    // Don't delete if it's the last branch
    let branch_count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_branches WHERE session_id = ?1",
            params![session_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    if branch_count <= 1 {
        return Err("Cannot delete the last branch".to_string());
    }

    // Delete the branch
    conn.execute(
        "DELETE FROM session_branches WHERE id = ?1",
        params![branch_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Get all branches across all sessions (for sidebar display)
#[tauri::command]
pub fn get_all_branches(state: State<'_, AppState>) -> Result<Vec<BranchInfo>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare(
            "SELECT sb.id, sb.session_id, sb.parent_branch_id, sb.name, sb.description, 
                    sb.created_at, sb.updated_at, sb.is_active, sb.message_count,
                    s.title
             FROM session_branches sb
             JOIN sessions s ON sb.session_id = s.id
             ORDER BY sb.updated_at DESC
             LIMIT 100",
        )
        .map_err(|e| e.to_string())?;

    let branches = stmt
        .query_map([], |r| {
            Ok(BranchInfo {
                id: r.get(0)?,
                session_id: r.get(1)?,
                parent_branch_id: r.get(2)?,
                name: r.get(3)?,
                description: r.get(4)?,
                created_at: r.get(5)?,
                updated_at: r.get(6)?,
                is_active: r.get::<_, i32>(7)? == 1,
                message_count: r.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(branches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_branch_info_serialization() {
        let branch = BranchInfo {
            id: "test-123".to_string(),
            session_id: "session-456".to_string(),
            parent_branch_id: None,
            name: "Test Branch".to_string(),
            description: Some("A test branch".to_string()),
            created_at: 1234567890,
            updated_at: 1234567890,
            is_active: false,
            message_count: 5,
        };

        let json = serde_json::to_string(&branch).unwrap();
        assert!(json.contains("test-123"));
        assert!(json.contains("Test Branch"));
    }
}
