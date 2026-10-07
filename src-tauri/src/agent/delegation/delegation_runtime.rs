//! Main-agent-owned orchestration for durable delegated attempts.
//!
//! This is deliberately a control plane. It owns admission, durable dispatch
//! claims, lease renewal, recovery, and parent-only delivery acceptance; it
//! does not construct prompts or run tools directly; workers receive a
//! capability-scoped execution context from the delegation host.

use std::{fmt, sync::MutexGuard};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use super::{
    delegation::DelegationRepository,
    delegation_contract::{
        validate_delivery, AcceptanceGate, ContractLimits, DelegationBrief, DelegationDelivery,
        DeliveryStatus,
    },
    shared_db::SharedDb,
};

pub type RuntimeResult<T> = Result<T, DelegationRuntimeError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegationRuntimeError {
    Storage(String),
    Admission(String),
    Worker(String),
    Sandbox(String),
    Contract(String),
    StaleHeartbeat,
    NotRunning(String),
    DuplicateAcceptance,
}

impl fmt::Display for DelegationRuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(e)
            | Self::Admission(e)
            | Self::Worker(e)
            | Self::Sandbox(e)
            | Self::Contract(e)
            | Self::NotRunning(e) => f.write_str(e),
            Self::StaleHeartbeat => f.write_str("stale delegation heartbeat"),
            Self::DuplicateAcceptance => f.write_str("delivery was already accepted"),
        }
    }
}

impl std::error::Error for DelegationRuntimeError {}

pub trait Clock {
    fn now(&self) -> i64;
}

#[derive(Debug, Clone, Copy)]
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }
}

/// The only imperative port exposed to a delegated worker. Implementations
/// must route all tool requests through the foreground execution gateway.
/// Structured events drained from the worker host.  Raw model/tool output is
/// intentionally not represented here; the service applies these events to
/// the durable heartbeat/inbox ports after the worker bridge has released its
/// own internal task state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerEvent {
    Heartbeat(Heartbeat),
    Delivery {
        delivery: DelegationDelivery,
        lease_epoch: u64,
    },
    Terminal {
        attempt_id: String,
        lease_epoch: u64,
        reason: String,
    },
}

pub trait WorkerAdapter {
    fn start(&mut self, dispatch: WorkerDispatch) -> Result<(), String>;
    fn stop(&mut self, attempt_id: &str, reason: StopReason) -> Result<(), String>;

    /// Drain structured worker events without invoking application callbacks
    /// while the service mutex is held.  Simple/fail-closed adapters have no
    /// async task and therefore use the default empty projection.
    fn poll_workers(&mut self) -> Result<Vec<WorkerEvent>, String> {
        Ok(Vec::new())
    }

    fn active_worker_attempts(&self) -> usize {
        0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerDispatch {
    pub outbox_id: String,
    pub delegation_id: String,
    pub attempt_id: String,
    pub payload_json: String,
    pub epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Cancelled,
    Paused,
    LeaseExpired,
    Recovery,
}

/// Sandboxes remain a separate filesystem primitive; runtime only requests
/// sealing/revocation through this port after durable lease revocation.
pub trait SandboxController {
    /// Seal and revoke the attempt's sandbox.  The attempt id is only a
    /// lookup key into the Main-Agent-owned resource binding ledger; it is
    /// deliberately not a path or a provider handle.  Concrete adapters must
    /// resolve the opaque binding/manifest tuple and fail closed when the row
    /// is absent (including legacy `sandbox_ref`-only attempts).
    fn seal_and_revoke(&mut self, attempt_id: &str) -> Result<(), String>;
}

#[derive(Default)]
pub struct NoopSandboxController;
impl SandboxController for NoopSandboxController {
    fn seal_and_revoke(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Explicit policy seam; there is no hidden global concurrency limit.
pub trait AdmissionPolicy {
    fn max_running(&self) -> usize;
    fn allows(&self, delegation_id: &str, attempt_id: &str) -> bool;
}

#[derive(Debug, Clone, Copy)]
pub struct FixedAdmissionPolicy {
    pub max_running_attempts: usize,
}
impl AdmissionPolicy for FixedAdmissionPolicy {
    fn max_running(&self) -> usize {
        self.max_running_attempts
    }
    fn allows(&self, _: &str, _: &str) -> bool {
        self.max_running_attempts > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heartbeat {
    pub attempt_id: String,
    /// Monotonic worker epoch. A restarted worker must obtain a new dispatch.
    pub epoch: u64,
    pub stage: String,
    pub progress_percent: u8,
    pub budget_remaining: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    pub sealed_unknown_effect_attempts: Vec<String>,
}

/// A deliberately synchronous, fakeable runtime. The application may call
/// `dispatch_one`, `expire_leases`, and `recover` from its own scheduler.
pub struct DelegationRuntime<W, C, P, S = NoopSandboxController> {
    db: SharedDb,
    worker: W,
    clock: C,
    policy: P,
    sandbox: S,
    heartbeat_ttl_secs: i64,
    acceptance_gate: AcceptanceGate,
}

impl<W, C, P> DelegationRuntime<W, C, P, NoopSandboxController>
where
    W: WorkerAdapter,
    C: Clock,
    P: AdmissionPolicy,
{
    pub fn new(
        conn: Connection,
        worker: W,
        clock: C,
        policy: P,
        heartbeat_ttl_secs: i64,
    ) -> RuntimeResult<Self> {
        Self::new_shared(
            SharedDb::new(conn),
            worker,
            clock,
            policy,
            heartbeat_ttl_secs,
        )
    }

    pub fn new_shared(
        db: SharedDb,
        worker: W,
        clock: C,
        policy: P,
        heartbeat_ttl_secs: i64,
    ) -> RuntimeResult<Self> {
        Self::with_sandbox(
            db,
            worker,
            clock,
            policy,
            NoopSandboxController,
            heartbeat_ttl_secs,
        )
    }
}

impl<W, C, P, S> DelegationRuntime<W, C, P, S>
where
    W: WorkerAdapter,
    C: Clock,
    P: AdmissionPolicy,
    S: SandboxController,
{
    pub fn with_sandbox(
        db: SharedDb,
        worker: W,
        clock: C,
        policy: P,
        sandbox: S,
        heartbeat_ttl_secs: i64,
    ) -> RuntimeResult<Self> {
        if heartbeat_ttl_secs <= 0 {
            return Err(DelegationRuntimeError::Admission(
                "heartbeat TTL must be positive".into(),
            ));
        }
        Ok(Self {
            db,
            worker,
            clock,
            policy,
            sandbox,
            heartbeat_ttl_secs,
            acceptance_gate: AcceptanceGate::default(),
        })
    }

    pub fn shared_db(&self) -> SharedDb {
        self.db.clone()
    }

    /// Execute a read-only database operation while holding the shared mutex
    /// only for the duration of the closure.
    pub fn with_connection<T>(
        &self,
        operation: impl FnOnce(&Connection) -> RuntimeResult<T>,
    ) -> RuntimeResult<T> {
        self.db
            .with_conn(operation)
            .map_err(|error| DelegationRuntimeError::Storage(error.to_string()))?
    }

    /// Execute a mutating database operation while holding the shared mutex
    /// only for the duration of the closure.  Callers must not invoke worker,
    /// sandbox, or other external code from inside the closure.
    pub fn with_connection_mut<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> RuntimeResult<T>,
    ) -> RuntimeResult<T> {
        self.db
            .with_conn_mut(operation)
            .map_err(|error| DelegationRuntimeError::Storage(error.to_string()))?
    }

    /// Transitional inspection seam for existing adapters. New production
    /// paths should prefer `with_connection`/`with_connection_mut`; callers
    /// must bind and drop this guard before invoking worker or sandbox code.
    pub fn connection(&self) -> MutexGuard<'_, Connection> {
        self.db
            .lock()
            .expect("delegation runtime database mutex poisoned")
    }

    #[cfg(test)]
    pub(crate) fn connection_mut(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db
            .lock()
            .expect("delegation runtime database mutex poisoned")
    }
    /// The scheduler uses the same injected clock as durable lease checks so
    /// test and production expiry decisions cannot diverge.
    pub fn now(&self) -> i64 {
        self.clock.now()
    }
    pub fn worker_mut(&mut self) -> &mut W {
        &mut self.worker
    }

    pub fn worker(&self) -> &W {
        &self.worker
    }

    /// Claims one durable outbox fact before invoking the worker. A process
    /// crash after this commit is intentionally treated as unknown effects by
    /// recovery, never replayed automatically.
    pub fn dispatch_one(&mut self) -> RuntimeResult<Option<WorkerDispatch>> {
        let now = self.clock.now();
        crate::agent::supervision::authorize_one_pending_delegation(self.db.clone(), now)
            .map_err(DelegationRuntimeError::Storage)?;
        let claim = self.claim_one(now)?;
        let Some(dispatch) = claim else {
            return Ok(None);
        };
        if let Err(error) = self.worker.start(dispatch.clone()) {
            self.record_worker_start_failure(&dispatch.attempt_id, &error)?;
            self.seal_attempt(
                &dispatch.attempt_id,
                StopReason::Recovery,
                "worker_start_failed",
                true,
            )?;
            return Err(DelegationRuntimeError::Worker(error));
        }
        Ok(Some(dispatch))
    }

    /// Persist a stable, non-diagnostic launch class so the Main Agent can
    /// explain a blocked delegated task without exposing provider, keychain,
    /// path, or tool-host internals to the user-facing projection.
    fn record_worker_start_failure(&self, attempt_id: &str, error: &str) -> RuntimeResult<()> {
        let class = worker_start_failure_class(error);
        let now = self.clock.now();
        self.with_connection_mut(|conn| {
            let tx = conn.transaction().map_err(storage)?;
            append_event_tx(
                &tx,
                attempt_id,
                "worker_start_rejected",
                &serde_json::json!({"class": class}).to_string(),
                now,
            )?;
            tx.commit().map_err(storage)
        })
    }

    fn claim_one(&mut self, now: i64) -> RuntimeResult<Option<WorkerDispatch>> {
        if self.policy.max_running() == 0 {
            return Ok(None);
        }
        // The transaction is scoped to the durable claim only.  The guard is
        // dropped before `dispatch_one` invokes the worker adapter, so no
        // worker code runs while the DB is locked.
        self.with_connection_mut(|conn| {
            let tx = conn.transaction().map_err(storage)?;
        let running: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM delegation_attempts WHERE status = 'running'",
                [],
                |r| r.get(0),
            )
            .map_err(storage)?;
        if running >= self.policy.max_running() as i64 {
            return Ok(None);
        }
        let candidate: Option<(String, String, String, String, i64)> = tx
            .query_row(
                "SELECT o.id, o.attempt_id, o.payload_json, d.id, a.attempt_number
             FROM delegation_outbox o JOIN delegation_attempts a ON a.id = o.attempt_id
             JOIN delegations d ON d.id = a.delegation_id
             WHERE o.dispatched_at IS NULL AND a.status = 'queued' AND d.status = 'queued'
               AND NOT EXISTS (
                    SELECT 1 FROM workspace_supervisor_delegations gate
                    WHERE gate.attempt_id = o.attempt_id AND gate.authorized_at IS NULL
               )
             ORDER BY o.created_at, o.id LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
            .map_err(storage)?;
        let Some((outbox_id, attempt_id, payload_json, delegation_id, attempt_number)) = candidate else {
            return Ok(None);
        };
        let epoch = worker_epoch_for_attempt_number(attempt_number)?;
        if !self.policy.allows(&delegation_id, &attempt_id) {
            return Ok(None);
        }
        let claimed = tx.execute("UPDATE delegation_outbox SET dispatched_at = ?1 WHERE id = ?2 AND dispatched_at IS NULL", params![now, outbox_id]).map_err(storage)?;
        if claimed != 1 {
            return Ok(None);
        }
        let started = tx.execute("UPDATE delegation_attempts SET status = 'running', started_at = ?1 WHERE id = ?2 AND status = 'queued'", params![now, attempt_id]).map_err(storage)?;
        if started != 1 {
            return Err(DelegationRuntimeError::Storage(
                "outbox belongs to a non-queued attempt".into(),
            ));
        }
        let version: i64 = tx
            .query_row(
                "SELECT state_version FROM delegations WHERE id = ?1 AND status = 'queued'",
                [&delegation_id],
                |r| r.get(0),
            )
            .map_err(storage)?;
        let moved = tx.execute("UPDATE delegations SET status = 'running', state_version = state_version + 1, updated_at = ?1 WHERE id = ?2 AND status = 'queued' AND state_version = ?3", params![now, delegation_id, version]).map_err(storage)?;
        if moved != 1 {
            return Err(DelegationRuntimeError::Storage(
                "delegation state changed during dispatch claim".into(),
            ));
        }
        append_event_tx(
            &tx,
            &attempt_id,
            "attempt_dispatched",
            &serde_json::json!({"epoch": epoch}).to_string(),
            now,
        )?;
        tx.commit().map_err(storage)?;
        Ok(Some(WorkerDispatch {
            outbox_id,
            delegation_id,
            attempt_id,
            payload_json,
            epoch,
        }))
        })
    }

    pub fn heartbeat(&mut self, heartbeat: Heartbeat) -> RuntimeResult<()> {
        if heartbeat.stage.trim().is_empty() || heartbeat.progress_percent > 100 {
            return Err(DelegationRuntimeError::Contract("invalid heartbeat".into()));
        }
        let now = self.clock.now();
        let ttl = self.heartbeat_ttl_secs;
        self.with_connection_mut(|conn| {
        let tx = conn.transaction().map_err(storage)?;
        let current: Option<u64> = tx.query_row(
            "SELECT payload_json FROM delegation_attempt_events WHERE attempt_id = ?1 AND event_type = 'heartbeat' ORDER BY sequence DESC LIMIT 1",
            [&heartbeat.attempt_id], |r| r.get::<_, String>(0)
        ).optional().map_err(storage)?.and_then(|json| serde_json::from_str::<Heartbeat>(&json).ok().map(|h| h.epoch));
        if current.is_some_and(|epoch| heartbeat.epoch <= epoch) {
            return Err(DelegationRuntimeError::StaleHeartbeat);
        }
        let active: Option<i64> = tx.query_row(
            "SELECT 1 FROM delegation_attempts a JOIN delegation_capability_leases l ON l.attempt_id = a.id
             WHERE a.id = ?1 AND a.status = 'running' AND l.status = 'active' AND l.expires_at > ?2", params![heartbeat.attempt_id, now], |r| r.get(0)
        ).optional().map_err(storage)?;
        if active.is_none() {
            return Err(DelegationRuntimeError::NotRunning(heartbeat.attempt_id));
        }
        tx.execute("UPDATE delegation_capability_leases SET expires_at = ?1 WHERE attempt_id = ?2 AND status = 'active'", params![now + ttl, heartbeat.attempt_id]).map_err(storage)?;
        append_event_tx(
            &tx,
            &heartbeat.attempt_id,
            "heartbeat",
            &serde_json::to_string(&heartbeat)
                .map_err(|e| DelegationRuntimeError::Contract(e.to_string()))?,
            now,
        )?;
        tx.commit().map_err(storage)
        })
    }

    pub fn cancel(&mut self, attempt_id: &str) -> RuntimeResult<()> {
        self.seal_attempt(
            attempt_id,
            StopReason::Cancelled,
            "attempt_cancelled",
            false,
        )
    }

    /// Seal a terminal attempt exactly once.  This is used by the composition
    /// root after a parent decision (including an already-completed delivery)
    /// so lease revocation and sandbox sealing are not left behind.  Repeated
    /// calls observe the durable state and avoid invoking worker/sandbox
    /// adapters again.
    pub fn finalize_attempt(&mut self, attempt_id: &str) -> RuntimeResult<()> {
        let active = self.with_connection(|conn| {
            conn.query_row(
                "SELECT 1 FROM delegation_attempts a
                 JOIN delegation_capability_leases l ON l.attempt_id=a.id
                 WHERE a.id=?1 AND a.status IN ('queued','running') AND l.status='active'",
                [attempt_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage)
        })?;
        if active.is_none() {
            return Ok(());
        }
        self.seal_attempt(
            attempt_id,
            StopReason::Cancelled,
            "attempt_finalized",
            false,
        )
    }
    pub fn pause(&mut self, attempt_id: &str) -> RuntimeResult<()> {
        self.seal_attempt(attempt_id, StopReason::Paused, "attempt_paused", false)
    }

    /// Seal an attempt whose durable policy is no longer eligible to run.
    ///
    /// This is deliberately narrower than a retry: no child is relaunched and
    /// no new lease is issued.  The Main Agent receives the durable
    /// `needs_decision` state and may later create a fresh attempt if it still
    /// wants to pursue the work.
    pub fn needs_decision(&mut self, attempt_id: &str) -> RuntimeResult<()> {
        self.seal_attempt(
            attempt_id,
            StopReason::Recovery,
            "attempt_policy_ineligible",
            true,
        )
    }

    /// Child completion enters the durable inbox only. It becomes
    /// `awaiting_summary`; no child path can transition a delegation to
    /// `completed` or produce a user-visible response.
    pub fn submit_delivery(
        &mut self,
        delivery: &DelegationDelivery,
        limits: &ContractLimits,
    ) -> RuntimeResult<()> {
        let now = self.clock.now();
        self.with_connection_mut(|conn| {
        let attempt_is_current: Option<i64> = conn.query_row(
            "SELECT 1 FROM delegation_attempts a JOIN delegations d ON d.id = a.delegation_id
             WHERE a.id = ?1 AND a.delegation_id = ?2 AND a.status = 'running' AND d.status = 'running'",
            params![delivery.identity.attempt_id, delivery.identity.delegation_id], |r| r.get(0)
        ).optional().map_err(storage)?;
        if attempt_is_current.is_none() {
            return Err(DelegationRuntimeError::NotRunning(
                delivery.identity.attempt_id.clone(),
            ));
        }
        let brief_json: String = conn.query_row(
                "SELECT brief_json FROM delegations WHERE id = ?1",
                [&delivery.identity.delegation_id],
                |r| r.get(0),
            )
            .map_err(storage)?;
        let brief: DelegationBrief = serde_json::from_str(&brief_json)
            .map_err(|e| DelegationRuntimeError::Contract(e.to_string()))?;
        validate_delivery(delivery, &brief, None, limits)
            .map_err(|e| DelegationRuntimeError::Contract(format!("{e:?}")))?;
        DelegationRepository::submit_delivery(
            &mut *conn,
            &delivery.delivery_id,
            &delivery.identity.delegation_id,
            &delivery.identity.attempt_id,
            delivery.delivery_revision as i64,
            delivery.schema_version as i64,
            &serde_json::to_string(delivery)
                .map_err(|e| DelegationRuntimeError::Contract(e.to_string()))?,
            now,
        )
        .map_err(DelegationRuntimeError::Storage)?;
        let delegation = DelegationRepository::get(&conn, &delivery.identity.delegation_id)
            .map_err(DelegationRuntimeError::Storage)?
            .ok_or_else(|| DelegationRuntimeError::Storage("delegation not found".into()))?;
        if delegation.status == super::delegation::DelegationStatus::Running {
            DelegationRepository::transition_status(
                &conn,
                &delegation.id,
                delegation.state_version,
                super::delegation::DelegationStatus::Running,
                super::delegation::DelegationStatus::AwaitingSummary,
                now,
            )
            .map_err(DelegationRuntimeError::Storage)?;
        }
        let seq = next_event_sequence(&conn, &delivery.identity.attempt_id)?;
        DelegationRepository::append_attempt_event(&conn, &delivery.identity.attempt_id, seq, "delivery_submitted", &serde_json::json!({"delivery_id": delivery.delivery_id, "revision": delivery.delivery_revision}).to_string(), now).map_err(DelegationRuntimeError::Storage)
        })
    }

    pub fn expire_leases(&mut self) -> RuntimeResult<Vec<String>> {
        let now = self.clock.now();
        let ids = self.active_attempt_ids("l.expires_at <= ?1", &[&now])?;
        for id in &ids {
            self.seal_attempt(id, StopReason::LeaseExpired, "lease_expired", true)?;
        }
        Ok(ids)
    }

    /// Every previously claimed/running attempt is sealed on recovery: worker
    /// effects after the last durable fact are unknowable, so replay is unsafe.
    /// A submitted delivery is already an immutable review subject, however;
    /// retain its parent `awaiting_summary` state so a restarted Main Agent can
    /// review and decide it without reviving the worker.
    pub fn recover(&mut self) -> RuntimeResult<RecoveryReport> {
        let delivered = self.active_attempt_ids(
            "EXISTS (
                SELECT 1 FROM delegation_deliveries delivery
                JOIN delegations delegation ON delegation.id=delivery.delegation_id
                WHERE delivery.attempt_id=a.id
                  AND delivery.acceptance_status='submitted'
                  AND delegation.status='awaiting_summary'
            )",
            &[],
        )?;
        for id in &delivered {
            self.seal_delivered_attempt(id)?;
        }
        let unknown = self.active_attempt_ids("1 = 1", &[])?;
        for id in &unknown {
            self.seal_attempt(id, StopReason::Recovery, "recovery_unknown_effects", true)?;
        }
        let mut ids = delivered;
        ids.extend(unknown);
        Ok(RecoveryReport {
            sealed_unknown_effect_attempts: ids,
        })
    }

    /// Durable active-attempt projection used by the composition root during
    /// shutdown. The returned IDs are guarded by active lease rows; callers
    /// must still use CAS terminal operations for each ID.
    pub fn active_attempts(&self) -> RuntimeResult<Vec<String>> {
        self.active_attempt_ids("1 = 1", &[])
    }

    fn active_attempt_ids(
        &self,
        predicate: &str,
        values: &[&dyn rusqlite::ToSql],
    ) -> RuntimeResult<Vec<String>> {
        let sql = format!("SELECT a.id FROM delegation_attempts a JOIN delegation_capability_leases l ON l.attempt_id = a.id WHERE a.status = 'running' AND l.status = 'active' AND {predicate}");
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(&sql).map_err(storage)?;
            let rows = stmt.query_map(values, |r| r.get(0)).map_err(storage)?;
            rows.collect::<Result<Vec<String>, _>>().map_err(storage)
        })
    }

    fn seal_attempt(
        &mut self,
        attempt_id: &str,
        reason: StopReason,
        event: &str,
        needs_decision: bool,
    ) -> RuntimeResult<()> {
        let now = self.clock.now();
        let (delegation_id, version): (String, i64) = self.with_connection_mut(|conn| {
        let (delegation_id, version): (String, i64) = conn.query_row(
            "SELECT a.delegation_id, d.state_version FROM delegation_attempts a JOIN delegations d ON d.id = a.delegation_id WHERE a.id = ?1", [attempt_id],
            |r| Ok((r.get(0)?, r.get(1)?))
        ).map_err(storage)?;
        // Durable revoke precedes the best-effort external stop/seal boundary.
        let tx = conn.transaction().map_err(storage)?;
        tx.execute("UPDATE delegation_capability_leases SET status = 'revoked', revoked_at = ?1 WHERE attempt_id = ?2 AND status = 'active'", params![now, attempt_id]).map_err(storage)?;
        tx.execute("UPDATE delegation_attempts SET status = 'sealed', ended_at = ?1 WHERE id = ?2 AND status IN ('queued', 'running')", params![now, attempt_id]).map_err(storage)?;
        let target = if needs_decision {
            "needs_decision"
        } else {
            "cancelled"
        };
        tx.execute("UPDATE delegations SET status = ?1, state_version = state_version + 1, updated_at = ?2 WHERE id = ?3 AND state_version = ?4 AND status IN ('queued', 'running', 'awaiting_confirmation', 'awaiting_summary')", params![target, now, delegation_id, version]).map_err(storage)?;
        append_event_tx(
            &tx,
            attempt_id,
            event,
            &serde_json::json!({"reason": format!("{reason:?}")}).to_string(),
            now,
        )?;
        tx.commit().map_err(storage)?;
        Ok((delegation_id, version))
        })?;
        self.worker
            .stop(attempt_id, reason)
            .map_err(DelegationRuntimeError::Worker)?;
        self.sandbox
            .seal_and_revoke(attempt_id)
            .map_err(DelegationRuntimeError::Sandbox)
    }

    /// Stop a producer after its structured delivery is durable, while keeping
    /// the Main-Agent decision path open. This is only used during recovery;
    /// it never replays a worker or leaves its capability lease active.
    fn seal_delivered_attempt(&mut self, attempt_id: &str) -> RuntimeResult<()> {
        let now = self.clock.now();
        self.with_connection_mut(|conn| {
            let tx = conn.transaction().map_err(storage)?;
            tx.execute(
                "UPDATE delegation_capability_leases
                 SET status='revoked', revoked_at=?1
                 WHERE attempt_id=?2 AND status='active'",
                params![now, attempt_id],
            )
            .map_err(storage)?;
            let sealed = tx
                .execute(
                    "UPDATE delegation_attempts SET status='sealed', ended_at=?1
                     WHERE id=?2 AND status='running'",
                    params![now, attempt_id],
                )
                .map_err(storage)?;
            if sealed != 1 {
                return Err(DelegationRuntimeError::NotRunning(attempt_id.into()));
            }
            append_event_tx(
                &tx,
                attempt_id,
                "recovery_delivery_retained",
                &serde_json::json!({"reason": "Recovery"}).to_string(),
                now,
            )?;
            tx.commit().map_err(storage)
        })?;
        self.worker
            .stop(attempt_id, StopReason::Recovery)
            .map_err(DelegationRuntimeError::Worker)?;
        self.sandbox
            .seal_and_revoke(attempt_id)
            .map_err(DelegationRuntimeError::Sandbox)
    }

    /// Only the main agent calls this after it has chosen to accept a complete
    /// delivery. Child submission is durable but never user-visible completion.
    pub fn accept_delivery(
        &mut self,
        delivery: &DelegationDelivery,
        limits: &ContractLimits,
    ) -> RuntimeResult<()> {
        let brief_json: String = self.with_connection(|conn| {
            conn.query_row(
                "SELECT brief_json FROM delegations WHERE id = ?1",
                [&delivery.identity.delegation_id],
                |r| r.get(0),
            )
            .map_err(storage)
        })?;
        let brief: DelegationBrief = serde_json::from_str(&brief_json)
            .map_err(|e| DelegationRuntimeError::Contract(e.to_string()))?;
        validate_delivery(delivery, &brief, None, limits)
            .map_err(|e| DelegationRuntimeError::Contract(format!("{e:?}")))?;
        if delivery.status != DeliveryStatus::Completed {
            return Err(DelegationRuntimeError::Contract(
                "only completed deliveries may be accepted".into(),
            ));
        }
        // `accept_ready` mutates the in-memory gate.  The read-only DB guard
        // above has already been dropped before entering this gate.
        self.acceptance_gate
            .accept_ready(delivery, &brief, limits)
            .map_err(|e| match e {
                super::delegation_contract::ContractError::AlreadyAccepted(_) => {
                    DelegationRuntimeError::DuplicateAcceptance
                }
                other => DelegationRuntimeError::Contract(format!("{other:?}")),
            })?;
        let now = self.clock.now();
        self.with_connection_mut(|conn| {
            let delegation = DelegationRepository::get(conn, &delivery.identity.delegation_id)
                .map_err(DelegationRuntimeError::Storage)?
                .ok_or_else(|| DelegationRuntimeError::Storage("delegation not found".into()))?;
            DelegationRepository::accept_delivery_and_complete(
                conn,
                &delegation.id,
                &delivery.delivery_id,
                delegation.state_version,
                now,
            )
            .map_err(DelegationRuntimeError::Storage)
        })?;
        Ok(())
    }
}

fn append_event_tx(
    tx: &Transaction<'_>,
    attempt_id: &str,
    event_type: &str,
    payload: &str,
    now: i64,
) -> RuntimeResult<()> {
    let previous: Option<i64> = tx
        .query_row(
            "SELECT MAX(sequence) FROM delegation_attempt_events WHERE attempt_id = ?1",
            [attempt_id],
            |r| r.get(0),
        )
        .map_err(storage)?;
    let sequence = previous.unwrap_or(0) + 1;
    tx.execute("INSERT INTO delegation_attempt_events (id, attempt_id, sequence, event_type, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![format!("{attempt_id}:{sequence}"), attempt_id, sequence, event_type, payload, now]).map_err(storage)?;
    Ok(())
}

fn next_event_sequence(conn: &Connection, attempt_id: &str) -> RuntimeResult<i64> {
    let previous: Option<i64> = conn
        .query_row(
            "SELECT MAX(sequence) FROM delegation_attempt_events WHERE attempt_id = ?1",
            [attempt_id],
            |r| r.get(0),
        )
        .map_err(storage)?;
    Ok(previous.unwrap_or(0) + 1)
}

fn storage(error: rusqlite::Error) -> DelegationRuntimeError {
    DelegationRuntimeError::Storage(error.to_string())
}

/// Each fresh delegated attempt gets a distinct worker epoch. Database values
/// are untrusted at this boundary: an invalid value must abort the claim before
/// it can dispatch a worker against a reused isolation generation.
fn worker_epoch_for_attempt_number(attempt_number: i64) -> RuntimeResult<u64> {
    let epoch = u64::try_from(attempt_number).map_err(|_| {
        DelegationRuntimeError::Storage("attempt number cannot be used as a worker epoch".into())
    })?;
    if epoch == 0 {
        return Err(DelegationRuntimeError::Storage(
            "attempt number cannot be used as a worker epoch".into(),
        ));
    }
    Ok(epoch)
}

fn worker_start_failure_class(error: &str) -> &'static str {
    let value = error.to_ascii_lowercase();
    if value.contains("delegated model unavailable") {
        "model_unavailable"
    } else if value.contains("worktreeunavailable") {
        "worktree_unavailable"
    } else if value.contains("bindingunavailable") {
        "binding_unavailable"
    } else if value.contains("host rejected") || value.contains("capabilitydenied") {
        "host_capability_denied"
    } else if value.contains("host unavailable") {
        "host_unavailable"
    } else if value.contains("binding") || value.contains("lease") {
        "binding_unavailable"
    } else {
        "launch_unavailable"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::delegation::{NewAttempt, NewCapabilityLease, NewDelegation};
    use crate::agent::delegation_contract::{
        CandidateArtifact, Confidence, KeyFact, Milestone, UsageFlags, VerificationRecord,
        VerificationStatus,
    };
    use std::cell::Cell;

    #[derive(Default)]
    struct FakeWorker {
        starts: Vec<WorkerDispatch>,
        stops: Vec<String>,
    }
    impl WorkerAdapter for FakeWorker {
        fn start(&mut self, d: WorkerDispatch) -> Result<(), String> {
            self.starts.push(d);
            Ok(())
        }
        fn stop(&mut self, id: &str, _: StopReason) -> Result<(), String> {
            self.stops.push(id.into());
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
        let c = Connection::open_in_memory().unwrap();
        crate::db::migrate(&c).unwrap();
        DelegationRuntime::new(
            c,
            FakeWorker::default(),
            FakeClock(Cell::new(10)),
            FixedAdmissionPolicy {
                max_running_attempts: 1,
            },
            30,
        )
        .unwrap()
    }
    fn seed(
        rt: &mut DelegationRuntime<FakeWorker, FakeClock, FixedAdmissionPolicy>,
        id: &str,
        attempt: &str,
        n: i64,
    ) {
        let mut c = rt.connection_mut();
        c.execute(
            "INSERT INTO sessions (id,title,created_at,updated_at) VALUES (?1,?1,1,1)",
            [format!("s{id}")],
        )
        .unwrap();
        c.execute("INSERT INTO messages (id,session_id,role,content,created_at) VALUES (?1,?2,'user','x',1)", params![format!("m{id}"),format!("s{id}")]).unwrap();
        c.execute("INSERT INTO task_runs (id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES (?1,?2,?3,'test','running','[]',1,1)", params![format!("r{id}"),format!("s{id}"),format!("m{id}")]).unwrap();
        let identity = super::super::delegation_contract::DelegationIdentity {
            session_id: format!("s{id}"),
            message_id: format!("m{id}"),
            parent_run_id: format!("r{id}"),
            delegation_id: id.into(),
            attempt_id: attempt.into(),
        };
        let brief = DelegationBrief {
            schema_version: 1,
            identity,
            goal: "test".into(),
            background: vec![],
            constraints: vec![],
            allowed_references: vec!["evidence://test".into()],
            allowed_capabilities: vec![],
            completion_criteria: vec!["done".into()],
        };
        DelegationRepository::create_queued(
            &mut *c,
            &NewDelegation {
                id: id.into(),
                session_id: format!("s{id}"),
                message_id: format!("m{id}"),
                parent_run_id: format!("r{id}"),
                work_package_id: None,
                idempotency_key: None,
                objective: "o".into(),
                brief_json: serde_json::to_string(&brief).unwrap(),
            },
            &NewAttempt {
                id: attempt.into(),
                attempt_number: n,
                sandbox_ref: format!("sandbox-{attempt}"),
                outbox_id: format!("o-{attempt}"),
                dispatch_payload_json: "{}".into(),
            },
            &NewCapabilityLease {
                id: format!("l-{attempt}"),
                read_roots_json: "[]".into(),
                write_roots_json: "[]".into(),
                tool_allowlist_json: "[]".into(),
                network_hosts_json: "[]".into(),
                budget_json: "{}".into(),
                expires_at: 100,
            },
            10,
        )
        .unwrap();
    }
    #[test]
    fn write_before_dispatch_and_duplicate_claim() {
        let mut rt = runtime();
        seed(&mut rt, "d1", "a1", 1);
        assert_eq!(rt.worker.starts.len(), 0);
        assert!(rt.dispatch_one().unwrap().is_some());
        assert_eq!(rt.worker.starts.len(), 1);
        assert!(rt.dispatch_one().unwrap().is_none());
        let claimed: i64 = rt
            .connection_mut()
            .query_row(
                "SELECT COUNT(*) FROM delegation_outbox WHERE dispatched_at IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(claimed, 1);
    }

    #[test]
    fn dispatch_epoch_tracks_attempt_number_and_rejects_invalid_values() {
        let mut rt = runtime();
        seed(&mut rt, "d1", "a2", 1);
        rt.connection_mut()
            .execute(
                "UPDATE delegation_attempts SET attempt_number = 2 WHERE id = 'a2'",
                [],
            )
            .unwrap();

        let dispatch = rt.dispatch_one().unwrap().unwrap();
        assert_eq!(dispatch.epoch, 2);
        assert_eq!(rt.worker.starts[0].epoch, 2);
        let event_payload: String = rt
            .connection_mut()
            .query_row(
                "SELECT payload_json FROM delegation_attempt_events
                 WHERE attempt_id = 'a2' AND event_type = 'attempt_dispatched'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(event_payload, r#"{"epoch":2}"#);

        assert!(matches!(
            worker_epoch_for_attempt_number(0),
            Err(DelegationRuntimeError::Storage(_))
        ));
        assert!(matches!(
            worker_epoch_for_attempt_number(-1),
            Err(DelegationRuntimeError::Storage(_))
        ));
    }

    #[test]
    fn supervisor_linked_outbox_requires_authorization_before_claim() {
        let mut rt = runtime();
        seed(&mut rt, "d1", "a1", 1);
        {
            let connection = rt.connection_mut();
            connection
                .execute(
                    "INSERT INTO projects (id, name, path, created_at, kind, updated_at)
                 VALUES ('p', 'p', 'C:/p', 1, 'project', 1)",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO work_packages
                    (id, session_id, owner_profile_id, workspace_key, task_shape,
                     worker_profile, worker_policy_version, scope_digest,
                     capability_scope_ref, capability_expires_at, candidate_version,
                     created_at, updated_at, status)
                 VALUES ('wp', 'sd1', 1, 'workspace', 'explore', 'explorer', 1,
                         'scope', 'angelbot.local.v1', 100, 0, 1, 1, 'active')",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE delegations SET work_package_id = 'wp' WHERE id = 'd1'",
                    [],
                )
                .unwrap();
            connection.execute(
                "INSERT INTO workspace_supervisor_work
                    (id, workspace_id, source_input_id, kind, objective, status, claim_token, created_at, updated_at)
                 VALUES ('wswork_1', 'p', NULL, 'explore', 'test', 'queued', NULL, 1, 1)",
                [],
            ).unwrap();
            connection
                .execute(
                    "INSERT INTO workspace_supervisor_delegations
                    (supervisor_work_id, delegation_id, attempt_id, work_package_id, authorized_at)
                 VALUES ('wswork_1', 'd1', 'a1', 'wp', NULL)",
                    [],
                )
                .unwrap();
        }

        assert!(
            rt.claim_one(10).unwrap().is_none(),
            "ungated work must not be claimed"
        );
        assert!(
            crate::agent::supervision::authorize_one_pending_delegation(rt.db.clone(), 10).unwrap()
        );
        assert!(
            !crate::agent::supervision::authorize_one_pending_delegation(rt.db.clone(), 10)
                .unwrap(),
            "a recovered work item may be authorized only once"
        );
        assert!(
            rt.claim_one(10).unwrap().is_some(),
            "authorized work becomes claimable"
        );
    }
    #[test]
    fn concurrency_serializes_attempts() {
        let mut rt = runtime();
        seed(&mut rt, "d1", "a1", 1);
        seed(&mut rt, "d2", "a2", 1);
        assert!(rt.dispatch_one().unwrap().is_some());
        assert!(rt.dispatch_one().unwrap().is_none());
        rt.cancel("a1").unwrap();
        assert!(rt.dispatch_one().unwrap().is_some());
    }
    #[test]
    fn stale_heartbeat_and_expiry_are_safe() {
        let mut rt = runtime();
        seed(&mut rt, "d1", "a1", 1);
        rt.dispatch_one().unwrap();
        let beat = Heartbeat {
            attempt_id: "a1".into(),
            epoch: 2,
            stage: "work".into(),
            progress_percent: 10,
            budget_remaining: "ok".into(),
            evidence_refs: vec![],
        };
        rt.heartbeat(beat.clone()).unwrap();
        assert!(matches!(
            rt.heartbeat(beat),
            Err(DelegationRuntimeError::StaleHeartbeat)
        ));
        rt.clock.0.set(41);
        assert_eq!(rt.expire_leases().unwrap(), vec!["a1"]);
    }
    #[test]
    fn cancellation_and_recovery_never_replay() {
        let mut rt = runtime();
        seed(&mut rt, "d1", "a1", 1);
        rt.dispatch_one().unwrap();
        let report = rt.recover().unwrap();
        assert_eq!(report.sealed_unknown_effect_attempts, vec!["a1"]);
        assert!(rt.dispatch_one().unwrap().is_none());
    }
    fn completed_delivery() -> DelegationDelivery {
        DelegationDelivery {
            schema_version: 1,
            delivery_id: "delivery".into(),
            delivery_revision: 1,
            identity: super::super::delegation_contract::DelegationIdentity {
                session_id: "sd1".into(),
                message_id: "md1".into(),
                parent_run_id: "rd1".into(),
                delegation_id: "d1".into(),
                attempt_id: "a1".into(),
            },
            status: DeliveryStatus::Completed,
            executive_summary: "done".into(),
            key_facts: vec![KeyFact {
                statement: "fact".into(),
                confidence: Confidence::High,
                evidence_refs: vec![],
            }],
            milestones: vec![Milestone {
                label: "done".into(),
                outcome: "done".into(),
                evidence_refs: vec![],
            }],
            verifications: vec![VerificationRecord {
                item: "check".into(),
                method: "test".into(),
                status: VerificationStatus::Passed,
                conclusion: "ok".into(),
                evidence_ref: "evidence://test".into(),
            }],
            candidate_artifacts: vec![CandidateArtifact {
                kind: "file".into(),
                relative_ref: "x".into(),
                description: "x".into(),
                evidence_ref: None,
            }],
            open_questions: vec![],
            risks: vec![],
            evidence_refs: vec!["evidence://test".into()],
            usage: UsageFlags {
                context_truncated: false,
                output_truncated: false,
                budget_exhausted: false,
            },
        }
    }
    #[test]
    fn duplicate_completion_acceptance_is_rejected() {
        let mut rt = runtime();
        seed(&mut rt, "d1", "a1", 1);
        rt.dispatch_one().unwrap();
        let d = completed_delivery();
        rt.submit_delivery(&d, &ContractLimits::default()).unwrap();
        rt.accept_delivery(&d, &ContractLimits::default()).unwrap();
        assert!(matches!(
            rt.accept_delivery(&d, &ContractLimits::default()),
            Err(DelegationRuntimeError::DuplicateAcceptance)
        ));
    }
}
