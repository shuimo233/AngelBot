//! Durable domain and repository boundary for delegated work.
//!
//! This module intentionally does not run sub-agents.  It is the small
//! persistence seam a future runtime must cross before dispatching work, so
//! retries, leases, deliveries, and parent acceptance remain auditable.

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

pub type DelegationResult<T> = Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationStatus {
    Queued,
    Running,
    AwaitingConfirmation,
    AwaitingSummary,
    Completed,
    Failed,
    Cancelled,
    NeedsDecision,
}

impl DelegationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::AwaitingConfirmation => "awaiting_confirmation",
            Self::AwaitingSummary => "awaiting_summary",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::NeedsDecision => "needs_decision",
        }
    }

    /// `completed` is deliberately absent: only accepting a delivery may
    /// complete a delegation.
    fn may_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Queued, Self::Running | Self::Cancelled | Self::Failed)
                | (
                    Self::Running,
                    Self::AwaitingConfirmation
                        | Self::AwaitingSummary
                        | Self::Failed
                        | Self::Cancelled
                        | Self::NeedsDecision
                )
                | (
                    Self::AwaitingConfirmation,
                    Self::Running | Self::Cancelled | Self::NeedsDecision
                )
                | (
                    Self::AwaitingSummary,
                    Self::Running | Self::Failed | Self::Cancelled | Self::NeedsDecision
                )
                | (Self::NeedsDecision, Self::Running | Self::Cancelled)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Queued,
    Running,
    Sealed,
    Failed,
    Cancelled,
}

impl AttemptStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Sealed => "sealed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseStatus {
    Active,
    Revoked,
    Expired,
}

impl LeaseStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewDelegation {
    pub id: String,
    pub session_id: String,
    pub message_id: String,
    pub parent_run_id: String,
    /// Immutable parent when this delegation was issued through a
    /// `WorkPackage`. Legacy durable delegations intentionally remain
    /// unscoped so their historical rows can migrate safely.
    pub work_package_id: Option<String>,
    /// Parent-scoped tool-call key. Historical rows may leave this NULL;
    /// newly issued foreground delegations must persist it for idempotency.
    pub idempotency_key: Option<String>,
    pub objective: String,
    /// Already-filtered, structured context. Raw conversation history belongs
    /// behind references and is never embedded here.
    pub brief_json: String,
}

#[derive(Debug, Clone)]
pub struct NewAttempt {
    pub id: String,
    pub attempt_number: i64,
    pub sandbox_ref: String,
    pub outbox_id: String,
    pub dispatch_payload_json: String,
}

#[derive(Debug, Clone)]
pub struct NewCapabilityLease {
    pub id: String,
    pub read_roots_json: String,
    pub write_roots_json: String,
    pub tool_allowlist_json: String,
    pub network_hosts_json: String,
    pub budget_json: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegation {
    pub id: String,
    pub session_id: String,
    pub message_id: String,
    pub parent_run_id: String,
    pub work_package_id: Option<String>,
    pub status: DelegationStatus,
    pub state_version: i64,
}

/// The single durable result of retry queuing.  A replay of the same
/// Main-Agent request resolves to the existing fresh attempt; it must never
/// allocate another execution slot for the same submitted delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryQueueOutcome {
    Queued,
    AlreadyQueued { attempt_id: String },
}

/// Repository methods are the only persistence API exposed to a future
/// DelegationRuntime. They intentionally require expected versions and keep
/// execution records append-only, making recovery idempotent.
pub struct DelegationRepository;

impl DelegationRepository {
    /// Persists a delegation, its first fresh sandbox attempt, an active lease,
    /// immutable journal fact, and dispatch outbox record in one transaction.
    pub fn create_queued(
        conn: &mut Connection,
        delegation: &NewDelegation,
        attempt: &NewAttempt,
        lease: &NewCapabilityLease,
        now: i64,
    ) -> DelegationResult<()> {
        Self::create_queued_with(conn, delegation, attempt, lease, now, |_| Ok(()))
    }

    /// Same durable queue transaction as [`create_queued`], with one narrow
    /// post-insert fact seam. Issuance uses this to persist a typed explorer
    /// plan before the transaction commits; callers cannot create a second
    /// queue path or observe a partially persisted delegation.
    pub fn create_queued_with<F>(
        conn: &mut Connection,
        delegation: &NewDelegation,
        attempt: &NewAttempt,
        lease: &NewCapabilityLease,
        now: i64,
        after_insert: F,
    ) -> DelegationResult<()>
    where
        F: FnOnce(&Transaction<'_>) -> DelegationResult<()>,
    {
        if attempt.attempt_number != 1 {
            return Err("a new delegation must begin with attempt number 1".into());
        }
        if lease.expires_at <= now {
            return Err("capability lease must expire after it is issued".into());
        }
        let tx = conn.transaction().map_err(sql_error)?;
        tx.execute(
            "INSERT INTO delegations (id, session_id, message_id, parent_run_id, work_package_id, idempotency_key, objective, brief_json, status, state_version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'queued', 0, ?9, ?9)",
            params![delegation.id, delegation.session_id, delegation.message_id, delegation.parent_run_id, delegation.work_package_id, delegation.idempotency_key, delegation.objective, delegation.brief_json, now],
        ).map_err(sql_error)?;
        Self::insert_attempt(&tx, &delegation.id, attempt, None, now)?;
        tx.execute(
            "INSERT INTO delegation_capability_leases (id, attempt_id, read_roots_json, write_roots_json, tool_allowlist_json, network_hosts_json, budget_json, status, issued_at, expires_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', ?8, ?9, NULL)",
            params![lease.id, attempt.id, lease.read_roots_json, lease.write_roots_json, lease.tool_allowlist_json, lease.network_hosts_json, lease.budget_json, now, lease.expires_at],
        ).map_err(sql_error)?;
        Self::insert_event(
            &tx,
            &attempt.id,
            1,
            "attempt_queued",
            &attempt.dispatch_payload_json,
            now,
        )?;
        tx.execute(
            "INSERT INTO delegation_outbox (id, attempt_id, sequence, event_type, payload_json, created_at, dispatched_at)
             VALUES (?1, ?2, 1, 'dispatch_attempt', ?3, ?4, NULL)",
            params![attempt.outbox_id, attempt.id, attempt.dispatch_payload_json, now],
        ).map_err(sql_error)?;
        after_insert(&tx)?;
        tx.commit().map_err(sql_error)
    }

    pub fn append_attempt_event(
        conn: &Connection,
        attempt_id: &str,
        sequence: i64,
        event_type: &str,
        payload_json: &str,
        now: i64,
    ) -> DelegationResult<()> {
        Self::insert_event(conn, attempt_id, sequence, event_type, payload_json, now)
    }

    /// Append an event while the caller's composition transaction is still
    /// open.  Issuance uses this to keep lease/admission facts and their
    /// journal event atomic; a post-commit event failure must not strand an
    /// active admission lease.
    pub(crate) fn append_attempt_event_in_tx(
        tx: &Transaction<'_>,
        attempt_id: &str,
        sequence: i64,
        event_type: &str,
        payload_json: &str,
        now: i64,
    ) -> DelegationResult<()> {
        Self::insert_event(tx, attempt_id, sequence, event_type, payload_json, now)
    }

    /// Resolves a durable retry lineage before any fresh sandbox or worktree
    /// is allocated.  This is intentionally keyed only by trusted database
    /// identities; callers never derive it from a model-owned brief.
    pub(crate) fn retry_attempt_for_source(
        conn: &Connection,
        delegation_id: &str,
        source_delivery_id: &str,
    ) -> DelegationResult<Option<String>> {
        conn.query_row(
            "SELECT id FROM delegation_attempts
             WHERE delegation_id = ?1 AND retry_source_delivery_id = ?2",
            params![delegation_id, source_delivery_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)
    }

    /// Atomically turns one sealed, submitted delivery into a fresh retry
    /// attempt.  The stable delegation and immutable work package remain in
    /// place, while the source delivery becomes rejected/superseded so it
    /// cannot be accepted later or reappear beside the new result.
    ///
    /// `after_insert` is the same narrow issuance seam as initial queueing:
    /// it may persist only attempt-bound resources, admission, plan, and
    /// Supervisor gate in this transaction.  It cannot run a worker.
    pub(crate) fn queue_retry_with<F>(
        conn: &mut Connection,
        delegation_id: &str,
        source_delivery_id: &str,
        source_delivery_revision: u32,
        source_attempt_id: &str,
        reason: &str,
        retry_brief_json: &str,
        attempt: &NewAttempt,
        lease: &NewCapabilityLease,
        now: i64,
        after_insert: F,
    ) -> DelegationResult<RetryQueueOutcome>
    where
        F: FnOnce(&Transaction<'_>) -> DelegationResult<()>,
    {
        if attempt.attempt_number < 2 {
            return Err("a retry attempt number must be at least 2".into());
        }
        if lease.expires_at <= now {
            return Err("capability lease must expire after it is issued".into());
        }
        if source_delivery_id.trim().is_empty()
            || source_attempt_id.trim().is_empty()
            || reason.trim().is_empty()
            || reason.len() > 600
            || reason.contains(['\0', '\r', '\n'])
            || retry_brief_json.trim().is_empty()
        {
            return Err("retry continuation is invalid".into());
        }
        let tx = conn.transaction().map_err(sql_error)?;

        // The lineage key makes a replay idempotent even after the source has
        // been superseded.  Do this lookup before requiring `submitted`.
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM delegation_attempts
                 WHERE delegation_id = ?1 AND retry_source_delivery_id = ?2",
                params![delegation_id, source_delivery_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        if let Some(attempt_id) = existing {
            tx.commit().map_err(sql_error)?;
            return Ok(RetryQueueOutcome::AlreadyQueued { attempt_id });
        }

        let source: Option<(String, i64, String)> = tx
            .query_row(
                "SELECT delivery.attempt_id, delivery.delivery_version, delivery.acceptance_status
                 FROM delegation_deliveries delivery
                 JOIN delegation_attempts source_attempt ON source_attempt.id = delivery.attempt_id
                 WHERE delivery.id = ?1
                   AND delivery.delegation_id = ?2
                   AND delivery.attempt_id = ?3
                   AND source_attempt.status = 'sealed'",
                params![source_delivery_id, delegation_id, source_attempt_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(sql_error)?;
        let Some((stored_attempt_id, stored_revision, acceptance_status)) = source else {
            return Err("retry source delivery is not a sealed scoped attempt".into());
        };
        if stored_attempt_id != source_attempt_id
            || stored_revision != i64::from(source_delivery_revision)
            || acceptance_status != "submitted"
        {
            return Err("retry source delivery changed concurrently".into());
        }

        // A retry must originate from DeliveryInbox, which records this
        // intent before the terminal attempt is sealed.  Checking that durable
        // fact prevents a lower layer from manufacturing a retry directly.
        let retry_request: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM delegation_attempt_events
                 WHERE attempt_id = ?1 AND event_type = 'retry_requested'
                 ORDER BY sequence DESC LIMIT 1",
                [source_attempt_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        let matches_request = retry_request
            .as_deref()
            .and_then(|payload| serde_json::from_str::<serde_json::Value>(payload).ok())
            .map(|payload| {
                payload.get("delivery_id").and_then(|value| value.as_str())
                    == Some(source_delivery_id)
                    && payload
                        .get("delivery_revision")
                        .and_then(|value| value.as_u64())
                        == Some(u64::from(source_delivery_revision))
                    && payload.get("reason").and_then(|value| value.as_str()) == Some(reason)
                    && payload
                        .get("replay_successful_side_effects")
                        .and_then(|value| value.as_bool())
                        == Some(false)
            })
            .unwrap_or(false);
        if !matches_request {
            return Err("retry continuation is not durably authorized".into());
        }

        let next: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(attempt_number), 0) + 1 FROM delegation_attempts WHERE delegation_id = ?1",
                [delegation_id],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if next != attempt.attempt_number {
            return Err("retry attempt number is not the next stable attempt".into());
        }
        let superseded = tx
            .execute(
                "UPDATE delegation_deliveries
                 SET acceptance_status = 'rejected', accepted_by = NULL, accepted_at = NULL
                 WHERE id = ?1 AND delegation_id = ?2 AND acceptance_status = 'submitted'",
                params![source_delivery_id, delegation_id],
            )
            .map_err(sql_error)?;
        if superseded != 1 {
            return Err("retry source delivery changed concurrently".into());
        }
        let requeued = tx
            .execute(
                "UPDATE delegations
                 SET status = 'queued', brief_json = ?1,
                     state_version = state_version + 1, updated_at = ?2
                 WHERE id = ?3 AND status = 'needs_decision'",
                params![retry_brief_json, now, delegation_id],
            )
            .map_err(sql_error)?;
        if requeued != 1 {
            return Err("delegation is not awaiting a retry decision".into());
        }
        Self::insert_attempt(&tx, delegation_id, attempt, Some(source_delivery_id), now)?;
        tx.execute(
            "INSERT INTO delegation_capability_leases (id, attempt_id, read_roots_json, write_roots_json, tool_allowlist_json, network_hosts_json, budget_json, status, issued_at, expires_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', ?8, ?9, NULL)",
            params![lease.id, attempt.id, lease.read_roots_json, lease.write_roots_json, lease.tool_allowlist_json, lease.network_hosts_json, lease.budget_json, now, lease.expires_at],
        ).map_err(sql_error)?;
        Self::insert_event(
            &tx,
            &attempt.id,
            1,
            "attempt_queued",
            &attempt.dispatch_payload_json,
            now,
        )?;
        tx.execute(
            "INSERT INTO delegation_outbox (id, attempt_id, sequence, event_type, payload_json, created_at, dispatched_at)
             VALUES (?1, ?2, 1, 'dispatch_attempt', ?3, ?4, NULL)",
            params![attempt.outbox_id, attempt.id, attempt.dispatch_payload_json, now],
        ).map_err(sql_error)?;
        Self::insert_event(
            &tx,
            &attempt.id,
            2,
            "retry_issued",
            &serde_json::json!({
                "source_delivery_id": source_delivery_id,
                "source_delivery_revision": source_delivery_revision,
                "source_attempt_id": source_attempt_id,
                "reason": reason,
                "replay_successful_side_effects": false,
            })
            .to_string(),
            now,
        )?;
        after_insert(&tx)?;
        tx.commit()
            .map_err(sql_error)
            .map(|()| RetryQueueOutcome::Queued)
    }

    /// A normal status transition requires an optimistic `state_version`.
    /// Completion is rejected here, forcing callers through
    /// `accept_delivery_and_complete`.
    pub fn transition_status(
        conn: &Connection,
        delegation_id: &str,
        expected_version: i64,
        from: DelegationStatus,
        to: DelegationStatus,
        now: i64,
    ) -> DelegationResult<i64> {
        if to == DelegationStatus::Completed || !from.may_transition_to(to) {
            return Err("invalid delegation lifecycle transition".into());
        }
        let changed = conn.execute(
            "UPDATE delegations SET status = ?1, state_version = state_version + 1, updated_at = ?2
             WHERE id = ?3 AND status = ?4 AND state_version = ?5",
            params![to.as_str(), now, delegation_id, from.as_str(), expected_version],
        ).map_err(sql_error)?;
        if changed != 1 {
            return Err("delegation state changed concurrently or does not match".into());
        }
        Ok(expected_version + 1)
    }

    pub fn submit_delivery(
        conn: &Connection,
        delivery_id: &str,
        delegation_id: &str,
        attempt_id: &str,
        delivery_version: i64,
        schema_version: i64,
        payload_json: &str,
        now: i64,
    ) -> DelegationResult<()> {
        conn.execute(
            "INSERT INTO delegation_deliveries (id, delegation_id, attempt_id, delivery_version, schema_version, payload_json, acceptance_status, accepted_by, accepted_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'submitted', NULL, NULL, ?7)",
            params![delivery_id, delegation_id, attempt_id, delivery_version, schema_version, payload_json, now],
        ).map_err(sql_error)?;
        Ok(())
    }

    /// The only repository operation permitted to set a delegation completed.
    /// It atomically records the parent acceptance and passes the schema trigger
    /// that rejects completion without an accepted structured delivery.
    pub fn accept_delivery_and_complete(
        conn: &mut Connection,
        delegation_id: &str,
        delivery_id: &str,
        expected_version: i64,
        now: i64,
    ) -> DelegationResult<i64> {
        let tx = conn.transaction().map_err(sql_error)?;
        let accepted = tx.execute(
            "UPDATE delegation_deliveries SET acceptance_status = 'accepted', accepted_by = 'parent_agent', accepted_at = ?1
             WHERE id = ?2 AND delegation_id = ?3 AND acceptance_status = 'submitted'",
            params![now, delivery_id, delegation_id],
        ).map_err(sql_error)?;
        if accepted != 1 {
            return Err("delivery is not a submitted delivery for this delegation".into());
        }
        let changed = tx.execute(
            "UPDATE delegations SET status = 'completed', state_version = state_version + 1, updated_at = ?1
             WHERE id = ?2 AND state_version = ?3 AND status IN ('running', 'awaiting_summary')",
            params![now, delegation_id, expected_version],
        ).map_err(sql_error)?;
        if changed != 1 {
            return Err("delegation state changed concurrently or is not completable".into());
        }
        tx.commit().map_err(sql_error)?;
        Ok(expected_version + 1)
    }

    pub fn get(conn: &Connection, delegation_id: &str) -> DelegationResult<Option<Delegation>> {
        conn.query_row(
            "SELECT id, session_id, message_id, parent_run_id, work_package_id, status, state_version FROM delegations WHERE id = ?1",
            [delegation_id],
            |row| {
                let status: String = row.get(5)?;
                Ok(Delegation {
                    id: row.get(0)?, session_id: row.get(1)?, message_id: row.get(2)?, parent_run_id: row.get(3)?,
                    work_package_id: row.get(4)?, status: parse_delegation_status(&status).map_err(to_sql_error)?, state_version: row.get(6)?,
                })
            },
        ).optional().map_err(sql_error)
    }

    fn insert_attempt(
        tx: &Transaction<'_>,
        delegation_id: &str,
        attempt: &NewAttempt,
        retry_source_delivery_id: Option<&str>,
        now: i64,
    ) -> DelegationResult<()> {
        tx.execute(
            "INSERT INTO delegation_attempts (id, delegation_id, attempt_number, status, sandbox_ref, retry_source_delivery_id, created_at, started_at, ended_at)
             VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6, NULL, NULL)",
            params![attempt.id, delegation_id, attempt.attempt_number, attempt.sandbox_ref, retry_source_delivery_id, now],
        ).map_err(sql_error)?;
        Ok(())
    }

    fn insert_event(
        conn: &Connection,
        attempt_id: &str,
        sequence: i64,
        event_type: &str,
        payload_json: &str,
        now: i64,
    ) -> DelegationResult<()> {
        if sequence < 1 {
            return Err("attempt event sequence must start at 1".into());
        }
        let existing: Option<(String, String)> = conn.query_row(
            "SELECT event_type, payload_json FROM delegation_attempt_events WHERE attempt_id = ?1 AND sequence = ?2",
            params![attempt_id, sequence], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(sql_error)?;
        if let Some((stored_type, stored_payload)) = existing {
            return if stored_type == event_type && stored_payload == payload_json {
                Ok(())
            } else {
                Err("attempt event sequence already belongs to different fact".into())
            };
        }
        let previous: Option<i64> = conn
            .query_row(
                "SELECT MAX(sequence) FROM delegation_attempt_events WHERE attempt_id = ?1",
                [attempt_id],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if let Some(previous) = previous {
            if sequence != previous + 1 {
                return Err("attempt events must be appended in sequence order".into());
            }
        }
        conn.execute(
            "INSERT INTO delegation_attempt_events (id, attempt_id, sequence, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![format!("{}:{}", attempt_id, sequence), attempt_id, sequence, event_type, payload_json, now],
        ).map_err(sql_error)?;
        Ok(())
    }
}

fn parse_delegation_status(value: &str) -> Result<DelegationStatus, String> {
    match value {
        "queued" => Ok(DelegationStatus::Queued),
        "running" => Ok(DelegationStatus::Running),
        "awaiting_confirmation" => Ok(DelegationStatus::AwaitingConfirmation),
        "awaiting_summary" => Ok(DelegationStatus::AwaitingSummary),
        "completed" => Ok(DelegationStatus::Completed),
        "failed" => Ok(DelegationStatus::Failed),
        "cancelled" => Ok(DelegationStatus::Cancelled),
        "needs_decision" => Ok(DelegationStatus::NeedsDecision),
        _ => Err(format!("unknown delegation status: {value}")),
    }
}

fn sql_error(error: rusqlite::Error) -> String {
    error.to_string()
}
fn to_sql_error(error: String) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(error.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn ready() -> (Connection, NewDelegation, NewAttempt, NewCapabilityLease) {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute("INSERT INTO sessions (id, title, created_at, updated_at) VALUES ('s', 'session', 1, 1)", []).unwrap();
        conn.execute("INSERT INTO messages (id, session_id, role, content, created_at) VALUES ('m', 's', 'user', 'hi', 1)", []).unwrap();
        conn.execute("INSERT INTO task_runs (id, session_id, message_id, goal, status, plan, created_at, updated_at) VALUES ('r', 's', 'm', 'goal', 'running', '[]', 1, 1)", []).unwrap();
        (
            conn,
            NewDelegation {
                id: "d".into(),
                session_id: "s".into(),
                message_id: "m".into(),
                parent_run_id: "r".into(),
                work_package_id: None,
                idempotency_key: None,
                objective: "research".into(),
                brief_json: "{}".into(),
            },
            NewAttempt {
                id: "a1".into(),
                attempt_number: 1,
                sandbox_ref: "sandbox://a1".into(),
                outbox_id: "o1".into(),
                dispatch_payload_json: "{}".into(),
            },
            NewCapabilityLease {
                id: "l1".into(),
                read_roots_json: "[]".into(),
                write_roots_json: "[]".into(),
                tool_allowlist_json: "[]".into(),
                network_hosts_json: "[]".into(),
                budget_json: "{}".into(),
                expires_at: 100,
            },
        )
    }

    #[test]
    fn queued_attempt_is_durable_before_dispatch_and_events_are_idempotent() {
        let (mut conn, delegation, attempt, lease) = ready();
        DelegationRepository::create_queued(&mut conn, &delegation, &attempt, &lease, 10).unwrap();
        assert_eq!(
            DelegationRepository::get(&conn, "d")
                .unwrap()
                .unwrap()
                .status,
            DelegationStatus::Queued
        );
        let pending: i64 = conn.query_row("SELECT COUNT(*) FROM delegation_outbox WHERE attempt_id = 'a1' AND dispatched_at IS NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(pending, 1);
        DelegationRepository::append_attempt_event(&conn, "a1", 1, "attempt_queued", "{}", 10)
            .unwrap();
        assert!(
            DelegationRepository::append_attempt_event(&conn, "a1", 1, "other", "{}", 10).is_err()
        );
        assert!(
            DelegationRepository::append_attempt_event(&conn, "a1", 3, "gap", "{}", 10).is_err()
        );
    }

    #[test]
    fn only_parent_delivery_acceptance_can_complete_delegation() {
        let (mut conn, delegation, attempt, lease) = ready();
        DelegationRepository::create_queued(&mut conn, &delegation, &attempt, &lease, 10).unwrap();
        let version = DelegationRepository::transition_status(
            &conn,
            "d",
            0,
            DelegationStatus::Queued,
            DelegationStatus::Running,
            11,
        )
        .unwrap();
        assert!(DelegationRepository::transition_status(
            &conn,
            "d",
            version,
            DelegationStatus::Running,
            DelegationStatus::Completed,
            12
        )
        .is_err());
        assert!(conn
            .execute(
                "UPDATE delegations SET status = 'completed' WHERE id = 'd'",
                []
            )
            .is_err());
        DelegationRepository::submit_delivery(
            &conn,
            "delivery-1",
            "d",
            "a1",
            1,
            1,
            "{\"status\":\"completed\"}",
            13,
        )
        .unwrap();
        let completed_version = DelegationRepository::accept_delivery_and_complete(
            &mut conn,
            "d",
            "delivery-1",
            version,
            14,
        )
        .unwrap();
        assert_eq!(completed_version, 2);
        assert_eq!(
            DelegationRepository::get(&conn, "d")
                .unwrap()
                .unwrap()
                .status,
            DelegationStatus::Completed
        );
    }

    #[test]
    fn retry_requeues_a_sealed_delivery_once_without_reexposing_it() {
        let (mut conn, delegation, attempt, lease) = ready();
        DelegationRepository::create_queued(&mut conn, &delegation, &attempt, &lease, 10).unwrap();
        let version = DelegationRepository::transition_status(
            &conn,
            "d",
            0,
            DelegationStatus::Queued,
            DelegationStatus::Running,
            11,
        )
        .unwrap();
        DelegationRepository::submit_delivery(&conn, "delivery-1", "d", "a1", 1, 1, "{}", 12)
            .unwrap();
        DelegationRepository::transition_status(
            &conn,
            "d",
            version,
            DelegationStatus::Running,
            DelegationStatus::NeedsDecision,
            13,
        )
        .unwrap();
        conn.execute(
            "UPDATE delegation_attempts SET status = 'sealed', ended_at = 13 WHERE id = 'a1'",
            [],
        )
        .unwrap();
        DelegationRepository::append_attempt_event(
            &conn,
            "a1",
            2,
            "retry_requested",
            r#"{"delivery_id":"delivery-1","delivery_revision":1,"reason":"retry safely","replay_successful_side_effects":false}"#,
            13,
        )
        .unwrap();
        let retry = NewAttempt {
            id: "a2".into(),
            attempt_number: 2,
            sandbox_ref: "sandbox://a2".into(),
            outbox_id: "o2".into(),
            dispatch_payload_json: "{}".into(),
        };
        let retry_lease = NewCapabilityLease {
            id: "l2".into(),
            expires_at: 100,
            ..lease
        };
        assert_eq!(
            DelegationRepository::queue_retry_with(
                &mut conn,
                "d",
                "delivery-1",
                1,
                "a1",
                "retry safely",
                "{}",
                &retry,
                &retry_lease,
                14,
                |_| Ok(()),
            )
            .unwrap(),
            RetryQueueOutcome::Queued,
        );
        assert_eq!(
            conn.query_row(
                "SELECT acceptance_status FROM delegation_deliveries WHERE id = 'delivery-1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "rejected"
        );
        assert_eq!(
            DelegationRepository::get(&conn, "d")
                .unwrap()
                .unwrap()
                .status,
            DelegationStatus::Queued
        );
        assert_eq!(
            DelegationRepository::queue_retry_with(
                &mut conn,
                "d",
                "delivery-1",
                1,
                "a1",
                "retry safely",
                "{}",
                &retry,
                &retry_lease,
                15,
                |_| Ok(()),
            )
            .unwrap(),
            RetryQueueOutcome::AlreadyQueued {
                attempt_id: "a2".into()
            },
        );
        let attempts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM delegation_attempts WHERE delegation_id = 'd'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(attempts, 2);
        assert_eq!(
            DelegationRepository::get(&conn, "d").unwrap().unwrap().id,
            "d"
        );
    }

    #[test]
    fn session_delete_cascades_all_delegation_records() {
        let (mut conn, delegation, attempt, lease) = ready();
        DelegationRepository::create_queued(&mut conn, &delegation, &attempt, &lease, 10).unwrap();
        DelegationRepository::submit_delivery(&conn, "delivery-1", "d", "a1", 1, 1, "{}", 11)
            .unwrap();
        conn.execute("DELETE FROM sessions WHERE id = 's'", [])
            .unwrap();
        for table in [
            "delegations",
            "delegation_attempts",
            "delegation_capability_leases",
            "delegation_deliveries",
            "delegation_attempt_events",
            "delegation_outbox",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table} should cascade with session deletion");
        }
    }
}
