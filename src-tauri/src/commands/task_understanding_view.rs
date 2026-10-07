//! User-facing, read-only projection of task-understanding decisions.
//!
//! The full understanding store remains internal.  This seam exposes only one
//! active question for the selected Workspace, never assumptions, raw tool
//! output, or another Workspace's records.

use crate::agent::task_understanding::{
    InformationAcquisitionAction, TaskUnderstanding, UnderstandingKind, UnderstandingSource,
    UnderstandingStatus,
};
use crate::AppState;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::State;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserInformationAction {
    Inspect,
    Ask,
    ActWithAssumption,
    Defer,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UserDecisionProjection {
    pub id: String,
    pub question: String,
    pub affects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkspaceTaskUnderstandingView {
    #[serde(rename = "workspaceId")]
    pub workspace_id: String,
    pub action: UserInformationAction,
    pub decision: Option<UserDecisionProjection>,
}

pub fn get_workspace_task_understanding_impl(
    conn: &Connection,
    workspace_id: &str,
) -> Result<WorkspaceTaskUnderstandingView, String> {
    let (workspace_kind, session_id): (String, String) = conn
        .query_row(
            "SELECT kind, active_session_id FROM projects WHERE id=?1",
            params![workspace_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "workspace does not exist".to_owned())?;
    if workspace_kind != "project" {
        return Ok(WorkspaceTaskUnderstandingView {
            workspace_id: workspace_id.to_owned(),
            action: UserInformationAction::Defer,
            decision: None,
        });
    }
    let projection = TaskUnderstanding::load_projection(conn, workspace_id, &session_id)?;
    let action = match TaskUnderstanding::recommend(&projection) {
        InformationAcquisitionAction::Inspect => UserInformationAction::Inspect,
        InformationAcquisitionAction::Ask => UserInformationAction::Ask,
        InformationAcquisitionAction::ActWithAssumption => UserInformationAction::ActWithAssumption,
        InformationAcquisitionAction::Defer => UserInformationAction::Defer,
    };
    let decision = (action == UserInformationAction::Ask)
        .then(|| {
            projection.records.iter().find(|record| {
                record.kind == UnderstandingKind::OpenQuestion
                    && record.source == UnderstandingSource::Agent
                    && matches!(
                        record.status,
                        UnderstandingStatus::Active | UnderstandingStatus::Confirmed
                    )
            })
        })
        .flatten()
        .map(|record| UserDecisionProjection {
            id: record.id.clone(),
            question: record.body.clone(),
            affects: record.affects.clone(),
        });
    Ok(WorkspaceTaskUnderstandingView {
        workspace_id: workspace_id.to_owned(),
        action,
        decision,
    })
}

#[tauri::command]
pub fn get_workspace_task_understanding(
    state: State<AppState>,
    workspace_id: String,
) -> Result<WorkspaceTaskUnderstandingView, String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    get_workspace_task_understanding_impl(&conn, &workspace_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_exposes_only_one_question_for_the_selected_workspace() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (id TEXT PRIMARY KEY, kind TEXT NOT NULL, active_session_id TEXT NOT NULL);
             CREATE TABLE sessions (id TEXT PRIMARY KEY);
             INSERT INTO projects VALUES ('w1','project','s1'),('w2','project','s2');
             INSERT INTO sessions VALUES ('s1'),('s2');",
        ).unwrap();
        conn.execute_batch(include_str!("../migrations/069_task_understanding.sql"))
            .unwrap();
        let insert = |workspace: &str, session: &str, id: &str, body: &str| {
            conn.execute(
                "INSERT INTO task_understanding_records (id,workspace_id,session_id,kind,source,scope,status,body,created_at,updated_at) VALUES (?1,?2,?3,'open_question','agent','task','active',?4,1,1)",
                params![id, workspace, session, body],
            ).unwrap();
        };
        insert("w1", "s1", "q1", "选择本地还是云端存储？");
        insert("w1", "s1", "q2", "这个问题不应同时显示");
        insert("w2", "s2", "q3", "另一个项目的问题");
        let view = get_workspace_task_understanding_impl(&conn, "w1").unwrap();
        assert_eq!(view.action, UserInformationAction::Ask);
        assert_eq!(view.decision.unwrap().id, "q1");
    }

    #[test]
    fn personal_workspace_has_no_task_understanding_projection() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (id TEXT PRIMARY KEY, kind TEXT NOT NULL, active_session_id TEXT NOT NULL);
             CREATE TABLE sessions (id TEXT PRIMARY KEY);
             INSERT INTO projects VALUES ('personal','personal','personal-main');
             INSERT INTO sessions VALUES ('personal-main');",
        ).unwrap();
        conn.execute_batch(include_str!("../migrations/069_task_understanding.sql"))
            .unwrap();
        let view = get_workspace_task_understanding_impl(&conn, "personal").unwrap();
        assert_eq!(view.action, UserInformationAction::Defer);
        assert!(view.decision.is_none());
    }
}
