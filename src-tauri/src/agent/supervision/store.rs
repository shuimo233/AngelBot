use std::fmt;

use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use uuid::Uuid;

use crate::agent::shared_db::{SharedDb, SharedDbError};

use super::{
    ActivityProjection, ActivityStage, ClaimedFollowUp, ClaimedInput, ClaimedWork, SupervisorInput,
    SupervisorInputKind, SupervisorInputStatus, SupervisorWork, SupervisorWorkKind,
    SupervisorWorkStatus,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorError {
    InvalidInput,
    WorkspaceNotFound,
    ClaimNotFound,
    Dispatch(String),
    Storage(String),
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput => formatter.write_str("invalid workspace supervisor input"),
            Self::WorkspaceNotFound => formatter.write_str("workspace not found"),
            Self::ClaimNotFound => formatter.write_str("workspace supervisor claim not found"),
            Self::Dispatch(message) => formatter.write_str(message),
            Self::Storage(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SupervisorError {}

impl From<SharedDbError> for SupervisorError {
    fn from(error: SharedDbError) -> Self {
        Self::Storage(error.to_string())
    }
}

/// The only scheduling interface a Workspace caller needs during P0.
#[derive(Clone)]
pub struct WorkspaceSupervisor {
    db: SharedDb,
}

impl WorkspaceSupervisor {
    pub fn new(db: SharedDb) -> Self {
        Self { db }
    }

    /// Resolves the stable Workspace container from an internal foreground
    /// session. Tool handlers never accept a Workspace id from model output.
    pub fn workspace_for_session(&self, session_id: &str) -> Result<String, SupervisorError> {
        if !valid_short(session_id) {
            return Err(SupervisorError::InvalidInput);
        }
        self.db.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT id FROM projects WHERE active_session_id = ?1",
                    [session_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?
                .ok_or(SupervisorError::WorkspaceNotFound)
        })?
    }

    pub fn submit(
        &self,
        workspace_id: impl Into<String>,
        kind: SupervisorInputKind,
        content: impl Into<String>,
        now: i64,
    ) -> Result<SupervisorInput, SupervisorError> {
        let workspace_id = workspace_id.into();
        let content = content.into();
        self.db.with_conn_mut(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let input = Self::submit_in_tx(&transaction, &workspace_id, kind, &content, now)?;
            transaction.commit().map_err(storage)?;
            Ok(input)
        })?
    }

    /// Inserts a queued input into a caller-owned transaction.
    ///
    /// Cross-domain producers use this narrow seam when their own durable
    /// source record and the supervisor input must become visible together.
    /// The Workspace is still validated here; callers never get a bypass for
    /// model-controlled or stale workspace ids.
    pub(crate) fn submit_in_tx(
        transaction: &Transaction<'_>,
        workspace_id: &str,
        kind: SupervisorInputKind,
        content: &str,
        now: i64,
    ) -> Result<SupervisorInput, SupervisorError> {
        if !valid_short(workspace_id) || content.trim().is_empty() || content.len() > 16_384 {
            return Err(SupervisorError::InvalidInput);
        }
        let exists: Option<String> = transaction
            .query_row(
                "SELECT id FROM projects WHERE id = ?1",
                [workspace_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        if exists.is_none() {
            return Err(SupervisorError::WorkspaceNotFound);
        }

        let input = SupervisorInput {
            id: format!("wsin_{}", Uuid::new_v4().simple()),
            workspace_id: workspace_id.into(),
            kind,
            content: content.into(),
            status: SupervisorInputStatus::Queued,
            created_at: now,
            updated_at: now,
        };
        transaction
            .execute(
                "INSERT INTO workspace_supervisor_inputs
                    (id, workspace_id, kind, content, priority, status, claim_token, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'queued', NULL, ?6, ?6)",
                params![
                    input.id,
                    input.workspace_id,
                    input.kind.as_str(),
                    input.content,
                    input.kind.priority(),
                    now
                ],
            )
            .map_err(storage)?;
        Ok(input)
    }

    /// Atomically claims the next eligible input. The returned token is needed
    /// to complete or cancel it, so late workers cannot settle a newer claim.
    pub fn claim_next(
        &self,
        workspace_id: &str,
        now: i64,
    ) -> Result<Option<ClaimedInput>, SupervisorError> {
        if !valid_short(workspace_id) {
            return Err(SupervisorError::InvalidInput);
        }
        self.db.with_conn_mut(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let queued = transaction
                .query_row(
                    "SELECT id, kind, content, created_at, updated_at
                 FROM workspace_supervisor_inputs
                 WHERE workspace_id = ?1 AND status = 'queued'
                 ORDER BY priority DESC, created_at ASC LIMIT 1",
                    [workspace_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage)?;
            let Some((id, kind, content, created_at, _)) = queued else {
                transaction.commit().map_err(storage)?;
                return Ok(None);
            };
            let kind = SupervisorInputKind::from_str(&kind)
                .ok_or_else(|| SupervisorError::Storage("unknown supervisor input kind".into()))?;
            let claim_token = format!("claim_{}", Uuid::new_v4().simple());
            let changed = transaction
                .execute(
                    "UPDATE workspace_supervisor_inputs
                 SET status = 'claimed', claim_token = ?1, updated_at = ?2
                 WHERE id = ?3 AND status = 'queued'",
                    params![claim_token, now, id],
                )
                .map_err(storage)?;
            if changed != 1 {
                return Err(SupervisorError::ClaimNotFound);
            }
            transaction.commit().map_err(storage)?;
            Ok(Some(ClaimedInput {
                input: SupervisorInput {
                    id,
                    workspace_id: workspace_id.into(),
                    kind,
                    content,
                    status: SupervisorInputStatus::Claimed,
                    created_at,
                    updated_at: now,
                },
                claim_token,
            }))
        })?
    }

    /// Claim the next durable Main-Agent follow-up for one Workspace and bind
    /// it to that Workspace's persisted primary session.  This is deliberately
    /// narrower than `claim_next`: automation dispatch must not consume future
    /// user-task or steering inputs, and it must never accept a session id from
    /// a caller.
    pub fn claim_next_follow_up(
        &self,
        workspace_id: &str,
        now: i64,
    ) -> Result<Option<ClaimedFollowUp>, SupervisorError> {
        if !valid_short(workspace_id) {
            return Err(SupervisorError::InvalidInput);
        }
        self.db.with_conn_mut(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let queued = transaction
                .query_row(
                    "SELECT input.id, input.content, input.created_at, project.active_session_id
                     FROM workspace_supervisor_inputs input
                     JOIN projects project ON project.id = input.workspace_id
                     JOIN sessions session ON session.id = project.active_session_id
                     WHERE input.workspace_id = ?1
                       AND input.kind = 'follow_up'
                       AND input.status = 'queued'
                     ORDER BY input.priority DESC, input.created_at ASC
                     LIMIT 1",
                    [workspace_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage)?;
            let Some((id, content, created_at, session_id)) = queued else {
                transaction.commit().map_err(storage)?;
                return Ok(None);
            };
            let claim_token = format!("claim_{}", Uuid::new_v4().simple());
            let changed = transaction
                .execute(
                    "UPDATE workspace_supervisor_inputs
                     SET status = 'claimed', claim_token = ?1, updated_at = ?2
                     WHERE id = ?3 AND workspace_id = ?4
                       AND kind = 'follow_up' AND status = 'queued'",
                    params![claim_token, now, id, workspace_id],
                )
                .map_err(storage)?;
            if changed != 1 {
                return Err(SupervisorError::ClaimNotFound);
            }
            transaction.commit().map_err(storage)?;
            Ok(Some(ClaimedFollowUp {
                claim: ClaimedInput {
                    input: SupervisorInput {
                        id,
                        workspace_id: workspace_id.into(),
                        kind: SupervisorInputKind::FollowUp,
                        content,
                        status: SupervisorInputStatus::Claimed,
                        created_at,
                        updated_at: now,
                    },
                    claim_token,
                },
                session_id,
            }))
        })?
    }

    /// Return Workspaces that have durable automation-compatible follow-ups
    /// ready for their primary Main Agent.  It is intentionally a compact
    /// recovery/wake surface; dispatch remains responsible for claiming each
    /// item with a CAS token.
    pub fn queued_follow_up_workspace_ids(&self) -> Result<Vec<String>, SupervisorError> {
        self.db.with_conn(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT DISTINCT input.workspace_id
                     FROM workspace_supervisor_inputs input
                     JOIN projects project ON project.id = input.workspace_id
                     JOIN sessions session ON session.id = project.active_session_id
                     WHERE input.kind = 'follow_up' AND input.status = 'queued'
                     ORDER BY input.workspace_id ASC",
                )
                .map_err(storage)?;
            let workspace_ids = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            Ok(workspace_ids)
        })?
    }

    pub fn settle(
        &self,
        claim: &ClaimedInput,
        status: SupervisorInputStatus,
        now: i64,
    ) -> Result<(), SupervisorError> {
        if !matches!(
            status,
            SupervisorInputStatus::Completed | SupervisorInputStatus::Cancelled
        ) {
            return Err(SupervisorError::InvalidInput);
        }
        self.db.with_conn_mut(|connection| {
            let changed = connection
                .execute(
                    "UPDATE workspace_supervisor_inputs SET status = ?1, updated_at = ?2
                 WHERE id = ?3 AND workspace_id = ?4 AND status = 'claimed' AND claim_token = ?5",
                    params![
                        status.as_str(),
                        now,
                        claim.input.id,
                        claim.input.workspace_id,
                        claim.claim_token
                    ],
                )
                .map_err(storage)?;
            if changed == 1 {
                Ok(())
            } else {
                Err(SupervisorError::ClaimNotFound)
            }
        })?
    }

    /// Makes a claimed input eligible again only when the caller still owns
    /// its token. A transient dispatcher failure can therefore never settle a
    /// newer claim or strand the Workspace queue.
    pub fn release(&self, claim: &ClaimedInput, now: i64) -> Result<(), SupervisorError> {
        self.db.with_conn_mut(|connection| {
            let changed = connection
                .execute(
                    "UPDATE workspace_supervisor_inputs
                 SET status = 'queued', claim_token = NULL, updated_at = ?1
                 WHERE id = ?2 AND workspace_id = ?3 AND status = 'claimed' AND claim_token = ?4",
                    params![
                        now,
                        claim.input.id,
                        claim.input.workspace_id,
                        claim.claim_token
                    ],
                )
                .map_err(storage)?;
            if changed == 1 {
                Ok(())
            } else {
                Err(SupervisorError::ClaimNotFound)
            }
        })?
    }

    /// Queues work only after the Main Agent has selected a worker shape. This
    /// is separate from `submit`, which stores user interaction input.
    pub fn queue_work(
        &self,
        workspace_id: impl Into<String>,
        source_input_id: Option<String>,
        kind: SupervisorWorkKind,
        objective: impl Into<String>,
        now: i64,
    ) -> Result<SupervisorWork, SupervisorError> {
        let workspace_id = workspace_id.into();
        let objective = objective.into();
        if !valid_short(&workspace_id)
            || objective.trim().is_empty()
            || objective.len() > 16_384
            || source_input_id.as_ref().is_some_and(|id| !valid_short(id))
        {
            return Err(SupervisorError::InvalidInput);
        }
        let work = SupervisorWork {
            id: format!("wswork_{}", Uuid::new_v4().simple()),
            workspace_id,
            source_input_id,
            kind,
            objective,
            status: SupervisorWorkStatus::Queued,
            created_at: now,
            updated_at: now,
        };
        self.db.with_conn_mut(|connection| {
            let exists: Option<String> = connection.query_row(
                "SELECT id FROM projects WHERE id = ?1", [&work.workspace_id], |row| row.get(0),
            ).optional().map_err(storage)?;
            if exists.is_none() { return Err(SupervisorError::WorkspaceNotFound); }
            if let Some(input_id) = &work.source_input_id {
                let linked: Option<String> = connection.query_row(
                    "SELECT id FROM workspace_supervisor_inputs WHERE id = ?1 AND workspace_id = ?2",
                    params![input_id, work.workspace_id], |row| row.get(0),
                ).optional().map_err(storage)?;
                if linked.is_none() { return Err(SupervisorError::InvalidInput); }
            }
            connection.execute(
                "INSERT INTO workspace_supervisor_work
                    (id, workspace_id, source_input_id, kind, objective, status, claim_token, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'queued', NULL, ?6, ?6)",
                params![work.id, work.workspace_id, work.source_input_id, work.kind.as_str(), work.objective, now],
            ).map_err(storage)?;
            Ok(work)
        })?
    }

    pub fn claim_next_work(
        &self,
        workspace_id: &str,
        now: i64,
    ) -> Result<Option<ClaimedWork>, SupervisorError> {
        if !valid_short(workspace_id) {
            return Err(SupervisorError::InvalidInput);
        }
        self.db.with_conn_mut(|connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let queued = transaction.query_row(
                "SELECT id, source_input_id, kind, objective, created_at
                 FROM workspace_supervisor_work WHERE workspace_id = ?1 AND status = 'queued'
                 ORDER BY created_at ASC LIMIT 1", [workspace_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, i64>(4)?)),
            ).optional().map_err(storage)?;
            let Some((id, source_input_id, kind, objective, created_at)) = queued else {
                transaction.commit().map_err(storage)?;
                return Ok(None);
            };
            let kind = SupervisorWorkKind::from_str(&kind).ok_or_else(|| SupervisorError::Storage("unknown supervisor work kind".into()))?;
            let claim_token = format!("workclaim_{}", Uuid::new_v4().simple());
            let changed = transaction.execute(
                "UPDATE workspace_supervisor_work SET status = 'claimed', claim_token = ?1, updated_at = ?2
                 WHERE id = ?3 AND status = 'queued'", params![claim_token, now, id],
            ).map_err(storage)?;
            if changed != 1 { return Err(SupervisorError::ClaimNotFound); }
            transaction.commit().map_err(storage)?;
            Ok(Some(ClaimedWork {
                work: SupervisorWork { id, workspace_id: workspace_id.into(), source_input_id, kind, objective, status: SupervisorWorkStatus::Claimed, created_at, updated_at: now },
                claim_token,
            }))
        })?
    }

    pub fn settle_work(
        &self,
        claim: &ClaimedWork,
        status: SupervisorWorkStatus,
        now: i64,
    ) -> Result<(), SupervisorError> {
        if !matches!(
            status,
            SupervisorWorkStatus::Completed | SupervisorWorkStatus::Cancelled
        ) {
            return Err(SupervisorError::InvalidInput);
        }
        self.update_claimed_work(claim, status.as_str(), false, now)
    }

    pub fn release_work(&self, claim: &ClaimedWork, now: i64) -> Result<(), SupervisorError> {
        self.update_claimed_work(claim, "queued", true, now)
    }

    /// Compensates a pre-created work item when admission fails before a
    /// durable delegation can be linked to it. It is intentionally narrower
    /// than general cancellation: only an untouched queued item may be
    /// cancelled by its creator.
    pub fn cancel_queued_work(
        &self,
        work: &SupervisorWork,
        now: i64,
    ) -> Result<(), SupervisorError> {
        self.db.with_conn_mut(|connection| {
            let changed = connection
                .execute(
                    "UPDATE workspace_supervisor_work SET status = 'cancelled', updated_at = ?1
                 WHERE id = ?2 AND workspace_id = ?3 AND status = 'queued'",
                    params![now, work.id, work.workspace_id],
                )
                .map_err(storage)?;
            if changed == 1 {
                Ok(())
            } else {
                Err(SupervisorError::ClaimNotFound)
            }
        })?
    }

    fn update_claimed_work(
        &self,
        claim: &ClaimedWork,
        status: &str,
        clear_token: bool,
        now: i64,
    ) -> Result<(), SupervisorError> {
        self.db.with_conn_mut(|connection| {
            let changed = connection
                .execute(
                    "UPDATE workspace_supervisor_work SET status = ?1,
                    claim_token = CASE WHEN ?2 THEN NULL ELSE claim_token END, updated_at = ?3
                 WHERE id = ?4 AND workspace_id = ?5 AND status = 'claimed' AND claim_token = ?6",
                    params![
                        status,
                        clear_token,
                        now,
                        claim.work.id,
                        claim.work.workspace_id,
                        claim.claim_token
                    ],
                )
                .map_err(storage)?;
            if changed == 1 {
                Ok(())
            } else {
                Err(SupervisorError::ClaimNotFound)
            }
        })?
    }

    pub fn publish_activity(&self, projection: ActivityProjection) -> Result<(), SupervisorError> {
        if !valid_short(&projection.workspace_id)
            || !valid_short(&projection.work_package_id)
            || projection.objective.trim().is_empty()
            || projection.summary.trim().is_empty()
            || projection.artifact_refs.len() > 32
            || projection
                .artifact_refs
                .iter()
                .any(|reference| !valid_short(reference))
        {
            return Err(SupervisorError::InvalidInput);
        }
        let artifacts = serde_json::to_string(&projection.artifact_refs)
            .map_err(|error| SupervisorError::Storage(error.to_string()))?;
        self.db.with_conn_mut(|connection| {
            connection.execute(
                "INSERT INTO workspace_activity_projections
                    (workspace_id, work_package_id, objective, stage, summary, artifact_refs_json, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(workspace_id, work_package_id) DO UPDATE SET
                    objective=excluded.objective, stage=excluded.stage, summary=excluded.summary,
                    artifact_refs_json=excluded.artifact_refs_json, updated_at=excluded.updated_at",
                params![projection.workspace_id, projection.work_package_id, projection.objective, projection.stage.as_str(), projection.summary, artifacts, projection.updated_at],
            ).map_err(storage)?;
            Ok(())
        })?
    }

    pub fn activities(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<ActivityProjection>, SupervisorError> {
        if !valid_short(workspace_id) {
            return Err(SupervisorError::InvalidInput);
        }
        self.db.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT work_package_id, objective, stage, summary, artifact_refs_json, updated_at
                 FROM workspace_activity_projections WHERE workspace_id = ?1 ORDER BY updated_at DESC",
            ).map_err(storage)?;
            let activities = statement.query_map([workspace_id], |row| {
                let stage: String = row.get(2)?;
                let refs: String = row.get(4)?;
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, stage, row.get::<_, String>(3)?, refs, row.get::<_, i64>(5)?))
            }).map_err(storage)?.map(|row| {
                let (work_package_id, objective, stage, summary, refs, updated_at) = row.map_err(storage)?;
                Ok(ActivityProjection {
                    workspace_id: workspace_id.into(), work_package_id, objective,
                    stage: ActivityStage::from_str(&stage).ok_or_else(|| SupervisorError::Storage("unknown activity stage".into()))?,
                    summary, artifact_refs: serde_json::from_str(&refs).map_err(|error| SupervisorError::Storage(error.to_string()))?, updated_at,
                })
            }).collect();
            activities
        })?
    }
}

fn valid_short(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':' | b'.' | b'/')
        })
}

fn storage(error: rusqlite::Error) -> SupervisorError {
    SupervisorError::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::{agent::shared_db::SharedDb, db::migrate};

    fn supervisor() -> WorkspaceSupervisor {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        connection.execute("INSERT INTO projects (id, name, path, created_at, kind, updated_at) VALUES ('p', 'p', 'C:/p', 1, 'project', 1)", []).unwrap();
        connection
            .execute(
                "INSERT INTO sessions (id, title, created_at, updated_at) VALUES ('s', 'Main', 1, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE projects SET active_session_id = 's' WHERE id = 'p'",
                [],
            )
            .unwrap();
        WorkspaceSupervisor::new(SharedDb::new(connection))
    }

    #[test]
    fn steer_preempts_new_task_and_follow_up() {
        let supervisor = supervisor();
        supervisor
            .submit("p", SupervisorInputKind::FollowUp, "later", 1)
            .unwrap();
        supervisor
            .submit("p", SupervisorInputKind::UserTask, "task", 2)
            .unwrap();
        supervisor
            .submit("p", SupervisorInputKind::Steer, "stop", 3)
            .unwrap();
        let claim = supervisor.claim_next("p", 4).unwrap().unwrap();
        assert_eq!(claim.input.kind, SupervisorInputKind::Steer);
        supervisor
            .settle(&claim, SupervisorInputStatus::Completed, 5)
            .unwrap();
        assert_eq!(
            supervisor.claim_next("p", 6).unwrap().unwrap().input.kind,
            SupervisorInputKind::UserTask
        );
    }

    #[test]
    fn claim_token_prevents_late_settlement() {
        let supervisor = supervisor();
        supervisor
            .submit("p", SupervisorInputKind::UserTask, "task", 1)
            .unwrap();
        let mut forged = supervisor.claim_next("p", 2).unwrap().unwrap();
        forged.claim_token = "claim_forged".into();
        assert_eq!(
            supervisor.settle(&forged, SupervisorInputStatus::Completed, 3),
            Err(SupervisorError::ClaimNotFound)
        );
    }

    #[test]
    fn automation_follow_up_claim_uses_only_the_workspace_main_session() {
        let supervisor = supervisor();
        supervisor
            .submit("p", SupervisorInputKind::UserTask, "do not consume", 1)
            .unwrap();
        supervisor
            .submit("p", SupervisorInputKind::FollowUp, "scheduled work", 2)
            .unwrap();

        assert_eq!(
            supervisor.queued_follow_up_workspace_ids().unwrap(),
            vec!["p"]
        );
        let claimed = supervisor.claim_next_follow_up("p", 3).unwrap().unwrap();
        assert_eq!(claimed.session_id, "s");
        assert_eq!(claimed.claim.input.kind, SupervisorInputKind::FollowUp);
        assert_eq!(claimed.claim.input.content, "scheduled work");
        assert_eq!(
            supervisor.claim_next("p", 4).unwrap().unwrap().input.kind,
            SupervisorInputKind::UserTask
        );
    }

    #[test]
    fn activity_projection_is_workspace_scoped_and_user_safe() {
        let supervisor = supervisor();
        supervisor
            .publish_activity(ActivityProjection {
                workspace_id: "p".into(),
                work_package_id: "work_1".into(),
                objective: "Inspect files".into(),
                stage: ActivityStage::DelegatedExploration,
                summary: "Checking project structure".into(),
                artifact_refs: vec!["artifact:summary".into()],
                updated_at: 3,
            })
            .unwrap();
        let activities = supervisor.activities("p").unwrap();
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].stage, ActivityStage::DelegatedExploration);
        assert_eq!(activities[0].artifact_refs, vec!["artifact:summary"]);
    }
}
