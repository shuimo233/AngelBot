//! Durable typed explorer intent and dispatch binding.
//!
//! This module is deliberately below the scheduler/worker seam.  It owns the
//! bounded plan document, its durable repository, and the fail-closed binding
//! resolver.  The resolver never parses `delegation_outbox.payload_json` and
//! never returns a filesystem path or capability root to a worker.

use std::fmt;

use chrono::Utc;
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    delegated_worker_launcher::{
        AsyncWorkerLauncher, AsyncWorkerLauncherBridge, WorkerAttemptBinding, WorkerEventCallback,
    },
    delegation_contract::{
        Confidence, DelegationDelivery, DelegationIdentity, KeyFact, UsageFlags,
        VerificationRecord, VerificationStatus,
    },
    delegation_runtime::{WorkerAdapter, WorkerDispatch},
    explorer_worker_runtime::{ExplorerOperation, ExplorerWorkPlan},
    shared_db::SharedDb,
    worker_policy::{TaskShape, WorkerProfile, WORKER_POLICY_VERSION},
};

pub const EXPLORER_PLAN_SCHEMA_VERSION: u16 = 1;
const MAX_PLAN_OPERATIONS: usize = 12;
const MAX_PLAN_SEARCHES: usize = 8;
const MAX_PLAN_FETCHES: usize = 8;
const MAX_PROVIDER_HOST_CHARS: usize = 253;
const MAX_QUERY_CHARS: usize = 2_048;
const MAX_URL_CHARS: usize = 4_096;

/// Model-visible operation intent.  It is bounded and contains no headers,
/// credentials, filesystem references, or transport configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExplorerPlanOperation {
    Search {
        provider_host: String,
        query: String,
    },
    Fetch {
        url: String,
        method: String,
    },
}

impl ExplorerPlanOperation {
    fn validate(&self) -> Result<(), ExplorerPlanError> {
        match self {
            Self::Search {
                provider_host,
                query,
            } => {
                if provider_host.trim().is_empty()
                    || provider_host.chars().count() > MAX_PROVIDER_HOST_CHARS
                    || provider_host.contains('/')
                    || provider_host.contains("://")
                    || provider_host.chars().any(char::is_whitespace)
                    || query.trim().is_empty()
                    || query.chars().count() > MAX_QUERY_CHARS
                    || query.contains('\0')
                {
                    return Err(ExplorerPlanError::InvalidPlan(
                        "invalid bounded explorer search operation".into(),
                    ));
                }
            }
            Self::Fetch { url, method } => {
                let method = method.trim().to_ascii_uppercase();
                if url.trim().is_empty()
                    || !url.starts_with("https://")
                    || url.chars().count() > MAX_URL_CHARS
                    || url.contains('\0')
                    || !(method == "GET" || method == "HEAD")
                    || reqwest::Url::parse(url)
                        .ok()
                        .and_then(|parsed| parsed.host_str().map(str::to_owned))
                        .is_none()
                {
                    return Err(ExplorerPlanError::InvalidPlan(
                        "invalid bounded explorer fetch operation".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn to_runtime(&self) -> ExplorerOperation {
        match self {
            Self::Search {
                provider_host,
                query,
            } => ExplorerOperation::Search {
                provider_host: provider_host.clone(),
                query: query.clone(),
            },
            Self::Fetch { url, method } => ExplorerOperation::Fetch {
                url: url.clone(),
                method: method.clone(),
            },
        }
    }
}

/// Canonical durable plan document.  A caller never supplies `plan_digest`;
/// it is computed from this typed serialization at the repository boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplorerPlanDocument {
    pub schema_version: u16,
    pub operations: Vec<ExplorerPlanOperation>,
}

impl ExplorerPlanDocument {
    pub fn new(operations: Vec<ExplorerPlanOperation>) -> Result<Self, ExplorerPlanError> {
        if operations.is_empty() || operations.len() > MAX_PLAN_OPERATIONS {
            return Err(ExplorerPlanError::InvalidPlan(
                "explorer operation bound exceeded".into(),
            ));
        }
        let searches = operations
            .iter()
            .filter(|operation| matches!(operation, ExplorerPlanOperation::Search { .. }))
            .count();
        let fetches = operations
            .iter()
            .filter(|operation| matches!(operation, ExplorerPlanOperation::Fetch { .. }))
            .count();
        if searches > MAX_PLAN_SEARCHES || fetches > MAX_PLAN_FETCHES {
            return Err(ExplorerPlanError::InvalidPlan(
                "explorer operation kind bound exceeded".into(),
            ));
        }
        for operation in &operations {
            operation.validate()?;
        }
        Ok(Self {
            schema_version: EXPLORER_PLAN_SCHEMA_VERSION,
            operations,
        })
    }

    fn canonical_json(&self) -> Result<String, ExplorerPlanError> {
        if self.schema_version != EXPLORER_PLAN_SCHEMA_VERSION {
            return Err(ExplorerPlanError::UnsupportedSchema(self.schema_version));
        }
        // Re-run all limits after deserialization; durable JSON is untrusted
        // even when it came from an earlier successful issuance.
        Self::new(self.operations.clone())?;
        serde_json::to_string(self)
            .map_err(|error| ExplorerPlanError::InvalidPlan(error.to_string()))
    }

    pub fn digest(&self) -> Result<String, ExplorerPlanError> {
        let json = self.canonical_json()?;
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
    }

    fn into_runtime_plan(
        &self,
        identity: DelegationIdentity,
        digest: &str,
    ) -> Result<ExplorerWorkPlan, ExplorerPlanError> {
        let delivery = DelegationDelivery {
            schema_version: 1,
            delivery_id: format!("explorer-delivery-{}", identity.attempt_id),
            delivery_revision: 1,
            identity,
            status: super::delegation_contract::DeliveryStatus::Completed,
            executive_summary: "Explorer worker completed bounded operations".into(),
            key_facts: vec![KeyFact {
                statement: "Bounded explorer operations completed".into(),
                confidence: Confidence::High,
                evidence_refs: vec![format!("evidence://explorer/plan/{digest}")],
            }],
            milestones: Vec::new(),
            verifications: vec![VerificationRecord {
                item: "Explorer operation plan".into(),
                method: "bounded worker host".into(),
                status: VerificationStatus::Passed,
                conclusion: "All planned operations returned to the host".into(),
                evidence_ref: format!("evidence://explorer/plan/{digest}"),
            }],
            candidate_artifacts: Vec::new(),
            open_questions: Vec::new(),
            risks: Vec::new(),
            evidence_refs: vec![format!("evidence://explorer/plan/{digest}")],
            usage: UsageFlags {
                context_truncated: false,
                output_truncated: false,
                budget_exhausted: false,
            },
        };
        ExplorerWorkPlan::new(
            self.operations
                .iter()
                .map(ExplorerPlanOperation::to_runtime)
                .collect(),
            delivery,
        )
        .map_err(|error| ExplorerPlanError::InvalidPlan(format!("{error:?}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerPlanRecord {
    pub id: String,
    pub delegation_id: String,
    pub attempt_id: String,
    pub work_package_id: String,
    pub scope_digest: String,
    pub worker_policy_version: u16,
    pub plan: ExplorerPlanDocument,
    pub plan_digest: String,
    pub created_at: i64,
}

impl ExplorerPlanRecord {
    pub fn new(
        id: impl Into<String>,
        delegation_id: impl Into<String>,
        attempt_id: impl Into<String>,
        work_package_id: impl Into<String>,
        scope_digest: impl Into<String>,
        worker_policy_version: u16,
        plan: ExplorerPlanDocument,
        created_at: i64,
    ) -> Result<Self, ExplorerPlanError> {
        let id = id.into();
        let delegation_id = delegation_id.into();
        let attempt_id = attempt_id.into();
        let work_package_id = work_package_id.into();
        let scope_digest = scope_digest.into();
        if [
            id.as_str(),
            delegation_id.as_str(),
            attempt_id.as_str(),
            work_package_id.as_str(),
        ]
        .iter()
        .any(|value| value.trim().is_empty())
            || scope_digest.trim().is_empty()
            || worker_policy_version == 0
            || created_at <= 0
        {
            return Err(ExplorerPlanError::InvalidPlan(
                "explorer plan identity is invalid".into(),
            ));
        }
        let plan_digest = plan.digest()?;
        Ok(Self {
            id,
            delegation_id,
            attempt_id,
            work_package_id,
            scope_digest,
            worker_policy_version,
            plan,
            plan_digest,
            created_at,
        })
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ExplorerPlanError {
    #[error("explorer plan was not found for dispatch")]
    NotFound,
    #[error("explorer dispatch identity is invalid")]
    InvalidDispatch,
    #[error("explorer plan is invalid: {0}")]
    InvalidPlan(String),
    #[error("unsupported explorer plan schema version: {0}")]
    UnsupportedSchema(u16),
    #[error("explorer plan binding drifted")]
    BindingMismatch,
    #[error("explorer capability lease is inactive or expired")]
    LeaseInactive,
    #[error("explorer plan storage failed: {0}")]
    Storage(String),
}

impl ExplorerPlanError {
    /// Stable, bounded classification for durable control-plane events.  Raw
    /// storage/model errors must never be persisted as worker-facing payload.
    pub fn class(&self) -> &'static str {
        match self {
            Self::NotFound => "missing_plan",
            Self::InvalidDispatch => "invalid_dispatch",
            Self::InvalidPlan(_) | Self::UnsupportedSchema(_) => "invalid_plan",
            Self::BindingMismatch => "binding_mismatch",
            Self::LeaseInactive => "lease_inactive",
            Self::Storage(_) => "storage_failure",
        }
    }
}

impl From<rusqlite::Error> for ExplorerPlanError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

/// Short-lock repository.  It exposes both an atomic insert seam for issuance
/// and a durable dispatch lookup; no connection or mutex guard escapes.
#[derive(Clone)]
pub struct ExplorerPlanRepository {
    db: SharedDb,
    now: fn() -> i64,
}

impl ExplorerPlanRepository {
    pub fn new(db: SharedDb) -> Self {
        Self {
            db,
            now: || Utc::now().timestamp(),
        }
    }

    pub fn with_clock(db: SharedDb, now: fn() -> i64) -> Self {
        Self { db, now }
    }

    pub fn db(&self) -> &SharedDb {
        &self.db
    }

    pub fn create(&self, record: &ExplorerPlanRecord) -> Result<(), ExplorerPlanError> {
        self.db
            .with_conn_mut(|conn| {
                let tx = conn.transaction().map_err(ExplorerPlanError::from)?;
                Self::insert_in_tx(&tx, record)?;
                tx.commit().map_err(ExplorerPlanError::from)
            })
            .map_err(|error| ExplorerPlanError::Storage(error.to_string()))?
    }

    /// Atomic issuance seam. The caller owns the surrounding transaction and
    /// must insert delegation/attempt/lease rows before invoking this method.
    pub fn insert_in_tx(
        tx: &Transaction<'_>,
        record: &ExplorerPlanRecord,
    ) -> Result<(), ExplorerPlanError> {
        let plan_json = record.plan.canonical_json()?;
        let digest = record.plan.digest()?;
        if digest != record.plan_digest {
            return Err(ExplorerPlanError::InvalidPlan(
                "plan digest does not match canonical typed document".into(),
            ));
        }
        tx.execute(
            "INSERT INTO delegation_explorer_plans (id, delegation_id, attempt_id, work_package_id, scope_digest, worker_policy_version, schema_version, plan_json, plan_digest, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                record.id,
                record.delegation_id,
                record.attempt_id,
                record.work_package_id,
                record.scope_digest,
                i64::from(record.worker_policy_version),
                i64::from(record.plan.schema_version),
                plan_json,
                record.plan_digest,
                record.created_at,
            ],
        )
        .map_err(ExplorerPlanError::from)?;
        Ok(())
    }

    /// Load the immutable source plan for one attempt without requiring that
    /// attempt to still be dispatchable. Retry issuance uses this seam to copy
    /// a reviewed source plan into a fresh queued attempt; runtime dispatch
    /// must continue to use [`Self::load_for_dispatch`].
    pub(crate) fn load_for_attempt_in_connection(
        conn: &rusqlite::Connection,
        delegation_id: &str,
        attempt_id: &str,
        work_package_id: &str,
    ) -> Result<Option<ExplorerPlanRecord>, ExplorerPlanError> {
        if [delegation_id, attempt_id, work_package_id]
            .iter()
            .any(|value| value.trim().is_empty())
        {
            return Err(ExplorerPlanError::InvalidPlan(
                "explorer plan lookup identity is invalid".into(),
            ));
        }
        let row: Option<(
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
            String,
            String,
            i64,
            String,
            String,
            String,
            i64,
        )> = conn
            .query_row(
                "SELECT ep.id, ep.delegation_id, ep.attempt_id, ep.work_package_id,
                        ep.scope_digest, ep.worker_policy_version, ep.schema_version,
                        ep.plan_json, ep.plan_digest, ep.created_at, p.task_shape,
                        p.worker_profile, p.scope_digest, p.worker_policy_version
                 FROM delegation_explorer_plans ep
                 JOIN delegations d ON d.id = ep.delegation_id
                 JOIN delegation_attempts a
                   ON a.id = ep.attempt_id
                  AND a.delegation_id = d.id
                 JOIN work_packages p ON p.id = ep.work_package_id
                 WHERE ep.delegation_id = ?1
                   AND ep.attempt_id = ?2
                   AND ep.work_package_id = ?3
                   AND d.work_package_id = ?3",
                params![delegation_id, attempt_id, work_package_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                        row.get(12)?,
                        row.get(13)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            id,
            delegation_id,
            attempt_id,
            work_package_id,
            plan_scope_digest,
            plan_policy_version,
            schema_version,
            plan_json,
            plan_digest,
            created_at,
            package_shape,
            package_profile,
            package_scope_digest,
            package_policy_version,
        )) = row
        else {
            return Ok(None);
        };
        if package_shape != TaskShape::Explore.as_str()
            || package_profile != WorkerProfile::Explorer.as_str()
            || plan_scope_digest != package_scope_digest
            || plan_policy_version != package_policy_version
            || package_policy_version != i64::from(WORKER_POLICY_VERSION)
        {
            return Err(ExplorerPlanError::BindingMismatch);
        }
        let schema_version =
            u16::try_from(schema_version).map_err(|_| ExplorerPlanError::UnsupportedSchema(0))?;
        if schema_version != EXPLORER_PLAN_SCHEMA_VERSION {
            return Err(ExplorerPlanError::UnsupportedSchema(schema_version));
        }
        let plan: ExplorerPlanDocument = serde_json::from_str(&plan_json)
            .map_err(|error| ExplorerPlanError::InvalidPlan(error.to_string()))?;
        if plan.schema_version != schema_version || plan.digest()? != plan_digest {
            return Err(ExplorerPlanError::BindingMismatch);
        }
        ExplorerPlanRecord::new(
            id,
            delegation_id,
            attempt_id,
            work_package_id,
            plan_scope_digest,
            u16::try_from(plan_policy_version).map_err(|_| ExplorerPlanError::BindingMismatch)?,
            plan,
            created_at,
        )
        .map(Some)
    }

    pub fn load_for_dispatch(
        &self,
        dispatch: &WorkerDispatch,
    ) -> Result<ExplorerPlanRecord, ExplorerPlanError> {
        if dispatch.outbox_id.trim().is_empty()
            || dispatch.attempt_id.trim().is_empty()
            || dispatch.delegation_id.trim().is_empty()
            || dispatch.epoch == 0
        {
            return Err(ExplorerPlanError::InvalidDispatch);
        }
        let now = (self.now)();
        self.db
            .with_conn(|conn| load_for_dispatch(conn, dispatch, now))
            .map_err(|error| ExplorerPlanError::Storage(error.to_string()))?
    }

    /// Record a bounded fail-closed resolver outcome in the attempt event
    /// stream.  This is best-effort and idempotent by attempt/sequence; the
    /// caller must not include raw plan, payload, path, or storage details.
    pub fn record_binding_failure(
        &self,
        dispatch: &WorkerDispatch,
        class: &'static str,
    ) -> Result<(), ExplorerPlanError> {
        let allowed = matches!(
            class,
            "missing_plan"
                | "invalid_dispatch"
                | "invalid_plan"
                | "binding_mismatch"
                | "lease_inactive"
                | "storage_failure"
        );
        if !allowed || dispatch.attempt_id.trim().is_empty() {
            return Err(ExplorerPlanError::InvalidDispatch);
        }
        let now = (self.now)();
        self.db
            .with_conn_mut(|conn| {
                let tx = conn.transaction().map_err(ExplorerPlanError::from)?;
                let sequence: i64 = tx
                    .query_row(
                        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM delegation_attempt_events WHERE attempt_id = ?1",
                        [dispatch.attempt_id.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(ExplorerPlanError::from)?;
                let payload = serde_json::json!({
                    "kind": "explorer_binding_rejected",
                    "class": class,
                    "epoch": dispatch.epoch,
                })
                .to_string();
                tx.execute(
                    "INSERT INTO delegation_attempt_events (id, attempt_id, sequence, event_type, payload_json, created_at)
                     VALUES (?1, ?2, ?3, 'binding_rejected', ?4, ?5)",
                    params![
                        format!("{}:{}", dispatch.attempt_id, sequence),
                        dispatch.attempt_id,
                        sequence,
                        payload,
                        now,
                    ],
                )
                .map_err(ExplorerPlanError::from)?;
                tx.commit().map_err(ExplorerPlanError::from)
            })
            .map_err(|error| ExplorerPlanError::Storage(error.to_string()))?
    }
}

/// Resolver seam between the durable scheduler and Explorer worker launcher.
#[derive(Clone)]
pub struct ExplorerBindingResolver {
    repository: ExplorerPlanRepository,
}

impl ExplorerBindingResolver {
    pub fn new(repository: ExplorerPlanRepository) -> Self {
        Self { repository }
    }

    pub fn repository(&self) -> &ExplorerPlanRepository {
        &self.repository
    }

    pub fn resolve(
        &self,
        dispatch: &WorkerDispatch,
    ) -> Result<(WorkerAttemptBinding, ExplorerWorkPlan), ExplorerPlanError> {
        let record = self.repository.load_for_dispatch(dispatch)?;
        let identity = load_identity(self.repository.db(), &record, dispatch)?;
        let plan = record
            .plan
            .into_runtime_plan(identity, &record.plan_digest)?;
        let capability_ref = load_capability_ref(self.repository.db(), &record)?;
        let binding = WorkerAttemptBinding {
            attempt_id: record.attempt_id.clone(),
            delegation_id: record.delegation_id.clone(),
            work_package_id: record.work_package_id.clone(),
            lease_epoch: dispatch.epoch,
            capability_ref,
            // The actual path remains inside the worker host. This opaque ref
            // is stable for the attempt and contains no filesystem location.
            sandbox_ref: format!("sandbox://{}", record.attempt_id),
        };
        Ok((binding, plan))
    }
}

/// Explicit injection seam for an Explorer worker.  The production
/// composition root intentionally does not construct this type: callers must
/// provide both the typed-plan repository and the async launcher host.
pub struct ExplorerWorkerLauncherFactory<L> {
    launcher: L,
    resolver: ExplorerBindingResolver,
}

impl<L> ExplorerWorkerLauncherFactory<L>
where
    L: AsyncWorkerLauncher,
{
    pub fn new(launcher: L, resolver: ExplorerBindingResolver) -> Self {
        Self { launcher, resolver }
    }

    pub fn build(self, callback: WorkerEventCallback) -> ResolvedExplorerWorkerAdapter<L> {
        ResolvedExplorerWorkerAdapter {
            resolver: self.resolver,
            bridge: AsyncWorkerLauncherBridge::new(self.launcher, callback),
        }
    }
}

/// Worker adapter that performs durable Explorer binding resolution before
/// handing an opaque request to the async launcher bridge.  It never parses
/// `WorkerDispatch::payload_json`; the dispatch is only an outbox identity and
/// epoch for the repository lookup.
pub struct ResolvedExplorerWorkerAdapter<L> {
    resolver: ExplorerBindingResolver,
    bridge: AsyncWorkerLauncherBridge<L>,
}

impl<L> ResolvedExplorerWorkerAdapter<L>
where
    L: AsyncWorkerLauncher,
{
    pub fn poll(&mut self) -> Result<(), super::delegated_worker_launcher::WorkerLaunchError> {
        self.bridge.poll()
    }

    pub fn active_attempts(&self) -> usize {
        self.bridge.active_attempts()
    }

    pub fn active_attempt_ids(&self) -> Vec<String> {
        self.bridge.active_attempt_ids()
    }

    pub fn stop_all(
        &mut self,
        reason: super::delegation_runtime::StopReason,
    ) -> Result<(), super::delegated_worker_launcher::WorkerLaunchError> {
        self.bridge.stop_all(reason)
    }

    pub fn resolver(&self) -> &ExplorerBindingResolver {
        &self.resolver
    }
}

impl<L> WorkerAdapter for ResolvedExplorerWorkerAdapter<L>
where
    L: AsyncWorkerLauncher,
{
    fn start(&mut self, dispatch: WorkerDispatch) -> Result<(), String> {
        let (binding, plan) = match self.resolver.resolve(&dispatch) {
            Ok(resolved) => resolved,
            Err(error) => {
                let class = error.class();
                let _ = self
                    .resolver
                    .repository()
                    .record_binding_failure(&dispatch, class);
                return Err(format!("explorer binding rejected: {class}"));
            }
        };
        if let Err(error) = self.bridge.bind(binding) {
            let _ = self
                .resolver
                .repository()
                .record_binding_failure(&dispatch, "binding_mismatch");
            return Err(format!("explorer binding rejected: {error:?}"));
        }
        self.bridge.start_with_plan(dispatch, Some(plan))
    }

    fn stop(
        &mut self,
        attempt_id: &str,
        reason: super::delegation_runtime::StopReason,
    ) -> Result<(), String> {
        self.bridge.stop(attempt_id, reason)
    }

    fn poll_workers(&mut self) -> Result<Vec<super::delegation_runtime::WorkerEvent>, String> {
        self.bridge.poll_workers()
    }

    fn active_worker_attempts(&self) -> usize {
        self.bridge.active_attempts()
    }
}

fn load_for_dispatch(
    conn: &rusqlite::Connection,
    dispatch: &WorkerDispatch,
    now: i64,
) -> Result<ExplorerPlanRecord, ExplorerPlanError> {
    let row: Option<(
        String,
        String,
        String,
        i64,
        String,
        String,
        i64,
        i64,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
    )> = conn
        .query_row(
            "SELECT ep.id, ep.delegation_id, ep.attempt_id, a.attempt_number,
                    ep.work_package_id,
                    ep.scope_digest, ep.worker_policy_version, ep.schema_version,
                    ep.plan_json, ep.plan_digest, d.session_id, d.message_id,
                    d.parent_run_id, p.task_shape, p.worker_profile,
                    p.scope_digest, p.worker_policy_version
             FROM delegation_explorer_plans ep
             JOIN delegations d ON d.id = ep.delegation_id
             JOIN delegation_attempts a
               ON a.id = ep.attempt_id
              AND a.delegation_id = d.id
             JOIN work_packages p ON p.id = ep.work_package_id
             JOIN delegation_capability_leases l ON l.attempt_id = a.id
             JOIN delegation_outbox o ON o.attempt_id = a.id
             WHERE o.id = ?1 AND o.attempt_id = ?2
               AND ep.attempt_id = ?2 AND d.id = ?3
               AND a.status = 'running' AND d.status = 'running'
               AND l.status = 'active' AND l.expires_at > ?4
               AND p.task_shape = 'explore' AND p.worker_profile = 'explorer'",
            params![
                dispatch.outbox_id,
                dispatch.attempt_id,
                dispatch.delegation_id,
                now
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                    row.get(15)?,
                    row.get(16)?,
                ))
            },
        )
        .optional()?;
    let Some((
        id,
        delegation_id,
        attempt_id,
        attempt_number,
        work_package_id,
        plan_scope_digest,
        plan_policy_version,
        schema_version,
        plan_json,
        plan_digest,
        _session_id,
        _message_id,
        _parent_run_id,
        package_shape,
        package_profile,
        package_scope_digest,
        package_policy_version,
    )) = row
    else {
        return Err(ExplorerPlanError::NotFound);
    };
    let attempt_epoch =
        u64::try_from(attempt_number).map_err(|_| ExplorerPlanError::BindingMismatch)?;
    if dispatch.attempt_id != attempt_id
        || dispatch.epoch != attempt_epoch
        || package_shape != TaskShape::Explore.as_str()
        || package_profile != WorkerProfile::Explorer.as_str()
        || plan_scope_digest != package_scope_digest
        || plan_policy_version != package_policy_version
        || package_policy_version != i64::from(WORKER_POLICY_VERSION)
    {
        return Err(ExplorerPlanError::BindingMismatch);
    }
    let schema_version =
        u16::try_from(schema_version).map_err(|_| ExplorerPlanError::UnsupportedSchema(0))?;
    if schema_version != EXPLORER_PLAN_SCHEMA_VERSION {
        return Err(ExplorerPlanError::UnsupportedSchema(schema_version));
    }
    let plan: ExplorerPlanDocument = serde_json::from_str(&plan_json)
        .map_err(|error| ExplorerPlanError::InvalidPlan(error.to_string()))?;
    if plan.schema_version != schema_version || plan.digest()? != plan_digest {
        return Err(ExplorerPlanError::BindingMismatch);
    }
    // The durable dispatch journal is the only source of the epoch. Do not
    // infer a lease epoch from arbitrary outbox JSON.
    let epoch_payload: Option<String> = conn
        .query_row(
            "SELECT payload_json FROM delegation_attempt_events WHERE attempt_id = ?1 AND event_type = 'attempt_dispatched' ORDER BY sequence DESC LIMIT 1",
            [attempt_id.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    let Some(epoch_payload) = epoch_payload else {
        return Err(ExplorerPlanError::BindingMismatch);
    };
    let epoch = serde_json::from_str::<DispatchEpoch>(&epoch_payload)
        .map_err(|_| ExplorerPlanError::BindingMismatch)?
        .epoch;
    if epoch != attempt_epoch || epoch != dispatch.epoch {
        return Err(ExplorerPlanError::BindingMismatch);
    }
    ExplorerPlanRecord::new(
        id,
        delegation_id,
        attempt_id,
        work_package_id,
        plan_scope_digest,
        u16::try_from(plan_policy_version).map_err(|_| ExplorerPlanError::BindingMismatch)?,
        plan,
        now,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchEpoch {
    epoch: u64,
}

fn load_identity(
    db: &SharedDb,
    record: &ExplorerPlanRecord,
    dispatch: &WorkerDispatch,
) -> Result<DelegationIdentity, ExplorerPlanError> {
    db.with_conn(|conn| {
        conn.query_row(
            "SELECT session_id, message_id, parent_run_id FROM delegations WHERE id = ?1 AND work_package_id = ?2",
            params![record.delegation_id, record.work_package_id],
            |row| {
                Ok(DelegationIdentity {
                    session_id: row.get(0)?,
                    message_id: row.get(1)?,
                    parent_run_id: row.get(2)?,
                    delegation_id: dispatch.delegation_id.clone(),
                    attempt_id: dispatch.attempt_id.clone(),
                })
            },
        )
        .optional()
        .map_err(ExplorerPlanError::from)
        .and_then(|identity| identity.ok_or(ExplorerPlanError::BindingMismatch))
    })
    .map_err(|error| ExplorerPlanError::Storage(error.to_string()))?
}

fn load_capability_ref(
    db: &SharedDb,
    record: &ExplorerPlanRecord,
) -> Result<String, ExplorerPlanError> {
    db.with_conn(|conn| {
        conn.query_row(
            "SELECT p.capability_scope_ref FROM work_packages p WHERE p.id = ?1 AND p.scope_digest = ?2",
            params![record.work_package_id, record.scope_digest],
            |row| row.get(0),
        )
        .optional()
        .map_err(ExplorerPlanError::from)
        .and_then(|value: Option<String>| value.ok_or(ExplorerPlanError::BindingMismatch))
    })
    .map_err(|error| ExplorerPlanError::Storage(error.to_string()))?
}

impl fmt::Debug for ExplorerPlanRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExplorerPlanRepository")
            .field("db", &"shared")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::delegated_attempt_scheduler::DelegatedAttemptScheduler;
    use crate::agent::delegated_worker_launcher::{
        WorkerLaunchError, WorkerLaunchRequest, WorkerTask, WorkerTaskEvent,
    };
    use crate::agent::delegation::{
        DelegationRepository, NewAttempt, NewCapabilityLease, NewDelegation,
    };
    use crate::agent::delegation_contract::{
        Confidence, ContractLimits, DelegationBrief, DelegationDelivery, DelegationIdentity,
        DeliveryStatus, KeyFact, UsageFlags, VerificationRecord, VerificationStatus,
    };
    use crate::agent::delegation_runtime::{
        Clock, FixedAdmissionPolicy, Heartbeat, StopReason, WorkerDispatch,
    };
    use crate::agent::delegation_service::DelegationService;
    use crate::agent::delivery_inbox::ParentDecision;
    use crate::agent::review_agent::FixedReviewExecutor;
    use crate::agent::review_contract::{ReviewOutcome, SemanticReviewVerdict};
    use crate::agent::review_coordinator::ReviewCoordinator;
    use crate::agent::review_store::ReviewJobStatus;
    use crate::agent::terminal_cleanup::{
        CleanupBinding, CleanupPending, CleanupReport, TerminalCleanupPort,
    };
    use crate::agent::workspace_admission::WorkspaceAdmission;
    use rusqlite::Connection;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    fn db() -> SharedDb {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        SharedDb::new(conn)
    }

    fn seed(db: &SharedDb) {
        db.with_conn_mut(|conn| {
            conn.execute("INSERT INTO sessions(id,title,created_at,updated_at) VALUES ('s','s',1,1)", []).unwrap();
            conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('m','s','user','m',1)", []).unwrap();
            conn.execute("INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES ('r','s','m','g','running','[]',1,1)", []).unwrap();
            conn.execute("INSERT INTO work_packages(id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,candidate_version,created_at,updated_at,status) VALUES ('wp','s',1,'wk','explore','explorer',1,'scope','approved',1000,0,1,1,'active')", []).unwrap();
            conn.execute("INSERT INTO delegations(id,session_id,message_id,parent_run_id,work_package_id,objective,brief_json,status,state_version,created_at,updated_at) VALUES ('d','s','m','r','wp','g','{}','queued',0,1,1)", []).unwrap();
            conn.execute("INSERT INTO delegation_attempts(id,delegation_id,attempt_number,status,sandbox_ref,created_at) VALUES ('a','d',1,'queued','/private/path',1)", []).unwrap();
        }).unwrap();
    }

    fn record() -> ExplorerPlanRecord {
        ExplorerPlanRecord::new(
            "ep",
            "d",
            "a",
            "wp",
            "scope",
            1,
            ExplorerPlanDocument::new(vec![ExplorerPlanOperation::Search {
                provider_host: "docs.example.com".into(),
                query: "bounded".into(),
            }])
            .unwrap(),
            1,
        )
        .unwrap()
    }

    fn dispatch() -> WorkerDispatch {
        WorkerDispatch {
            outbox_id: "o".into(),
            delegation_id: "d".into(),
            attempt_id: "a".into(),
            payload_json: "malformed should be ignored".into(),
            epoch: 1,
        }
    }

    fn make_dispatchable(db: &SharedDb) {
        db.with_conn_mut(|conn| {
            conn.execute("UPDATE delegation_attempts SET status='running', started_at=1 WHERE id='a'", []).unwrap();
            conn.execute("INSERT INTO delegation_capability_leases(id,attempt_id,read_roots_json,write_roots_json,tool_allowlist_json,network_hosts_json,budget_json,status,issued_at,expires_at,revoked_at) VALUES ('l','a','[]','[]','[\"network.search\"]','[\"docs.example.com\"]','{}','active',1,1000,NULL)", []).unwrap();
            conn.execute("INSERT INTO delegation_outbox(id,attempt_id,sequence,event_type,payload_json,created_at,dispatched_at) VALUES ('o','a',1,'dispatch_attempt','not-plan-json',1,1)", []).unwrap();
            conn.execute("INSERT INTO delegation_attempt_events(id,attempt_id,sequence,event_type,payload_json,created_at) VALUES ('a:1','a',1,'attempt_dispatched','{\"epoch\":1}',1)", []).unwrap();
            conn.execute("UPDATE delegations SET status='running' WHERE id='d'", []).unwrap();
        }).unwrap();
    }

    #[test]
    fn create_and_load_ignores_outbox_payload_and_survives_shared_db_clone() {
        let db = db();
        seed(&db);
        let repo = ExplorerPlanRepository::with_clock(db.clone(), || 2);
        repo.create(&record()).unwrap();
        make_dispatchable(&db);
        let restarted = ExplorerPlanRepository::with_clock(db.clone(), || 2);
        let loaded = restarted.load_for_dispatch(&dispatch()).unwrap();
        assert_eq!(loaded.plan_digest, record().plan_digest);
        assert_eq!(loaded.plan.operations.len(), 1);
        let resolver = ExplorerBindingResolver::new(restarted);
        let (binding, plan) = resolver.resolve(&dispatch()).unwrap();
        assert_eq!(binding.sandbox_ref, "sandbox://a");
        assert_eq!(binding.capability_ref, "approved");
        assert_eq!(plan.operations.len(), 1);
    }

    #[test]
    fn retry_plan_is_bound_to_its_new_attempt_and_epoch() {
        let db = db();
        seed(&db);
        let repo = ExplorerPlanRepository::with_clock(db.clone(), || 2);
        let initial = record();
        repo.create(&initial).unwrap();
        let source = db
            .with_conn(|conn| {
                ExplorerPlanRepository::load_for_attempt_in_connection(conn, "d", "a", "wp")
            })
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(source.attempt_id, "a");
        assert_eq!(source.plan_digest, initial.plan_digest);
        db.with_conn_mut(|conn| {
            // Retry issuance seals its source attempt before re-queueing the
            // stable delegation, preserving the one-active-attempt invariant.
            conn.execute(
                "UPDATE delegations SET status='needs_decision' WHERE id='d'",
                [],
            )
            .unwrap();
            conn.execute(
                "UPDATE delegation_attempts SET status='sealed', ended_at=2 WHERE id='a'",
                [],
            )
            .unwrap();
            conn.execute("UPDATE delegations SET status='queued' WHERE id='d'", [])
                .unwrap();
            conn.execute(
                "INSERT INTO delegation_attempts(id,delegation_id,attempt_number,status,sandbox_ref,created_at) VALUES ('a2','d',2,'queued','/private/path-2',2)",
                [],
            )
            .unwrap();
        })
        .unwrap();
        let retry =
            ExplorerPlanRecord::new("ep-2", "d", "a2", "wp", "scope", 1, initial.plan.clone(), 2)
                .unwrap();
        repo.create(&retry).unwrap();
        db.with_conn(|conn| {
            let attempts: Vec<String> = conn
                .prepare(
                    "SELECT attempt_id FROM delegation_explorer_plans WHERE delegation_id='d' ORDER BY attempt_id",
                )
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            assert_eq!(attempts, vec!["a".to_string(), "a2".to_string()]);
        })
        .unwrap();
        db.with_conn_mut(|conn| {
            conn.execute(
                "UPDATE delegation_attempts SET status='running', started_at=2 WHERE id='a2'",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO delegation_capability_leases(id,attempt_id,read_roots_json,write_roots_json,tool_allowlist_json,network_hosts_json,budget_json,status,issued_at,expires_at,revoked_at) VALUES ('l2','a2','[]','[]','[\"network.search\"]','[\"docs.example.com\"]','{}','active',2,1000,NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO delegation_outbox(id,attempt_id,sequence,event_type,payload_json,created_at,dispatched_at) VALUES ('o2','a2',1,'dispatch_attempt','not-plan-json',2,2)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO delegation_attempt_events(id,attempt_id,sequence,event_type,payload_json,created_at) VALUES ('a2:1','a2',1,'attempt_dispatched','{\"epoch\":2}',2)",
                [],
            )
            .unwrap();
            conn.execute("UPDATE delegations SET status='running' WHERE id='d'", [])
                .unwrap();
        })
        .unwrap();

        let retry_dispatch = WorkerDispatch {
            outbox_id: "o2".into(),
            delegation_id: "d".into(),
            attempt_id: "a2".into(),
            payload_json: "malformed should be ignored".into(),
            epoch: 2,
        };
        let loaded = repo.load_for_dispatch(&retry_dispatch).unwrap();
        assert_eq!(loaded.attempt_id, "a2");
        assert_eq!(loaded.plan_digest, initial.plan_digest);

        let mut stale = retry_dispatch;
        stale.epoch = 1;
        assert_eq!(
            repo.load_for_dispatch(&stale),
            Err(ExplorerPlanError::BindingMismatch)
        );
    }

    #[test]
    fn unknown_plan_and_scope_drift_fail_closed() {
        let db = db();
        seed(&db);
        let repo = ExplorerPlanRepository::with_clock(db.clone(), || 2);
        assert_eq!(
            repo.load_for_dispatch(&dispatch()),
            Err(ExplorerPlanError::NotFound)
        );
        repo.create(&record()).unwrap();
        make_dispatchable(&db);
        db.with_conn_mut(|conn| {
            conn.execute(
                "UPDATE work_packages SET scope_digest='drift' WHERE id='wp'",
                [],
            )
        })
        .unwrap()
        .unwrap();
        assert_eq!(
            repo.load_for_dispatch(&dispatch()),
            Err(ExplorerPlanError::BindingMismatch)
        );
    }

    #[test]
    fn oversized_or_unknown_document_is_rejected() {
        let too_many = (0..13)
            .map(|_| ExplorerPlanOperation::Search {
                provider_host: "docs.example.com".into(),
                query: "q".into(),
            })
            .collect();
        assert!(matches!(
            ExplorerPlanDocument::new(too_many),
            Err(ExplorerPlanError::InvalidPlan(_))
        ));
        let unknown = serde_json::from_str::<ExplorerPlanDocument>(
            r#"{"schema_version":1,"operations":[],"unexpected":true}"#,
        );
        assert!(unknown.is_err());
    }

    #[test]
    fn resolved_adapter_records_bounded_binding_rejection_without_launch() {
        let db = db();
        seed(&db);
        make_dispatchable(&db);
        let repo = ExplorerPlanRepository::with_clock(db.clone(), || 2);
        let resolver = ExplorerBindingResolver::new(repo);
        let callback: WorkerEventCallback = Box::new(|_| Ok(()));
        let mut adapter = ExplorerWorkerLauncherFactory::new(
            super::super::delegated_worker_launcher::FailClosedAsyncLauncher,
            resolver,
        )
        .build(callback);

        let error = adapter.start(dispatch()).unwrap_err();
        assert!(error.contains("missing_plan"));
        assert_eq!(adapter.active_attempts(), 0);
        db.with_conn(|conn| {
            let (event_type, payload): (String, String) = conn.query_row(
                "SELECT event_type, payload_json FROM delegation_attempt_events WHERE attempt_id='a' ORDER BY sequence DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).unwrap();
            assert_eq!(event_type, "binding_rejected");
            let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
            assert_eq!(value["kind"], "explorer_binding_rejected");
            assert_eq!(value["class"], "missing_plan");
            assert_eq!(value["epoch"], 1);
            assert!(!payload.contains("malformed"));
        }).unwrap();
    }

    #[test]
    fn stale_epoch_rejected_and_duplicate_start_is_idempotently_blocked() {
        let db = db();
        seed(&db);
        let repo = ExplorerPlanRepository::with_clock(db.clone(), || 2);
        repo.create(&record()).unwrap();
        make_dispatchable(&db);
        let launches = Arc::new(Mutex::new(Vec::new()));
        let callback: WorkerEventCallback = Box::new(|_| Ok(()));
        let mut adapter = ExplorerWorkerLauncherFactory::new(
            FakeLauncher {
                launches: launches.clone(),
            },
            ExplorerBindingResolver::new(repo),
        )
        .build(callback);
        let mut stale = dispatch();
        stale.epoch = 2;
        assert!(adapter
            .start(stale)
            .unwrap_err()
            .contains("binding_mismatch"));
        assert_eq!(launches.lock().unwrap().len(), 0);

        adapter.start(dispatch()).unwrap();
        let duplicate = adapter.start(dispatch()).unwrap_err();
        assert!(duplicate.contains("DuplicateBinding"));
        assert_eq!(launches.lock().unwrap().len(), 1);
    }

    struct TestClock(i64);
    impl Clock for TestClock {
        fn now(&self) -> i64 {
            self.0
        }
    }

    struct FakeTask {
        events: VecDeque<WorkerTaskEvent>,
    }
    impl WorkerTask for FakeTask {
        fn poll(&mut self) -> Result<Option<WorkerTaskEvent>, WorkerLaunchError> {
            Ok(self.events.pop_front())
        }
        fn cancel(&mut self, _: StopReason) -> Result<(), WorkerLaunchError> {
            self.events.clear();
            Ok(())
        }
        fn join(&mut self) -> Result<Vec<WorkerTaskEvent>, WorkerLaunchError> {
            Ok(self.events.drain(..).collect())
        }
    }

    struct FakeLauncher {
        launches: Arc<Mutex<Vec<WorkerLaunchRequest>>>,
    }
    impl AsyncWorkerLauncher for FakeLauncher {
        fn launch(
            &mut self,
            request: WorkerLaunchRequest,
        ) -> Result<Box<dyn WorkerTask>, WorkerLaunchError> {
            self.launches.lock().unwrap().push(request.clone());
            let identity = DelegationIdentity {
                session_id: "s".into(),
                message_id: "m".into(),
                parent_run_id: "r".into(),
                delegation_id: request.dispatch.delegation_id.clone(),
                attempt_id: request.dispatch.attempt_id.clone(),
            };
            let heartbeat = Heartbeat {
                attempt_id: request.dispatch.attempt_id.clone(),
                epoch: request.dispatch.epoch,
                stage: "explore".into(),
                progress_percent: 50,
                budget_remaining: "1".into(),
                evidence_refs: vec!["evidence://test".into()],
            };
            let delivery = DelegationDelivery {
                schema_version: 1,
                delivery_id: "delivery-a".into(),
                delivery_revision: 1,
                identity,
                status: DeliveryStatus::Completed,
                executive_summary: "exploration completed".into(),
                key_facts: vec![KeyFact {
                    statement: "bounded result".into(),
                    confidence: Confidence::High,
                    evidence_refs: vec!["evidence://test".into()],
                }],
                milestones: vec![],
                verifications: vec![VerificationRecord {
                    item: "result".into(),
                    method: "fake".into(),
                    status: VerificationStatus::Passed,
                    conclusion: "ok".into(),
                    evidence_ref: "evidence://test".into(),
                }],
                candidate_artifacts: vec![],
                open_questions: vec![],
                risks: vec![],
                evidence_refs: vec!["evidence://test".into()],
                usage: UsageFlags {
                    context_truncated: false,
                    output_truncated: false,
                    budget_exhausted: false,
                },
            };
            Ok(Box::new(FakeTask {
                events: VecDeque::from([
                    WorkerTaskEvent::Heartbeat(heartbeat),
                    WorkerTaskEvent::Delivery {
                        delivery,
                        lease_epoch: request.dispatch.epoch,
                    },
                ]),
            }))
        }
    }

    #[derive(Clone, Default)]
    struct CleanupSpy(Arc<Mutex<Vec<String>>>);
    impl TerminalCleanupPort for CleanupSpy {
        fn cleanup(&mut self, binding: &CleanupBinding) -> Result<CleanupReport, CleanupPending> {
            self.0.lock().unwrap().push(binding.attempt_id.clone());
            Ok(CleanupReport {
                attempt_id: binding.attempt_id.clone(),
                already_clean: false,
                completed_steps: Vec::new(),
            })
        }

        fn residual_bindings(&self) -> Vec<CleanupBinding> {
            Vec::new()
        }

        fn cleanup_attempt(&mut self, attempt_id: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(attempt_id.into());
            Ok(())
        }
    }

    fn service_brief() -> DelegationBrief {
        DelegationBrief {
            schema_version: 1,
            identity: DelegationIdentity {
                session_id: "s".into(),
                message_id: "m".into(),
                parent_run_id: "r".into(),
                delegation_id: "d".into(),
                attempt_id: "a".into(),
            },
            goal: "explore".into(),
            background: vec![],
            constraints: vec![],
            allowed_references: vec!["evidence://test".into()],
            allowed_capabilities: vec![],
            completion_criteria: vec!["done".into()],
        }
    }

    fn seed_service_queue(db: &SharedDb) {
        let brief = service_brief();
        let plan = record();
        db.with_conn_mut(|conn| {
            conn.execute("INSERT INTO sessions(id,title,created_at,updated_at) VALUES ('s','s',1,1)", []).unwrap();
            conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('m','s','user','m',1)", []).unwrap();
            conn.execute("INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES ('r','s','m','g','running','[]',1,1)", []).unwrap();
            conn.execute("INSERT INTO work_packages(id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,candidate_version,created_at,updated_at,status) VALUES ('wp','s',1,'wk','explore','explorer',1,'scope','approved',1000,0,1,1,'active')", []).unwrap();
            DelegationRepository::create_queued_with(
                conn,
                &NewDelegation {
                    id: "d".into(), session_id: "s".into(), message_id: "m".into(),
                    parent_run_id: "r".into(), work_package_id: Some("wp".into()),
                    idempotency_key: Some("e2e".into()), objective: "explore".into(),
                    brief_json: serde_json::to_string(&brief).unwrap(),
                },
                &NewAttempt {
                    id: "a".into(), attempt_number: 1, sandbox_ref: "sandbox-a".into(),
                    outbox_id: "o".into(), dispatch_payload_json: "opaque".into(),
                },
                &NewCapabilityLease {
                    id: "l".into(), read_roots_json: "[]".into(), write_roots_json: "[]".into(),
                    tool_allowlist_json: "[\"network.search\"]".into(), network_hosts_json: "[\"docs.example.com\"]".into(),
                    budget_json: "{}".into(), expires_at: 1000,
                },
                10,
                |tx| ExplorerPlanRepository::insert_in_tx(tx, &plan).map_err(|e| e.to_string()),
            ).unwrap();
        }).unwrap();
    }

    #[test]
    fn service_resolves_launches_delivers_accepts_and_cleans_up() {
        let db = db();
        seed_service_queue(&db);
        let launches = Arc::new(Mutex::new(Vec::new()));
        let callback: WorkerEventCallback = Box::new(|_| Ok(()));
        let worker = ExplorerWorkerLauncherFactory::new(
            FakeLauncher {
                launches: launches.clone(),
            },
            ExplorerBindingResolver::new(ExplorerPlanRepository::with_clock(db.clone(), || 10)),
        )
        .build(callback);
        let runtime = crate::agent::delegation_runtime::DelegationRuntime::new_shared(
            db.clone(),
            worker,
            TestClock(10),
            FixedAdmissionPolicy {
                max_running_attempts: 1,
            },
            30,
        )
        .unwrap();
        let scheduler = DelegatedAttemptScheduler::new(runtime);
        let cleanup_calls = Arc::new(Mutex::new(Vec::new()));
        let cleanup = CleanupSpy(cleanup_calls.clone());
        let reviewed_outcome = ReviewOutcome {
            verdict: SemanticReviewVerdict::Passed,
            summary: "independent review passed".into(),
            findings: vec![],
            missing_evidence: vec![],
            evidence_refs: vec!["evidence://review/delivery-a".into()],
        };
        let mut service = DelegationService::with_cleanup(
            db.clone(),
            scheduler,
            WorkspaceAdmission::new(),
            cleanup,
        )
        .unwrap()
        .with_review(
            ReviewCoordinator::new(db.clone()),
            Box::new(FixedReviewExecutor::new(reviewed_outcome)),
        );
        service.start().unwrap();
        let report = service.tick().unwrap();
        assert_eq!(report.scheduler.dispatched, vec!["a"]);
        assert_eq!(launches.lock().unwrap().len(), 1);

        // The service drains the same bridge that the scheduler used for the
        // launch; the pump no longer owns a second bridge or requires a manual
        // `bind`. A duplicate tick is idempotent and has no events to replay.
        service.poll_workers(&ContractLimits::default()).unwrap();
        service.poll_workers(&ContractLimits::default()).unwrap();
        let review = ReviewCoordinator::new(db.clone())
            .load_for_delivery("delivery-a", 1)
            .unwrap();
        assert_eq!(review.status, ReviewJobStatus::Passed);
        let outcome = service
            .decide_for_session(
                "s",
                "delivery-a",
                ParentDecision::Accept,
                None,
                &ContractLimits::default(),
            )
            .unwrap();
        assert!(matches!(
            outcome,
            crate::agent::delivery_inbox::DecisionOutcome::Accepted(_)
        ));
        assert_eq!(cleanup_calls.lock().unwrap().as_slice(), &["a"]);
        db.with_conn(|conn| {
            let (delegation, attempt, lease): (String, String, String) = conn.query_row(
                "SELECT d.status, a.status, l.status FROM delegations d JOIN delegation_attempts a ON a.delegation_id=d.id JOIN delegation_capability_leases l ON l.attempt_id=a.id WHERE d.id='d'",
                [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).unwrap();
            assert_eq!((delegation, attempt, lease), ("completed".into(), "sealed".into(), "revoked".into()));
        }).unwrap();
    }
}
