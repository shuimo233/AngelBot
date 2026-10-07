//! Durable provenance for a Main-Agent-confirmed delegated network scope.
//!
//! A source policy alone is not proof that a user approved it: a local model
//! must never be able to mint one by writing a policy-shaped row. This module
//! keeps the confirmation provenance, policy fingerprint, and policy write in
//! one small persistence seam. The transport gateway deliberately remains
//! unaware of this record; production composition supplies an approval-aware
//! policy lookup before the gateway reaches DNS or transport.

use std::collections::BTreeSet;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::agent::{shared_db::SharedDb, work_package::WorkPackageScope};

use super::{
    network_gateway::{
        NewSourcePolicy, SourcePolicy, SourcePolicyError, SourcePolicyLookup,
        SourcePolicyRepository,
    },
    worker_policy::NetworkAction,
};

/// Response types that can safely be returned as untrusted Explorer evidence.
/// The Main Agent cannot widen this list through tool arguments.
const EXPLORER_MIME_TYPES: &[&str] = &[
    "application/json",
    "application/xhtml+xml",
    "text/html",
    "text/plain",
];
const EXPLORER_MAX_RESPONSE_BYTES: usize = 512 * 1024;
const EXPLORER_MAX_REDIRECTS: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkScopeApprovalProvenance {
    pub owner_profile_id: i64,
    pub workspace_key: String,
    pub session_id: String,
    pub message_id: String,
    pub parent_run_id: String,
    pub tool_call_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkScopeApproval {
    pub id: String,
    pub policy_id: String,
    pub provenance: NetworkScopeApprovalProvenance,
    pub scope_digest: String,
    pub policy_digest: String,
}

/// One still-effective approval paired with the policy it attests to.
///
/// This remains crate-private because callers outside the delegated-network
/// persistence seam must never learn a source-policy identifier or digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CurrentNetworkScopeApproval {
    pub approval_id: String,
    pub approved_at: i64,
    pub policy: SourcePolicy,
}

/// One exact user confirmation, represented as a policy plus immutable
/// provenance. The caller creates it in memory; `persist_in_tx` makes it
/// durable only alongside a queued delegation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedNetworkScope {
    policy: SourcePolicy,
    approval: NetworkScopeApproval,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NetworkScopeApprovalError {
    #[error("network scope approval is invalid")]
    Invalid,
    #[error("network scope approval was not found")]
    NotFound,
    #[error("network scope approval storage failed: {0}")]
    Storage(String),
}

impl ConfirmedNetworkScope {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provenance: NetworkScopeApprovalProvenance,
        capability_scope_ref: String,
        scope_digest: String,
        actions: BTreeSet<NetworkAction>,
        allowed_hosts: BTreeSet<String>,
    ) -> Result<Self, NetworkScopeApprovalError> {
        validate_provenance(&provenance)?;
        if scope_digest.trim().is_empty()
            || scope_digest.len() > 256
            || scope_digest.contains(['\0', '\r', '\n', '/', '\\'])
        {
            return Err(NetworkScopeApprovalError::Invalid);
        }
        let policy = SourcePolicyRepository::new_policy(NewSourcePolicy {
            owner_profile_id: provenance.owner_profile_id,
            workspace_key: provenance.workspace_key.clone(),
            capability_scope_ref,
            actions,
            allowed_hosts,
            allowed_mime_types: EXPLORER_MIME_TYPES
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            max_response_bytes: EXPLORER_MAX_RESPONSE_BYTES,
            max_redirects: EXPLORER_MAX_REDIRECTS,
        })
        .map_err(map_policy_error)?;
        let approval = NetworkScopeApproval {
            id: format!("nsa_{}", Uuid::new_v4().simple()),
            policy_id: policy.id.clone(),
            provenance,
            scope_digest,
            policy_digest: policy_digest(&policy),
        };
        Ok(Self { policy, approval })
    }

    pub fn policy(&self) -> &SourcePolicy {
        &self.policy
    }

    pub fn approval(&self) -> &NetworkScopeApproval {
        &self.approval
    }

    /// The policy and its confirmation provenance must appear together. A
    /// rollback leaves neither row behind, preventing a failed request from
    /// becoming a reusable permission grant.
    pub fn persist_in_tx(
        &self,
        tx: &Transaction<'_>,
        now: i64,
    ) -> Result<(), NetworkScopeApprovalError> {
        SourcePolicyRepository::insert_in_tx(tx, &self.policy, now).map_err(map_policy_error)?;
        tx.execute(
            "INSERT INTO delegated_network_scope_approvals
                (id, policy_id, owner_profile_id, workspace_key, session_id,
                 message_id, parent_run_id, tool_call_id, scope_digest,
                 policy_digest, approved_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                self.approval.id,
                self.approval.policy_id,
                self.approval.provenance.owner_profile_id,
                self.approval.provenance.workspace_key,
                self.approval.provenance.session_id,
                self.approval.provenance.message_id,
                self.approval.provenance.parent_run_id,
                self.approval.provenance.tool_call_id,
                self.approval.scope_digest,
                self.approval.policy_digest,
                now,
            ],
        )
        .map_err(storage)?;
        Ok(())
    }
}

/// Approval validation is intentionally separate from raw source-policy
/// persistence. Both foreground issuance and the runtime lookup adapter reuse
/// it, so a policy-shaped row never becomes user authority by itself.
pub struct ApprovedNetworkScopeRepository;

impl ApprovedNetworkScopeRepository {
    /// List only policies that are both enabled and still covered by the exact
    /// confirmation record which was persisted with them.  This is the shared
    /// integrity check used by user-facing permission management; disabled,
    /// revoked, or policy-drifted rows are deliberately absent.
    pub(crate) fn list_current(
        conn: &Connection,
        owner_profile_id: i64,
        workspace_key: &str,
    ) -> Result<Vec<CurrentNetworkScopeApproval>, NetworkScopeApprovalError> {
        if owner_profile_id <= 0
            || workspace_key.trim().is_empty()
            || workspace_key.len() > 512
            || workspace_key.contains(['\0', '\r', '\n'])
        {
            return Err(NetworkScopeApprovalError::Invalid);
        }

        let mut approvals = Vec::new();
        for policy in
            SourcePolicyRepository::enabled_for_workspace(conn, owner_profile_id, workspace_key)
                .map_err(map_policy_error)?
        {
            if let Some((approval_id, approved_at)) = Self::current_approval_record(conn, &policy)?
            {
                approvals.push(CurrentNetworkScopeApproval {
                    approval_id,
                    approved_at,
                    policy,
                });
            }
        }
        approvals.sort_by(|left, right| {
            right
                .approved_at
                .cmp(&left.approved_at)
                .then_with(|| left.approval_id.cmp(&right.approval_id))
        });
        Ok(approvals)
    }

    /// List current approvals by the durable product workspace which owns the
    /// confirmation's foreground session.  Project permission management must
    /// remain available even when the project's root directory has moved or
    /// disappeared, so this deliberately does not resolve a live path.
    pub(crate) fn list_current_for_project(
        conn: &Connection,
        owner_profile_id: i64,
        project_id: &str,
    ) -> Result<Vec<CurrentNetworkScopeApproval>, NetworkScopeApprovalError> {
        if owner_profile_id <= 0
            || project_id.trim().is_empty()
            || project_id.len() > 128
            || project_id.contains('\0')
        {
            return Err(NetworkScopeApprovalError::Invalid);
        }

        let mut statement = conn
            .prepare(
                "SELECT approval.id, approval.policy_id, approval.approved_at
                 FROM delegated_network_scope_approvals AS approval
                 JOIN sessions AS session ON session.id=approval.session_id
                 JOIN projects AS project ON project.id=session.project_id
                 WHERE approval.owner_profile_id=?1
                   AND session.project_id=?2
                   AND project.kind='project'
                   AND approval.revoked_at IS NULL
                 ORDER BY approval.approved_at DESC, approval.id ASC",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map(params![owner_profile_id, project_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(storage)?;

        let mut approvals = Vec::new();
        for row in rows {
            let (approval_id, policy_id, approved_at) = row.map_err(storage)?;
            let policy = match SourcePolicyRepository::load_enabled_by_id(conn, &policy_id) {
                Ok(policy) => policy,
                Err(SourcePolicyError::NotFound) => continue,
                Err(error) => return Err(map_policy_error(error)),
            };
            let Some((current_id, current_at)) = Self::current_approval_record(conn, &policy)?
            else {
                continue;
            };
            if current_id == approval_id && current_at == approved_at {
                approvals.push(CurrentNetworkScopeApproval {
                    approval_id,
                    approved_at,
                    policy,
                });
            }
        }
        Ok(approvals)
    }

    pub fn find_covering(
        conn: &Connection,
        owner_profile_id: i64,
        workspace_key: &str,
        actions: &BTreeSet<NetworkAction>,
        hosts: &BTreeSet<String>,
    ) -> Result<SourcePolicy, NetworkScopeApprovalError> {
        for policy in
            SourcePolicyRepository::enabled_for_workspace(conn, owner_profile_id, workspace_key)
                .map_err(map_policy_error)?
        {
            if actions.is_subset(&policy.actions)
                && hosts.is_subset(&policy.allowed_hosts)
                && Self::is_current_approval(conn, &policy)?
            {
                return Ok(policy);
            }
        }
        Err(NetworkScopeApprovalError::NotFound)
    }

    pub fn load(
        conn: &Connection,
        owner_profile_id: i64,
        workspace_key: &str,
        capability_scope_ref: &str,
    ) -> Result<SourcePolicy, NetworkScopeApprovalError> {
        let policy = SourcePolicyRepository::load(
            conn,
            owner_profile_id,
            workspace_key,
            capability_scope_ref,
        )
        .map_err(map_policy_error)?;
        if Self::is_current_approval(conn, &policy)? {
            Ok(policy)
        } else {
            Err(NetworkScopeApprovalError::NotFound)
        }
    }

    /// Revoke one exact confirmed scope while preserving its provenance for
    /// audit and diagnosis. Runtime policy lookup checks this row before each
    /// request, so an already-running Explorer loses network access on its
    /// next operation without relying on a stale in-memory policy snapshot.
    ///
    /// The caller must provide the trusted workspace/profile boundary as well
    /// as the opaque policy id; a model cannot use this method to revoke an
    /// unrelated project's scope.
    pub fn revoke(
        conn: &Connection,
        owner_profile_id: i64,
        workspace_key: &str,
        policy_id: &str,
        now: i64,
    ) -> Result<bool, NetworkScopeApprovalError> {
        if owner_profile_id <= 0
            || workspace_key.trim().is_empty()
            || workspace_key.len() > 512
            || policy_id.trim().is_empty()
            || policy_id.len() > 128
        {
            return Err(NetworkScopeApprovalError::Invalid);
        }
        let changed = conn
            .execute(
                "UPDATE delegated_network_scope_approvals
                 SET revoked_at=?1
                 WHERE policy_id=?2
                   AND owner_profile_id=?3
                   AND workspace_key=?4
                   AND revoked_at IS NULL",
                params![now, policy_id, owner_profile_id, workspace_key],
            )
            .map_err(storage)?;
        Ok(changed == 1)
    }

    /// Revoke by the confirmation record's opaque id rather than exposing a
    /// source-policy id to a user-facing caller.  A missing, cross-workspace,
    /// or already-revoked record is intentionally indistinguishable to keep
    /// this idempotent and prevent permission enumeration across projects.
    pub(crate) fn revoke_by_approval_id(
        conn: &Connection,
        owner_profile_id: i64,
        workspace_key: &str,
        approval_id: &str,
        now: i64,
    ) -> Result<bool, NetworkScopeApprovalError> {
        if !safe_id(approval_id) {
            return Ok(false);
        }
        let policy_id: Option<String> = conn
            .query_row(
                "SELECT policy_id
                 FROM delegated_network_scope_approvals
                 WHERE id=?1
                   AND owner_profile_id=?2
                   AND workspace_key=?3
                   AND revoked_at IS NULL",
                params![approval_id, owner_profile_id, workspace_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let Some(policy_id) = policy_id else {
            return Ok(false);
        };
        Self::revoke(conn, owner_profile_id, workspace_key, &policy_id, now)
    }

    /// Revoke by opaque confirmation id within the durable owning project.
    ///
    /// This intentionally updates a disabled or policy-drifted approval too:
    /// revocation only reduces authority, and prevents a later restoration of
    /// that source-policy row from making it eligible again.  A stale or
    /// cross-project id remains indistinguishable from an already-inactive
    /// approval.
    pub(crate) fn revoke_by_approval_id_for_project(
        conn: &Connection,
        owner_profile_id: i64,
        project_id: &str,
        approval_id: &str,
        now: i64,
    ) -> Result<bool, NetworkScopeApprovalError> {
        if owner_profile_id <= 0
            || project_id.trim().is_empty()
            || project_id.len() > 128
            || project_id.contains('\0')
            || !safe_id(approval_id)
        {
            return Ok(false);
        }
        let changed = conn
            .execute(
                "UPDATE delegated_network_scope_approvals
                 SET revoked_at=?1
                 WHERE id=?2
                   AND owner_profile_id=?3
                   AND revoked_at IS NULL
                   AND EXISTS (
                       SELECT 1
                       FROM sessions AS session
                       JOIN projects AS project ON project.id=session.project_id
                       WHERE session.id=delegated_network_scope_approvals.session_id
                         AND session.project_id=?4
                         AND project.kind='project'
                   )",
                params![now, approval_id, owner_profile_id, project_id],
            )
            .map_err(storage)?;
        Ok(changed == 1)
    }

    fn is_current_approval(
        conn: &Connection,
        policy: &SourcePolicy,
    ) -> Result<bool, NetworkScopeApprovalError> {
        Ok(Self::current_approval_record(conn, policy)?.is_some())
    }

    fn current_approval_record(
        conn: &Connection,
        policy: &SourcePolicy,
    ) -> Result<Option<(String, i64)>, NetworkScopeApprovalError> {
        let stored: Option<(String, String, i64)> = conn
            .query_row(
                "SELECT id, policy_digest, approved_at
                 FROM delegated_network_scope_approvals
                 WHERE policy_id=?1
                   AND owner_profile_id=?2
                   AND workspace_key=?3
                   AND revoked_at IS NULL",
                params![policy.id, policy.owner_profile_id, policy.workspace_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(storage)?;
        Ok(stored.and_then(|(approval_id, digest, approved_at)| {
            (digest == policy_digest(policy)).then_some((approval_id, approved_at))
        }))
    }
}

/// Production policy lookup for delegated network operations.
///
/// A raw source-policy row is deliberately insufficient at runtime: it could
/// be revoked after an attempt starts, or its contents could drift after the
/// user confirmed them. This adapter reuses the approval repository on every
/// gateway request, keeping the transport layer free of provenance tables
/// while making those conditions fail closed before DNS or transport.
#[derive(Clone)]
pub struct SharedDbApprovedNetworkScopeLookup {
    db: SharedDb,
}

impl SharedDbApprovedNetworkScopeLookup {
    pub fn new(db: SharedDb) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &SharedDb {
        &self.db
    }
}

impl SourcePolicyLookup for SharedDbApprovedNetworkScopeLookup {
    fn load(
        &self,
        scope: &WorkPackageScope,
        scope_ref: &str,
    ) -> Result<SourcePolicy, SourcePolicyError> {
        self.db
            .with_conn(|connection| {
                ApprovedNetworkScopeRepository::load(
                    connection,
                    scope.owner_profile_id,
                    &scope.workspace_key,
                    scope_ref,
                )
            })
            .map_err(|error| SourcePolicyError::Storage(error.to_string()))?
            .map_err(map_approval_lookup_error)
    }
}

fn validate_provenance(
    provenance: &NetworkScopeApprovalProvenance,
) -> Result<(), NetworkScopeApprovalError> {
    if provenance.owner_profile_id <= 0
        || provenance.workspace_key.trim().is_empty()
        || provenance.workspace_key.len() > 512
        || provenance.workspace_key.contains(['\0', '\r', '\n'])
        || [
            &provenance.session_id,
            &provenance.message_id,
            &provenance.parent_run_id,
            &provenance.tool_call_id,
        ]
        .iter()
        .any(|value| !safe_id(value))
    {
        return Err(NetworkScopeApprovalError::Invalid);
    }
    Ok(())
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn policy_digest(policy: &SourcePolicy) -> String {
    let canonical = json!({
        "id": policy.id,
        "owner_profile_id": policy.owner_profile_id,
        "workspace_key": policy.workspace_key,
        "capability_scope_ref": policy.capability_scope_ref,
        "policy_version": policy.policy_version,
        "actions": policy.actions,
        "allowed_hosts": policy.allowed_hosts,
        "allowed_mime_types": policy.allowed_mime_types,
        "max_response_bytes": policy.max_response_bytes,
        "max_redirects": policy.max_redirects,
        "enabled": policy.enabled,
    });
    let mut hasher = Sha256::new();
    // serde_json's representation of this fixed object and BTree-backed sets
    // is deterministic. If serialization somehow fails, there is no safe
    // fallback digest, so use an impossible empty value that never validates.
    match serde_json::to_vec(&canonical) {
        Ok(bytes) => {
            hasher.update(bytes);
            format!("sha256:{}", hex::encode(hasher.finalize()))
        }
        Err(_) => String::new(),
    }
}

fn map_policy_error(error: SourcePolicyError) -> NetworkScopeApprovalError {
    match error {
        SourcePolicyError::NotFound => NetworkScopeApprovalError::NotFound,
        SourcePolicyError::Invalid(_) => NetworkScopeApprovalError::Invalid,
        SourcePolicyError::Storage(error) => NetworkScopeApprovalError::Storage(error),
    }
}

fn map_approval_lookup_error(error: NetworkScopeApprovalError) -> SourcePolicyError {
    match error {
        NetworkScopeApprovalError::Invalid => {
            SourcePolicyError::Invalid("approved network scope is invalid".into())
        }
        NetworkScopeApprovalError::NotFound => SourcePolicyError::NotFound,
        NetworkScopeApprovalError::Storage(error) => SourcePolicyError::Storage(error),
    }
}

fn storage(error: rusqlite::Error) -> NetworkScopeApprovalError {
    NetworkScopeApprovalError::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::{
        net::IpAddr,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    use crate::agent::{
        delegation::LeaseStatus,
        execution_gateway::{CapabilityGrant, CapabilityLease},
        network_gateway::{
            HostResolver, NetworkDenied, NetworkGateway, NetworkTransport, SearchIntent,
            TransportFetchRequest, TransportFetchResponse, TransportSearchRequest,
            TransportSearchResponse,
        },
        work_package::{WorkPackage, WorkPackageScope, WorkPackageStatus},
        worker_policy::{TaskShape, WorkerProfile},
    };

    fn provenance() -> NetworkScopeApprovalProvenance {
        NetworkScopeApprovalProvenance {
            owner_profile_id: 1,
            workspace_key: "workspace".into(),
            session_id: "session".into(),
            message_id: "message".into(),
            parent_run_id: "run".into(),
            tool_call_id: "call".into(),
        }
    }

    fn scope() -> ConfirmedNetworkScope {
        ConfirmedNetworkScope::new(
            provenance(),
            "network_scope".into(),
            "sha256:scope".into(),
            BTreeSet::from([NetworkAction::Search]),
            BTreeSet::from(["docs.example.com".into()]),
        )
        .unwrap()
    }

    struct PublicResolver;

    impl HostResolver for PublicResolver {
        fn resolve(&self, _: &str) -> Result<Vec<IpAddr>, String> {
            Ok(vec!["8.8.8.8".parse().unwrap()])
        }
    }

    struct CountingTransport(Arc<AtomicUsize>);

    #[async_trait]
    impl NetworkTransport for CountingTransport {
        async fn search(
            &self,
            _: TransportSearchRequest,
            _: &crate::agent::ToolCancellation,
        ) -> Result<TransportSearchResponse, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(TransportSearchResponse {
                content_type: "application/json".into(),
                byte_count: 0,
                results: vec![],
            })
        }

        async fn fetch(
            &self,
            _: TransportFetchRequest,
            _: &crate::agent::ToolCancellation,
        ) -> Result<TransportFetchResponse, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err("transport must not be reached".into())
        }
    }

    fn runtime_fixture() -> (SharedDb, WorkPackage, CapabilityLease, String) {
        let mut connection = Connection::open_in_memory().unwrap();
        crate::db::migrate(&connection).unwrap();
        let confirmed = ConfirmedNetworkScope::new(
            provenance(),
            "network_scope".into(),
            "sha256:runtime".into(),
            BTreeSet::from([NetworkAction::Search]),
            BTreeSet::from(["search.example.com".into()]),
        )
        .unwrap();
        let policy_id = confirmed.policy().id.clone();
        let transaction = connection.transaction().unwrap();
        confirmed.persist_in_tx(&transaction, 1).unwrap();
        transaction.commit().unwrap();

        let scope = WorkPackageScope {
            session_id: "session".into(),
            owner_profile_id: 1,
            workspace_key: "workspace".into(),
        };
        let package = WorkPackage {
            id: "package".into(),
            scope: scope.clone(),
            task_shape: TaskShape::Explore,
            worker_profile: WorkerProfile::Explorer,
            worker_policy_version: 1,
            scope_digest: "sha256:runtime".into(),
            capability_scope_ref: "network_scope".into(),
            capability_expires_at: 1_000_000,
            candidate_version: 0,
            status: WorkPackageStatus::Active,
        };
        let lease = CapabilityLease::from_grant(CapabilityGrant {
            id: "lease".into(),
            attempt_id: "attempt".into(),
            status: LeaseStatus::Active,
            expires_at_ms: 1_000_000,
            read_roots: vec![],
            write_roots: vec![],
            tool_allowlist: vec!["network.search".into()],
            network_hosts: vec!["search.example.com".into()],
        })
        .unwrap();
        (SharedDb::new(connection), package, lease, policy_id)
    }

    async fn assert_runtime_scope_is_denied_before_transport(
        db: SharedDb,
        package: &WorkPackage,
        lease: &CapabilityLease,
    ) {
        let calls = Arc::new(AtomicUsize::new(0));
        let lookup = SharedDbApprovedNetworkScopeLookup::new(db.clone());
        assert!(lookup.db().same_instance(&db));
        let gateway = NetworkGateway::new(lookup, CountingTransport(calls.clone()), PublicResolver);

        assert!(matches!(
            gateway
                .search(
                    lease,
                    package,
                    &package.scope,
                    SearchIntent {
                        provider_host: "search.example.com".into(),
                        query: "safe query".into(),
                    },
                    1,
                    &crate::agent::ToolCancellation::new(),
                )
                .await,
            Err(NetworkDenied::Policy(_))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn policy_without_confirmation_provenance_never_covers_delegation() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/047_delegated_network_source_policy.sql"
        ))
        .unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/075_delegated_network_scope_approvals.sql"
        ))
        .unwrap();
        SourcePolicyRepository::create(
            &conn,
            NewSourcePolicy {
                owner_profile_id: 1,
                workspace_key: "workspace".into(),
                capability_scope_ref: "injected".into(),
                actions: BTreeSet::from([NetworkAction::Search]),
                allowed_hosts: BTreeSet::from(["docs.example.com".into()]),
                allowed_mime_types: BTreeSet::from(["text/html".into()]),
                max_response_bytes: 1024,
                max_redirects: 1,
            },
        )
        .unwrap();

        assert_eq!(
            ApprovedNetworkScopeRepository::find_covering(
                &conn,
                1,
                "workspace",
                &BTreeSet::from([NetworkAction::Search]),
                &BTreeSet::from(["docs.example.com".into()]),
            ),
            Err(NetworkScopeApprovalError::NotFound)
        );
    }

    #[test]
    fn transaction_persists_an_exact_approval_and_rejects_tampering() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/047_delegated_network_source_policy.sql"
        ))
        .unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/075_delegated_network_scope_approvals.sql"
        ))
        .unwrap();
        let scope = scope();
        let policy_id = scope.policy().id.clone();
        let tx = conn.transaction().unwrap();
        scope.persist_in_tx(&tx, 10).unwrap();
        tx.commit().unwrap();

        assert_eq!(
            ApprovedNetworkScopeRepository::find_covering(
                &conn,
                1,
                "workspace",
                &BTreeSet::from([NetworkAction::Search]),
                &BTreeSet::from(["docs.example.com".into()]),
            )
            .unwrap()
            .id,
            policy_id
        );
        conn.execute(
            "UPDATE delegated_network_source_policies SET allowed_hosts_json='[\"other.example.com\"]' WHERE id=?1",
            [policy_id],
        )
        .unwrap();
        assert_eq!(
            ApprovedNetworkScopeRepository::load(&conn, 1, "workspace", "network_scope"),
            Err(NetworkScopeApprovalError::NotFound)
        );
    }

    #[tokio::test]
    async fn runtime_lookup_rejects_a_revoked_scope_before_transport() {
        let (db, package, lease, policy_id) = runtime_fixture();
        assert!(db
            .with_conn(|connection| {
                ApprovedNetworkScopeRepository::revoke(connection, 1, "workspace", &policy_id, 2)
            })
            .unwrap()
            .unwrap());
        assert!(!db
            .with_conn(|connection| {
                ApprovedNetworkScopeRepository::revoke(connection, 1, "workspace", &policy_id, 3)
            })
            .unwrap()
            .unwrap());
        // The raw policy still exists, which proves the runtime denial comes
        // from the approval guard rather than incidental policy deletion.
        assert!(db
            .with_conn(|connection| {
                SourcePolicyRepository::load(connection, 1, "workspace", "network_scope")
            })
            .unwrap()
            .is_ok());

        assert_runtime_scope_is_denied_before_transport(db, &package, &lease).await;
    }

    #[tokio::test]
    async fn runtime_lookup_rejects_a_tampered_scope_before_transport() {
        let (db, package, lease, policy_id) = runtime_fixture();
        db.with_conn_mut(|connection| {
            // The modified policy remains syntactically valid and still
            // permits this host/action. Only provenance-digest validation can
            // block it before the request reaches transport.
            connection
                .execute(
                    "UPDATE delegated_network_source_policies
                     SET max_response_bytes=?1
                     WHERE id=?2",
                    params![(EXPLORER_MAX_RESPONSE_BYTES - 1) as i64, policy_id],
                )
                .unwrap();
        })
        .unwrap();
        assert!(db
            .with_conn(|connection| {
                SourcePolicyRepository::load(connection, 1, "workspace", "network_scope")
            })
            .unwrap()
            .is_ok());

        assert_runtime_scope_is_denied_before_transport(db, &package, &lease).await;
    }
}
