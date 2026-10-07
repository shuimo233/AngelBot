//! Project-scoped management of delegated-network approvals.
//!
//! This module is the only user-facing seam over confirmed delegated network
//! scopes.  Callers identify a product workspace and receive a small safe
//! projection; trusted project/session resolution, policy-digest checks, and
//! source-policy identifiers stay here.

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use super::{
    delegated_network_scope::{ApprovedNetworkScopeRepository, NetworkScopeApprovalError},
    shared_db::SharedDb,
    worker_policy::NetworkAction,
};

/// The safe projection for one currently effective project approval.
///
/// `approval_ref` is an opaque random confirmation id.  It is usable only for
/// a later revoke request scoped to this same project; no source-policy id,
/// path, query, URL, credential, or policy digest crosses this interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectNetworkApproval {
    pub approval_ref: String,
    pub granted_at: i64,
    pub hosts: Vec<String>,
    pub actions: Vec<NetworkAction>,
    pub max_response_bytes: usize,
    pub max_redirects: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectNetworkApprovalList {
    pub approvals: Vec<ProjectNetworkApproval>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectNetworkApprovalRevocationStatus {
    Revoked,
    AlreadyInactive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectNetworkApprovalRevocation {
    pub status: ProjectNetworkApprovalRevocationStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProjectNetworkApprovalError {
    #[error("联网权限仅可在项目工作区中管理。")]
    ProjectUnavailable,
    #[error("项目联网权限当前不可用，请稍后重试。")]
    Storage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectNetworkScope {
    owner_profile_id: i64,
    project_id: String,
}

/// A deep module for the two permission-management actions the product needs.
///
/// It deliberately has no editing or renewal interface: widening a scope must
/// go through a fresh Main-Agent confirmation, while revocation is an
/// idempotent reduction of authority.
#[derive(Clone)]
pub struct ProjectNetworkApprovalManager {
    db: SharedDb,
}

impl ProjectNetworkApprovalManager {
    pub fn new(db: SharedDb) -> Self {
        Self { db }
    }

    /// Return only approvals that are still enabled, unrevoked, and unchanged
    /// since their user confirmation.
    pub fn list(
        &self,
        workspace_id: &str,
    ) -> Result<ProjectNetworkApprovalList, ProjectNetworkApprovalError> {
        self.db
            .with_conn(|connection| list_from(connection, workspace_id))
            .map_err(|_| ProjectNetworkApprovalError::Storage)?
    }

    /// Revoke a known approval in the selected project.  Repeating a click,
    /// passing a stale ref, or passing a ref from another project all produce
    /// the same safe idempotent result.
    pub fn revoke(
        &self,
        workspace_id: &str,
        approval_ref: &str,
    ) -> Result<ProjectNetworkApprovalRevocation, ProjectNetworkApprovalError> {
        self.db
            .with_conn_mut(|connection| {
                revoke_from(
                    connection,
                    workspace_id,
                    approval_ref,
                    Utc::now().timestamp(),
                )
            })
            .map_err(|_| ProjectNetworkApprovalError::Storage)?
    }
}

fn list_from(
    conn: &Connection,
    workspace_id: &str,
) -> Result<ProjectNetworkApprovalList, ProjectNetworkApprovalError> {
    let scope = resolve_project_scope(conn, workspace_id)?;
    let approvals = ApprovedNetworkScopeRepository::list_current_for_project(
        conn,
        scope.owner_profile_id,
        &scope.project_id,
    )
    .map_err(map_scope_error)?
    .into_iter()
    .map(|approval| ProjectNetworkApproval {
        approval_ref: approval.approval_id,
        granted_at: approval.approved_at,
        hosts: approval.policy.allowed_hosts.into_iter().collect(),
        actions: approval.policy.actions.into_iter().collect(),
        max_response_bytes: approval.policy.max_response_bytes,
        max_redirects: approval.policy.max_redirects,
    })
    .collect();
    Ok(ProjectNetworkApprovalList { approvals })
}

fn revoke_from(
    conn: &mut Connection,
    workspace_id: &str,
    approval_ref: &str,
    now: i64,
) -> Result<ProjectNetworkApprovalRevocation, ProjectNetworkApprovalError> {
    let transaction = conn
        .transaction()
        .map_err(|_| ProjectNetworkApprovalError::Storage)?;
    let scope = resolve_project_scope(&transaction, workspace_id)?;
    let revoked = ApprovedNetworkScopeRepository::revoke_by_approval_id_for_project(
        &transaction,
        scope.owner_profile_id,
        &scope.project_id,
        approval_ref,
        now,
    )
    .map_err(map_scope_error)?;
    transaction
        .commit()
        .map_err(|_| ProjectNetworkApprovalError::Storage)?;
    Ok(ProjectNetworkApprovalRevocation {
        status: if revoked {
            ProjectNetworkApprovalRevocationStatus::Revoked
        } else {
            ProjectNetworkApprovalRevocationStatus::AlreadyInactive
        },
    })
}

fn resolve_project_scope(
    conn: &Connection,
    workspace_id: &str,
) -> Result<ProjectNetworkScope, ProjectNetworkApprovalError> {
    if workspace_id.trim().is_empty() || workspace_id.len() > 128 || workspace_id.contains('\0') {
        return Err(ProjectNetworkApprovalError::ProjectUnavailable);
    }
    let project_id: Option<String> = conn
        .query_row(
            "SELECT id FROM projects WHERE id=?1 AND kind='project'",
            [workspace_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| ProjectNetworkApprovalError::Storage)?;
    let Some(project_id) = project_id else {
        return Err(ProjectNetworkApprovalError::ProjectUnavailable);
    };
    let owner_profile_id = conn
        .query_row("SELECT id FROM profile WHERE id=1", [], |row| row.get(0))
        .map_err(|_| ProjectNetworkApprovalError::Storage)?;
    Ok(ProjectNetworkScope {
        owner_profile_id,
        project_id,
    })
}

fn map_scope_error(_: NetworkScopeApprovalError) -> ProjectNetworkApprovalError {
    ProjectNetworkApprovalError::Storage
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use rusqlite::{params, Connection};

    use super::*;
    use crate::{
        agent::{
            delegated_network_scope::{ConfirmedNetworkScope, NetworkScopeApprovalProvenance},
            shared_db::SharedDb,
        },
        db::migrate,
    };

    struct Fixture {
        directory: tempfile::TempDir,
        db: SharedDb,
        workspace_id: String,
        approval_ref: String,
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();

        let root = crate::commands::file::display_work_dir(
            &std::fs::canonicalize(directory.path()).unwrap(),
        );
        connection
            .execute(
                "INSERT INTO projects (id, name, path, created_at, kind, updated_at, active_session_id)
                 VALUES ('project-a', 'Project A', ?1, 1, 'project', 1, NULL)",
                params![&root],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sessions (id, title, created_at, updated_at, context_version, work_dir, project_id)
                 VALUES ('session-a', 'Project A', 1, 1, 0, ?1, 'project-a')",
                params![&root],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE projects SET active_session_id='session-a' WHERE id='project-a'",
                [],
            )
            .unwrap();

        let (_, workspace_key) =
            crate::agent::workspace_scope_key::canonical_workspace_scope_key(directory.path())
                .unwrap();
        let confirmed = ConfirmedNetworkScope::new(
            NetworkScopeApprovalProvenance {
                owner_profile_id: 1,
                workspace_key,
                session_id: "session-a".into(),
                message_id: "message-a".into(),
                parent_run_id: "run-a".into(),
                tool_call_id: "call-a".into(),
            },
            "network_scope_a".into(),
            "sha256:scope-a".into(),
            BTreeSet::from([NetworkAction::Search, NetworkAction::Fetch]),
            BTreeSet::from(["docs.example.com".into(), "search.example.com".into()]),
        )
        .unwrap();
        let approval_ref = confirmed.approval().id.clone();
        let transaction = connection.transaction().unwrap();
        confirmed.persist_in_tx(&transaction, 123).unwrap();
        transaction.commit().unwrap();

        Fixture {
            directory,
            db: SharedDb::new(connection),
            workspace_id: "project-a".into(),
            approval_ref,
        }
    }

    #[test]
    fn lists_only_current_project_approvals_as_a_safe_projection() {
        let fixture = fixture();
        let manager = ProjectNetworkApprovalManager::new(fixture.db.clone());

        let approvals = manager.list(&fixture.workspace_id).unwrap();

        assert_eq!(approvals.approvals.len(), 1);
        assert_eq!(approvals.approvals[0].approval_ref, fixture.approval_ref);
        assert_eq!(approvals.approvals[0].granted_at, 123);
        assert_eq!(
            approvals.approvals[0].hosts,
            vec!["docs.example.com", "search.example.com"]
        );
        assert_eq!(
            approvals.approvals[0].actions,
            vec![NetworkAction::Search, NetworkAction::Fetch]
        );
        assert_eq!(approvals.approvals[0].max_response_bytes, 512 * 1024);
        assert_eq!(approvals.approvals[0].max_redirects, 3);
    }

    #[test]
    fn hides_revoked_and_policy_drifted_approvals() {
        let fixture = fixture();
        let manager = ProjectNetworkApprovalManager::new(fixture.db.clone());
        fixture
            .db
            .with_conn(|connection| {
                connection.execute(
                    "UPDATE delegated_network_source_policies
                     SET allowed_hosts_json='[\"different.example.com\"]'
                     WHERE id=(SELECT policy_id FROM delegated_network_scope_approvals WHERE id=?1)",
                    [fixture.approval_ref.as_str()],
                )
            })
            .unwrap()
            .unwrap();
        assert!(manager
            .list(&fixture.workspace_id)
            .unwrap()
            .approvals
            .is_empty());

        fixture
            .db
            .with_conn(|connection| {
                connection.execute(
                    "UPDATE delegated_network_source_policies
                     SET allowed_hosts_json='[\"docs.example.com\",\"search.example.com\"]'
                     WHERE id=(SELECT policy_id FROM delegated_network_scope_approvals WHERE id=?1)",
                    [fixture.approval_ref.as_str()],
                )
            })
            .unwrap()
            .unwrap();
        let revocation = manager
            .revoke(&fixture.workspace_id, &fixture.approval_ref)
            .unwrap();
        assert_eq!(
            revocation.status,
            ProjectNetworkApprovalRevocationStatus::Revoked
        );
        assert!(manager
            .list(&fixture.workspace_id)
            .unwrap()
            .approvals
            .is_empty());
    }

    #[test]
    fn revocation_is_idempotent_and_cannot_cross_project_scope() {
        let fixture = fixture();
        let other_directory = tempfile::tempdir().unwrap();
        fixture
            .db
            .with_conn(|connection| {
                connection.execute(
                    "INSERT INTO projects (id, name, path, created_at, kind, updated_at, active_session_id)
                     VALUES ('project-b', 'Project B', ?1, 1, 'project', 1, NULL)",
                    [crate::commands::file::display_work_dir(
                        &std::fs::canonicalize(other_directory.path()).unwrap(),
                    )],
                )
            })
            .unwrap()
            .unwrap();
        let manager = ProjectNetworkApprovalManager::new(fixture.db.clone());

        assert_eq!(
            manager
                .revoke("project-b", &fixture.approval_ref)
                .unwrap()
                .status,
            ProjectNetworkApprovalRevocationStatus::AlreadyInactive
        );
        assert_eq!(
            manager
                .revoke(&fixture.workspace_id, "nsa_unknown")
                .unwrap()
                .status,
            ProjectNetworkApprovalRevocationStatus::AlreadyInactive
        );
        assert_eq!(
            manager
                .revoke(&fixture.workspace_id, &fixture.approval_ref)
                .unwrap()
                .status,
            ProjectNetworkApprovalRevocationStatus::Revoked
        );
        assert_eq!(
            manager
                .revoke(&fixture.workspace_id, &fixture.approval_ref)
                .unwrap()
                .status,
            ProjectNetworkApprovalRevocationStatus::AlreadyInactive
        );
    }

    #[test]
    fn lists_and_revokes_a_project_approval_after_its_root_is_unavailable() {
        let fixture = fixture();
        std::fs::remove_dir_all(fixture.directory.path()).unwrap();
        let manager = ProjectNetworkApprovalManager::new(fixture.db.clone());

        assert_eq!(
            manager.list(&fixture.workspace_id).unwrap().approvals.len(),
            1
        );
        assert_eq!(
            manager
                .revoke(&fixture.workspace_id, &fixture.approval_ref)
                .unwrap()
                .status,
            ProjectNetworkApprovalRevocationStatus::Revoked
        );
    }

    #[test]
    fn lists_and_revokes_a_project_approval_after_its_stored_root_moves() {
        let fixture = fixture();
        fixture
            .db
            .with_conn(|connection| {
                connection.execute(
                    "UPDATE projects SET path='D:/moved-or-missing/project-a' WHERE id='project-a'",
                    [],
                )
            })
            .unwrap()
            .unwrap();
        let manager = ProjectNetworkApprovalManager::new(fixture.db.clone());

        assert_eq!(
            manager.list(&fixture.workspace_id).unwrap().approvals.len(),
            1
        );
        assert_eq!(
            manager
                .revoke(&fixture.workspace_id, &fixture.approval_ref)
                .unwrap()
                .status,
            ProjectNetworkApprovalRevocationStatus::Revoked
        );
    }

    #[test]
    fn rejects_personal_or_missing_workspaces_without_exposing_scope_state() {
        let fixture = fixture();
        let manager = ProjectNetworkApprovalManager::new(fixture.db);

        assert_eq!(
            manager.list("personal").unwrap_err(),
            ProjectNetworkApprovalError::ProjectUnavailable
        );
        assert_eq!(
            manager.list("not-a-workspace").unwrap_err(),
            ProjectNetworkApprovalError::ProjectUnavailable
        );
    }

    #[test]
    fn serializes_the_frontend_contract_without_policy_identifiers() {
        let fixture = fixture();
        let manager = ProjectNetworkApprovalManager::new(fixture.db);
        let listed = serde_json::to_value(manager.list(&fixture.workspace_id).unwrap()).unwrap();
        let approval = &listed["approvals"][0];

        assert!(listed.get("approvals").is_some());
        assert_eq!(approval["approvalRef"], fixture.approval_ref);
        assert_eq!(approval["grantedAt"], 123);
        assert_eq!(
            approval["hosts"],
            serde_json::json!(["docs.example.com", "search.example.com"])
        );
        assert_eq!(approval["actions"], serde_json::json!(["search", "fetch"]));
        assert_eq!(approval["maxResponseBytes"], 512 * 1024);
        assert_eq!(approval["maxRedirects"], 3);
        assert!(approval.get("policyId").is_none());
        assert!(approval.get("workspaceKey").is_none());
        assert!(approval.get("policyDigest").is_none());

        let revoked = serde_json::to_value(
            manager
                .revoke(&fixture.workspace_id, &fixture.approval_ref)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(revoked, serde_json::json!({ "status": "revoked" }));

        let already_inactive = serde_json::to_value(
            manager
                .revoke(&fixture.workspace_id, &fixture.approval_ref)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            already_inactive,
            serde_json::json!({ "status": "already_inactive" })
        );
    }
}
