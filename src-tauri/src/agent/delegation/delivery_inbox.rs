//! Main-Agent-owned inbox and continuation boundary for delegated work.
//!
//! Child workers can submit one bounded, structured [`DelegationDelivery`],
//! but they cannot complete a delegation or inject raw execution logs into
//! the foreground conversation.  This module reads the durable delivery rows,
//! validates the trusted parent scope, and exposes only [`ParentContext`].
//! Accept, needs-decision, and retry are explicit Main-Agent decisions and
//! retain optimistic/CAS semantics in the existing [`DelegationRuntime`].

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{
    delegation::{DelegationRepository, DelegationStatus},
    delegation_contract::{
        ContractLimits, DelegationDelivery, ParentContext, ParentContextAssembler,
    },
    delegation_runtime::{
        AdmissionPolicy, Clock, DelegationRuntime, DelegationRuntimeError, SandboxController,
        WorkerAdapter,
    },
};

/// Trusted identity of the foreground Main Agent.  All delivery queries are
/// constrained by this tuple; a worker-supplied id can never widen it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryScope {
    pub session_id: String,
    pub message_id: String,
    pub parent_run_id: String,
}

impl DeliveryScope {
    fn matches_identity(&self, delivery: &DelegationDelivery) -> bool {
        delivery.identity.session_id == self.session_id
            && delivery.identity.message_id == self.message_id
            && delivery.identity.parent_run_id == self.parent_run_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxAcceptanceStatus {
    Submitted,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxDelivery {
    pub delivery: DelegationDelivery,
    /// Structured projection only.  Raw payload/log fields never cross this
    /// seam into Main-Agent context.
    pub context: ParentContext,
    pub acceptance_status: InboxAcceptanceStatus,
    pub state_version: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitOutcome {
    Submitted,
    AlreadySubmitted,
    AlreadyAccepted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionOutcome {
    Accepted(ParentContext),
    AlreadyAccepted(ParentContext),
    NeedsDecision(ParentContext),
    AlreadyNeedsDecision(ParentContext),
}

/// Decision surface used by the foreground Main-Agent control tool. Child
/// workers never receive this value and the user does not manipulate it
/// directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentDecision {
    Accept,
    NeedsDecision,
}

/// A retry is an instruction for the Main Agent's continuation/issuance path,
/// not a replay command.  In particular, successful child side effects are
/// never re-run implicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryContinuation {
    pub delegation_id: String,
    pub attempt_id: String,
    pub source_delivery_id: String,
    pub source_delivery_revision: u32,
    pub reason: String,
    pub context: ParentContext,
    pub replay_successful_side_effects: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxError {
    Storage(String),
    Runtime(String),
    Contract(String),
    ScopeMismatch,
    AttemptMismatch,
    DeliveryNotFound,
    DeliveryNotPending,
    DecisionConflict,
    InvalidRetryReason,
}

impl std::fmt::Display for InboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(value) | Self::Runtime(value) | Self::Contract(value) => {
                f.write_str(value)
            }
            Self::ScopeMismatch => f.write_str("delivery is outside the foreground scope"),
            Self::AttemptMismatch => f.write_str("delivery attempt does not match its delegation"),
            Self::DeliveryNotFound => f.write_str("delegation delivery was not found"),
            Self::DeliveryNotPending => f.write_str("delegation delivery is not awaiting summary"),
            Self::DecisionConflict => f.write_str("delegation decision changed concurrently"),
            Self::InvalidRetryReason => f.write_str("retry reason is invalid"),
        }
    }
}

impl std::error::Error for InboxError {}

/// Stateless deep module.  The runtime remains the owner of delivery
/// submission/acceptance and lifecycle transitions; this module only adds the
/// parent scope, projection, idempotency, and continuation seam.
pub struct DeliveryInbox;

impl DeliveryInbox {
    /// Reload the accepted delivery payload from durable storage.  Callers
    /// must provide the trusted foreground identity; a caller-supplied
    /// delivery body is never treated as the source of truth.
    pub(crate) fn accepted_delivery_from_connection(
        conn: &Connection,
        scope: &DeliveryScope,
        delivery_id: &str,
        _limits: &ContractLimits,
    ) -> Result<DelegationDelivery, InboxError> {
        let loaded = load_delivery_any(conn, scope, delivery_id)?;
        if loaded.acceptance_status != InboxAcceptanceStatus::Accepted {
            return Err(InboxError::DeliveryNotPending);
        }
        Ok(loaded.delivery)
    }

    pub fn submit<W, C, P, S>(
        runtime: &mut DelegationRuntime<W, C, P, S>,
        delivery: &DelegationDelivery,
        limits: &ContractLimits,
    ) -> Result<SubmitOutcome, InboxError>
    where
        W: WorkerAdapter,
        C: Clock,
        P: AdmissionPolicy,
        S: SandboxController,
    {
        let payload = serde_json::to_string(delivery)
            .map_err(|error| InboxError::Contract(error.to_string()))?;
        if let Some(existing) = {
            let conn = runtime.connection();
            existing_delivery(&conn, delivery)?
        } {
            return classify_duplicate(existing, delivery, &payload);
        }

        match runtime.submit_delivery(delivery, limits) {
            Ok(()) => Ok(SubmitOutcome::Submitted),
            Err(error) => {
                // A concurrent submit may win the unique delivery key between
                // the read above and runtime insertion.  Re-read and classify
                // it instead of leaking a SQLite uniqueness error.
                if let Some(existing) = {
                    let conn = runtime.connection();
                    existing_delivery(&conn, delivery)?
                } {
                    classify_duplicate(existing, delivery, &payload)
                } else {
                    Err(map_runtime_error(error))
                }
            }
        }
    }

    pub fn pending<W, C, P, S>(
        runtime: &DelegationRuntime<W, C, P, S>,
        scope: &DeliveryScope,
        limits: &ContractLimits,
    ) -> Result<Vec<InboxDelivery>, InboxError>
    where
        W: WorkerAdapter,
        C: Clock,
        P: AdmissionPolicy,
        S: SandboxController,
    {
        Self::pending_from_connection(&runtime.connection(), scope, limits)
    }

    /// Read pending deliveries for one foreground session without requiring
    /// the caller to know which provisional parent produced them. A session
    /// is the Main-Agent ownership scope; child identities are still checked
    /// against the durable row before any projection crosses this seam.
    pub fn pending_for_session_from_connection(
        conn: &Connection,
        session_id: &str,
        limits: &ContractLimits,
    ) -> Result<Vec<InboxDelivery>, InboxError> {
        let mut statement = conn
            .prepare(
                "SELECT d.payload_json, d.acceptance_status, d.attempt_id, g.state_version
                 FROM delegation_deliveries d
                 JOIN delegations g ON g.id = d.delegation_id
                 WHERE d.acceptance_status = 'submitted'
                   AND g.status = 'awaiting_summary'
                   AND g.session_id = ?1
                 ORDER BY d.created_at ASC, d.id ASC",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map([session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        project_pending_rows(conn, rows, None, limits)
    }

    /// Read pending deliveries for one exact foreground parent scope.
    pub fn pending_from_connection(
        conn: &Connection,
        scope: &DeliveryScope,
        limits: &ContractLimits,
    ) -> Result<Vec<InboxDelivery>, InboxError> {
        let mut statement = conn
            .prepare(
                "SELECT d.payload_json, d.acceptance_status, d.attempt_id, g.state_version
                 FROM delegation_deliveries d
                 JOIN delegations g ON g.id = d.delegation_id
                 WHERE d.acceptance_status = 'submitted'
                   AND g.status = 'awaiting_summary'
                   AND g.session_id = ?1
                   AND g.message_id = ?2
                   AND g.parent_run_id = ?3
                 ORDER BY d.created_at ASC, d.id ASC",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map(
                params![&scope.session_id, &scope.message_id, &scope.parent_run_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        project_pending_rows(conn, rows, Some(scope), limits)
    }

    pub fn accept<W, C, P, S>(
        runtime: &mut DelegationRuntime<W, C, P, S>,
        scope: &DeliveryScope,
        delivery_id: &str,
        limits: &ContractLimits,
    ) -> Result<DecisionOutcome, InboxError>
    where
        W: WorkerAdapter,
        C: Clock,
        P: AdmissionPolicy,
        S: SandboxController,
    {
        let row = {
            let conn = runtime.connection();
            load_delivery(&conn, scope, delivery_id, limits)?
        };
        let context = row.context.clone();
        match row.acceptance_status {
            InboxAcceptanceStatus::Accepted => {
                return Ok(DecisionOutcome::AlreadyAccepted(context))
            }
            InboxAcceptanceStatus::Submitted => {}
            InboxAcceptanceStatus::Rejected => return Err(InboxError::DeliveryNotPending),
        }
        if let Err(error) = runtime.accept_delivery(&row.delivery, limits) {
            // Another Main-Agent continuation may have won the CAS between
            // the read and acceptance.  Re-read the durable status so a
            // repeated accept remains a safe idempotent operation.
            let accepted = {
                let conn = runtime.connection();
                load_delivery_any(&conn, scope, delivery_id)
                    .ok()
                    .is_some_and(|current| {
                        current.acceptance_status == InboxAcceptanceStatus::Accepted
                    })
            };
            if accepted {
                return Ok(DecisionOutcome::AlreadyAccepted(context));
            }
            return Err(map_runtime_error(error));
        }
        Ok(DecisionOutcome::Accepted(context))
    }

    pub fn needs_decision<W, C, P, S>(
        runtime: &mut DelegationRuntime<W, C, P, S>,
        scope: &DeliveryScope,
        delivery_id: &str,
        limits: &ContractLimits,
    ) -> Result<DecisionOutcome, InboxError>
    where
        W: WorkerAdapter,
        C: Clock,
        P: AdmissionPolicy,
        S: SandboxController,
    {
        let row = {
            let conn = runtime.connection();
            load_delivery_any(&conn, scope, delivery_id)?
        };
        if row.acceptance_status == InboxAcceptanceStatus::Accepted {
            return Err(InboxError::DeliveryNotPending);
        }
        let already_needs_decision = {
            let conn = runtime.connection();
            delegation_status(&conn, &row.delivery.identity.delegation_id)?
                == DelegationStatus::NeedsDecision
        };
        if already_needs_decision {
            return Ok(DecisionOutcome::AlreadyNeedsDecision(row.context));
        }
        runtime
            .needs_decision(&row.delivery.identity.attempt_id)
            .map_err(map_runtime_error)?;
        // Re-validate the structured projection at the decision boundary.  It
        // remains the only context shape that can be returned to the parent.
        let context = {
            let conn = runtime.connection();
            context_for(&conn, &row.delivery, limits)?
        };
        Ok(DecisionOutcome::NeedsDecision(context))
    }

    pub fn retry<W, C, P, S>(
        runtime: &mut DelegationRuntime<W, C, P, S>,
        scope: &DeliveryScope,
        delivery_id: &str,
        reason: &str,
        limits: &ContractLimits,
    ) -> Result<RetryContinuation, InboxError>
    where
        W: WorkerAdapter,
        C: Clock,
        P: AdmissionPolicy,
        S: SandboxController,
    {
        if reason.trim().is_empty() || reason.len() > 600 || reason.contains(['\0', '\r', '\n']) {
            return Err(InboxError::InvalidRetryReason);
        }
        let row = {
            let conn = runtime.connection();
            load_delivery_any(&conn, scope, delivery_id)?
        };
        if row.acceptance_status == InboxAcceptanceStatus::Accepted {
            return Err(InboxError::DecisionConflict);
        }
        let delegation_id = row.delivery.identity.delegation_id.clone();
        let status = {
            let conn = runtime.connection();
            delegation_status(&conn, &delegation_id)?
        };
        if status != DelegationStatus::NeedsDecision {
            runtime
                .needs_decision(&row.delivery.identity.attempt_id)
                .map_err(map_runtime_error)?;
        }
        let durable_reason = {
            let conn = runtime.connection();
            record_retry_request(&conn, &row.delivery, reason, runtime.now())?
        };
        let context = {
            let conn = runtime.connection();
            context_for(&conn, &row.delivery, limits)?
        };
        Ok(RetryContinuation {
            delegation_id,
            attempt_id: row.delivery.identity.attempt_id.clone(),
            source_delivery_id: row.delivery.delivery_id.clone(),
            source_delivery_revision: row.delivery.delivery_revision,
            // The durable request is the retry authority.  If an earlier
            // issuance failed after recording it, a later retry must resume
            // that same authority rather than manufacture a new reason that
            // the queue transaction would (correctly) reject.
            reason: durable_reason,
            context,
            replay_successful_side_effects: false,
        })
    }
}

#[derive(Debug, Clone)]
struct LoadedDelivery {
    delivery: DelegationDelivery,
    context: ParentContext,
    acceptance_status: InboxAcceptanceStatus,
}

fn project_pending_rows(
    conn: &Connection,
    rows: Vec<(String, String, String, i64)>,
    scope: Option<&DeliveryScope>,
    limits: &ContractLimits,
) -> Result<Vec<InboxDelivery>, InboxError> {
    rows.into_iter()
        .map(|(payload, status, attempt_id, state_version)| {
            let delivery: DelegationDelivery = serde_json::from_str(&payload)
                .map_err(|error| InboxError::Contract(error.to_string()))?;
            if delivery.identity.attempt_id != attempt_id {
                return Err(InboxError::AttemptMismatch);
            }
            if let Some(scope) = scope {
                if !scope.matches_identity(&delivery) {
                    return Err(InboxError::ScopeMismatch);
                }
            }
            let context = context_for(conn, &delivery, limits)?;
            Ok(InboxDelivery {
                delivery,
                context,
                acceptance_status: parse_acceptance_status(&status)?,
                state_version,
            })
        })
        .collect()
}

fn existing_delivery(
    conn: &Connection,
    delivery: &DelegationDelivery,
) -> Result<Option<(String, String, String, String)>, InboxError> {
    conn.query_row(
        "SELECT id, payload_json, acceptance_status, attempt_id
         FROM delegation_deliveries
         WHERE id = ?1 OR (attempt_id = ?2 AND delivery_version = ?3)
         ORDER BY CASE WHEN id = ?1 THEN 0 ELSE 1 END
         LIMIT 1",
        params![
            &delivery.delivery_id,
            &delivery.identity.attempt_id,
            delivery.delivery_revision as i64
        ],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .optional()
    .map_err(storage)
}

fn classify_duplicate(
    existing: (String, String, String, String),
    delivery: &DelegationDelivery,
    payload: &str,
) -> Result<SubmitOutcome, InboxError> {
    let (_, existing_payload, status, existing_attempt) = existing;
    let existing_identity = serde_json::from_str::<DelegationDelivery>(&existing_payload)
        .map_err(|error| InboxError::Contract(error.to_string()))?;
    if existing_attempt != delivery.identity.attempt_id
        || existing_identity.identity != delivery.identity
        || existing_payload != payload
    {
        return Err(InboxError::DecisionConflict);
    }
    match status.as_str() {
        "accepted" => Ok(SubmitOutcome::AlreadyAccepted),
        "submitted" => Ok(SubmitOutcome::AlreadySubmitted),
        _ => Err(InboxError::DecisionConflict),
    }
}

fn load_delivery(
    conn: &Connection,
    scope: &DeliveryScope,
    delivery_id: &str,
    limits: &ContractLimits,
) -> Result<LoadedDelivery, InboxError> {
    let row = conn
        .query_row(
            "SELECT d.payload_json, d.acceptance_status, d.attempt_id
             FROM delegation_deliveries d JOIN delegations g ON g.id=d.delegation_id
             WHERE d.id = ?1 AND g.session_id = ?2 AND g.message_id = ?3 AND g.parent_run_id = ?4",
            params![
                delivery_id,
                &scope.session_id,
                &scope.message_id,
                &scope.parent_run_id
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage)?
        .ok_or(InboxError::DeliveryNotFound)?;
    decode_loaded(conn, scope, row, limits)
}

fn load_delivery_any(
    conn: &Connection,
    scope: &DeliveryScope,
    delivery_id: &str,
) -> Result<LoadedDelivery, InboxError> {
    let row = conn
        .query_row(
            "SELECT d.payload_json, d.acceptance_status, d.attempt_id
             FROM delegation_deliveries d WHERE d.id = ?1",
            [delivery_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage)?
        .ok_or(InboxError::DeliveryNotFound)?;
    decode_loaded(conn, scope, row, &ContractLimits::default())
}

fn decode_loaded(
    conn: &Connection,
    scope: &DeliveryScope,
    row: (String, String, String),
    limits: &ContractLimits,
) -> Result<LoadedDelivery, InboxError> {
    let (payload, status, attempt_id) = row;
    let delivery: DelegationDelivery =
        serde_json::from_str(&payload).map_err(|error| InboxError::Contract(error.to_string()))?;
    if delivery.identity.attempt_id != attempt_id {
        return Err(InboxError::AttemptMismatch);
    }
    if !scope.matches_identity(&delivery) {
        return Err(InboxError::ScopeMismatch);
    }
    let context = context_for(conn, &delivery, limits)?;
    Ok(LoadedDelivery {
        delivery,
        context,
        acceptance_status: parse_acceptance_status(&status)?,
    })
}

fn context_for(
    conn: &Connection,
    delivery: &DelegationDelivery,
    limits: &ContractLimits,
) -> Result<ParentContext, InboxError> {
    let brief_json: String = conn
        .query_row(
            "SELECT brief_json FROM delegations WHERE id = ?1",
            [&delivery.identity.delegation_id],
            |row| row.get(0),
        )
        .map_err(storage)?;
    let brief = serde_json::from_str(&brief_json)
        .map_err(|error| InboxError::Contract(error.to_string()))?;
    ParentContextAssembler::assemble(delivery, &brief, limits)
        .map_err(|error| InboxError::Contract(format!("{error:?}")))
}

fn delegation_status(
    conn: &Connection,
    delegation_id: &str,
) -> Result<DelegationStatus, InboxError> {
    let value: String = conn
        .query_row(
            "SELECT status FROM delegations WHERE id = ?1",
            [delegation_id],
            |row| row.get(0),
        )
        .map_err(storage)?;
    match value.as_str() {
        "queued" => Ok(DelegationStatus::Queued),
        "running" => Ok(DelegationStatus::Running),
        "awaiting_confirmation" => Ok(DelegationStatus::AwaitingConfirmation),
        "awaiting_summary" => Ok(DelegationStatus::AwaitingSummary),
        "completed" => Ok(DelegationStatus::Completed),
        "failed" => Ok(DelegationStatus::Failed),
        "cancelled" => Ok(DelegationStatus::Cancelled),
        "needs_decision" => Ok(DelegationStatus::NeedsDecision),
        _ => Err(InboxError::Storage("unknown delegation status".into())),
    }
}

fn record_retry_request(
    conn: &Connection,
    delivery: &DelegationDelivery,
    reason: &str,
    now: i64,
) -> Result<String, InboxError> {
    let payload = serde_json::json!({
        "delivery_id": delivery.delivery_id,
        "delivery_revision": delivery.delivery_revision,
        "reason": reason,
        "replay_successful_side_effects": false,
    })
    .to_string();
    let existing: Option<String> = conn
        .query_row(
            "SELECT payload_json FROM delegation_attempt_events
             WHERE attempt_id = ?1 AND event_type = 'retry_requested'
             ORDER BY sequence DESC LIMIT 1",
            [&delivery.identity.attempt_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    if let Some(existing) = existing {
        let existing: serde_json::Value = serde_json::from_str(&existing)
            .map_err(|error| InboxError::Storage(error.to_string()))?;
        if existing.get("delivery_id").and_then(|v| v.as_str())
            == Some(delivery.delivery_id.as_str())
            && existing.get("delivery_revision").and_then(|v| v.as_u64())
                == Some(delivery.delivery_revision as u64)
            && existing
                .get("replay_successful_side_effects")
                .and_then(|v| v.as_bool())
                == Some(false)
        {
            let durable_reason = existing
                .get("reason")
                .and_then(|v| v.as_str())
                .filter(|value| {
                    !value.trim().is_empty()
                        && value.len() <= 600
                        && !value.contains(['\0', '\r', '\n'])
                })
                .ok_or(InboxError::DecisionConflict)?;
            return Ok(durable_reason.to_owned());
        }
        return Err(InboxError::DecisionConflict);
    }
    let previous: Option<i64> = conn
        .query_row(
            "SELECT MAX(sequence) FROM delegation_attempt_events WHERE attempt_id = ?1",
            [&delivery.identity.attempt_id],
            |row| row.get(0),
        )
        .map_err(storage)?;
    DelegationRepository::append_attempt_event(
        conn,
        &delivery.identity.attempt_id,
        previous.unwrap_or(0) + 1,
        "retry_requested",
        &payload,
        now,
    )
    .map_err(InboxError::Storage)?;
    Ok(reason.to_owned())
}

fn parse_acceptance_status(value: &str) -> Result<InboxAcceptanceStatus, InboxError> {
    match value {
        "submitted" => Ok(InboxAcceptanceStatus::Submitted),
        "accepted" => Ok(InboxAcceptanceStatus::Accepted),
        "rejected" => Ok(InboxAcceptanceStatus::Rejected),
        _ => Err(InboxError::Storage(
            "unknown delivery acceptance status".into(),
        )),
    }
}

fn storage(error: rusqlite::Error) -> InboxError {
    InboxError::Storage(error.to_string())
}

fn map_runtime_error(error: DelegationRuntimeError) -> InboxError {
    match error {
        DelegationRuntimeError::Contract(value) => InboxError::Contract(value),
        DelegationRuntimeError::Storage(value) => InboxError::Storage(value),
        DelegationRuntimeError::NotRunning(_) => InboxError::DecisionConflict,
        other => InboxError::Runtime(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    use crate::agent::delegation::{NewAttempt, NewCapabilityLease, NewDelegation};
    use crate::agent::delegation_contract::{
        Confidence, DelegationBrief, DelegationIdentity, DeliveryStatus, KeyFact, UsageFlags,
        VerificationRecord, VerificationStatus,
    };
    use crate::agent::delegation_runtime::{FixedAdmissionPolicy, StopReason, WorkerDispatch};

    #[derive(Default)]
    struct FakeWorker;
    impl WorkerAdapter for FakeWorker {
        fn start(&mut self, _: WorkerDispatch) -> Result<(), String> {
            Ok(())
        }
        fn stop(&mut self, _: &str, _: StopReason) -> Result<(), String> {
            Ok(())
        }
    }
    struct FakeClock(Cell<i64>);
    impl Clock for FakeClock {
        fn now(&self) -> i64 {
            self.0.get()
        }
    }

    fn runtime() -> DelegationRuntime<FakeWorker, FakeClock, FixedAdmissionPolicy> {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let mut runtime = DelegationRuntime::new(
            conn,
            FakeWorker,
            FakeClock(Cell::new(10)),
            FixedAdmissionPolicy {
                max_running_attempts: 1,
            },
            30,
        )
        .unwrap();
        seed(&mut runtime, "d1", "a1");
        runtime.dispatch_one().unwrap().unwrap();
        runtime
    }

    fn seed(
        runtime: &mut DelegationRuntime<FakeWorker, FakeClock, FixedAdmissionPolicy>,
        id: &str,
        attempt_id: &str,
    ) {
        let mut conn = runtime.connection_mut();
        conn.execute(
            "INSERT INTO sessions (id,title,created_at,updated_at) VALUES ('s1','s1',1,1)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO messages (id,session_id,role,content,created_at) VALUES ('m1','s1','user','goal',1)", []).unwrap();
        conn.execute("INSERT INTO task_runs (id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES ('r1','s1','m1','goal','running','[]',1,1)", []).unwrap();
        let identity = DelegationIdentity {
            session_id: "s1".into(),
            message_id: "m1".into(),
            parent_run_id: "r1".into(),
            delegation_id: id.into(),
            attempt_id: attempt_id.into(),
        };
        let brief = DelegationBrief {
            schema_version: 1,
            identity,
            goal: "goal".into(),
            background: vec![],
            constraints: vec![],
            allowed_references: vec![],
            allowed_capabilities: vec![],
            completion_criteria: vec!["done".into()],
        };
        let delegation = NewDelegation {
            id: id.into(),
            session_id: "s1".into(),
            message_id: "m1".into(),
            parent_run_id: "r1".into(),
            work_package_id: None,
            idempotency_key: None,
            objective: "goal".into(),
            brief_json: serde_json::to_string(&brief).unwrap(),
        };
        let attempt = NewAttempt {
            id: attempt_id.into(),
            attempt_number: 1,
            sandbox_ref: format!("sandbox://{attempt_id}"),
            outbox_id: format!("outbox-{attempt_id}"),
            dispatch_payload_json: "{}".into(),
        };
        let lease = NewCapabilityLease {
            id: format!("lease-{attempt_id}"),
            read_roots_json: "[]".into(),
            write_roots_json: "[]".into(),
            tool_allowlist_json: "[]".into(),
            network_hosts_json: "[]".into(),
            budget_json: "{}".into(),
            expires_at: 100,
        };
        DelegationRepository::create_queued(&mut *conn, &delegation, &attempt, &lease, 10).unwrap();
    }

    fn scope() -> DeliveryScope {
        DeliveryScope {
            session_id: "s1".into(),
            message_id: "m1".into(),
            parent_run_id: "r1".into(),
        }
    }

    fn delivery(status: DeliveryStatus) -> DelegationDelivery {
        DelegationDelivery {
            schema_version: 1,
            delivery_id: "delivery-1".into(),
            delivery_revision: 1,
            identity: DelegationIdentity {
                session_id: "s1".into(),
                message_id: "m1".into(),
                parent_run_id: "r1".into(),
                delegation_id: "d1".into(),
                attempt_id: "a1".into(),
            },
            status,
            executive_summary: "done".into(),
            key_facts: vec![KeyFact {
                statement: "fact".into(),
                confidence: Confidence::High,
                evidence_refs: vec!["evidence://1".into()],
            }],
            milestones: vec![],
            verifications: vec![VerificationRecord {
                item: "goal".into(),
                method: "test".into(),
                status: VerificationStatus::Passed,
                conclusion: "ok".into(),
                evidence_ref: "evidence://1".into(),
            }],
            candidate_artifacts: vec![],
            open_questions: vec![],
            risks: vec![],
            evidence_refs: vec!["evidence://1".into()],
            usage: UsageFlags {
                context_truncated: false,
                output_truncated: false,
                budget_exhausted: false,
            },
        }
    }

    #[test]
    fn submit_is_idempotent_and_pending_is_structured_only() {
        let mut runtime = runtime();
        let limits = ContractLimits::default();
        let d = delivery(DeliveryStatus::Completed);
        assert_eq!(
            DeliveryInbox::submit(&mut runtime, &d, &limits).unwrap(),
            SubmitOutcome::Submitted
        );
        assert_eq!(
            DeliveryInbox::submit(&mut runtime, &d, &limits).unwrap(),
            SubmitOutcome::AlreadySubmitted
        );
        let pending = DeliveryInbox::pending(&runtime, &scope(), &limits).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].context.summary, "done");
        assert_eq!(pending[0].context.facts[0].statement, "fact");
    }

    #[test]
    fn accept_is_parent_only_and_repeated_accept_is_safe() {
        let mut runtime = runtime();
        let limits = ContractLimits::default();
        let d = delivery(DeliveryStatus::Completed);
        DeliveryInbox::submit(&mut runtime, &d, &limits).unwrap();
        assert!(matches!(
            DeliveryInbox::accept(&mut runtime, &scope(), "delivery-1", &limits),
            Ok(DecisionOutcome::Accepted(_))
        ));
        assert!(matches!(
            DeliveryInbox::accept(&mut runtime, &scope(), "delivery-1", &limits),
            Ok(DecisionOutcome::AlreadyAccepted(_))
        ));
        assert_eq!(
            DeliveryInbox::pending(&runtime, &scope(), &limits)
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn retry_records_no_replay_and_is_idempotent() {
        let mut runtime = runtime();
        let limits = ContractLimits::default();
        let d = delivery(DeliveryStatus::Blocked);
        DeliveryInbox::submit(&mut runtime, &d, &limits).unwrap();
        let first = DeliveryInbox::retry(
            &mut runtime,
            &scope(),
            "delivery-1",
            "retry with fresh attempt",
            &limits,
        )
        .unwrap();
        assert!(!first.replay_successful_side_effects);
        let second = DeliveryInbox::retry(
            &mut runtime,
            &scope(),
            "delivery-1",
            "a later review wording must resume the original authorization",
            &limits,
        )
        .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn wrong_scope_cannot_read_or_decide_delivery() {
        let mut runtime = runtime();
        let limits = ContractLimits::default();
        let d = delivery(DeliveryStatus::Completed);
        DeliveryInbox::submit(&mut runtime, &d, &limits).unwrap();
        let wrong = DeliveryScope {
            session_id: "other".into(),
            ..scope()
        };
        assert_eq!(
            DeliveryInbox::accept(&mut runtime, &wrong, "delivery-1", &limits),
            Err(InboxError::DeliveryNotFound)
        );
    }
}
