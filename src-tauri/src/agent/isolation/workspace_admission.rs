//! Durable admission for delegated workspace work.
//!
//! Admission is deliberately a small control-plane module.  Work package,
//! attempt, and capability-lease rows remain the source of truth for policy
//! and identity; this table only records the short-lived concurrency lease.
//! The caller supplies the trusted `WorktreeCapability::collaboration_key` as
//! the workspace key.  The module never derives a key from a child path and
//! never copies policy facts from [`super::policy_lease_issuer`].

use std::{collections::HashMap, sync::Mutex};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use uuid::Uuid;

/// The global delegated-attempt bound.  This intentionally matches the
/// foreground policy lease issuer's capacity, while keeping the admission
/// decision durable and race-safe.
pub const MAX_ACTIVE_ADMISSIONS: usize = super::policy_lease_issuer::MAX_RUNNING_ATTEMPTS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAdmissionMode {
    Read,
    Review,
    Write,
    Materialize,
}

impl WorkspaceAdmissionMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Review => "review",
            Self::Write => "write",
            Self::Materialize => "materialize",
        }
    }

    fn is_writer(self) -> bool {
        matches!(self, Self::Write | Self::Materialize)
    }
}

/// Short request accepted by the admission boundary.  `workspace_key` must
/// be copied from the trusted worktree capability's `collaboration_key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAdmissionRequest {
    pub workspace_key: String,
    pub work_package_id: String,
    pub attempt_id: String,
    pub lease_id: String,
    pub mode: WorkspaceAdmissionMode,
    pub expires_at: i64,
}

impl WorkspaceAdmissionRequest {
    pub fn new(
        workspace_key: impl Into<String>,
        work_package_id: impl Into<String>,
        attempt_id: impl Into<String>,
        lease_id: impl Into<String>,
        mode: WorkspaceAdmissionMode,
        expires_at: i64,
    ) -> Self {
        Self {
            workspace_key: workspace_key.into(),
            work_package_id: work_package_id.into(),
            attempt_id: attempt_id.into(),
            lease_id: lease_id.into(),
            mode,
            expires_at,
        }
    }

    /// Validate/use a Main-Agent-issued capability without introducing a
    /// second workspace identity representation at this boundary.
    pub fn from_capability(
        capability: &super::workspace_isolation::WorktreeCapability,
        work_package_id: impl Into<String>,
        attempt_id: impl Into<String>,
        lease_id: impl Into<String>,
        expires_at: i64,
    ) -> Self {
        let mode = match capability.operation {
            super::workspace_isolation::WorktreeOperation::Read => WorkspaceAdmissionMode::Read,
            super::workspace_isolation::WorktreeOperation::Write => WorkspaceAdmissionMode::Write,
        };
        Self::new(
            capability.collaboration_key.clone(),
            work_package_id,
            attempt_id,
            lease_id,
            mode,
            expires_at,
        )
    }
}

/// A lease returned by [`WorkspaceAdmission::acquire`].  `state_version` is
/// the CAS token used by release; callers should retain this value and avoid
/// fabricating it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAdmissionLease {
    pub id: String,
    pub workspace_key: String,
    pub work_package_id: String,
    pub attempt_id: String,
    pub lease_id: String,
    pub mode: WorkspaceAdmissionMode,
    pub state_version: i64,
    pub expires_at: i64,
    pub status: WorkspaceAdmissionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAdmissionStatus {
    Active,
    Released,
    Expired,
    Revoked,
}

impl WorkspaceAdmissionStatus {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "released" => Some(Self::Released),
            "expired" => Some(Self::Expired),
            "revoked" => Some(Self::Revoked),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceAdmissionError {
    #[error("workspace admission request is invalid")]
    InvalidRequest,
    #[error("workspace admission lease has expired")]
    Expired,
    #[error("workspace admission capacity is exhausted")]
    Capacity,
    #[error("another writer already owns this workspace")]
    WriterConflict,
    #[error("attempt already has a different active admission")]
    AttemptConflict,
    #[error("workspace admission was not found")]
    NotFound,
    #[error("workspace admission storage failed: {0}")]
    Storage(String),
}

type AdmissionResult<T> = Result<T, WorkspaceAdmissionError>;

/// Concise aliases for scheduler-facing call sites.
pub type AdmissionMode = WorkspaceAdmissionMode;
pub type AdmissionRequest = WorkspaceAdmissionRequest;
pub type AdmissionLease = WorkspaceAdmissionLease;
pub type AdmissionError = WorkspaceAdmissionError;

/// SQLite-backed admission repository.  A value carries only the configured
/// capacity; all active leases and uniqueness are durable in SQLite.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceAdmission {
    max_active: usize,
}

impl Default for WorkspaceAdmission {
    fn default() -> Self {
        Self {
            max_active: MAX_ACTIVE_ADMISSIONS,
        }
    }
}

impl WorkspaceAdmission {
    pub const fn new() -> Self {
        Self {
            max_active: MAX_ACTIVE_ADMISSIONS,
        }
    }

    pub const fn with_max_active(max_active: usize) -> Self {
        Self { max_active }
    }

    /// Acquire in one `BEGIN IMMEDIATE` transaction.  Repeating the same
    /// attempt while its row is active returns the existing lease (idempotent
    /// retry), even when the caller generated a new request id.
    pub fn acquire(
        &self,
        conn: &mut Connection,
        request: WorkspaceAdmissionRequest,
        now: i64,
    ) -> AdmissionResult<WorkspaceAdmissionLease> {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;

        let lease = self.acquire_in_tx(&tx, request, now)?;
        tx.commit().map_err(storage)?;
        Ok(lease)
    }

    /// Acquire while the caller owns the composition transaction.  This is
    /// used by issuance after the attempt and capability-lease rows have been
    /// inserted but before the transaction becomes visible.  Keeping the
    /// admission insert in that same transaction prevents a durable attempt
    /// from becoming runnable without its concurrency lease, and avoids the
    /// foreign-key race of opening a nested transaction.
    pub fn acquire_in_tx(
        &self,
        tx: &Transaction<'_>,
        request: WorkspaceAdmissionRequest,
        now: i64,
    ) -> AdmissionResult<WorkspaceAdmissionLease> {
        validate_request(&request, now)?;

        expire_tx(&tx, now)?;

        if let Some(existing) = load_active_for_attempt(&tx, &request.attempt_id)? {
            if existing.workspace_key == request.workspace_key
                && existing.work_package_id == request.work_package_id
                && existing.lease_id == request.lease_id
                && existing.mode == request.mode
            {
                return Ok(existing);
            }
            return Err(WorkspaceAdmissionError::AttemptConflict);
        }

        if request.mode.is_writer() {
            let writer: Option<String> = tx
                .query_row(
                    "SELECT id FROM workspace_admissions
                     WHERE workspace_key=?1 AND status='active'
                       AND mode IN ('write','materialize') LIMIT 1",
                    params![request.workspace_key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            if writer.is_some() {
                return Err(WorkspaceAdmissionError::WriterConflict);
            }
        }

        let active: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM workspace_admissions WHERE status='active'",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if active >= self.max_active as i64 {
            return Err(WorkspaceAdmissionError::Capacity);
        }

        let id = format!("wa-{}", Uuid::new_v4());
        tx.execute(
            "INSERT INTO workspace_admissions
                (id, workspace_key, work_package_id, attempt_id, lease_id,
                 mode, status, state_version, expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 0, ?7, ?8)",
            params![
                id,
                request.workspace_key,
                request.work_package_id,
                request.attempt_id,
                request.lease_id,
                request.mode.as_str(),
                request.expires_at,
                now
            ],
        )
        .map_err(map_insert_error)?;
        let lease = load_by_id(&tx, &id)?.ok_or(WorkspaceAdmissionError::NotFound)?;
        Ok(lease)
    }

    /// Convenience spelling that makes capability reuse explicit at call
    /// sites; all checks still happen in `acquire`.
    pub fn acquire_with_capability(
        &self,
        conn: &mut Connection,
        capability: &super::workspace_isolation::WorktreeCapability,
        work_package_id: impl Into<String>,
        attempt_id: impl Into<String>,
        lease_id: impl Into<String>,
        expires_at: i64,
        now: i64,
    ) -> AdmissionResult<WorkspaceAdmissionLease> {
        self.acquire(
            conn,
            WorkspaceAdmissionRequest::from_capability(
                capability,
                work_package_id,
                attempt_id,
                lease_id,
                expires_at,
            ),
            now,
        )
    }

    /// Release with a compare-and-set on id and state version.  A repeated
    /// release is harmless and returns `false`; a stale lease cannot release
    /// a renewed/recovered row.
    pub fn release(
        &self,
        conn: &mut Connection,
        lease: &WorkspaceAdmissionLease,
        now: i64,
    ) -> AdmissionResult<bool> {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let changed = tx
            .execute(
                "UPDATE workspace_admissions
                 SET status='released', released_at=?1, state_version=state_version+1
                 WHERE id=?2 AND state_version=?3 AND status='active'",
                params![now, lease.id, lease.state_version],
            )
            .map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(changed == 1)
    }

    pub fn release_id(
        &self,
        conn: &mut Connection,
        id: &str,
        expected_state_version: i64,
        now: i64,
    ) -> AdmissionResult<bool> {
        self.release(
            conn,
            &WorkspaceAdmissionLease {
                id: id.into(),
                workspace_key: String::new(),
                work_package_id: String::new(),
                attempt_id: String::new(),
                lease_id: String::new(),
                mode: WorkspaceAdmissionMode::Read,
                state_version: expected_state_version,
                expires_at: now + 1,
                status: WorkspaceAdmissionStatus::Active,
            },
            now,
        )
    }

    /// Release every active admission owned by an attempt using the durable
    /// id/state-version CAS. This is the terminal cleanup coordinator's
    /// single attempt-scoped entrypoint; repeated calls are harmless.
    pub fn release_attempt(
        &self,
        conn: &mut Connection,
        attempt_id: &str,
        now: i64,
    ) -> AdmissionResult<usize> {
        if attempt_id.trim().is_empty() {
            return Err(WorkspaceAdmissionError::InvalidRequest);
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let leases: Vec<(String, i64)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id, state_version FROM workspace_admissions
                     WHERE attempt_id=?1 AND status='active'",
                )
                .map_err(storage)?;
            let rows = stmt
                .query_map([attempt_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(storage)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(storage)?
        };
        let mut released = 0usize;
        for (id, state_version) in leases {
            released += tx
                .execute(
                    "UPDATE workspace_admissions
                     SET status='released', released_at=?1, state_version=state_version+1
                     WHERE id=?2 AND state_version=?3 AND attempt_id=?4 AND status='active'",
                    params![now, id, state_version, attempt_id],
                )
                .map_err(storage)?;
        }
        tx.commit().map_err(storage)?;
        Ok(released)
    }

    /// Mark all due active rows expired.  This is idempotent and can be run
    /// by startup recovery or a periodic scheduler.
    pub fn expire(&self, conn: &mut Connection, now: i64) -> AdmissionResult<usize> {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let count = expire_tx(&tx, now)?;
        tx.commit().map_err(storage)?;
        Ok(count)
    }

    pub fn recover(&self, conn: &mut Connection, now: i64) -> AdmissionResult<usize> {
        self.expire(conn, now)
    }
}

/// Stateless repository façade for callers that prefer the existing
/// `Repository::method(connection, ...)` style.  Capacity remains the single
/// policy-lease issuer bound of two.
pub struct WorkspaceAdmissionRepository;

impl WorkspaceAdmissionRepository {
    pub fn acquire(
        conn: &mut Connection,
        request: WorkspaceAdmissionRequest,
        now: i64,
    ) -> AdmissionResult<WorkspaceAdmissionLease> {
        WorkspaceAdmission::new().acquire(conn, request, now)
    }

    pub fn release(
        conn: &mut Connection,
        lease: &WorkspaceAdmissionLease,
        now: i64,
    ) -> AdmissionResult<bool> {
        WorkspaceAdmission::new().release(conn, lease, now)
    }

    pub fn expire(conn: &mut Connection, now: i64) -> AdmissionResult<usize> {
        WorkspaceAdmission::new().expire(conn, now)
    }

    pub fn recover(conn: &mut Connection, now: i64) -> AdmissionResult<usize> {
        Self::expire(conn, now)
    }
}

/// In-memory implementation used by isolated scheduler tests and lightweight
/// callers.  It mirrors the durable admission invariants but has no DB facts.
#[derive(Debug)]
pub struct InMemoryWorkspaceAdmission {
    max_active: usize,
    state: Mutex<InMemoryState>,
}

#[derive(Debug, Default)]
struct InMemoryState {
    active: HashMap<String, WorkspaceAdmissionLease>,
}

impl Default for InMemoryWorkspaceAdmission {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryWorkspaceAdmission {
    pub fn new() -> Self {
        Self {
            max_active: MAX_ACTIVE_ADMISSIONS,
            state: Mutex::new(InMemoryState::default()),
        }
    }

    pub fn with_max_active(max_active: usize) -> Self {
        Self {
            max_active,
            state: Mutex::new(InMemoryState::default()),
        }
    }

    pub fn acquire(
        &self,
        request: WorkspaceAdmissionRequest,
        now: i64,
    ) -> AdmissionResult<WorkspaceAdmissionLease> {
        validate_request(&request, now)?;
        let mut state = self.state.lock().map_err(|_| storage("poisoned lock"))?;
        expire_state(&mut state, now);
        if let Some(existing) = state
            .active
            .values()
            .find(|lease| lease.attempt_id == request.attempt_id)
            .cloned()
        {
            if existing.workspace_key == request.workspace_key
                && existing.work_package_id == request.work_package_id
                && existing.lease_id == request.lease_id
                && existing.mode == request.mode
            {
                return Ok(existing);
            }
            return Err(WorkspaceAdmissionError::AttemptConflict);
        }
        if request.mode.is_writer()
            && state
                .active
                .values()
                .any(|lease| lease.workspace_key == request.workspace_key && lease.mode.is_writer())
        {
            return Err(WorkspaceAdmissionError::WriterConflict);
        }
        if state.active.len() >= self.max_active {
            return Err(WorkspaceAdmissionError::Capacity);
        }
        let lease = WorkspaceAdmissionLease {
            id: format!("wa-{}", Uuid::new_v4()),
            workspace_key: request.workspace_key,
            work_package_id: request.work_package_id,
            attempt_id: request.attempt_id,
            lease_id: request.lease_id,
            mode: request.mode,
            state_version: 0,
            expires_at: request.expires_at,
            status: WorkspaceAdmissionStatus::Active,
        };
        state.active.insert(lease.id.clone(), lease.clone());
        Ok(lease)
    }

    pub fn release(&self, lease: &WorkspaceAdmissionLease, _now: i64) -> AdmissionResult<bool> {
        let mut state = self.state.lock().map_err(|_| storage("poisoned lock"))?;
        let Some(existing) = state.active.get(&lease.id) else {
            return Ok(false);
        };
        if existing.state_version != lease.state_version {
            return Ok(false);
        }
        state.active.remove(&lease.id);
        Ok(true)
    }

    pub fn expire(&self, now: i64) -> AdmissionResult<usize> {
        let mut state = self.state.lock().map_err(|_| storage("poisoned lock"))?;
        Ok(expire_state(&mut state, now))
    }

    pub fn recover(&self, now: i64) -> AdmissionResult<usize> {
        self.expire(now)
    }
}

fn validate_request(request: &WorkspaceAdmissionRequest, now: i64) -> AdmissionResult<()> {
    if request.workspace_key.trim().is_empty()
        || request.work_package_id.trim().is_empty()
        || request.attempt_id.trim().is_empty()
        || request.lease_id.trim().is_empty()
        || request.expires_at <= now
    {
        return Err(WorkspaceAdmissionError::InvalidRequest);
    }
    Ok(())
}

fn storage(error: impl std::fmt::Display) -> WorkspaceAdmissionError {
    WorkspaceAdmissionError::Storage(error.to_string())
}

fn map_insert_error(error: rusqlite::Error) -> WorkspaceAdmissionError {
    let text = error.to_string();
    if text.contains("writer_serial")
        || text.contains("UNIQUE constraint failed: workspace_admissions.workspace_key")
    {
        WorkspaceAdmissionError::WriterConflict
    } else if text.contains("workspace_admissions.attempt_id")
        || text.contains("idx_workspace_admissions_attempt_active")
    {
        WorkspaceAdmissionError::AttemptConflict
    } else {
        storage(error)
    }
}

fn expire_tx(tx: &Transaction<'_>, now: i64) -> AdmissionResult<usize> {
    tx.execute(
        "UPDATE workspace_admissions
         SET status='expired', released_at=expires_at, state_version=state_version+1
         WHERE status='active' AND expires_at<=?1",
        params![now],
    )
    .map_err(storage)
}

fn load_active_for_attempt(
    tx: &Transaction<'_>,
    attempt_id: &str,
) -> AdmissionResult<Option<WorkspaceAdmissionLease>> {
    tx.query_row(
        "SELECT id, workspace_key, work_package_id, attempt_id, lease_id,
                mode, state_version, expires_at, status
         FROM workspace_admissions WHERE attempt_id=?1 AND status='active'",
        params![attempt_id],
        row_to_lease,
    )
    .optional()
    .map_err(storage)
}

fn load_by_id(tx: &Transaction<'_>, id: &str) -> AdmissionResult<Option<WorkspaceAdmissionLease>> {
    tx.query_row(
        "SELECT id, workspace_key, work_package_id, attempt_id, lease_id,
                mode, state_version, expires_at, status
         FROM workspace_admissions WHERE id=?1",
        params![id],
        row_to_lease,
    )
    .optional()
    .map_err(storage)
}

fn row_to_lease(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceAdmissionLease> {
    let mode: String = row.get(5)?;
    let status: String = row.get(8)?;
    let mode = match mode.as_str() {
        "read" => WorkspaceAdmissionMode::Read,
        "review" => WorkspaceAdmissionMode::Review,
        "write" => WorkspaceAdmissionMode::Write,
        "materialize" => WorkspaceAdmissionMode::Materialize,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let status =
        WorkspaceAdmissionStatus::from_str(&status).ok_or(rusqlite::Error::InvalidQuery)?;
    Ok(WorkspaceAdmissionLease {
        id: row.get(0)?,
        workspace_key: row.get(1)?,
        work_package_id: row.get(2)?,
        attempt_id: row.get(3)?,
        lease_id: row.get(4)?,
        mode,
        state_version: row.get(6)?,
        expires_at: row.get(7)?,
        status,
    })
}

fn expire_state(state: &mut InMemoryState, now: i64) -> usize {
    let expired: Vec<String> = state
        .active
        .values()
        .filter(|lease| lease.expires_at <= now)
        .map(|lease| lease.id.clone())
        .collect();
    let count = expired.len();
    for id in expired {
        state.active.remove(&id);
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn request(
        workspace: &str,
        attempt: &str,
        mode: WorkspaceAdmissionMode,
    ) -> WorkspaceAdmissionRequest {
        WorkspaceAdmissionRequest::new(
            workspace,
            "pkg",
            attempt,
            &format!("lease-{attempt}"),
            mode,
            100,
        )
    }

    #[test]
    fn in_memory_reads_parallel_but_writers_are_serial() {
        let admission = InMemoryWorkspaceAdmission::new();
        let read_a = admission
            .acquire(request("w", "a", WorkspaceAdmissionMode::Read), 1)
            .unwrap();
        let read_b = admission
            .acquire(request("w", "b", WorkspaceAdmissionMode::Review), 1)
            .unwrap();
        assert_ne!(read_a.id, read_b.id);
        assert_eq!(
            admission.acquire(request("w", "c", WorkspaceAdmissionMode::Write), 1),
            Err(WorkspaceAdmissionError::Capacity)
        );
        admission.release(&read_a, 2).unwrap();
        admission.release(&read_b, 2).unwrap();
        let writer = admission
            .acquire(request("w", "c", WorkspaceAdmissionMode::Write), 2)
            .unwrap();
        assert_eq!(
            admission.acquire(request("w", "d", WorkspaceAdmissionMode::Write), 2),
            Err(WorkspaceAdmissionError::WriterConflict)
        );
        admission.release(&writer, 3).unwrap();
    }

    #[test]
    fn in_memory_is_idempotent_and_expires() {
        let admission = InMemoryWorkspaceAdmission::new();
        let mut first_request = request("w", "a", WorkspaceAdmissionMode::Write);
        first_request.expires_at = 100;
        let first = admission.acquire(first_request, 1).unwrap();
        let mut retry_request = request("w", "a", WorkspaceAdmissionMode::Write);
        retry_request.expires_at = 100;
        let again = admission.acquire(retry_request, 2).unwrap();
        assert_eq!(first, again);
        assert_eq!(admission.expire(100).unwrap(), 1);
        let mut expired_request = request("w", "b", WorkspaceAdmissionMode::Read);
        expired_request.expires_at = 100;
        assert!(admission.acquire(expired_request, 100).is_err());
        let mut fresh_request = request("w", "b", WorkspaceAdmissionMode::Read);
        fresh_request.expires_at = 101;
        assert!(admission.acquire(fresh_request, 100).is_ok());
    }

    #[test]
    fn durable_admission_uses_cas_and_global_capacity() {
        let mut conn = Connection::open_in_memory().unwrap();
        db::migrate(&conn).unwrap();
        // The admission table's triggers require authoritative rows.  Build a
        // minimal fixture using the existing v41 schema's parent records.
        conn.execute(
            "INSERT INTO sessions(id,title,created_at,updated_at) VALUES ('s','s',1,1)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('m','s','user','x',1)", []).unwrap();
        conn.execute("INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES ('r','s','m','goal','running','{}',1,1)", []).unwrap();
        conn.execute("INSERT INTO work_packages(id,session_id,owner_profile_id,workspace_key,task_shape,scope_digest,capability_scope_ref,capability_expires_at,candidate_version,created_at,updated_at) VALUES ('pkg','s',1,'w','explore','d','scope',1000,0,1,1)", []).unwrap();
        conn.execute("INSERT INTO delegations(id,session_id,message_id,parent_run_id,work_package_id,objective,brief_json,status,state_version,created_at,updated_at) VALUES ('d','s','m','r','pkg','x','{}','queued',0,1,1)", []).unwrap();
        conn.execute("INSERT INTO delegation_attempts(id,delegation_id,attempt_number,status,sandbox_ref,created_at) VALUES ('a','d',1,'queued','sandbox',1)", []).unwrap();
        conn.execute("INSERT INTO delegation_capability_leases(id,attempt_id,read_roots_json,write_roots_json,tool_allowlist_json,network_hosts_json,budget_json,status,issued_at,expires_at) VALUES ('l','a','[]','[]','[]','[]','{}','active',1,1000)", []).unwrap();
        let admission = WorkspaceAdmission::new();
        let lease = admission
            .acquire(
                &mut conn,
                WorkspaceAdmissionRequest::new(
                    "w",
                    "pkg",
                    "a",
                    "l",
                    WorkspaceAdmissionMode::Read,
                    100,
                ),
                2,
            )
            .unwrap();
        assert_eq!(
            admission
                .release_id(&mut conn, &lease.id, lease.state_version, 3)
                .unwrap(),
            true
        );
    }
}
