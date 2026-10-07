//! User-facing workspace lifecycle.
//!
//! A workspace is the stable unit shown in the sidebar.  A project workspace
//! owns one directory; the Personal workspace owns none.  Sessions remain an
//! internal conversation/branch mechanism, with `active_session_id` selecting
//! the one currently projected to the user.
use crate::{commands::session::Session, AppState};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

pub const PERSONAL_WORKSPACE_ID: &str = "personal";

#[derive(Debug, Clone, Serialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(rename = "rootPath", skip_serializing_if = "Option::is_none")]
    pub root_path: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    #[serde(rename = "activeSessionId")]
    pub active_session_id: String,
}

#[derive(Debug, Serialize)]
pub struct OpenWorkspace {
    pub workspace: Workspace,
    pub session: Session,
}

#[tauri::command]
pub fn get_workspaces(state: State<AppState>) -> Result<Vec<Workspace>, String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    list_workspaces(&conn)
}

#[tauri::command]
pub fn create_project_workspace(
    state: State<AppState>,
    path: String,
    name: Option<String>,
    agent_provider: Option<String>,
    agent_model: Option<String>,
) -> Result<OpenWorkspace, String> {
    let root = canonical_project_root(&path)?;
    let suggested_name = root
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("未命名项目")
        .to_owned();
    let display_name = name
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(suggested_name);
    let root_path = crate::commands::file::display_work_dir(&root);
    let now = Utc::now().timestamp();
    let workspace_id = Uuid::new_v4().to_string();
    let session_id = Uuid::new_v4().to_string();
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;

    let duplicate: Option<String> = transaction
        .query_row(
            "SELECT id FROM projects WHERE kind = 'project' AND path = ?1",
            params![root_path],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if duplicate.is_some() {
        return Err("该目录已经是一个项目工作区。".to_owned());
    }

    transaction
        .execute(
            "INSERT INTO projects (id, name, path, created_at, kind, updated_at, active_session_id)
             VALUES (?1, ?2, ?3, ?4, 'project', ?4, NULL)",
            params![workspace_id, display_name, root_path, now],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO sessions (id, title, created_at, updated_at, context_version, work_dir, project_id, agent_provider, agent_model)
             VALUES (?1, '', ?2, ?2, 0, ?3, ?4, ?5, ?6)",
            params![session_id, now, root_path, workspace_id, agent_provider, agent_model],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE projects SET active_session_id = ?1 WHERE id = ?2",
            params![session_id, workspace_id],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;

    // The desktop E2E build needs one real, domain-owned approval to exercise
    // the project permission UI.  Keep the fixture internal to this command:
    // it is not a WebView-invokable test backdoor, is compiled out of normal
    // builds, and only activates for the explicit harness marker.
    #[cfg(feature = "desktop-e2e")]
    seed_desktop_e2e_network_approval(&conn, &root, &session_id, now)?;

    open_workspace_by_id(&conn, &workspace_id)
}

#[cfg(feature = "desktop-e2e")]
fn seed_desktop_e2e_network_approval(
    conn: &Connection,
    project_root: &std::path::Path,
    session_id: &str,
    now: i64,
) -> Result<(), String> {
    if std::env::var("ANGELBOT_DESKTOP_E2E_SEED_NETWORK_APPROVAL")
        .ok()
        .as_deref()
        != Some("1")
    {
        return Ok(());
    }

    use std::collections::BTreeSet;

    use crate::agent::{
        delegated_network_scope::{ConfirmedNetworkScope, NetworkScopeApprovalProvenance},
        worker_policy::NetworkAction,
        workspace_scope_key::canonical_workspace_scope_key,
    };

    let owner_profile_id = conn
        .query_row("SELECT id FROM profile WHERE id = 1", [], |row| row.get(0))
        .map_err(|_| "desktop E2E approval fixture requires an owner profile".to_owned())?;
    let (_, workspace_key) = canonical_workspace_scope_key(project_root).map_err(|_| {
        "desktop E2E approval fixture could not derive the project scope".to_owned()
    })?;
    let confirmed = ConfirmedNetworkScope::new(
        NetworkScopeApprovalProvenance {
            owner_profile_id,
            workspace_key,
            session_id: session_id.to_owned(),
            message_id: "desktop-e2e-network-message".to_owned(),
            parent_run_id: "desktop-e2e-network-run".to_owned(),
            tool_call_id: "desktop-e2e-network-call".to_owned(),
        },
        "desktop-e2e-network-scope".to_owned(),
        "desktop-e2e-network-approval".to_owned(),
        BTreeSet::from([NetworkAction::Search, NetworkAction::Fetch]),
        BTreeSet::from(["docs.example.test".to_owned()]),
    )
    .map_err(|error| format!("desktop E2E approval fixture is invalid: {error}"))?;
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    confirmed
        .persist_in_tx(&transaction, now)
        .map_err(|error| format!("desktop E2E approval fixture could not persist: {error}"))?;
    transaction.commit().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn open_workspace(state: State<AppState>, id: String) -> Result<OpenWorkspace, String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    open_workspace_by_id(&conn, &id)
}

fn canonical_project_root(path: &str) -> Result<std::path::PathBuf, String> {
    let requested = std::path::Path::new(path);
    if !requested.is_dir() {
        return Err(format!("项目目录不存在：{path}"));
    }
    std::fs::canonicalize(requested).map_err(|error| format!("无法解析项目目录：{error}"))
}

fn list_workspaces(conn: &Connection) -> Result<Vec<Workspace>, String> {
    let mut statement = conn
        .prepare(
            "SELECT p.id, p.name, p.kind, p.path, p.created_at,
                    COALESCE(s.updated_at, p.updated_at), p.active_session_id
             FROM projects p
             LEFT JOIN sessions s ON s.id = p.active_session_id
             ORDER BY CASE p.kind WHEN 'personal' THEN 0 ELSE 1 END,
                      COALESCE(s.updated_at, p.updated_at) DESC, p.name COLLATE NOCASE",
        )
        .map_err(|error| error.to_string())?;
    let workspaces = statement
        .query_map([], workspace_from_row)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(workspaces)
}

fn open_workspace_by_id(conn: &Connection, id: &str) -> Result<OpenWorkspace, String> {
    let workspace = conn
        .query_row(
            "SELECT p.id, p.name, p.kind, p.path, p.created_at,
                    COALESCE(s.updated_at, p.updated_at), p.active_session_id
             FROM projects p LEFT JOIN sessions s ON s.id = p.active_session_id
             WHERE p.id = ?1",
            params![id],
            workspace_from_row,
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "工作区不存在。".to_owned())?;
    let session = session_by_id(conn, &workspace.active_session_id)?;
    Ok(OpenWorkspace { workspace, session })
}

fn workspace_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
    let kind: String = row.get(2)?;
    let path: String = row.get(3)?;
    let stored_name: String = row.get(1)?;
    Ok(Workspace {
        id: row.get(0)?,
        // Keep this product label stable when upgrading an already-migrated
        // database that used the old internal name "AngelBot".
        name: (kind == "personal")
            .then_some("AngelBot 日常".to_owned())
            .unwrap_or(stored_name),
        root_path: (kind == "project").then_some(path),
        kind,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        active_session_id: row.get(6)?,
    })
}

fn session_by_id(conn: &Connection, id: &str) -> Result<Session, String> {
    conn.query_row(
        "SELECT s.id, s.title, s.parent_id, s.created_at, s.updated_at, s.context_version,
                s.last_compressed_at, s.work_dir,
                (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id),
                s.agent_provider, s.agent_model
         FROM sessions s WHERE s.id = ?1",
        params![id],
        |row| {
            Ok(Session {
                id: row.get(0)?,
                title: row.get(1)?,
                parent_id: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                context_version: row.get::<_, i32>(5).unwrap_or(0),
                last_compressed_at: row.get(6)?,
                work_dir: row.get(7)?,
                message_count: row.get::<_, Option<i32>>(8).unwrap_or(None),
                agent_provider: row.get(9)?,
                agent_model: row.get(10)?,
            })
        },
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_listing_and_opening_project_only_expose_its_main_session() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();

        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, kind, updated_at, active_session_id)\
             VALUES ('project-a', 'Project A', 'D:/Projects/project-a', 10, 'project', 20, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at, context_version, work_dir, project_id)\
             VALUES ('project-a-main', 'Project A', 10, 20, 0, 'D:/Projects/project-a', 'project-a')",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE projects SET active_session_id = 'project-a-main' WHERE id = 'project-a'",
            [],
        )
        .unwrap();

        let workspaces = list_workspaces(&conn).unwrap();
        assert_eq!(workspaces.len(), 2);
        assert_eq!(workspaces[0].id, PERSONAL_WORKSPACE_ID);
        assert!(workspaces[0].root_path.is_none());
        assert_eq!(workspaces[1].id, "project-a");
        assert_eq!(
            workspaces[1].root_path.as_deref(),
            Some("D:/Projects/project-a")
        );
        assert_eq!(workspaces[1].active_session_id, "project-a-main");

        let opened = open_workspace_by_id(&conn, "project-a").unwrap();
        assert_eq!(opened.workspace.active_session_id, opened.session.id);
        assert_eq!(
            opened.session.work_dir.as_deref(),
            Some("D:/Projects/project-a")
        );
    }
}
