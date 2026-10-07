//! Main-Agent-only issuance boundary for delegated work.
//!
//! This module is deliberately a deep, policy-first seam: the model supplies
//! only a bounded brief and a task/profile selection.  Trusted foreground
//! identity, workspace, sandbox root, capabilities, and model binding are
//! supplied by the application and never accepted from tool arguments.
//!
//! The first slice below contains the value objects and pure admission checks.
//! Persistence orchestration is added behind this boundary in small follow-up
//! changes; callers must not re-create a second queue/register path.

use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    delegated_model_binding::{
        DelegatedModelBindingRepository, DelegatedModelBindingRequest, ExactModelBindingSelection,
    },
    delegated_network_scope::{
        ApprovedNetworkScopeRepository, ConfirmedNetworkScope, NetworkScopeApprovalProvenance,
    },
    delegated_sandbox::{DelegatedSandboxLifecycle, VerifiedSandboxRef},
    delegation::{NewAttempt, NewDelegation},
    delegation_contract::{
        validate_brief, ContextItem, ContractLimits, DelegationBrief, DelegationIdentity,
    },
    delivery_inbox::RetryContinuation,
    explorer_plan::{ExplorerPlanDocument, ExplorerPlanRecord, ExplorerPlanRepository},
    policy_lease_issuer::{
        allowed_tools_for_policy, AttemptAllocation, LeasePlan, PolicyLeaseError,
        PolicyLeaseIssuer, RetryAttemptAllocation, RetryIssuedLease,
    },
    resource_binding::{
        AdmissionCapture, NewResourceBinding, ResourceBindingIdentity, ResourceBindingRepository,
        ResourceKind,
    },
    work_package::{NewWorkPackage, WorkPackage, WorkPackageRepository, WorkPackageScope},
    worker_policy::{CapabilityPolicy, NetworkAction, TaskShape, WorkerProfile},
    workspace_admission::{WorkspaceAdmission, WorkspaceAdmissionMode, WorkspaceAdmissionRequest},
    workspace_isolation::{
        GitWorktreeProvider, ReleaseDisposition, WorkspaceProvider, WorkspaceRequest,
        WorktreeHandle, WorktreeManifestIdentity, WorktreeManifestProvider,
    },
    workspace_scope_key::{canonical_workspace_scope_key, WorkspaceScopeKeyError},
};

const MAX_GOAL_BYTES: usize = 600;
const MAX_BACKGROUND_REFS: usize = 12;
const MAX_ACTIVE_ATTEMPTS: i64 = 2;
/// A trusted scope for local project work. It never carries a network policy;
/// a worker host must expose only its capability-scoped local tools.
const LOCAL_CAPABILITY_SCOPE_REF: &str = "angelbot.local.v1";

/// Stable identity captured from a running foreground turn.  None of these
/// fields are model-controlled; the constructor proves the provisional parent
/// still exists before a delegation request can be admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundRunIdentity {
    pub session_id: String,
    pub message_id: String,
    pub parent_run_id: String,
    pub owner_profile_id: i64,
    pub workspace_key: String,
    pub work_dir: PathBuf,
}

impl ForegroundRunIdentity {
    pub fn prepare(
        conn: &Connection,
        session_id: &str,
        message_id: &str,
        work_dir: PathBuf,
    ) -> Result<Self, String> {
        let parent: Option<(String, String)> = conn
            .query_row(
                "SELECT message_id, status FROM task_runs WHERE id=?1 AND session_id=?2",
                rusqlite::params![message_id, session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if !matches!(parent, Some((ref parent_message, ref status)) if parent_message == message_id && status == "running")
        {
            return Err("foreground provisional parent is unavailable".into());
        }
        let message_ok: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM messages WHERE id=?1 AND session_id=?2 AND role='assistant' AND is_provisional=1",
                rusqlite::params![message_id, session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if message_ok.is_none() {
            return Err("foreground provisional message is unavailable".into());
        }
        Self::from_trusted_parent(conn, session_id, message_id, work_dir)
    }

    /// Reconstruct the same trusted identity when a foreground action is
    /// resumed from one durable pending confirmation. The original provisional
    /// reply has already been finalized by then, so it cannot use `prepare`'s
    /// running/provisional predicate. Instead, require the exact pending call
    /// to still be attached to the finalized assistant reply.
    pub fn prepare_pending_confirmation(
        conn: &Connection,
        session_id: &str,
        message_id: &str,
        call_id: &str,
        work_dir: PathBuf,
    ) -> Result<Self, String> {
        let pending: Option<i64> = conn
            .query_row(
                "SELECT 1
                   FROM task_runs r
                   JOIN messages m
                     ON m.id = r.message_id AND m.session_id = r.session_id
                   JOIN agent_steps s
                     ON s.session_id = r.session_id
                    AND COALESCE(s.call_id, s.id) = ?3
                    AND s.created_at = m.created_at
                  WHERE r.id = ?1
                    AND r.session_id = ?2
                    AND r.message_id = ?1
                    AND r.status = 'awaiting_confirmation'
                    AND r.confirmation_state = 'pending'
                    AND r.resumable = 1
                    AND m.role = 'assistant'
                    AND s.tool_name = 'delegate_network_exploration'
                    AND s.success = 0
                    AND s.tool_output LIKE '%Confirmation required%'",
                rusqlite::params![message_id, session_id, call_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if pending.is_none() {
            return Err("foreground pending confirmation is unavailable".into());
        }
        Self::from_trusted_parent(conn, session_id, message_id, work_dir)
    }

    fn from_trusted_parent(
        conn: &Connection,
        session_id: &str,
        message_id: &str,
        work_dir: PathBuf,
    ) -> Result<Self, String> {
        let owner_profile_id = conn
            .query_row("SELECT id FROM profile WHERE id=1", [], |row| row.get(0))
            .map_err(|_| "owner profile is unavailable".to_string())?;
        let (canonical, workspace_key) =
            canonical_workspace_scope_key(&work_dir).map_err(|error| {
                match error {
                    WorkspaceScopeKeyError::Unavailable => "foreground workspace is unavailable",
                    WorkspaceScopeKeyError::NotDirectory => {
                        "foreground workspace is not a directory"
                    }
                    WorkspaceScopeKeyError::IdentityUnavailable => {
                        "foreground workspace identity is unavailable"
                    }
                }
                .to_string()
            })?;
        Ok(Self {
            session_id: session_id.into(),
            message_id: message_id.into(),
            parent_run_id: message_id.into(),
            owner_profile_id,
            workspace_key,
            work_dir: canonical,
        })
    }

    pub fn scope(&self) -> WorkPackageScope {
        WorkPackageScope {
            session_id: self.session_id.clone(),
            owner_profile_id: self.owner_profile_id,
            workspace_key: self.workspace_key.clone(),
        }
    }
}

/// Only fields which can be selected by the foreground model.  Paths, hosts,
/// credentials, and concrete permission grants intentionally do not exist in
/// this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegationRequest {
    pub goal: String,
    #[serde(default)]
    pub background_refs: Vec<String>,
    pub task_shape: TaskShape,
    pub worker_profile: WorkerProfile,
    /// Bounded network intent is accepted only for the Explorer/Explore
    /// profile pair. Paths, credentials, and transport policy remain absent.
    #[serde(default)]
    pub explorer_operations: Vec<super::explorer_plan::ExplorerPlanOperation>,
}

#[derive(Debug, Clone)]
struct ResolvedCapabilityScope {
    capability_scope_ref: String,
}

/// Idempotency is scoped to the exact foreground run and tool call.  A model
/// cannot choose or override the parent portion of this key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DelegationIdempotencyKey {
    pub parent_run_id: String,
    pub tool_call_id: String,
}

impl DelegationIdempotencyKey {
    pub fn new(
        identity: &ForegroundRunIdentity,
        tool_call_id: &str,
    ) -> Result<Self, DelegationIssuanceError> {
        if !safe_identity(&identity.parent_run_id) || !safe_identity(tool_call_id) {
            return Err(DelegationIssuanceError::InvalidRequest);
        }
        Ok(Self {
            parent_run_id: identity.parent_run_id.clone(),
            tool_call_id: tool_call_id.into(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationPreflight {
    pub policy: CapabilityPolicy,
    pub brief: DelegationBrief,
    pub scope: WorkPackageScope,
    pub scope_digest: String,
    pub idempotency_key: DelegationIdempotencyKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationReceipt {
    pub delegation_id: String,
    pub attempt_id: String,
    pub work_package_id: String,
}

/// An initial delegation may be admitted exactly once for a foreground tool
/// call. Callers that have already created a transient Supervisor work item
/// need to distinguish the first admission from an idempotent replay so they
/// can discard that unused work item instead of leaving it at the queue head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DelegationIssuanceOutcome {
    Queued(DelegationReceipt),
    Existing(DelegationReceipt),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryIssuanceOutcome {
    Queued(DelegationReceipt),
    Existing(DelegationReceipt),
}

/// Durable retry lineage has three distinct meanings.  Keeping terminal
/// successors separate prevents the Main Agent from presenting a completed or
/// failed retry as if it were still queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryReceiptState {
    None,
    Active(DelegationReceipt),
    Terminal(DelegationReceipt),
}

/// Typed facts that must be persisted beside a queued attempt.  Both initial
/// issuance and retry use this one seam, so a new attempt cannot accidentally
/// miss its admission, sandbox/worktree binding, Explorer plan, or Supervisor
/// gate.
#[derive(Clone)]
struct AttemptResourceFacts {
    delegation_id: String,
    attempt_id: String,
    work_package_id: String,
    lease_id: String,
    scope_digest: String,
    lease_epoch: i64,
    sandbox_manifest_nonce: String,
    sandbox_manifest_locator: String,
    sandbox_manifest_digest: String,
    admission_request: WorkspaceAdmissionRequest,
    explorer_plan: Option<ExplorerPlanRecord>,
    worktree_manifest: Option<WorktreeManifestIdentity>,
    supervisor_work_id: Option<String>,
}

#[derive(Clone)]
struct RetrySource {
    delegation_id: String,
    objective: String,
    package: WorkPackage,
    brief: DelegationBrief,
    source_plan: Option<ExplorerPlanRecord>,
}

#[derive(Debug, thiserror::Error)]
pub enum DelegationIssuanceError {
    #[error("delegation request is outside the bounded contract")]
    InvalidRequest,
    #[error("capability scope is not approved for this workspace")]
    CapabilityScope,
    #[error("at most two delegated attempts may be active")]
    AdmissionDenied,
    #[error("same foreground tool call was already issued")]
    IdempotencyConflict,
    #[error("sandbox allocation failed")]
    Sandbox,
    #[error("change delegation requires an ephemeral git worktree")]
    WorkspaceUnavailable,
    #[error("delegation persistence failed")]
    Storage,
    #[error("delegation binding or lease was rejected")]
    Policy,
    #[error("the prior retry is already terminal")]
    RetryAlreadyTerminal,
}

/// Port for the single delegated-work issuance operation.  The concrete
/// implementation owns the cross-resource composition; worker schedulers and
/// tool handlers only depend on this interface.
pub trait DelegationIssuer: Send + Sync {
    fn issue(
        &self,
        identity: &ForegroundRunIdentity,
        tool_call_id: &str,
        request: DelegationRequest,
        supervisor_work_id: Option<&str>,
    ) -> Result<DelegationReceipt, DelegationIssuanceError>;
}

/// Cross-resource issuer.  The fields are retained here so the eventual
/// implementation can allocate the sandbox before the database transaction,
/// quarantine it on transaction failure, and persist work-package, binding,
/// lease, delegation, and outbox facts exactly once.
#[derive(Clone)]
pub struct DelegationIssuance {
    pub(crate) db: Arc<Mutex<Connection>>,
    pub(crate) sandbox_root: PathBuf,
    pub(crate) model_binding: DelegatedModelBindingRequest,
    admission: WorkspaceAdmission,
    workspace_provider: Option<Arc<GitWorktreeProvider>>,
}

impl DelegationIssuance {
    pub fn new(
        db: Arc<Mutex<Connection>>,
        sandbox_root: PathBuf,
        model_binding: DelegatedModelBindingRequest,
    ) -> Self {
        let workspace_provider = sandbox_root.parent().and_then(|root| {
            let worktree_root = root.join("worktrees");
            let manifest_root = worktree_root.join("manifests");
            GitWorktreeProvider::new(worktree_root, manifest_root)
                .ok()
                .map(Arc::new)
        });
        Self {
            db,
            sandbox_root,
            model_binding,
            admission: WorkspaceAdmission::new(),
            workspace_provider,
        }
    }

    /// Pure request validation and structured brief construction.  No
    /// database or filesystem writes occur here.
    pub fn preflight(
        &self,
        identity: &ForegroundRunIdentity,
        tool_call_id: &str,
        request: &DelegationRequest,
        capability_scope_ref: &str,
        delegation_id: &str,
        attempt_id: &str,
    ) -> Result<DelegationPreflight, DelegationIssuanceError> {
        if request.goal.trim().is_empty()
            || request.goal.len() > MAX_GOAL_BYTES
            || request.background_refs.len() > MAX_BACKGROUND_REFS
            || !safe_scope_ref(capability_scope_ref)
            || request
                .background_refs
                .iter()
                .any(|value| !safe_evidence_ref(value))
            || !safe_identity(delegation_id)
            || !safe_identity(attempt_id)
        {
            return Err(DelegationIssuanceError::InvalidRequest);
        }
        if !matches!(
            (request.task_shape, request.worker_profile),
            (TaskShape::Explore, WorkerProfile::Explorer)
                | (
                    TaskShape::Change,
                    WorkerProfile::Implementer | WorkerProfile::Verifier
                )
        ) {
            // Do not admit a package that no concrete worker host can run.
            // Task shape is a policy ceiling, but routing remains explicit.
            return Err(DelegationIssuanceError::InvalidRequest);
        }
        let policy = CapabilityPolicy::compile(request.task_shape, request.worker_profile);
        policy
            .validate()
            .map_err(|_| DelegationIssuanceError::Policy)?;
        let idempotency_key = DelegationIdempotencyKey::new(identity, tool_call_id)?;
        let brief = DelegationBrief {
            schema_version: 1,
            identity: DelegationIdentity {
                session_id: identity.session_id.clone(),
                message_id: identity.message_id.clone(),
                parent_run_id: identity.parent_run_id.clone(),
                delegation_id: delegation_id.into(),
                attempt_id: attempt_id.into(),
            },
            goal: request.goal.clone(),
            background: request
                .background_refs
                .iter()
                .map(|evidence_ref| ContextItem {
                    kind: "evidence_ref".into(),
                    value: "authorized evidence".into(),
                    evidence_ref: Some(evidence_ref.clone()),
                })
                .collect(),
            constraints: vec!["sandbox_only".into()],
            allowed_references: request.background_refs.clone(),
            allowed_capabilities: vec![],
            completion_criteria: vec!["structured_delivery".into()],
        };
        validate_brief(&brief, &ContractLimits::default())
            .map_err(|_| DelegationIssuanceError::InvalidRequest)?;
        Ok(DelegationPreflight {
            policy,
            brief,
            scope: identity.scope(),
            scope_digest: scope_digest(identity, request, capability_scope_ref),
            idempotency_key,
        })
    }

    /// Resolve the capability scope from trusted policy state. The model may
    /// describe bounded network intents, but it never names or expands a
    /// capability scope. Local work receives a zero-network scope; network
    /// work must be covered by an existing Main-Agent-approved policy.
    fn resolve_capability_scope(
        conn: &Connection,
        identity: &ForegroundRunIdentity,
        request: &DelegationRequest,
    ) -> Result<ResolvedCapabilityScope, DelegationIssuanceError> {
        if request.explorer_operations.is_empty() {
            return Ok(ResolvedCapabilityScope {
                capability_scope_ref: LOCAL_CAPABILITY_SCOPE_REF.into(),
            });
        }
        let (actions, hosts) = requested_network_scope(request)?;
        let source_policy = ApprovedNetworkScopeRepository::find_covering(
            conn,
            identity.owner_profile_id,
            &identity.workspace_key,
            &actions,
            &hosts,
        )
        .map_err(|_| DelegationIssuanceError::CapabilityScope)?;
        Ok(ResolvedCapabilityScope {
            capability_scope_ref: source_policy.capability_scope_ref.clone(),
        })
    }

    pub fn max_active_attempts() -> i64 {
        MAX_ACTIVE_ATTEMPTS
    }

    fn lookup_existing(
        conn: &Connection,
        key: &DelegationIdempotencyKey,
    ) -> Result<Option<DelegationReceipt>, DelegationIssuanceError> {
        conn.query_row(
            "SELECT d.id, a.id, d.work_package_id
             FROM delegations d
             JOIN delegation_attempts a ON a.delegation_id = d.id AND a.attempt_number = 1
             WHERE d.parent_run_id = ?1 AND d.idempotency_key = ?2",
            rusqlite::params![key.parent_run_id, key.tool_call_id],
            |row| {
                Ok(DelegationReceipt {
                    delegation_id: row.get(0)?,
                    attempt_id: row.get(1)?,
                    work_package_id: row
                        .get::<_, Option<String>>(2)?
                        .ok_or(rusqlite::Error::InvalidQuery)?,
                })
            },
        )
        .optional()
        .map_err(|_| DelegationIssuanceError::Storage)
    }

    fn compensate(
        conn: &mut Connection,
        scope: &WorkPackageScope,
        package: Option<&WorkPackage>,
        lifecycle: &DelegatedSandboxLifecycle,
        sandbox: &VerifiedSandboxRef,
    ) {
        if let Some(package) = package {
            if let Err(error) =
                WorkPackageRepository::cancel(conn, scope, &package.id, Utc::now().timestamp())
            {
                eprintln!("[delegation] work-package compensation failed: {error:?}");
            }
        }
        // A failed DB composition must never leave an active writable sandbox.
        let _ = lifecycle
            .revoke_verified(sandbox)
            .and_then(|revoked| lifecycle.quarantine_verified(&revoked));
    }

    fn compensate_worktree(
        provider: Option<&Arc<GitWorktreeProvider>>,
        worktree: Option<&(WorktreeHandle, WorktreeManifestIdentity)>,
    ) {
        if let (Some(provider), Some((handle, _))) = (provider, worktree) {
            let _ = provider.release(handle, ReleaseDisposition::Cleanup);
        }
    }

    /// Retry compensation never cancels the existing WorkPackage: a failed
    /// fresh allocation must leave the Main Agent's prior `needs_decision`
    /// state intact for another safe decision.
    fn compensate_retry(lifecycle: &DelegatedSandboxLifecycle, sandbox: &VerifiedSandboxRef) {
        let _ = lifecycle
            .revoke_verified(sandbox)
            .and_then(|revoked| lifecycle.quarantine_verified(&revoked));
    }

    fn persist_attempt_resources(
        &self,
        tx: &Transaction<'_>,
        facts: &AttemptResourceFacts,
        now: i64,
    ) -> Result<(), PolicyLeaseError> {
        let admission = self
            .admission
            .acquire_in_tx(tx, facts.admission_request.clone(), now)
            .map_err(|error| PolicyLeaseError::Storage(error.to_string()))?;
        let sandbox_binding = NewResourceBinding {
            identity: ResourceBindingIdentity {
                id: format!("resource_{}", Uuid::new_v4().simple()),
                delegation_id: facts.delegation_id.clone(),
                work_package_id: facts.work_package_id.clone(),
                attempt_id: facts.attempt_id.clone(),
                lease_id: facts.lease_id.clone(),
                resource_kind: ResourceKind::Sandbox,
                provider_kind: "delegated_sandbox_v2".into(),
                resource_ref: format!("sandbox_{}", facts.sandbox_manifest_nonce),
                manifest_locator: facts.sandbox_manifest_locator.clone(),
                manifest_version: 1,
                manifest_nonce: facts.sandbox_manifest_nonce.clone(),
                manifest_digest: facts.sandbox_manifest_digest.clone(),
                scope_digest: facts.scope_digest.clone(),
                lease_epoch: facts.lease_epoch,
                admission: Some(AdmissionCapture {
                    id: admission.id,
                    state_version: admission.state_version,
                }),
            },
            created_at: now,
        };
        if let Some(record) = facts.explorer_plan.as_ref() {
            ExplorerPlanRepository::insert_in_tx(tx, record)
                .map_err(|error| PolicyLeaseError::Storage(error.to_string()))?;
        }
        ResourceBindingRepository::create_in_tx(tx, &sandbox_binding)
            .map_err(|error| PolicyLeaseError::Storage(error.to_string()))?;
        if let Some(manifest) = facts.worktree_manifest.as_ref() {
            let worktree_binding = NewResourceBinding {
                identity: ResourceBindingIdentity {
                    id: format!("resource_{}", Uuid::new_v4().simple()),
                    delegation_id: facts.delegation_id.clone(),
                    work_package_id: facts.work_package_id.clone(),
                    attempt_id: facts.attempt_id.clone(),
                    lease_id: facts.lease_id.clone(),
                    resource_kind: ResourceKind::Worktree,
                    provider_kind: manifest.provider_kind.clone(),
                    resource_ref: manifest.handle_id.clone(),
                    manifest_locator: manifest.manifest_locator.clone(),
                    manifest_version: 1,
                    manifest_nonce: manifest.manifest_nonce.clone(),
                    manifest_digest: manifest.manifest_digest.clone(),
                    scope_digest: manifest.scope_digest.clone(),
                    lease_epoch: i64::try_from(manifest.lease_epoch).map_err(|_| {
                        PolicyLeaseError::Storage("worktree lease epoch overflow".into())
                    })?,
                    admission: None,
                },
                created_at: now,
            };
            ResourceBindingRepository::create_in_tx(tx, &worktree_binding)
                .map_err(|error| PolicyLeaseError::Storage(error.to_string()))?;
        }
        if let Some(supervisor_work_id) = facts.supervisor_work_id.as_ref() {
            tx.execute(
                "INSERT INTO workspace_supervisor_delegations
                    (supervisor_work_id, delegation_id, attempt_id, work_package_id, authorized_at)
                 VALUES (?1, ?2, ?3, ?4, NULL)",
                rusqlite::params![
                    supervisor_work_id,
                    facts.delegation_id,
                    facts.attempt_id,
                    facts.work_package_id,
                ],
            )
            .map_err(|error| PolicyLeaseError::Storage(error.to_string()))?;
        }
        Ok(())
    }

    /// A receipt is an externally visible promise to the foreground Agent.
    /// Do not make that promise until every durable fact the scheduler needs
    /// is readable from the same database connection.  This is deliberately
    /// stronger than checking only the work package: a partially persisted
    /// package cannot be dispatched and must never look like a queued child.
    fn receipt_is_durably_queued(
        conn: &Connection,
        delegation_id: &str,
        attempt_id: &str,
        work_package_id: &str,
    ) -> Result<bool, DelegationIssuanceError> {
        conn.query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM delegations d
                JOIN delegation_attempts a ON a.delegation_id = d.id
                JOIN delegation_capability_leases l ON l.attempt_id = a.id
                JOIN delegation_outbox o ON o.attempt_id = a.id
                WHERE d.id = ?1
                  AND a.id = ?2
                  AND d.work_package_id = ?3
                  AND d.status = 'queued'
                  AND a.status = 'queued'
                  AND l.status = 'active'
                  AND o.dispatched_at IS NULL
            )",
            (delegation_id, attempt_id, work_package_id),
            |row| row.get::<_, i64>(0),
        )
        .map(|exists| exists == 1)
        .map_err(|_| DelegationIssuanceError::Storage)
    }

    /// Resolve the durable state of an earlier retry before allocating another
    /// sandbox or Supervisor work item. A delivery id is globally durable,
    /// but the session and work-package scope are still verified before its
    /// receipt reaches the current foreground turn.
    pub fn retry_receipt_state(
        &self,
        identity: &ForegroundRunIdentity,
        source_delivery_id: &str,
    ) -> Result<RetryReceiptState, DelegationIssuanceError> {
        let conn = self
            .db
            .lock()
            .map_err(|_| DelegationIssuanceError::Storage)?;
        Self::retry_receipt_state_in_connection(&conn, identity, source_delivery_id)
    }

    /// A retried delivery is only idempotently resumable while its successor
    /// is actually live. A terminal successor is reported explicitly rather
    /// than masquerading as `already_queued`.
    fn retry_receipt_state_in_connection(
        conn: &Connection,
        identity: &ForegroundRunIdentity,
        source_delivery_id: &str,
    ) -> Result<RetryReceiptState, DelegationIssuanceError> {
        let row: Option<(String, String, String, String, String, i64)> = conn
            .query_row(
                "SELECT d.id, a.id, d.work_package_id, d.status, a.status,
                        EXISTS (
                            SELECT 1 FROM delegation_capability_leases lease
                            WHERE lease.attempt_id = a.id AND lease.status = 'active'
                        )
                 FROM delegation_attempts a
                 JOIN delegations d ON d.id = a.delegation_id
                 WHERE a.retry_source_delivery_id = ?1
                   AND d.session_id = ?2",
                rusqlite::params![source_delivery_id, identity.session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| DelegationIssuanceError::Storage)?;
        let Some((
            delegation_id,
            attempt_id,
            work_package_id,
            delegation_status,
            attempt_status,
            active_lease,
        )) = row
        else {
            return Ok(RetryReceiptState::None);
        };
        WorkPackageRepository::load(&conn, &identity.scope(), &work_package_id)
            .map_err(|_| DelegationIssuanceError::Policy)?;
        let receipt = DelegationReceipt {
            delegation_id,
            attempt_id,
            work_package_id,
        };
        if matches!(delegation_status.as_str(), "queued" | "running")
            && matches!(attempt_status.as_str(), "queued" | "running")
            && active_lease == 1
        {
            Ok(RetryReceiptState::Active(receipt))
        } else {
            Ok(RetryReceiptState::Terminal(receipt))
        }
    }

    /// Supplies the immutable, user-legible routing facts for a fresh
    /// Supervisor work item. Paths, capabilities, model selection and network
    /// scope remain absent from this tool-adapter-facing seam.
    pub fn retry_worker_route(
        &self,
        identity: &ForegroundRunIdentity,
        continuation: &RetryContinuation,
    ) -> Result<(TaskShape, WorkerProfile, String), DelegationIssuanceError> {
        let conn = self
            .db
            .lock()
            .map_err(|_| DelegationIssuanceError::Storage)?;
        let source = Self::load_retry_source(&conn, identity, continuation)?;
        Ok((
            source.package.task_shape,
            source.package.worker_profile,
            source.objective,
        ))
    }

    /// Queue the one fresh attempt authorized by a durable retry continuation.
    /// No caller can alter the original package/model/policy, and no successful
    /// source side effect is replayed.  A race resolves to `Existing` so the
    /// tool adapter can discard its unused Supervisor work item.
    pub fn retry(
        &self,
        identity: &ForegroundRunIdentity,
        continuation: &RetryContinuation,
        supervisor_work_id: Option<&str>,
    ) -> Result<RetryIssuanceOutcome, DelegationIssuanceError> {
        match self.retry_receipt_state(identity, &continuation.source_delivery_id)? {
            RetryReceiptState::Active(existing) => {
                return Ok(RetryIssuanceOutcome::Existing(existing));
            }
            RetryReceiptState::Terminal(_) => {
                return Err(DelegationIssuanceError::RetryAlreadyTerminal);
            }
            RetryReceiptState::None => {}
        }
        if continuation.reason.trim().is_empty()
            || continuation.reason.len() > MAX_GOAL_BYTES
            || continuation.reason.contains(['\0', '\r', '\n'])
        {
            return Err(DelegationIssuanceError::InvalidRequest);
        }
        let now = Utc::now().timestamp();
        let lifecycle = DelegatedSandboxLifecycle::new(&self.sandbox_root)
            .map_err(|_| DelegationIssuanceError::Sandbox)?;
        let mut conn = self
            .db
            .lock()
            .map_err(|_| DelegationIssuanceError::Storage)?;
        let source = Self::load_retry_source(&conn, identity, continuation)?;
        let package = source.package.clone();
        let scope = package.scope.clone();
        let next_attempt_number: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(attempt_number), 0) + 1
                 FROM delegation_attempts WHERE delegation_id = ?1",
                [&source.delegation_id],
                |row| row.get(0),
            )
            .map_err(|_| DelegationIssuanceError::Storage)?;
        let lease_epoch = u64::try_from(next_attempt_number)
            .ok()
            .filter(|epoch| *epoch > 1)
            .ok_or(DelegationIssuanceError::Storage)?;
        let attempt_id = format!("att_{}", Uuid::new_v4().simple());
        let source_operations = source
            .source_plan
            .as_ref()
            .map(|record| record.plan.operations.clone())
            .unwrap_or_default();
        let replay_request = DelegationRequest {
            goal: source.objective.clone(),
            background_refs: source.brief.allowed_references.clone(),
            task_shape: package.task_shape,
            worker_profile: package.worker_profile,
            explorer_operations: source_operations,
        };
        let expected_tools = effective_tool_allowlist(
            &CapabilityPolicy::compile(package.task_shape, package.worker_profile),
            &replay_request,
            source.source_plan.is_some(),
        );
        let allowed_tools = source
            .brief
            .allowed_capabilities
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if allowed_tools != expected_tools {
            return Err(DelegationIssuanceError::Policy);
        }
        let network_hosts =
            retry_network_hosts(&conn, identity, &package, source.source_plan.as_ref())?;
        let mut retry_brief = source.brief.clone();
        retry_brief.identity.attempt_id = attempt_id.clone();
        if retry_brief.constraints.len() >= ContractLimits::default().max_constraints {
            return Err(DelegationIssuanceError::Policy);
        }
        retry_brief
            .constraints
            .push(format!("retry_reason: {}", continuation.reason));
        validate_brief(&retry_brief, &ContractLimits::default())
            .map_err(|_| DelegationIssuanceError::Policy)?;
        let brief_json =
            serde_json::to_string(&retry_brief).map_err(|_| DelegationIssuanceError::Storage)?;
        let sandbox = lifecycle
            .create_bound(
                &source.delegation_id,
                &attempt_id,
                &package.scope_digest,
                lease_epoch,
            )
            .map_err(|_| DelegationIssuanceError::Sandbox)?;
        let needs_worktree = package.task_shape == TaskShape::Change
            || (package.task_shape == TaskShape::Explore && source.source_plan.is_none());
        let worktree: Option<(WorktreeHandle, WorktreeManifestIdentity)> = if needs_worktree {
            let Some(provider) = self.workspace_provider.as_ref() else {
                Self::compensate_retry(&lifecycle, &sandbox);
                return Err(DelegationIssuanceError::WorkspaceUnavailable);
            };
            let workspace_request = WorkspaceRequest {
                scope: scope.clone(),
                work_package_id: package.id.clone(),
                attempt_id: attempt_id.clone(),
                canonical_workspace: identity.work_dir.clone(),
                base_revision: "HEAD".into(),
                lease_epoch,
            };
            let handle = match provider.prepare(&workspace_request) {
                Ok(handle) => handle,
                Err(error) => {
                    eprintln!("[delegation] retry worktree allocation rejected: {error:?}");
                    Self::compensate_retry(&lifecycle, &sandbox);
                    return Err(DelegationIssuanceError::WorkspaceUnavailable);
                }
            };
            let manifest =
                match WorktreeManifestIdentity::for_handle(&handle, package.scope_digest.clone()) {
                    Ok(manifest) => manifest,
                    Err(_) => {
                        let _ = provider.release(&handle, ReleaseDisposition::Cleanup);
                        Self::compensate_retry(&lifecycle, &sandbox);
                        return Err(DelegationIssuanceError::WorkspaceUnavailable);
                    }
                };
            if provider.persist_manifest(&handle, &manifest).is_err() {
                let _ = provider.release(&handle, ReleaseDisposition::Cleanup);
                Self::compensate_retry(&lifecycle, &sandbox);
                return Err(DelegationIssuanceError::WorkspaceUnavailable);
            }
            Some((handle, manifest))
        } else {
            None
        };
        let retry_plan = source
            .source_plan
            .as_ref()
            .map(|record| {
                ExplorerPlanRecord::new(
                    format!("eplan_{}", Uuid::new_v4().simple()),
                    source.delegation_id.clone(),
                    attempt_id.clone(),
                    package.id.clone(),
                    package.scope_digest.clone(),
                    package.worker_policy_version,
                    record.plan.clone(),
                    now,
                )
            })
            .transpose()
            .map_err(|_| {
                Self::compensate_worktree(self.workspace_provider.as_ref(), worktree.as_ref());
                Self::compensate_retry(&lifecycle, &sandbox);
                DelegationIssuanceError::Policy
            })?;
        let write_roots = if package.task_shape == TaskShape::Change
            && package.worker_profile == WorkerProfile::Implementer
        {
            vec![PathBuf::from(".")]
        } else {
            Vec::new()
        };
        let admission_mode = match (package.task_shape, package.worker_profile) {
            (TaskShape::Explore, _) => WorkspaceAdmissionMode::Read,
            (TaskShape::Change, WorkerProfile::Implementer) => WorkspaceAdmissionMode::Write,
            (TaskShape::Change, WorkerProfile::Verifier) => WorkspaceAdmissionMode::Review,
            _ => {
                Self::compensate_worktree(self.workspace_provider.as_ref(), worktree.as_ref());
                Self::compensate_retry(&lifecycle, &sandbox);
                return Err(DelegationIssuanceError::Policy);
            }
        };
        let lease_id = format!("lease_{}", Uuid::new_v4().simple());
        let sandbox_identity = sandbox.identity();
        let resource_facts = AttemptResourceFacts {
            delegation_id: source.delegation_id.clone(),
            attempt_id: attempt_id.clone(),
            work_package_id: package.id.clone(),
            lease_id: lease_id.clone(),
            scope_digest: package.scope_digest.clone(),
            lease_epoch: next_attempt_number,
            sandbox_manifest_nonce: sandbox_identity.manifest_nonce.clone(),
            sandbox_manifest_locator: sandbox_identity.manifest_locator.clone(),
            sandbox_manifest_digest: sandbox_identity.manifest_digest.clone(),
            admission_request: WorkspaceAdmissionRequest::new(
                identity.workspace_key.clone(),
                package.id.clone(),
                attempt_id.clone(),
                lease_id.clone(),
                admission_mode,
                package.capability_expires_at,
            ),
            explorer_plan: retry_plan,
            worktree_manifest: worktree.as_ref().map(|(_, manifest)| manifest.clone()),
            supervisor_work_id: supervisor_work_id.map(str::to_owned),
        };
        let dispatch_payload = json!({
            "delegation_id": source.delegation_id.clone(),
            "attempt_id": attempt_id.clone(),
            "work_package_id": package.id.clone(),
            "retry_source_delivery_id": continuation.source_delivery_id.clone(),
        })
        .to_string();
        let issue_result = PolicyLeaseIssuer::reissue_with_after(
            &mut conn,
            &scope,
            &package,
            &LeasePlan {
                policy: CapabilityPolicy::compile(package.task_shape, package.worker_profile),
                capability_scope_ref: package.capability_scope_ref.clone(),
                max_expires_at: package.capability_expires_at,
                allowed_tools: expected_tools,
                allowed_network_hosts: network_hosts.clone(),
            },
            RetryAttemptAllocation {
                delegation_id: source.delegation_id.clone(),
                session_id: identity.session_id.clone(),
                work_package_id: package.id.clone(),
                source_delivery_id: continuation.source_delivery_id.clone(),
                source_delivery_revision: continuation.source_delivery_revision,
                source_attempt_id: continuation.attempt_id.clone(),
                reason: continuation.reason.clone(),
                retry_brief_json: brief_json,
                attempt: NewAttempt {
                    id: attempt_id.clone(),
                    attempt_number: next_attempt_number,
                    sandbox_ref: format!("sandbox://{}", resource_facts.sandbox_manifest_nonce),
                    outbox_id: format!("out_{}", Uuid::new_v4().simple()),
                    dispatch_payload_json: dispatch_payload,
                },
                lease_id,
                capability_scope_ref: package.capability_scope_ref.clone(),
                expires_at: package.capability_expires_at,
                read_roots: vec![identity.work_dir.clone()],
                write_roots,
                tool_allowlist: allowed_tools,
                network_hosts,
                budget_json: format!(r#"{{"attempt":{next_attempt_number}}}"#),
            },
            now,
            move |tx| self.persist_attempt_resources(tx, &resource_facts, now),
        );
        match issue_result {
            Ok(RetryIssuedLease::Issued(issued)) => {
                let receipt = DelegationReceipt {
                    delegation_id: source.delegation_id,
                    attempt_id,
                    work_package_id: issued.work_package_id,
                };
                if Self::receipt_is_durably_queued(
                    &conn,
                    &receipt.delegation_id,
                    &receipt.attempt_id,
                    &receipt.work_package_id,
                )? {
                    Ok(RetryIssuanceOutcome::Queued(receipt))
                } else {
                    Err(DelegationIssuanceError::Storage)
                }
            }
            Ok(RetryIssuedLease::Existing {
                attempt_id,
                work_package_id,
            }) => {
                Self::compensate_worktree(self.workspace_provider.as_ref(), worktree.as_ref());
                Self::compensate_retry(&lifecycle, &sandbox);
                let receipt = DelegationReceipt {
                    delegation_id: source.delegation_id,
                    attempt_id,
                    work_package_id,
                };
                if matches!(
                    Self::retry_receipt_state_in_connection(
                        &conn,
                        identity,
                        &continuation.source_delivery_id,
                    )?,
                    RetryReceiptState::Active(_)
                ) {
                    Ok(RetryIssuanceOutcome::Existing(receipt))
                } else {
                    Err(DelegationIssuanceError::RetryAlreadyTerminal)
                }
            }
            Err(error) => {
                Self::compensate_worktree(self.workspace_provider.as_ref(), worktree.as_ref());
                Self::compensate_retry(&lifecycle, &sandbox);
                Err(match error {
                    PolicyLeaseError::AdmissionDenied => DelegationIssuanceError::AdmissionDenied,
                    PolicyLeaseError::Storage(_) => DelegationIssuanceError::Storage,
                    _ => DelegationIssuanceError::Policy,
                })
            }
        }
    }

    fn load_retry_source(
        conn: &Connection,
        identity: &ForegroundRunIdentity,
        continuation: &RetryContinuation,
    ) -> Result<RetrySource, DelegationIssuanceError> {
        if continuation.replay_successful_side_effects
            || continuation.delegation_id.trim().is_empty()
            || continuation.attempt_id.trim().is_empty()
            || continuation.source_delivery_id.trim().is_empty()
        {
            return Err(DelegationIssuanceError::InvalidRequest);
        }
        let row: Option<(String, String, String, String, String, String)> = conn
            .query_row(
                "SELECT session_id, message_id, parent_run_id, work_package_id, objective, brief_json
                 FROM delegations
                 WHERE id = ?1 AND session_id = ?2 AND status = 'needs_decision'",
                rusqlite::params![continuation.delegation_id, identity.session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| DelegationIssuanceError::Storage)?;
        let Some((session_id, message_id, parent_run_id, work_package_id, objective, brief_json)) =
            row
        else {
            return Err(DelegationIssuanceError::Policy);
        };
        let package = WorkPackageRepository::load(conn, &identity.scope(), &work_package_id)
            .map_err(|_| DelegationIssuanceError::Policy)?;
        if session_id != identity.session_id
            || package.status != super::work_package::WorkPackageStatus::Active
        {
            return Err(DelegationIssuanceError::Policy);
        }
        DelegatedModelBindingRepository::load(conn, &package.scope, &package.id)
            .map_err(|_| DelegationIssuanceError::Policy)?;
        let brief: DelegationBrief =
            serde_json::from_str(&brief_json).map_err(|_| DelegationIssuanceError::Policy)?;
        if brief.identity.session_id != session_id
            || brief.identity.message_id != message_id
            || brief.identity.parent_run_id != parent_run_id
            || brief.identity.delegation_id != continuation.delegation_id
            || brief.identity.attempt_id != continuation.attempt_id
            || brief.goal != objective
            || validate_brief(&brief, &ContractLimits::default()).is_err()
        {
            return Err(DelegationIssuanceError::Policy);
        }
        let source_plan = ExplorerPlanRepository::load_for_attempt_in_connection(
            conn,
            &continuation.delegation_id,
            &continuation.attempt_id,
            &package.id,
        )
        .map_err(|_| DelegationIssuanceError::Policy)?;
        if source_plan.is_some()
            != matches!(
                (package.task_shape, package.worker_profile),
                (TaskShape::Explore, WorkerProfile::Explorer)
            )
            && package.capability_scope_ref != LOCAL_CAPABILITY_SCOPE_REF
        {
            return Err(DelegationIssuanceError::Policy);
        }
        Ok(RetrySource {
            delegation_id: continuation.delegation_id.clone(),
            objective,
            package,
            brief,
            source_plan,
        })
    }
}

impl DelegationIssuance {
    /// Queue one Explorer network task after the generic foreground
    /// confirmation flow has supplied an exact, single-use approval context.
    /// The policy and provenance are persisted only in the final issuance
    /// transaction, so a rejected or failed request never becomes a reusable
    /// permission grant.
    pub(crate) fn issue_confirmed_network(
        &self,
        identity: &ForegroundRunIdentity,
        tool_call_id: &str,
        request: DelegationRequest,
        supervisor_work_id: Option<&str>,
    ) -> Result<DelegationReceipt, DelegationIssuanceError> {
        self.issue_confirmed_network_outcome(identity, tool_call_id, request, supervisor_work_id)
            .map(DelegationIssuanceOutcome::into_receipt)
    }

    pub(crate) fn issue_confirmed_network_outcome(
        &self,
        identity: &ForegroundRunIdentity,
        tool_call_id: &str,
        request: DelegationRequest,
        supervisor_work_id: Option<&str>,
    ) -> Result<DelegationIssuanceOutcome, DelegationIssuanceError> {
        self.issue_internal(identity, tool_call_id, request, supervisor_work_id, true)
    }

    pub(crate) fn issue_outcome(
        &self,
        identity: &ForegroundRunIdentity,
        tool_call_id: &str,
        request: DelegationRequest,
        supervisor_work_id: Option<&str>,
    ) -> Result<DelegationIssuanceOutcome, DelegationIssuanceError> {
        self.issue_internal(identity, tool_call_id, request, supervisor_work_id, false)
    }

    fn issue_internal(
        &self,
        identity: &ForegroundRunIdentity,
        tool_call_id: &str,
        request: DelegationRequest,
        supervisor_work_id: Option<&str>,
        confirmed_network_scope: bool,
    ) -> Result<DelegationIssuanceOutcome, DelegationIssuanceError> {
        let supervisor_work_id = supervisor_work_id.map(str::to_owned);
        let delegation_id = format!("dlg_{}", Uuid::new_v4().simple());
        let attempt_id = format!("att_{}", Uuid::new_v4().simple());
        let explorer_document = match (request.task_shape, request.worker_profile) {
            (TaskShape::Explore, WorkerProfile::Explorer)
                if !request.explorer_operations.is_empty() =>
            {
                Some(
                    ExplorerPlanDocument::new(request.explorer_operations.clone())
                        .map_err(|_| DelegationIssuanceError::InvalidRequest)?,
                )
            }
            _ if request.explorer_operations.is_empty() => None,
            _ => return Err(DelegationIssuanceError::InvalidRequest),
        };
        // A verifier must consume an immutable implementer artifact through
        // the future reviewer workflow.  It must never receive a fresh
        // writable checkout from the generic issuance path.
        if matches!(
            (request.task_shape, request.worker_profile),
            (TaskShape::Change, WorkerProfile::Verifier)
        ) {
            return Err(DelegationIssuanceError::WorkspaceUnavailable);
        }
        let now = Utc::now().timestamp();
        let lifecycle = DelegatedSandboxLifecycle::new(&self.sandbox_root)
            .map_err(|_| DelegationIssuanceError::Sandbox)?;
        let mut conn = self
            .db
            .lock()
            .map_err(|_| DelegationIssuanceError::Storage)?;

        let (resolved_scope, requested_scope) = if confirmed_network_scope {
            let (actions, hosts) = requested_network_scope(&request)?;
            (
                ResolvedCapabilityScope {
                    capability_scope_ref: format!("network_scope_{}", Uuid::new_v4().simple()),
                },
                Some((actions, hosts)),
            )
        } else {
            (
                Self::resolve_capability_scope(&conn, identity, &request)?,
                None,
            )
        };
        let preflight = self.preflight(
            identity,
            tool_call_id,
            &request,
            &resolved_scope.capability_scope_ref,
            &delegation_id,
            &attempt_id,
        )?;

        if let Some(existing) = Self::lookup_existing(&conn, &preflight.idempotency_key)? {
            return Ok(DelegationIssuanceOutcome::Existing(existing));
        }
        let running: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM delegation_attempts WHERE status IN ('queued','running')",
                [],
                |row| row.get(0),
            )
            .map_err(|_| DelegationIssuanceError::Storage)?;
        if running >= MAX_ACTIVE_ATTEMPTS {
            return Err(DelegationIssuanceError::AdmissionDenied);
        }
        let confirmed_scope = requested_scope
            .map(|(actions, hosts)| {
                ConfirmedNetworkScope::new(
                    NetworkScopeApprovalProvenance {
                        owner_profile_id: identity.owner_profile_id,
                        workspace_key: identity.workspace_key.clone(),
                        session_id: identity.session_id.clone(),
                        message_id: identity.message_id.clone(),
                        parent_run_id: identity.parent_run_id.clone(),
                        tool_call_id: tool_call_id.to_owned(),
                    },
                    resolved_scope.capability_scope_ref.clone(),
                    preflight.scope_digest.clone(),
                    actions,
                    hosts,
                )
            })
            .transpose()
            .map_err(|_| DelegationIssuanceError::Policy)?;
        let scope = preflight.scope.clone();
        let package = match WorkPackageRepository::create(
            &mut conn,
            NewWorkPackage {
                scope: scope.clone(),
                task_shape: request.task_shape,
                worker_profile: request.worker_profile,
                scope_digest: preflight.scope_digest.clone(),
                capability_scope_ref: resolved_scope.capability_scope_ref.clone(),
                capability_expires_at: now + 15 * 60,
            },
        ) {
            Ok(package) => package,
            Err(_) => return Err(DelegationIssuanceError::Storage),
        };
        let selection = ExactModelBindingSelection::new(self.model_binding.clone());
        if DelegatedModelBindingRepository::create(
            &conn,
            &scope,
            &package.id,
            &self.model_binding,
            &selection,
            now,
        )
        .is_err()
        {
            let _ = WorkPackageRepository::cancel(&mut conn, &scope, &package.id, now);
            return Err(DelegationIssuanceError::Policy);
        }

        // The sandbox manifest is bound to the immutable package scope and
        // lease epoch. It is created only after policy/model admission so a
        // partially rejected request never leaves a writable residual.
        let sandbox = lifecycle
            .create_bound(&delegation_id, &attempt_id, &package.scope_digest, 1)
            .map_err(|_| {
                let _ = WorkPackageRepository::cancel(&mut conn, &scope, &package.id, now);
                DelegationIssuanceError::Sandbox
            })?;

        // Change work is always rooted in a temporary Git worktree.  The
        // provider owns the checkout path and manifest; the issuer stores only
        // its opaque identity below.  Explorer remains read-only and does not
        // need a project checkout.
        let needs_worktree = request.task_shape == TaskShape::Change
            || (request.task_shape == TaskShape::Explore
                && request.worker_profile == WorkerProfile::Explorer
                && request.explorer_operations.is_empty());
        let worktree: Option<(WorktreeHandle, WorktreeManifestIdentity)> = if needs_worktree {
            let Some(provider) = self.workspace_provider.as_ref() else {
                Self::compensate(&mut conn, &scope, Some(&package), &lifecycle, &sandbox);
                return Err(DelegationIssuanceError::WorkspaceUnavailable);
            };
            let workspace_request = WorkspaceRequest {
                scope: scope.clone(),
                work_package_id: package.id.clone(),
                attempt_id: attempt_id.clone(),
                canonical_workspace: identity.work_dir.clone(),
                base_revision: "HEAD".into(),
                lease_epoch: 1,
            };
            let handle = match provider.prepare(&workspace_request) {
                Ok(handle) => handle,
                Err(error) => {
                    // Keep the user/model-facing failure coarse, but retain a
                    // path-free provider category in the application log for
                    // operational diagnosis.
                    eprintln!("[delegation] worktree allocation rejected: {error:?}");
                    Self::compensate(&mut conn, &scope, Some(&package), &lifecycle, &sandbox);
                    return Err(DelegationIssuanceError::WorkspaceUnavailable);
                }
            };
            let manifest =
                match WorktreeManifestIdentity::for_handle(&handle, package.scope_digest.clone()) {
                    Ok(manifest) => manifest,
                    Err(_) => {
                        let _ = provider.release(&handle, ReleaseDisposition::Cleanup);
                        Self::compensate(&mut conn, &scope, Some(&package), &lifecycle, &sandbox);
                        return Err(DelegationIssuanceError::WorkspaceUnavailable);
                    }
                };
            if provider.persist_manifest(&handle, &manifest).is_err() {
                let _ = provider.release(&handle, ReleaseDisposition::Cleanup);
                Self::compensate(&mut conn, &scope, Some(&package), &lifecycle, &sandbox);
                return Err(DelegationIssuanceError::WorkspaceUnavailable);
            }
            Some((handle, manifest))
        } else {
            None
        };

        let has_network_explorer_plan = explorer_document.is_some();
        // A persisted source scope is an approval ceiling that can cover later
        // work in the same project. Each attempt still receives only the
        // hosts named by its immutable Explorer plan, so reuse cannot widen a
        // concrete worker's lease to dormant hosts in that source scope.
        let requested_network_hosts = if has_network_explorer_plan {
            network_scope_for_operations(&request.explorer_operations)?.1
        } else {
            BTreeSet::new()
        };
        let explorer_plan = match explorer_document {
            Some(document) => Some(
                ExplorerPlanRecord::new(
                    format!("eplan_{}", Uuid::new_v4().simple()),
                    delegation_id.clone(),
                    attempt_id.clone(),
                    package.id.clone(),
                    package.scope_digest.clone(),
                    package.worker_policy_version,
                    document,
                    now,
                )
                .map_err(|_| {
                    Self::compensate(&mut conn, &scope, Some(&package), &lifecycle, &sandbox);
                    DelegationIssuanceError::InvalidRequest
                })?,
            ),
            None => None,
        };

        // A lease must describe the surface that a concrete worker host can
        // actually execute, not every capability its abstract policy could
        // theoretically allow.  This keeps future adapters from accidentally
        // turning a dormant policy bit (for example `local.build`) into a
        // privilege escalation.
        let allowed_tools =
            effective_tool_allowlist(&preflight.policy, &request, has_network_explorer_plan);
        // The concrete provider keeps its filesystem path private.  A change
        // implementer receives only a relative candidate root; the eventual
        // worker host resolves it through the opaque worktree manifest.  Read
        // and explore workers remain strictly read-only.
        let write_roots = if request.task_shape == TaskShape::Change
            && request.worker_profile == WorkerProfile::Implementer
        {
            vec![PathBuf::from(".")]
        } else {
            Vec::new()
        };
        let dispatch_payload = json!({
            "delegation_id": delegation_id,
            "attempt_id": attempt_id,
            "work_package_id": package.id,
            "idempotency_key": preflight.idempotency_key.tool_call_id,
        })
        .to_string();
        let brief = DelegationBrief {
            allowed_capabilities: allowed_tools.iter().cloned().collect(),
            ..preflight.brief
        };
        let brief_json = match serde_json::to_string(&brief) {
            Ok(value) => value,
            Err(_) => {
                Self::compensate_worktree(self.workspace_provider.as_ref(), worktree.as_ref());
                Self::compensate(&mut conn, &scope, Some(&package), &lifecycle, &sandbox);
                return Err(DelegationIssuanceError::Storage);
            }
        };
        let plan = LeasePlan {
            policy: preflight.policy,
            capability_scope_ref: resolved_scope.capability_scope_ref.clone(),
            max_expires_at: package.capability_expires_at,
            allowed_tools: allowed_tools.clone(),
            allowed_network_hosts: requested_network_hosts.clone(),
        };
        let lease_id = format!("lease_{}", Uuid::new_v4().simple());
        let admission_mode = match (request.task_shape, request.worker_profile) {
            // Explore is the hard capability ceiling; a legacy/explicit
            // profile label may still be attached, but it can only obtain a
            // read admission and remains subject to the worker-host profile
            // gate at dispatch time.
            (TaskShape::Explore, _) => WorkspaceAdmissionMode::Read,
            (TaskShape::Change, WorkerProfile::Implementer) => WorkspaceAdmissionMode::Write,
            (TaskShape::Change, WorkerProfile::Verifier) => WorkspaceAdmissionMode::Review,
            _ => {
                Self::compensate_worktree(self.workspace_provider.as_ref(), worktree.as_ref());
                Self::compensate(&mut conn, &scope, Some(&package), &lifecycle, &sandbox);
                return Err(DelegationIssuanceError::InvalidRequest);
            }
        };
        let admission_request = WorkspaceAdmissionRequest::new(
            identity.workspace_key.clone(),
            package.id.clone(),
            attempt_id.clone(),
            lease_id.clone(),
            admission_mode,
            package.capability_expires_at,
        );
        let sandbox_identity = sandbox.identity();
        let resource_facts = AttemptResourceFacts {
            delegation_id: delegation_id.clone(),
            attempt_id: attempt_id.clone(),
            work_package_id: package.id.clone(),
            lease_id: lease_id.clone(),
            scope_digest: package.scope_digest.clone(),
            lease_epoch: 1,
            sandbox_manifest_nonce: sandbox_identity.manifest_nonce.clone(),
            sandbox_manifest_locator: sandbox_identity.manifest_locator.clone(),
            sandbox_manifest_digest: sandbox_identity.manifest_digest.clone(),
            admission_request,
            explorer_plan,
            worktree_manifest: worktree.as_ref().map(|(_, manifest)| manifest.clone()),
            supervisor_work_id: supervisor_work_id.clone(),
        };
        let lease_result = PolicyLeaseIssuer::issue_with_after(
            &mut conn,
            &scope,
            &package,
            &plan,
            AttemptAllocation {
                delegation: NewDelegation {
                    id: delegation_id.clone(),
                    session_id: identity.session_id.clone(),
                    message_id: identity.message_id.clone(),
                    parent_run_id: identity.parent_run_id.clone(),
                    work_package_id: Some(package.id.clone()),
                    idempotency_key: Some(preflight.idempotency_key.tool_call_id.clone()),
                    objective: request.goal.clone(),
                    brief_json,
                },
                attempt: NewAttempt {
                    id: attempt_id.clone(),
                    attempt_number: 1,
                    sandbox_ref: format!("sandbox://{}", resource_facts.sandbox_manifest_nonce),
                    outbox_id: format!("out_{}", Uuid::new_v4().simple()),
                    dispatch_payload_json: dispatch_payload,
                },
                lease_id,
                capability_scope_ref: resolved_scope.capability_scope_ref,
                expires_at: package.capability_expires_at,
                read_roots: vec![identity.work_dir.clone()],
                write_roots,
                tool_allowlist: allowed_tools,
                network_hosts: requested_network_hosts,
                budget_json: "{\"attempt\":1}".into(),
            },
            now,
            move |tx| {
                if let Some(confirmed_scope) = confirmed_scope.as_ref() {
                    confirmed_scope
                        .persist_in_tx(tx, now)
                        .map_err(|error| PolicyLeaseError::Storage(error.to_string()))?;
                }
                self.persist_attempt_resources(tx, &resource_facts, now)
            },
        );
        match lease_result {
            Ok(issued) => {
                let receipt = DelegationReceipt {
                    delegation_id,
                    attempt_id,
                    work_package_id: issued.work_package_id,
                };
                match Self::receipt_is_durably_queued(
                    &conn,
                    &receipt.delegation_id,
                    &receipt.attempt_id,
                    &receipt.work_package_id,
                ) {
                    Ok(true) => Ok(DelegationIssuanceOutcome::Queued(receipt)),
                    Ok(false) | Err(_) => {
                        Self::compensate_worktree(
                            self.workspace_provider.as_ref(),
                            worktree.as_ref(),
                        );
                        Self::compensate(&mut conn, &scope, Some(&package), &lifecycle, &sandbox);
                        Err(DelegationIssuanceError::Storage)
                    }
                }
            }
            Err(error) if error.to_string().contains("UNIQUE") => {
                match Self::lookup_existing(&conn, &preflight.idempotency_key) {
                    Ok(Some(existing)) => {
                        Self::compensate_worktree(
                            self.workspace_provider.as_ref(),
                            worktree.as_ref(),
                        );
                        return Ok(DelegationIssuanceOutcome::Existing(existing));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        Self::compensate_worktree(
                            self.workspace_provider.as_ref(),
                            worktree.as_ref(),
                        );
                        return Err(error);
                    }
                }
                Self::compensate_worktree(self.workspace_provider.as_ref(), worktree.as_ref());
                Self::compensate(&mut conn, &scope, Some(&package), &lifecycle, &sandbox);
                Err(DelegationIssuanceError::IdempotencyConflict)
            }
            Err(error) => {
                eprintln!("[delegation] durable issuance rejected: {error:?}");
                Self::compensate_worktree(self.workspace_provider.as_ref(), worktree.as_ref());
                Self::compensate(&mut conn, &scope, Some(&package), &lifecycle, &sandbox);
                Err(match error {
                    super::policy_lease_issuer::PolicyLeaseError::AdmissionDenied => {
                        DelegationIssuanceError::AdmissionDenied
                    }
                    super::policy_lease_issuer::PolicyLeaseError::Storage(_) => {
                        DelegationIssuanceError::Storage
                    }
                    _ => DelegationIssuanceError::Policy,
                })
            }
        }
    }
}

impl DelegationIssuer for DelegationIssuance {
    fn issue(
        &self,
        identity: &ForegroundRunIdentity,
        tool_call_id: &str,
        request: DelegationRequest,
        supervisor_work_id: Option<&str>,
    ) -> Result<DelegationReceipt, DelegationIssuanceError> {
        self.issue_outcome(identity, tool_call_id, request, supervisor_work_id)
            .map(DelegationIssuanceOutcome::into_receipt)
    }
}

impl DelegationIssuanceOutcome {
    fn into_receipt(self) -> DelegationReceipt {
        match self {
            Self::Queued(receipt) | Self::Existing(receipt) => receipt,
        }
    }
}

fn safe_evidence_ref(value: &str) -> bool {
    value.len() <= 256
        && value.contains("://")
        && !value.chars().any(char::is_whitespace)
        && !value.contains('\0')
}

fn safe_scope_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':' | b'.'))
}

fn safe_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn effective_tool_allowlist(
    policy: &CapabilityPolicy,
    request: &DelegationRequest,
    has_network_explorer_plan: bool,
) -> BTreeSet<String> {
    let concrete = match (
        request.task_shape,
        request.worker_profile,
        has_network_explorer_plan,
    ) {
        // The network explorer host is intentionally network-only.  The
        // typed durable plan is its complete tool surface.
        (TaskShape::Explore, WorkerProfile::Explorer, true) => request
            .explorer_operations
            .iter()
            .map(|operation| match operation {
                super::explorer_plan::ExplorerPlanOperation::Search { .. } => "network.search",
                super::explorer_plan::ExplorerPlanOperation::Fetch { .. } => "network.fetch",
            })
            .map(str::to_owned)
            .collect(),
        // A local Explorer runs in a temporary read-only worktree.  It has no
        // implicit network or local-process authority.
        (TaskShape::Explore, WorkerProfile::Explorer, false) => {
            BTreeSet::from(["file.read".to_owned()])
        }
        // Implementer and Verifier hosts currently expose only the scoped
        // file operations below. Build/registry/network adapters can be
        // added later only by extending this concrete host mapping.
        (TaskShape::Change, WorkerProfile::Implementer, false) => {
            BTreeSet::from(["file.read".to_owned(), "file.write_candidate".to_owned()])
        }
        (TaskShape::Change, WorkerProfile::Verifier, false) => {
            BTreeSet::from(["file.read".to_owned()])
        }
        _ => BTreeSet::new(),
    };

    let policy_allowed = allowed_tools_for_policy(policy);
    concrete
        .into_iter()
        .filter(|tool| policy_allowed.contains(tool))
        .collect()
}

/// Revalidates the original typed network plan against the current enabled
/// policy record, then returns the plan's own host set.  The lease therefore
/// narrows to the immutable operations instead of inheriting every host that
/// happens to be approved under the same capability scope today.
fn retry_network_hosts(
    conn: &Connection,
    identity: &ForegroundRunIdentity,
    package: &WorkPackage,
    plan: Option<&ExplorerPlanRecord>,
) -> Result<BTreeSet<String>, DelegationIssuanceError> {
    let Some(plan) = plan else {
        if package.capability_scope_ref != LOCAL_CAPABILITY_SCOPE_REF
            && matches!(
                (package.task_shape, package.worker_profile),
                (TaskShape::Explore, WorkerProfile::Explorer)
            )
        {
            return Err(DelegationIssuanceError::Policy);
        }
        return Ok(BTreeSet::new());
    };
    if !matches!(
        (package.task_shape, package.worker_profile),
        (TaskShape::Explore, WorkerProfile::Explorer)
    ) || package.capability_scope_ref == LOCAL_CAPABILITY_SCOPE_REF
    {
        return Err(DelegationIssuanceError::Policy);
    }
    let policy = ApprovedNetworkScopeRepository::load(
        conn,
        identity.owner_profile_id,
        &identity.workspace_key,
        &package.capability_scope_ref,
    )
    .map_err(|_| DelegationIssuanceError::CapabilityScope)?;
    let (actions, hosts) = network_scope_for_operations(&plan.plan.operations)
        .map_err(|_| DelegationIssuanceError::Policy)?;
    if actions.is_empty()
        || !actions.is_subset(&policy.actions)
        || !hosts.is_subset(&policy.allowed_hosts)
    {
        return Err(DelegationIssuanceError::CapabilityScope);
    }
    Ok(hosts)
}

fn requested_network_scope(
    request: &DelegationRequest,
) -> Result<(BTreeSet<NetworkAction>, BTreeSet<String>), DelegationIssuanceError> {
    if request.task_shape != TaskShape::Explore
        || request.worker_profile != WorkerProfile::Explorer
        || request.explorer_operations.is_empty()
    {
        return Err(DelegationIssuanceError::InvalidRequest);
    }
    network_scope_for_operations(&request.explorer_operations)
}

fn network_scope_for_operations(
    operations: &[super::explorer_plan::ExplorerPlanOperation],
) -> Result<(BTreeSet<NetworkAction>, BTreeSet<String>), DelegationIssuanceError> {
    let mut actions = BTreeSet::new();
    let mut hosts = BTreeSet::new();
    for operation in operations {
        match operation {
            super::explorer_plan::ExplorerPlanOperation::Search { provider_host, .. } => {
                actions.insert(NetworkAction::Search);
                hosts.insert(
                    super::network_gateway::validated_host(provider_host)
                        .map_err(|_| DelegationIssuanceError::InvalidRequest)?,
                );
            }
            super::explorer_plan::ExplorerPlanOperation::Fetch { url, .. } => {
                let parsed = reqwest::Url::parse(url)
                    .map_err(|_| DelegationIssuanceError::InvalidRequest)?;
                super::network_gateway::parse_checked_url(&parsed)
                    .map_err(|_| DelegationIssuanceError::InvalidRequest)?;
                let host = parsed
                    .host_str()
                    .ok_or(DelegationIssuanceError::InvalidRequest)?;
                actions.insert(NetworkAction::Fetch);
                hosts.insert(
                    super::network_gateway::validated_host(host)
                        .map_err(|_| DelegationIssuanceError::InvalidRequest)?,
                );
            }
        }
    }
    if actions.is_empty() || hosts.is_empty() {
        return Err(DelegationIssuanceError::InvalidRequest);
    }
    Ok((actions, hosts))
}

fn scope_digest(
    identity: &ForegroundRunIdentity,
    request: &DelegationRequest,
    capability_scope_ref: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(identity.session_id.as_bytes());
    hasher.update(b":");
    hasher.update(identity.parent_run_id.as_bytes());
    hasher.update(b":");
    hasher.update(request.task_shape.as_str().as_bytes());
    hasher.update(b":");
    hasher.update(request.worker_profile.as_str().as_bytes());
    hasher.update(b":");
    hasher.update(capability_scope_ref.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ForegroundRunIdentity {
        ForegroundRunIdentity {
            session_id: "session".into(),
            message_id: "message".into(),
            parent_run_id: "run".into(),
            owner_profile_id: 1,
            workspace_key: "workspace".into(),
            work_dir: PathBuf::from("workspace"),
        }
    }

    fn request() -> DelegationRequest {
        DelegationRequest {
            goal: "inspect the repository".into(),
            background_refs: vec!["evidence://brief".into()],
            task_shape: TaskShape::Explore,
            worker_profile: WorkerProfile::Explorer,
            explorer_operations: vec![],
        }
    }

    fn issuer() -> DelegationIssuance {
        DelegationIssuance::new(
            Arc::new(Mutex::new(Connection::open_in_memory().unwrap())),
            PathBuf::from("sandbox"),
            DelegatedModelBindingRequest::new(
                "provider:default",
                "model:test",
                "keychain:default",
                1,
            )
            .unwrap(),
        )
    }

    #[test]
    fn preflight_compiles_scope_and_parent_tool_idempotency() {
        let preflight = issuer()
            .preflight(
                &identity(),
                "call-1",
                &request(),
                LOCAL_CAPABILITY_SCOPE_REF,
                "dlg_1",
                "att_1",
            )
            .unwrap();
        assert_eq!(preflight.scope.session_id, "session");
        assert_eq!(preflight.idempotency_key.parent_run_id, "run");
        assert_eq!(preflight.idempotency_key.tool_call_id, "call-1");
        assert!(preflight.scope_digest.starts_with("sha256:"));
    }

    #[test]
    fn preflight_rejects_unbounded_or_model_supplied_authority() {
        let mut oversized = request();
        oversized.goal = "x".repeat(MAX_GOAL_BYTES + 1);
        assert!(matches!(
            issuer().preflight(
                &identity(),
                "call-1",
                &oversized,
                LOCAL_CAPABILITY_SCOPE_REF,
                "dlg_1",
                "att_1",
            ),
            Err(DelegationIssuanceError::InvalidRequest)
        ));

        let parsed = serde_json::from_value::<DelegationRequest>(serde_json::json!({
            "goal": "inspect", "task_shape": "explore", "worker_profile": "explorer",
            "capability_scope_ref": "model-selected", "network_hosts": ["evil.example"]
        }));
        assert!(parsed.is_err(), "unknown authority fields must fail closed");
    }

    #[test]
    fn idempotency_key_rejects_model_controlled_parent_override() {
        let mut forged = identity();
        forged.parent_run_id = "run with spaces".into();
        assert!(DelegationIdempotencyKey::new(&forged, "call-1").is_err());
        assert!(DelegationIdempotencyKey::new(&identity(), "call/1").is_err());
    }

    #[test]
    fn lease_allowlist_is_the_concrete_worker_surface() {
        let local = request();
        assert_eq!(
            effective_tool_allowlist(
                &CapabilityPolicy::compile(TaskShape::Explore, WorkerProfile::Explorer),
                &local,
                false,
            ),
            BTreeSet::from(["file.read".to_owned()])
        );

        let mut network = request();
        network.explorer_operations =
            vec![super::super::explorer_plan::ExplorerPlanOperation::Search {
                provider_host: "search.example".into(),
                query: "architecture".into(),
            }];
        assert_eq!(
            effective_tool_allowlist(
                &CapabilityPolicy::compile(TaskShape::Explore, WorkerProfile::Explorer),
                &network,
                true,
            ),
            BTreeSet::from(["network.search".to_owned()])
        );
    }
}
