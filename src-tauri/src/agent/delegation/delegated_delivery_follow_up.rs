//! Durable routing for Main-Agent summaries of reviewed Explorer deliveries.
//!
//! A producer delivery is never injected directly into the foreground model.
//! Once an independent reviewer passes a read-only Explorer result, this
//! module atomically binds its immutable delivery revision to one generic
//! Workspace follow-up input. The actual evidence remains in the delivery
//! inbox, where the foreground context assembler can apply its normal bounds.

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use crate::agent::{
    shared_db::{SharedDb, SharedDbError},
    supervision::{SupervisorError, SupervisorInputKind, WorkspaceSupervisor},
};

const DELIVERY_SUMMARY_PROMPT_PREFIX: &str = "系统事件（非用户指令）：已审校的只读委派结果 ";
const DELIVERY_SUMMARY_PROMPT_SUFFIX: &str = " 已就绪。\n请仅依据 <delegated_deliveries> 中该 ID 的结构化数据，调用 review_delegated_delivery 处理该结果，并向用户简洁说明结论、证据及不确定性。不要调用其他工具。";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegatedDeliveryFollowUpStatus {
    Queued,
    Running,
    Summarized,
    Cancelled,
}

impl DelegatedDeliveryFollowUpStatus {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "summarized" => Some(Self::Summarized),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// Durable source binding for one foreground-only Main-Agent follow-up.
///
/// `workspace_id` and `session_id` are read through the stored supervisor
/// input and delegation, rather than duplicated in the mapping table. This
/// makes the durable source records the only authority for routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatedDeliveryFollowUp {
    pub delivery_id: String,
    pub delivery_revision: i64,
    pub supervisor_input_id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub foreground_run_id: Option<String>,
    pub status: DelegatedDeliveryFollowUpStatus,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegatedDeliveryFollowUpEnqueue {
    Enqueued(DelegatedDeliveryFollowUp),
    /// A duplicate review event found the original immutable source binding.
    /// The binding may already be running or terminal; it is still the only
    /// follow-up ever created for this delivery revision.
    AlreadyEnqueued(DelegatedDeliveryFollowUp),
    /// The source is missing or has ceased to be a passed read-only Explorer
    /// delivery that awaits Main-Agent summary.
    NotEligible,
}

impl DelegatedDeliveryFollowUpEnqueue {
    pub fn newly_enqueued(&self) -> bool {
        matches!(self, Self::Enqueued(_))
    }

    pub fn binding(&self) -> Option<&DelegatedDeliveryFollowUp> {
        match self {
            Self::Enqueued(binding) | Self::AlreadyEnqueued(binding) => Some(binding),
            Self::NotEligible => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DelegatedDeliveryFollowUpStart {
    Started(DelegatedDeliveryFollowUp),
    /// The claimed supervisor input no longer belongs to a runnable passed
    /// Explorer result. The command layer should settle/cancel that claim;
    /// it must not start a foreground model turn.
    NotRunnable,
}

#[derive(Debug, thiserror::Error)]
pub enum DelegatedDeliveryFollowUpError {
    #[error("invalid delegated delivery follow-up input")]
    InvalidInput,
    #[error(transparent)]
    Supervisor(#[from] SupervisorError),
    #[error("delegated delivery follow-up storage error: {0}")]
    Storage(String),
}

impl From<SharedDbError> for DelegatedDeliveryFollowUpError {
    fn from(error: SharedDbError) -> Self {
        Self::Storage(error.to_string())
    }
}

/// Deep persistence seam for the reviewed-Explorer → Main-Agent handoff.
///
/// The public entry point intentionally accepts only a durable delivery id.
/// Callers cannot provide a workspace, session, prompt body, review verdict,
/// or worker profile that would widen the follow-up's scope.
#[derive(Clone)]
pub struct DelegatedDeliveryFollowUpQueue {
    db: SharedDb,
}

impl DelegatedDeliveryFollowUpQueue {
    pub fn new(db: SharedDb) -> Self {
        Self { db }
    }

    /// Atomically enqueue the one foreground follow-up allowed for a passed
    /// read-only Explorer delivery revision.
    pub fn enqueue_reviewed_explorer_delivery(
        &self,
        delivery_id: &str,
        now: i64,
    ) -> Result<DelegatedDeliveryFollowUpEnqueue, DelegatedDeliveryFollowUpError> {
        if !valid_identifier(delivery_id) {
            return Err(DelegatedDeliveryFollowUpError::InvalidInput);
        }
        self.db.with_conn_mut(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let outcome = Self::enqueue_in_tx(&transaction, delivery_id, now)?;
            transaction.commit().map_err(storage)?;
            Ok(outcome)
        })?
    }

    /// Load a source binding from the generic supervisor input claimed by the
    /// foreground pump. The caller receives durable identities, not payload.
    pub fn load_by_supervisor_input_id(
        &self,
        supervisor_input_id: &str,
    ) -> Result<Option<DelegatedDeliveryFollowUp>, DelegatedDeliveryFollowUpError> {
        if !valid_identifier(supervisor_input_id) {
            return Err(DelegatedDeliveryFollowUpError::InvalidInput);
        }
        self.db.with_conn(|connection| {
            Self::load_by_supervisor_input_id_from_connection(connection, supervisor_input_id)
        })?
    }

    pub fn load_by_delivery(
        &self,
        delivery_id: &str,
        delivery_revision: i64,
    ) -> Result<Option<DelegatedDeliveryFollowUp>, DelegatedDeliveryFollowUpError> {
        if !valid_identifier(delivery_id) || delivery_revision <= 0 {
            return Err(DelegatedDeliveryFollowUpError::InvalidInput);
        }
        self.db.with_conn(|connection| {
            Self::load_by_delivery_from_connection(connection, delivery_id, delivery_revision)
        })?
    }

    /// Read a binding while a foreground admission transaction is already
    /// open. This avoids a second connection lock and lets the command layer
    /// build its reservation from the same durable identity it later claims.
    pub(crate) fn load_by_supervisor_input_id_in_tx(
        &self,
        transaction: &Transaction<'_>,
        supervisor_input_id: &str,
    ) -> Result<Option<DelegatedDeliveryFollowUp>, DelegatedDeliveryFollowUpError> {
        if !valid_identifier(supervisor_input_id) {
            return Err(DelegatedDeliveryFollowUpError::InvalidInput);
        }
        Self::load_by_supervisor_input_id_from_connection(transaction, supervisor_input_id)
    }

    /// Claim the source binding in the same transaction that reserves a
    /// foreground run. The claim token proves the generic queue item is still
    /// owned by this pump, while the correlated source checks prevent a stale
    /// delivery from starting a new Main-Agent turn.
    pub(crate) fn begin_claimed_in_tx(
        transaction: &Transaction<'_>,
        supervisor_input_id: &str,
        claim_token: &str,
        foreground_run_id: &str,
        now: i64,
    ) -> Result<DelegatedDeliveryFollowUpStart, DelegatedDeliveryFollowUpError> {
        if !valid_identifier(supervisor_input_id)
            || !valid_identifier(claim_token)
            || !valid_identifier(foreground_run_id)
        {
            return Err(DelegatedDeliveryFollowUpError::InvalidInput);
        }

        let changed = transaction
            .execute(
                "UPDATE delegated_delivery_follow_ups
                 SET status = 'running', foreground_run_id = ?1, updated_at = ?2
                 WHERE supervisor_input_id = ?3
                   AND status = 'queued'
                   AND foreground_run_id IS NULL
                   AND EXISTS (
                       SELECT 1
                       FROM workspace_supervisor_inputs input
                       JOIN delegation_deliveries delivery
                         ON delivery.id = delegated_delivery_follow_ups.delivery_id
                       JOIN delegations delegation
                         ON delegation.id = delivery.delegation_id
                       JOIN work_packages package
                         ON package.id = delegation.work_package_id
                       JOIN delegation_review_jobs review
                         ON review.delivery_id = delivery.id
                        AND review.delivery_revision = delivery.delivery_version
                        AND review.implementation_attempt_id = delivery.attempt_id
                       JOIN projects workspace
                         ON workspace.id = input.workspace_id
                        AND workspace.active_session_id = delegation.session_id
                       WHERE input.id = delegated_delivery_follow_ups.supervisor_input_id
                         AND input.status = 'claimed'
                         AND input.claim_token = ?4
                         AND delivery.delivery_version = delegated_delivery_follow_ups.delivery_revision
                         AND delivery.acceptance_status = 'submitted'
                         AND delegation.status = 'awaiting_summary'
                         AND package.task_shape = 'explore'
                         AND package.worker_profile = 'explorer'
                         AND review.status = 'passed'
                   )",
                params![foreground_run_id, now, supervisor_input_id, claim_token],
            )
            .map_err(storage)?;
        if changed == 0 {
            return Ok(DelegatedDeliveryFollowUpStart::NotRunnable);
        }

        let binding =
            Self::load_by_supervisor_input_id_from_connection(transaction, supervisor_input_id)?
                .ok_or_else(|| {
                    DelegatedDeliveryFollowUpError::Storage(
                        "started delegated delivery follow-up disappeared".into(),
                    )
                })?;
        Ok(DelegatedDeliveryFollowUpStart::Started(binding))
    }

    /// Record that the one constrained foreground turn has settled. The
    /// foreground run id is part of the CAS condition, so an older recovery
    /// path cannot settle a newer claim.
    pub(crate) fn mark_summarized_in_tx(
        transaction: &Transaction<'_>,
        supervisor_input_id: &str,
        foreground_run_id: &str,
        now: i64,
    ) -> Result<bool, DelegatedDeliveryFollowUpError> {
        Self::update_running_status_in_tx(
            transaction,
            supervisor_input_id,
            foreground_run_id,
            "summarized",
            false,
            now,
        )
    }

    /// Cancel a started binding once its source no longer admits an automatic
    /// summary (for example, another owner has already terminalized it).  As
    /// with summary completion, the exact foreground run id is the compare-
    /// and-swap guard against an older recovery path settling newer work.
    pub(crate) fn cancel_running_in_tx(
        transaction: &Transaction<'_>,
        supervisor_input_id: &str,
        foreground_run_id: &str,
        now: i64,
    ) -> Result<bool, DelegatedDeliveryFollowUpError> {
        Self::update_running_status_in_tx(
            transaction,
            supervisor_input_id,
            foreground_run_id,
            "cancelled",
            false,
            now,
        )
    }

    /// Release a reserved but unstarted follow-up so the generic queue can be
    /// retried after a user foreground turn has finished. The run id is
    /// cleared only while this exact binding remains running.
    pub(crate) fn release_running_in_tx(
        transaction: &Transaction<'_>,
        supervisor_input_id: &str,
        foreground_run_id: &str,
        now: i64,
    ) -> Result<bool, DelegatedDeliveryFollowUpError> {
        Self::update_running_status_in_tx(
            transaction,
            supervisor_input_id,
            foreground_run_id,
            "queued",
            true,
            now,
        )
    }

    /// Cancel a queued binding that failed a pre-run source revalidation.
    /// A running binding needs an exact foreground-run CAS and therefore uses
    /// `mark_summarized_in_tx` or `release_running_in_tx` instead.
    pub(crate) fn cancel_queued_in_tx(
        transaction: &Transaction<'_>,
        supervisor_input_id: &str,
        now: i64,
    ) -> Result<bool, DelegatedDeliveryFollowUpError> {
        if !valid_identifier(supervisor_input_id) {
            return Err(DelegatedDeliveryFollowUpError::InvalidInput);
        }
        let changed = transaction
            .execute(
                "UPDATE delegated_delivery_follow_ups
                 SET status = 'cancelled', updated_at = ?1
                 WHERE supervisor_input_id = ?2
                   AND status = 'queued'
                   AND foreground_run_id IS NULL",
                params![now, supervisor_input_id],
            )
            .map_err(storage)?;
        Ok(changed == 1)
    }

    fn enqueue_in_tx(
        transaction: &Transaction<'_>,
        delivery_id: &str,
        now: i64,
    ) -> Result<DelegatedDeliveryFollowUpEnqueue, DelegatedDeliveryFollowUpError> {
        let existing = transaction
            .query_row(
                "SELECT delivery_revision
                 FROM delegated_delivery_follow_ups
                 WHERE delivery_id = ?1",
                [delivery_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage)?;
        if let Some(delivery_revision) = existing {
            let binding = Self::load_by_delivery_from_connection(
                transaction,
                delivery_id,
                delivery_revision,
            )?
            .ok_or_else(|| {
                DelegatedDeliveryFollowUpError::Storage(
                    "existing delegated delivery follow-up is incomplete".into(),
                )
            })?;
            return Ok(DelegatedDeliveryFollowUpEnqueue::AlreadyEnqueued(binding));
        }

        let eligible = transaction
            .query_row(
                "SELECT delivery.delivery_version, workspace.id, delegation.session_id
                 FROM delegation_deliveries delivery
                 JOIN delegations delegation
                   ON delegation.id = delivery.delegation_id
                 JOIN work_packages package
                   ON package.id = delegation.work_package_id
                 JOIN delegation_review_jobs review
                   ON review.delivery_id = delivery.id
                  AND review.delivery_revision = delivery.delivery_version
                  AND review.implementation_attempt_id = delivery.attempt_id
                 JOIN projects workspace
                   ON workspace.active_session_id = delegation.session_id
                 WHERE delivery.id = ?1
                   AND delivery.acceptance_status = 'submitted'
                   AND delegation.status = 'awaiting_summary'
                   AND package.task_shape = 'explore'
                   AND package.worker_profile = 'explorer'
                   AND review.status = 'passed'
                 ORDER BY workspace.updated_at DESC, workspace.id ASC
                 LIMIT 1",
                [delivery_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(storage)?;
        let Some((delivery_revision, workspace_id, session_id)) = eligible else {
            return Ok(DelegatedDeliveryFollowUpEnqueue::NotEligible);
        };

        // The text is trusted fixed protocol. It names only the immutable
        // source identifier; the child payload remains in the inbox.
        let input = WorkspaceSupervisor::submit_in_tx(
            transaction,
            &workspace_id,
            SupervisorInputKind::FollowUp,
            &delivery_summary_prompt(delivery_id),
            now,
        )?;
        transaction
            .execute(
                "INSERT INTO delegated_delivery_follow_ups
                    (delivery_id, delivery_revision, supervisor_input_id, foreground_run_id,
                     status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, NULL, 'queued', ?4, ?4)",
                params![delivery_id, delivery_revision, input.id, now],
            )
            .map_err(storage)?;

        let binding = DelegatedDeliveryFollowUp {
            delivery_id: delivery_id.into(),
            delivery_revision,
            supervisor_input_id: input.id,
            workspace_id,
            session_id,
            foreground_run_id: None,
            status: DelegatedDeliveryFollowUpStatus::Queued,
            created_at: now,
            updated_at: now,
        };
        Ok(DelegatedDeliveryFollowUpEnqueue::Enqueued(binding))
    }

    fn update_running_status_in_tx(
        transaction: &Transaction<'_>,
        supervisor_input_id: &str,
        foreground_run_id: &str,
        status: &str,
        clear_foreground_run_id: bool,
        now: i64,
    ) -> Result<bool, DelegatedDeliveryFollowUpError> {
        if !valid_identifier(supervisor_input_id) || !valid_identifier(foreground_run_id) {
            return Err(DelegatedDeliveryFollowUpError::InvalidInput);
        }
        let changed = transaction
            .execute(
                "UPDATE delegated_delivery_follow_ups
                 SET status = ?1,
                     foreground_run_id = CASE WHEN ?2 THEN NULL ELSE foreground_run_id END,
                     updated_at = ?3
                 WHERE supervisor_input_id = ?4
                   AND status = 'running'
                   AND foreground_run_id = ?5",
                params![
                    status,
                    clear_foreground_run_id,
                    now,
                    supervisor_input_id,
                    foreground_run_id
                ],
            )
            .map_err(storage)?;
        Ok(changed == 1)
    }

    fn load_by_supervisor_input_id_from_connection(
        connection: &Connection,
        supervisor_input_id: &str,
    ) -> Result<Option<DelegatedDeliveryFollowUp>, DelegatedDeliveryFollowUpError> {
        Self::load_from_connection(
            connection,
            "follow_up.supervisor_input_id = ?1",
            [supervisor_input_id],
        )
    }

    fn load_by_delivery_from_connection(
        connection: &Connection,
        delivery_id: &str,
        delivery_revision: i64,
    ) -> Result<Option<DelegatedDeliveryFollowUp>, DelegatedDeliveryFollowUpError> {
        Self::load_from_connection(
            connection,
            "follow_up.delivery_id = ?1 AND follow_up.delivery_revision = ?2",
            params![delivery_id, delivery_revision],
        )
    }

    fn load_from_connection<P>(
        connection: &Connection,
        predicate: &str,
        parameters: P,
    ) -> Result<Option<DelegatedDeliveryFollowUp>, DelegatedDeliveryFollowUpError>
    where
        P: rusqlite::Params,
    {
        let query = format!(
            "SELECT follow_up.delivery_id, follow_up.delivery_revision,
                    follow_up.supervisor_input_id, input.workspace_id,
                    delegation.session_id, follow_up.foreground_run_id,
                    follow_up.status, follow_up.created_at, follow_up.updated_at
             FROM delegated_delivery_follow_ups follow_up
             JOIN workspace_supervisor_inputs input
               ON input.id = follow_up.supervisor_input_id
             JOIN delegation_deliveries delivery
               ON delivery.id = follow_up.delivery_id
              AND delivery.delivery_version = follow_up.delivery_revision
             JOIN delegations delegation
               ON delegation.id = delivery.delegation_id
             WHERE {predicate}"
        );
        let row = connection
            .query_row(&query, parameters, |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            })
            .optional()
            .map_err(storage)?;
        row.map(
            |(
                delivery_id,
                delivery_revision,
                supervisor_input_id,
                workspace_id,
                session_id,
                foreground_run_id,
                status,
                created_at,
                updated_at,
            )| {
                Ok(DelegatedDeliveryFollowUp {
                    delivery_id,
                    delivery_revision,
                    supervisor_input_id,
                    workspace_id,
                    session_id,
                    foreground_run_id,
                    status: DelegatedDeliveryFollowUpStatus::from_str(&status).ok_or_else(
                        || {
                            DelegatedDeliveryFollowUpError::Storage(
                                "unknown delegated delivery follow-up status".into(),
                            )
                        },
                    )?,
                    created_at,
                    updated_at,
                })
            },
        )
        .transpose()
    }
}

fn delivery_summary_prompt(delivery_id: &str) -> String {
    format!("{DELIVERY_SUMMARY_PROMPT_PREFIX}{delivery_id}{DELIVERY_SUMMARY_PROMPT_SUFFIX}")
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':' | b'.' | b'/')
        })
}

fn storage(error: rusqlite::Error) -> DelegatedDeliveryFollowUpError {
    DelegatedDeliveryFollowUpError::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection, TransactionBehavior};

    use super::*;
    use crate::agent::{
        shared_db::SharedDb,
        supervision::{SupervisorInputStatus, WorkspaceSupervisor},
    };

    fn fixture(
        task_shape: &str,
        worker_profile: &str,
        review_status: &str,
        delivery_status: &str,
        delegation_status: &str,
        with_workspace: bool,
    ) -> SharedDb {
        let connection = Connection::open_in_memory().unwrap();
        crate::db::migrate(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO sessions(id,title,created_at,updated_at)
                 VALUES ('session','session',1,1)",
                [],
            )
            .unwrap();
        if with_workspace {
            connection
                .execute(
                    "INSERT INTO projects(id,name,path,kind,active_session_id,created_at,updated_at)
                     VALUES ('workspace','workspace','C:/workspace','project','session',1,1)",
                    [],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO messages(id,session_id,role,content,created_at)
                 VALUES ('message','session','user','inspect',1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at)
                 VALUES ('run','session','message','inspect','running','[]',1,1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO work_packages(
                    id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,
                    worker_policy_version,scope_digest,capability_scope_ref,
                    capability_expires_at,candidate_version,status,created_at,updated_at
                 ) VALUES (
                    'package','session',1,'workspace',?1,?2,1,'scope','approved',100,
                    0,'active',1,1
                 )",
                params![task_shape, worker_profile],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO delegations(
                    id,session_id,message_id,parent_run_id,work_package_id,objective,
                    brief_json,status,state_version,created_at,updated_at
                 ) VALUES (
                    'delegation','session','message','run','package','inspect','{}',?1,0,1,1
                 )",
                [delegation_status],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO delegation_attempts(
                    id,delegation_id,attempt_number,status,sandbox_ref,created_at,ended_at
                 ) VALUES ('attempt','delegation',1,'sealed','sandbox://attempt',1,2)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO delegation_deliveries(
                    id,delegation_id,attempt_id,delivery_version,schema_version,payload_json,
                    acceptance_status,accepted_by,accepted_at,created_at
                 ) VALUES (
                    'delivery','delegation','attempt',1,1,
                    '{\"untrusted\":\"RAW_CHILD_LOG_MUST_NOT_BE_QUEUED\"}',?1,
                    CASE WHEN ?1 = 'accepted' THEN 'parent_agent' ELSE NULL END,
                    CASE WHEN ?1 = 'accepted' THEN 2 ELSE NULL END,
                    2
                 )",
                [delivery_status],
            )
            .unwrap();
        let terminal = matches!(review_status, "passed" | "failed" | "needs_decision");
        connection
            .execute(
                "INSERT INTO delegation_review_jobs(
                    id,implementation_attempt_id,delivery_id,delivery_revision,subject_json,
                    status,reviewer_attempt_id,outcome_json,created_at,updated_at,started_at,ended_at
                 ) VALUES (
                    'review','attempt','delivery',1,'{}',?1,
                    CASE WHEN ?2 THEN 'reviewer' ELSE NULL END,
                    CASE WHEN ?2 THEN '{}' ELSE NULL END,
                    2,2,2,CASE WHEN ?2 THEN 2 ELSE NULL END
                 )",
                params![review_status, terminal],
            )
            .unwrap();
        SharedDb::new(connection)
    }

    #[test]
    fn passed_explorer_delivery_enqueues_exactly_one_redacted_follow_up() {
        let db = fixture(
            "explore",
            "explorer",
            "passed",
            "submitted",
            "awaiting_summary",
            true,
        );
        let queue = DelegatedDeliveryFollowUpQueue::new(db.clone());

        let first = queue
            .enqueue_reviewed_explorer_delivery("delivery", 10)
            .unwrap();
        let binding = first.binding().unwrap().clone();
        assert!(first.newly_enqueued());
        assert_eq!(binding.workspace_id, "workspace");
        assert_eq!(binding.session_id, "session");
        assert_eq!(binding.status, DelegatedDeliveryFollowUpStatus::Queued);
        assert_eq!(binding.foreground_run_id, None);

        let prompt = db
            .with_conn(|connection| {
                connection
                    .query_row(
                        "SELECT content FROM workspace_supervisor_inputs WHERE id = ?1",
                        [&binding.supervisor_input_id],
                        |row| row.get::<_, String>(0),
                    )
                    .unwrap()
            })
            .unwrap();
        assert!(prompt.contains("delivery"));
        assert!(!prompt.contains("RAW_CHILD_LOG_MUST_NOT_BE_QUEUED"));

        let duplicate = queue
            .enqueue_reviewed_explorer_delivery("delivery", 11)
            .unwrap();
        assert!(!duplicate.newly_enqueued());
        assert_eq!(duplicate.binding().unwrap(), &binding);
        let counts = db
            .with_conn(|connection| {
                let bindings: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM delegated_delivery_follow_ups",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                let inputs: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM workspace_supervisor_inputs",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                (bindings, inputs)
            })
            .unwrap();
        assert_eq!(counts, (1, 1));
        assert_eq!(
            queue
                .load_by_supervisor_input_id(&binding.supervisor_input_id)
                .unwrap(),
            Some(binding)
        );
    }

    #[test]
    fn only_passed_read_only_explorer_results_with_an_active_workspace_are_eligible() {
        for (
            task_shape,
            worker_profile,
            review_status,
            delivery_status,
            delegation_status,
            workspace,
        ) in [
            (
                "change",
                "implementer",
                "passed",
                "submitted",
                "awaiting_summary",
                true,
            ),
            (
                "explore",
                "explorer",
                "failed",
                "submitted",
                "awaiting_summary",
                true,
            ),
            (
                "explore",
                "explorer",
                "passed",
                "accepted",
                "awaiting_summary",
                true,
            ),
            (
                "explore",
                "explorer",
                "passed",
                "submitted",
                "running",
                true,
            ),
            (
                "explore",
                "explorer",
                "passed",
                "submitted",
                "awaiting_summary",
                false,
            ),
        ] {
            let db = fixture(
                task_shape,
                worker_profile,
                review_status,
                delivery_status,
                delegation_status,
                workspace,
            );
            let result = DelegatedDeliveryFollowUpQueue::new(db)
                .enqueue_reviewed_explorer_delivery("delivery", 10)
                .unwrap();
            assert_eq!(result, DelegatedDeliveryFollowUpEnqueue::NotEligible);
        }
    }

    #[test]
    fn foreground_transition_rechecks_scope_and_uses_foreground_run_cas() {
        let db = fixture(
            "explore",
            "explorer",
            "passed",
            "submitted",
            "awaiting_summary",
            true,
        );
        let queue = DelegatedDeliveryFollowUpQueue::new(db.clone());
        let binding = queue
            .enqueue_reviewed_explorer_delivery("delivery", 10)
            .unwrap()
            .binding()
            .unwrap()
            .clone();
        let supervisor = WorkspaceSupervisor::new(db.clone());
        let claim = supervisor
            .claim_next_follow_up("workspace", 11)
            .unwrap()
            .unwrap();
        assert_eq!(claim.claim.input.id, binding.supervisor_input_id);

        let start = db
            .with_conn_mut(|connection| {
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .unwrap();
                let start = DelegatedDeliveryFollowUpQueue::begin_claimed_in_tx(
                    &transaction,
                    &claim.claim.input.id,
                    &claim.claim.claim_token,
                    "foreground-run",
                    12,
                )
                .unwrap();
                transaction.commit().unwrap();
                start
            })
            .unwrap();
        let DelegatedDeliveryFollowUpStart::Started(running) = start else {
            panic!("eligible claimed delivery should start")
        };
        assert_eq!(running.status, DelegatedDeliveryFollowUpStatus::Running);
        assert_eq!(running.foreground_run_id.as_deref(), Some("foreground-run"));

        db.with_conn_mut(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert!(!DelegatedDeliveryFollowUpQueue::mark_summarized_in_tx(
                &transaction,
                &claim.claim.input.id,
                "stale-run",
                13,
            )
            .unwrap());
            assert!(DelegatedDeliveryFollowUpQueue::mark_summarized_in_tx(
                &transaction,
                &claim.claim.input.id,
                "foreground-run",
                14,
            )
            .unwrap());
            transaction.commit().unwrap();
        })
        .unwrap();
        let summarized = queue
            .load_by_supervisor_input_id(&claim.claim.input.id)
            .unwrap()
            .unwrap();
        assert_eq!(
            summarized.status,
            DelegatedDeliveryFollowUpStatus::Summarized
        );
        assert_eq!(
            summarized.foreground_run_id.as_deref(),
            Some("foreground-run")
        );
        assert_eq!(claim.claim.input.status, SupervisorInputStatus::Claimed);
    }

    #[test]
    fn stale_source_cannot_start_a_claimed_foreground_follow_up() {
        let db = fixture(
            "explore",
            "explorer",
            "passed",
            "submitted",
            "awaiting_summary",
            true,
        );
        let queue = DelegatedDeliveryFollowUpQueue::new(db.clone());
        let binding = queue
            .enqueue_reviewed_explorer_delivery("delivery", 10)
            .unwrap()
            .binding()
            .unwrap()
            .clone();
        let supervisor = WorkspaceSupervisor::new(db.clone());
        let claim = supervisor
            .claim_next_follow_up("workspace", 11)
            .unwrap()
            .unwrap();
        db.with_conn_mut(|connection| {
            connection
                .execute(
                    "UPDATE work_packages SET task_shape = 'change', worker_profile = 'implementer' WHERE id = 'package'",
                    [],
                )
                .unwrap();
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert_eq!(
                DelegatedDeliveryFollowUpQueue::begin_claimed_in_tx(
                    &transaction,
                    &claim.claim.input.id,
                    &claim.claim.claim_token,
                    "foreground-run",
                    12,
                )
                .unwrap(),
                DelegatedDeliveryFollowUpStart::NotRunnable
            );
            transaction.commit().unwrap();
        })
        .unwrap();
        assert_eq!(
            queue
                .load_by_supervisor_input_id(&binding.supervisor_input_id)
                .unwrap()
                .unwrap()
                .status,
            DelegatedDeliveryFollowUpStatus::Queued
        );
    }

    #[test]
    fn deleting_the_source_delivery_cleans_its_generic_follow_up_input() {
        let db = fixture(
            "explore",
            "explorer",
            "passed",
            "submitted",
            "awaiting_summary",
            true,
        );
        let queue = DelegatedDeliveryFollowUpQueue::new(db.clone());
        let binding = queue
            .enqueue_reviewed_explorer_delivery("delivery", 10)
            .unwrap()
            .binding()
            .unwrap()
            .clone();

        db.with_conn_mut(|connection| {
            connection
                .execute(
                    "DELETE FROM delegation_deliveries WHERE id = 'delivery'",
                    [],
                )
                .unwrap();
        })
        .unwrap();
        let counts = db
            .with_conn(|connection| {
                let mappings: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM delegated_delivery_follow_ups",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                let inputs: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM workspace_supervisor_inputs WHERE id = ?1",
                        [&binding.supervisor_input_id],
                        |row| row.get(0),
                    )
                    .unwrap();
                (mappings, inputs)
            })
            .unwrap();
        assert_eq!(counts, (0, 0));
    }
}
