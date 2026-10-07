//! Adapter that opens a durable delegated outbox gate after Workspace
//! supervision has claimed the matching work item. It never creates work or
//! grants capability; issuance owns those immutable facts.

use rusqlite::{params, OptionalExtension};

use crate::agent::shared_db::SharedDb;

use super::{
    ClaimedWork, DispatchedWork, WorkDispatcher, WorkspaceSupervisor, WorkspaceSupervisorActor,
};

#[derive(Clone)]
pub struct SupervisorDelegationDispatcher {
    db: SharedDb,
    now: fn() -> i64,
}

impl SupervisorDelegationDispatcher {
    pub fn new(db: SharedDb) -> Self {
        Self {
            db,
            now: || chrono::Utc::now().timestamp(),
        }
    }
}

impl WorkDispatcher for SupervisorDelegationDispatcher {
    fn dispatch(&self, claim: &ClaimedWork) -> Result<DispatchedWork, String> {
        let now = (self.now)();
        self.db
            .with_conn_mut(|connection| {
                let row: Option<(String, String)> = connection
                    .query_row(
                        "SELECT work_package_id, delegation_id
                 FROM workspace_supervisor_delegations
                 WHERE supervisor_work_id = ?1 AND authorized_at IS NULL",
                        [&claim.work.id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(|error| error.to_string())?;
                let Some((work_package_id, delegation_id)) = row else {
                    return Err("supervisor work has no pending delegated execution".into());
                };
                let changed = connection
                    .execute(
                        "UPDATE workspace_supervisor_delegations
                 SET authorized_at = ?1
                 WHERE supervisor_work_id = ?2 AND authorized_at IS NULL
                   AND EXISTS (
                     SELECT 1 FROM workspace_supervisor_work
                     WHERE id = ?2 AND workspace_id = ?3 AND status = 'claimed' AND claim_token = ?4
                   )",
                        params![
                            now,
                            claim.work.id,
                            claim.work.workspace_id,
                            claim.claim_token
                        ],
                    )
                    .map_err(|error| error.to_string())?;
                if changed != 1 {
                    return Err("supervisor execution authorization changed concurrently".into());
                }
                Ok(DispatchedWork {
                    work_package_id,
                    objective: claim.work.objective.clone(),
                    stage: claim.work.kind.initial_stage(),
                    summary: format!("Main Agent authorized delegated work {delegation_id}."),
                    artifact_refs: vec![format!("delegation:{delegation_id}")],
                })
            })
            .map_err(|error| error.to_string())?
    }
}

/// Bounded recovery hook for the existing delegated runtime pump. It only
/// authorizes a supervisor-linked item; ordinary historical outbox records
/// remain on their legacy path. Calling it before every outbox claim makes a
/// crash between issuance and foreground completion recoverable without a
/// second worker loop.
pub fn authorize_one_pending_delegation(db: SharedDb, now: i64) -> Result<bool, String> {
    let workspace_id = db
        .with_conn(|connection| {
            connection
                .query_row(
                    "SELECT work.workspace_id
             FROM workspace_supervisor_work work
             JOIN workspace_supervisor_delegations gate ON gate.supervisor_work_id = work.id
             WHERE work.status = 'queued' AND gate.authorized_at IS NULL
             ORDER BY work.created_at ASC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|error| error.to_string())
        })
        .map_err(|error| error.to_string())??;
    let Some(workspace_id) = workspace_id else {
        return Ok(false);
    };
    let supervisor = WorkspaceSupervisor::new(db.clone());
    let actor = WorkspaceSupervisorActor::new(supervisor, SupervisorDelegationDispatcher::new(db));
    actor
        .dispatch_next(&workspace_id, now)
        .map_err(|error| error.to_string())?;
    Ok(true)
}
