//! Main-agent-only issuance of a concrete, attempt-bound gateway lease.
//!
//! This is the single seam between an immutable WorkPackage policy and the
//! durable delegation lease. It deliberately allocates no sandbox, starts no
//! worker, and has no materialization operation.

use std::{collections::BTreeSet, path::PathBuf};

use super::{
    delegation::{
        DelegationRepository, NewAttempt, NewCapabilityLease, NewDelegation, RetryQueueOutcome,
    },
    execution_gateway::{CapabilityGrant, CapabilityLease},
    work_package::{WorkPackage, WorkPackageScope, WorkPackageStatus},
    worker_policy::{CapabilityPolicy, NetworkAction, WORKER_POLICY_VERSION},
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

pub const MAX_RUNNING_ATTEMPTS: usize = 2;

#[derive(Debug, Clone)]
pub struct LeasePlan {
    pub policy: CapabilityPolicy,
    pub capability_scope_ref: String,
    pub max_expires_at: i64,
    pub allowed_tools: BTreeSet<String>,
    pub allowed_network_hosts: BTreeSet<String>,
}

/// Supplied by Main Agent only after it has allocated an existing sandbox.
/// All paths are sandbox paths; this type intentionally has no real-workspace
/// write or materialization field.
#[derive(Debug, Clone)]
pub struct AttemptAllocation {
    pub delegation: NewDelegation,
    pub attempt: NewAttempt,
    pub lease_id: String,
    pub capability_scope_ref: String,
    pub expires_at: i64,
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
    pub tool_allowlist: BTreeSet<String>,
    pub network_hosts: BTreeSet<String>,
    pub budget_json: String,
}

/// Retry-specific facts which cannot be selected by the foreground model.
/// The WorkPackage and model binding remain immutable; only the execution
/// identities below are fresh for this attempt.
#[derive(Debug, Clone)]
pub struct RetryAttemptAllocation {
    pub delegation_id: String,
    pub session_id: String,
    pub work_package_id: String,
    pub source_delivery_id: String,
    pub source_delivery_revision: u32,
    pub source_attempt_id: String,
    pub reason: String,
    /// The original compact brief with only execution identity and bounded
    /// retry instruction updated.  It becomes the current delegation brief
    /// atomically with the fresh attempt, so worker deliveries validate
    /// against the new attempt id.
    pub retry_brief_json: String,
    pub attempt: NewAttempt,
    pub lease_id: String,
    pub capability_scope_ref: String,
    pub expires_at: i64,
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
    pub tool_allowlist: BTreeSet<String>,
    pub network_hosts: BTreeSet<String>,
    pub budget_json: String,
}

#[derive(Debug, Clone)]
pub struct IssuedLease {
    pub lease: CapabilityLease,
    pub work_package_id: String,
}

#[derive(Debug, Clone)]
pub enum RetryIssuedLease {
    Issued(IssuedLease),
    Existing {
        attempt_id: String,
        work_package_id: String,
    },
}

struct LeaseAllocation<'a> {
    attempt: &'a NewAttempt,
    lease_id: &'a str,
    capability_scope_ref: &'a str,
    expires_at: i64,
    read_roots: &'a [PathBuf],
    write_roots: &'a [PathBuf],
    tool_allowlist: &'a BTreeSet<String>,
    network_hosts: &'a BTreeSet<String>,
    budget_json: &'a str,
}

impl<'a> From<&'a AttemptAllocation> for LeaseAllocation<'a> {
    fn from(value: &'a AttemptAllocation) -> Self {
        Self {
            attempt: &value.attempt,
            lease_id: &value.lease_id,
            capability_scope_ref: &value.capability_scope_ref,
            expires_at: value.expires_at,
            read_roots: &value.read_roots,
            write_roots: &value.write_roots,
            tool_allowlist: &value.tool_allowlist,
            network_hosts: &value.network_hosts,
            budget_json: &value.budget_json,
        }
    }
}

impl<'a> From<&'a RetryAttemptAllocation> for LeaseAllocation<'a> {
    fn from(value: &'a RetryAttemptAllocation) -> Self {
        Self {
            attempt: &value.attempt,
            lease_id: &value.lease_id,
            capability_scope_ref: &value.capability_scope_ref,
            expires_at: value.expires_at,
            read_roots: &value.read_roots,
            write_roots: &value.write_roots,
            tool_allowlist: &value.tool_allowlist,
            network_hosts: &value.network_hosts,
            budget_json: &value.budget_json,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyLeaseError {
    #[error("work package scope, status, or immutable policy does not match")]
    ScopeOrPolicyMismatch,
    #[error("allocation widens the immutable policy or plan")]
    WideningRejected,
    #[error("allocation is expired or exceeds its capability scope")]
    Expired,
    #[error("concurrency admission denied")]
    AdmissionDenied,
    #[error("invalid capability grant: {0:?}")]
    Grant(super::execution_gateway::CapabilityGrantError),
    #[error("durable delegation persistence failed: {0}")]
    Storage(String),
}

/// A deep module: callers supply immutable policy plus an allocation; all
/// scope checks, subset checks, durable issue facts and fail-closed decisions
/// live here rather than in a future WorkerAdapter.
pub struct PolicyLeaseIssuer;
impl PolicyLeaseIssuer {
    pub fn issue(
        conn: &mut Connection,
        scope: &WorkPackageScope,
        package: &WorkPackage,
        plan: &LeasePlan,
        allocation: AttemptAllocation,
        now: i64,
    ) -> Result<IssuedLease, PolicyLeaseError> {
        Self::issue_with_after(conn, scope, package, plan, allocation, now, |_| Ok(()))
    }

    /// Issues the lease and executes one typed post-insert fact callback in
    /// the same delegation/attempt/lease/outbox transaction.  The callback is
    /// intentionally not a general queue hook: it cannot start workers or
    /// commit independently.
    pub fn issue_with_after<F>(
        conn: &mut Connection,
        scope: &WorkPackageScope,
        package: &WorkPackage,
        plan: &LeasePlan,
        allocation: AttemptAllocation,
        now: i64,
        after_create: F,
    ) -> Result<IssuedLease, PolicyLeaseError>
    where
        F: FnOnce(&Transaction<'_>) -> Result<(), PolicyLeaseError>,
    {
        Self::validate_package(conn, scope, package, plan, &allocation, now)?;
        Self::ensure_capacity(conn)?;
        let lease_input = LeaseAllocation::from(&allocation);
        Self::validate_allocation_ceiling(plan, &lease_input)?;
        let (lease, durable) = Self::build_lease(&lease_input)?;
        let policy_event = Self::policy_event(package);
        DelegationRepository::create_queued_with(
            conn,
            &allocation.delegation,
            &allocation.attempt,
            &durable,
            now,
            |tx| {
                after_create(tx).map_err(|error| error.to_string())?;
                DelegationRepository::append_attempt_event_in_tx(
                    tx,
                    &lease.attempt_id,
                    2,
                    "policy_lease_issued",
                    &policy_event,
                    now,
                )
                .map_err(|error| error.to_string())
            },
        )
        .map_err(PolicyLeaseError::Storage)?;
        Ok(IssuedLease {
            lease,
            work_package_id: package.id.clone(),
        })
    }

    /// Reissues a lease for the sole retry successor of a submitted delivery.
    /// It shares the immutable WorkPackage policy and concrete capability
    /// checks with initial issuance, but queues through the retry lineage
    /// transaction so the old result is superseded before the new attempt can
    /// dispatch.
    pub fn reissue_with_after<F>(
        conn: &mut Connection,
        scope: &WorkPackageScope,
        package: &WorkPackage,
        plan: &LeasePlan,
        allocation: RetryAttemptAllocation,
        now: i64,
        after_create: F,
    ) -> Result<RetryIssuedLease, PolicyLeaseError>
    where
        F: FnOnce(&Transaction<'_>) -> Result<(), PolicyLeaseError>,
    {
        if let Some(attempt_id) = DelegationRepository::retry_attempt_for_source(
            conn,
            &allocation.delegation_id,
            &allocation.source_delivery_id,
        )
        .map_err(PolicyLeaseError::Storage)?
        {
            return Ok(RetryIssuedLease::Existing {
                attempt_id,
                work_package_id: package.id.clone(),
            });
        }
        Self::validate_retry_package(conn, scope, package, plan, &allocation, now)?;
        Self::ensure_capacity(conn)?;
        let lease_input = LeaseAllocation::from(&allocation);
        Self::validate_allocation_ceiling(plan, &lease_input)?;
        let (lease, durable) = Self::build_lease(&lease_input)?;
        let policy_event = Self::policy_event(package);
        let result = DelegationRepository::queue_retry_with(
            conn,
            &allocation.delegation_id,
            &allocation.source_delivery_id,
            allocation.source_delivery_revision,
            &allocation.source_attempt_id,
            &allocation.reason,
            &allocation.retry_brief_json,
            &allocation.attempt,
            &durable,
            now,
            |tx| {
                after_create(tx).map_err(|error| error.to_string())?;
                DelegationRepository::append_attempt_event_in_tx(
                    tx,
                    &lease.attempt_id,
                    3,
                    "policy_lease_issued",
                    &policy_event,
                    now,
                )
                .map_err(|error| error.to_string())
            },
        )
        .map_err(PolicyLeaseError::Storage)?;
        Ok(match result {
            RetryQueueOutcome::Queued => RetryIssuedLease::Issued(IssuedLease {
                lease,
                work_package_id: package.id.clone(),
            }),
            RetryQueueOutcome::AlreadyQueued { attempt_id } => RetryIssuedLease::Existing {
                attempt_id,
                work_package_id: package.id.clone(),
            },
        })
    }

    /// Renewal is a decision, not a mutation: a runtime must persist it only
    /// after this revalidates all immutable bindings and a fresh heartbeat.
    pub fn may_renew(
        package: &WorkPackage,
        plan: &LeasePlan,
        requested_expiry: i64,
        now: i64,
    ) -> bool {
        package.worker_policy_version == WORKER_POLICY_VERSION
            && plan.policy.validate().is_ok()
            && plan.policy.task_shape == package.task_shape
            && plan.policy.worker_profile == package.worker_profile
            && plan.capability_scope_ref == package.capability_scope_ref
            && requested_expiry > now
            && requested_expiry <= package.capability_expires_at
            && requested_expiry <= plan.max_expires_at
    }
    pub fn revoke(conn: &Connection, attempt_id: &str, now: i64) -> Result<bool, PolicyLeaseError> {
        let changed=conn.execute("UPDATE delegation_capability_leases SET status='revoked',revoked_at=?1 WHERE attempt_id=?2 AND status='active'",params![now,attempt_id]).map_err(storage)?;
        Ok(changed == 1)
    }

    fn ensure_capacity(conn: &Connection) -> Result<(), PolicyLeaseError> {
        let running: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM delegation_attempts WHERE status IN ('queued','running')",
                [],
                |r| r.get(0),
            )
            .map_err(storage)?;
        if running >= MAX_RUNNING_ATTEMPTS as i64 {
            Err(PolicyLeaseError::AdmissionDenied)
        } else {
            Ok(())
        }
    }

    fn validate_allocation_ceiling(
        plan: &LeasePlan,
        allocation: &LeaseAllocation<'_>,
    ) -> Result<(), PolicyLeaseError> {
        let policy_tools = allowed_tools_for_policy(&plan.policy);
        if allocation.capability_scope_ref != plan.capability_scope_ref {
            return Err(PolicyLeaseError::ScopeOrPolicyMismatch);
        }
        if !allocation.tool_allowlist.is_subset(&plan.allowed_tools)
            || !allocation.tool_allowlist.is_subset(&policy_tools)
            || !allocation
                .network_hosts
                .is_subset(&plan.allowed_network_hosts)
            || (!plan.policy.candidate_write && !allocation.write_roots.is_empty())
        {
            return Err(PolicyLeaseError::WideningRejected);
        }
        Ok(())
    }

    fn build_lease(
        allocation: &LeaseAllocation<'_>,
    ) -> Result<(CapabilityLease, NewCapabilityLease), PolicyLeaseError> {
        let grant = CapabilityGrant {
            id: allocation.lease_id.to_owned(),
            attempt_id: allocation.attempt.id.clone(),
            status: super::delegation::LeaseStatus::Active,
            expires_at_ms: allocation
                .expires_at
                .checked_mul(1000)
                .ok_or(PolicyLeaseError::Expired)?,
            read_roots: allocation.read_roots.to_vec(),
            write_roots: allocation.write_roots.to_vec(),
            tool_allowlist: allocation.tool_allowlist.iter().cloned().collect(),
            network_hosts: allocation.network_hosts.iter().cloned().collect(),
        };
        let lease = CapabilityLease::from_grant(grant).map_err(PolicyLeaseError::Grant)?;
        let durable = NewCapabilityLease {
            id: lease.id.clone(),
            read_roots_json: json_paths(allocation.read_roots)?,
            write_roots_json: json_paths(allocation.write_roots)?,
            tool_allowlist_json: json_set(allocation.tool_allowlist)?,
            network_hosts_json: json_set(allocation.network_hosts)?,
            budget_json: allocation.budget_json.to_owned(),
            expires_at: allocation.expires_at,
        };
        Ok((lease, durable))
    }

    fn policy_event(package: &WorkPackage) -> String {
        serde_json::json!({
            "work_package_id": package.id,
            "scope_digest": package.scope_digest,
            "capability_scope_ref": package.capability_scope_ref,
            "worker_policy_version": package.worker_policy_version,
            "task_shape": format!("{:?}", package.task_shape),
        })
        .to_string()
    }

    fn validate_package(
        conn: &Connection,
        scope: &WorkPackageScope,
        package: &WorkPackage,
        plan: &LeasePlan,
        allocation: &AttemptAllocation,
        now: i64,
    ) -> Result<(), PolicyLeaseError> {
        Self::validate_package_claims(
            conn,
            scope,
            package,
            plan,
            &allocation.delegation.session_id,
            allocation.delegation.work_package_id.as_deref(),
            &allocation.capability_scope_ref,
            allocation.expires_at,
            now,
        )
    }

    fn validate_retry_package(
        conn: &Connection,
        scope: &WorkPackageScope,
        package: &WorkPackage,
        plan: &LeasePlan,
        allocation: &RetryAttemptAllocation,
        now: i64,
    ) -> Result<(), PolicyLeaseError> {
        let parent: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT session_id, work_package_id FROM delegations WHERE id = ?1",
                [&allocation.delegation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(storage)?;
        let Some((session_id, work_package_id)) = parent else {
            return Err(PolicyLeaseError::ScopeOrPolicyMismatch);
        };
        if session_id != allocation.session_id
            || allocation.work_package_id != package.id
            || work_package_id.as_deref() != Some(package.id.as_str())
        {
            return Err(PolicyLeaseError::ScopeOrPolicyMismatch);
        }
        Self::validate_package_claims(
            conn,
            scope,
            package,
            plan,
            &session_id,
            work_package_id.as_deref(),
            &allocation.capability_scope_ref,
            allocation.expires_at,
            now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_package_claims(
        conn: &Connection,
        scope: &WorkPackageScope,
        package: &WorkPackage,
        plan: &LeasePlan,
        delegation_session_id: &str,
        delegation_work_package_id: Option<&str>,
        capability_scope_ref: &str,
        expires_at: i64,
        now: i64,
    ) -> Result<(), PolicyLeaseError> {
        let exists:Option<i64>=conn.query_row("SELECT 1 FROM work_packages WHERE id=?1 AND session_id=?2 AND owner_profile_id=?3 AND workspace_key=?4",params![package.id,scope.session_id,scope.owner_profile_id,scope.workspace_key],|r|r.get(0)).optional().map_err(storage)?;
        if exists.is_none()
            || package.status != WorkPackageStatus::Active
            || package.scope != *scope
            || delegation_session_id != scope.session_id
            || delegation_work_package_id != Some(package.id.as_str())
            || package.worker_policy_version != WORKER_POLICY_VERSION
            || plan.policy.validate().is_err()
            || plan.policy.task_shape != package.task_shape
            || plan.policy.worker_profile != package.worker_profile
            || plan.capability_scope_ref != package.capability_scope_ref
            || capability_scope_ref != package.capability_scope_ref
        {
            return Err(PolicyLeaseError::ScopeOrPolicyMismatch);
        }
        if expires_at <= now
            || expires_at > package.capability_expires_at
            || expires_at > plan.max_expires_at
        {
            return Err(PolicyLeaseError::Expired);
        }
        Ok(())
    }
}

pub(crate) fn allowed_tools_for_policy(p: &CapabilityPolicy) -> BTreeSet<String> {
    let mut out = BTreeSet::from(["file.read".into()]);
    if p.candidate_write {
        out.insert("file.write_candidate".into());
    }
    if p.network_actions.contains(&NetworkAction::Search) {
        out.insert("network.search".into());
    }
    if p.network_actions.contains(&NetworkAction::Fetch) {
        out.insert("network.fetch".into());
    }
    if p.network_actions.contains(&NetworkAction::Registry) {
        out.insert("network.registry".into());
    }
    if !p.local_execution.is_empty() {
        out.insert("local.build".into());
        out.insert("local.test".into());
    }
    out
}
fn json_set(s: &BTreeSet<String>) -> Result<String, PolicyLeaseError> {
    serde_json::to_string(s).map_err(|e| PolicyLeaseError::Storage(e.to_string()))
}
// CapabilityLease deliberately keeps canonical roots private. Persist the supplied
// roots only after `from_grant` validates them; no caller can bypass Gateway.
fn json_paths(paths: &[PathBuf]) -> Result<String, PolicyLeaseError> {
    serde_json::to_string(
        &paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>(),
    )
    .map_err(|e| PolicyLeaseError::Storage(e.to_string()))
}
fn storage(e: rusqlite::Error) -> PolicyLeaseError {
    PolicyLeaseError::Storage(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        work_package::{NewWorkPackage, WorkPackageRepository},
        worker_policy::{TaskShape, WorkerProfile},
    };
    use rusqlite::Connection;
    fn scope() -> WorkPackageScope {
        WorkPackageScope {
            session_id: "s".into(),
            owner_profile_id: 1,
            workspace_key: "wk".into(),
        }
    }
    fn setup() -> (Connection, WorkPackage, tempfile::TempDir) {
        let mut c = Connection::open_in_memory().unwrap();
        crate::db::migrate(&c).unwrap();
        c.execute(
            "INSERT INTO sessions(id,title,created_at,updated_at)VALUES('s','s',1,1)",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO messages(id,session_id,role,content,created_at)VALUES('m','s','user','x',1)",[]).unwrap();
        c.execute("INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at)VALUES('r','s','m','x','running','[]',1,1)",[]).unwrap();
        let p = WorkPackageRepository::create(
            &mut c,
            NewWorkPackage {
                scope: scope(),
                task_shape: TaskShape::Change,
                worker_profile: WorkerProfile::Implementer,
                scope_digest: "digest".into(),
                capability_scope_ref: "cap".into(),
                capability_expires_at: 1000,
            },
        )
        .unwrap();
        (c, p, tempfile::tempdir().unwrap())
    }
    fn plan(p: &WorkPackage) -> LeasePlan {
        let policy = CapabilityPolicy::compile(p.task_shape, p.worker_profile);
        LeasePlan {
            policy,
            capability_scope_ref: "cap".into(),
            max_expires_at: 900,
            allowed_tools: BTreeSet::from([
                "file.read".into(),
                "file.write_candidate".into(),
                "network.fetch".into(),
                "network.registry".into(),
                "local.build".into(),
                "local.test".into(),
            ]),
            allowed_network_hosts: BTreeSet::from(["docs.example".into()]),
        }
    }
    fn allocation(root: &std::path::Path, work_package_id: &str) -> AttemptAllocation {
        AttemptAllocation {
            delegation: NewDelegation {
                id: "d".into(),
                session_id: "s".into(),
                message_id: "m".into(),
                parent_run_id: "r".into(),
                work_package_id: Some(work_package_id.into()),
                idempotency_key: None,
                objective: "o".into(),
                brief_json: "{}".into(),
            },
            attempt: NewAttempt {
                id: "a".into(),
                attempt_number: 1,
                sandbox_ref: "sandbox:a".into(),
                outbox_id: "o".into(),
                dispatch_payload_json: "{}".into(),
            },
            lease_id: "l".into(),
            capability_scope_ref: "cap".into(),
            expires_at: 100,
            read_roots: vec![root.into()],
            write_roots: vec![root.into()],
            tool_allowlist: BTreeSet::from(["file.read".into(), "file.write_candidate".into()]),
            network_hosts: BTreeSet::new(),
            budget_json: "{}".into(),
        }
    }
    #[test]
    fn scope_mismatch_and_widening_fail_closed() {
        let (mut c, p, tmp) = setup();
        let mut a = allocation(tmp.path(), &p.id);
        a.capability_scope_ref = "other".into();
        assert!(matches!(
            PolicyLeaseIssuer::issue(&mut c, &scope(), &p, &plan(&p), a, 1),
            Err(PolicyLeaseError::ScopeOrPolicyMismatch)
        ));
        let mut a = allocation(tmp.path(), &p.id);
        a.tool_allowlist.insert("materialize.workspace".into());
        assert!(matches!(
            PolicyLeaseIssuer::issue(&mut c, &scope(), &p, &plan(&p), a, 1),
            Err(PolicyLeaseError::WideningRejected)
        ));
    }
    #[test]
    fn expiry_uniqueness_revocation_and_admission_are_durable() {
        let (mut c, p, tmp) = setup();
        let issued = PolicyLeaseIssuer::issue(
            &mut c,
            &scope(),
            &p,
            &plan(&p),
            allocation(tmp.path(), &p.id),
            1,
        )
        .unwrap();
        assert!(PolicyLeaseIssuer::revoke(&c, "a", 2).unwrap());
        assert!(!PolicyLeaseIssuer::revoke(&c, "a", 3).unwrap());
        assert!(!PolicyLeaseIssuer::may_renew(&p, &plan(&p), 1001, 1));
        assert_eq!(issued.work_package_id, p.id);
    }
    #[test]
    fn explore_is_hard_ceiling_even_for_implementer() {
        let (mut c, _, tmp) = setup();
        let p = WorkPackageRepository::create(
            &mut c,
            NewWorkPackage {
                scope: scope(),
                task_shape: TaskShape::Explore,
                worker_profile: WorkerProfile::Implementer,
                scope_digest: "e".into(),
                capability_scope_ref: "cap".into(),
                capability_expires_at: 1000,
            },
        )
        .unwrap();
        let mut plan = plan(&p);
        plan.policy = CapabilityPolicy::compile(TaskShape::Explore, WorkerProfile::Implementer);
        plan.allowed_tools = BTreeSet::from([
            "file.read".into(),
            "network.search".into(),
            "network.fetch".into(),
        ]);
        let a = allocation(tmp.path(), &p.id);
        assert!(matches!(
            PolicyLeaseIssuer::issue(&mut c, &scope(), &p, &plan, a, 1),
            Err(PolicyLeaseError::WideningRejected)
        ));
    }
}
