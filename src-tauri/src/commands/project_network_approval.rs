//! User-facing commands for project delegated-network permissions.
//!
//! The command adapter intentionally delegates all policy and workspace logic
//! to the agent module.  IPC receives only an existing workspace id and an
//! opaque approval reference; it never accepts filesystem paths or policy ids.

use tauri::State;

use crate::{
    agent::{
        project_network_approval::{
            ProjectNetworkApprovalError, ProjectNetworkApprovalList, ProjectNetworkApprovalManager,
            ProjectNetworkApprovalRevocation,
        },
        shared_db::SharedDb,
    },
    AppState,
};

pub fn get_project_network_approvals_impl(
    state: &AppState,
    workspace_id: &str,
) -> Result<ProjectNetworkApprovalList, String> {
    ProjectNetworkApprovalManager::new(SharedDb::from_arc(state.db.clone()))
        .list(workspace_id)
        .map_err(user_message)
}

pub fn revoke_project_network_approval_impl(
    state: &AppState,
    workspace_id: &str,
    approval_ref: &str,
) -> Result<ProjectNetworkApprovalRevocation, String> {
    ProjectNetworkApprovalManager::new(SharedDb::from_arc(state.db.clone()))
        .revoke(workspace_id, approval_ref)
        .map_err(user_message)
}

#[tauri::command]
pub fn get_project_network_approvals(
    state: State<AppState>,
    workspace_id: String,
) -> Result<ProjectNetworkApprovalList, String> {
    get_project_network_approvals_impl(&state, &workspace_id)
}

#[tauri::command]
pub fn revoke_project_network_approval(
    state: State<AppState>,
    workspace_id: String,
    approval_ref: String,
) -> Result<ProjectNetworkApprovalRevocation, String> {
    revoke_project_network_approval_impl(&state, &workspace_id, &approval_ref)
}

fn user_message(error: ProjectNetworkApprovalError) -> String {
    error.to_string()
}
