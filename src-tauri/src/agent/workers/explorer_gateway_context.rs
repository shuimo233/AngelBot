//! Durable, per-operation Explorer gateway context.
//!
//! This adapter is deliberately a short-lived projection boundary.  The
//! worker host passes only an opaque [`WorkerHostBinding`]; for every search or
//! fetch we resolve the current attempt, delegation, work package, capability
//! lease, dispatch epoch, and immutable policy facts from the shared database.
//! No connection guard, lease, path, or snapshot is retained between calls.

use std::path::PathBuf;

use rusqlite::{params, OptionalExtension};

use super::{
    delegated_worker_adapter::DelegatedAttempt,
    delegation::LeaseStatus,
    execution_gateway::{CapabilityGrant, CapabilityLease},
    explorer_worker_host_adapter::ExplorerGatewaySnapshot,
    explorer_worker_runtime::ExplorerGatewayContextLoader as SnapshotLoader,
    shared_db::SharedDb,
    work_package::{WorkPackage, WorkPackageScope, WorkPackageStatus},
    worker_host_contract::WorkerHostBinding,
    worker_policy::{TaskShape, WorkerProfile},
};

/// Durable implementation of the runtime's snapshot loader seam.
///
/// `SharedDb` is cloned cheaply and points at the application's one SQLite
/// connection.  The loader never caches a successful lookup: lease status,
/// expiry, dispatch epoch, and package bindings are all checked again for
/// every operation.
#[derive(Clone)]
pub struct ExplorerGatewayContextLoader {
    db: SharedDb,
}

impl ExplorerGatewayContextLoader {
    pub fn new(db: SharedDb) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &SharedDb {
        &self.db
    }

    fn load_snapshot(
        &self,
        binding: &WorkerHostBinding,
        now_ms: i64,
    ) -> Result<ExplorerGatewaySnapshot, ExplorerGatewayContextError> {
        if now_ms <= 0 {
            return Err(ExplorerGatewayContextError::InvalidRequest(
                "gateway clock must be positive".into(),
            ));
        }
        let identity = binding.identity();
        let now_secs = now_ms / 1_000;
        self.db
            .with_conn(|conn| load_snapshot(conn, binding, &identity, now_secs))
            .map_err(|error| ExplorerGatewayContextError::Storage(error.to_string()))?
    }
}

impl SnapshotLoader for ExplorerGatewayContextLoader {
    fn snapshot(
        &self,
        binding: &WorkerHostBinding,
        now_ms: i64,
    ) -> Result<ExplorerGatewaySnapshot, String> {
        self.load_snapshot(binding, now_ms)
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExplorerGatewayContextError {
    #[error("explorer gateway binding was not found")]
    NotFound,
    #[error("explorer gateway attempt is stale or not running")]
    StaleAttempt,
    #[error("explorer gateway lease is inactive")]
    LeaseInactive,
    #[error("explorer gateway lease is expired")]
    Expired,
    #[error("explorer gateway scope or policy drifted")]
    ScopeDrift,
    #[error("explorer gateway binding is invalid: {0}")]
    InvalidRequest(String),
    #[error("explorer gateway storage failed: {0}")]
    Storage(String),
}

#[derive(Debug)]
struct SnapshotRow {
    attempt_id: String,
    delegation_id: String,
    attempt_status: String,
    delegation_status: String,
    package_id: String,
    session_id: String,
    owner_profile_id: i64,
    workspace_key: String,
    task_shape: String,
    worker_profile: String,
    worker_policy_version: i64,
    scope_digest: String,
    capability_scope_ref: String,
    capability_expires_at: i64,
    candidate_version: i64,
    package_status: String,
    lease_id: String,
    lease_attempt_id: String,
    read_roots_json: String,
    write_roots_json: String,
    tool_allowlist_json: String,
    network_hosts_json: String,
    lease_status: String,
    lease_expires_at: i64,
    plan_scope_digest: String,
    plan_policy_version: i64,
    epoch_payload: Option<String>,
}

fn load_snapshot(
    conn: &rusqlite::Connection,
    binding: &WorkerHostBinding,
    identity: &super::worker_host_contract::WorkerHostIdentity,
    now_secs: i64,
) -> Result<ExplorerGatewaySnapshot, ExplorerGatewayContextError> {
    let row: Option<SnapshotRow> = conn
        .query_row(
            "SELECT a.id, a.delegation_id, a.status,
                    d.status,
                    p.id, p.session_id, p.owner_profile_id, p.workspace_key,
                    p.task_shape, p.worker_profile, p.worker_policy_version,
                    p.scope_digest, p.capability_scope_ref,
                    p.capability_expires_at, p.candidate_version, p.status,
                    l.id, l.attempt_id, l.read_roots_json, l.write_roots_json,
                    l.tool_allowlist_json, l.network_hosts_json, l.status,
                    l.expires_at,
                    ep.scope_digest, ep.worker_policy_version,
                    (SELECT payload_json
                       FROM delegation_attempt_events e
                      WHERE e.attempt_id = a.id
                        AND e.event_type = 'attempt_dispatched'
                      ORDER BY e.sequence DESC LIMIT 1)
               FROM delegation_attempts a
               JOIN delegations d ON d.id = a.delegation_id
               JOIN work_packages p ON p.id = d.work_package_id
               JOIN delegation_capability_leases l ON l.attempt_id = a.id
               JOIN delegation_explorer_plans ep
                 ON ep.delegation_id = d.id
                AND ep.work_package_id = p.id
              WHERE a.id = ?1
                AND a.delegation_id = ?2
                AND d.work_package_id = ?3",
            params![
                identity.attempt_id(),
                identity.delegation_id(),
                identity.work_package_id(),
            ],
            |row| {
                Ok(SnapshotRow {
                    attempt_id: row.get(0)?,
                    delegation_id: row.get(1)?,
                    attempt_status: row.get(2)?,
                    delegation_status: row.get(3)?,
                    package_id: row.get(4)?,
                    session_id: row.get(5)?,
                    owner_profile_id: row.get(6)?,
                    workspace_key: row.get(7)?,
                    task_shape: row.get(8)?,
                    worker_profile: row.get(9)?,
                    worker_policy_version: row.get(10)?,
                    scope_digest: row.get(11)?,
                    capability_scope_ref: row.get(12)?,
                    capability_expires_at: row.get(13)?,
                    candidate_version: row.get(14)?,
                    package_status: row.get(15)?,
                    lease_id: row.get(16)?,
                    lease_attempt_id: row.get(17)?,
                    read_roots_json: row.get(18)?,
                    write_roots_json: row.get(19)?,
                    tool_allowlist_json: row.get(20)?,
                    network_hosts_json: row.get(21)?,
                    lease_status: row.get(22)?,
                    lease_expires_at: row.get(23)?,
                    plan_scope_digest: row.get(24)?,
                    plan_policy_version: row.get(25)?,
                    epoch_payload: row.get(26)?,
                })
            },
        )
        .optional()
        .map_err(|error| ExplorerGatewayContextError::Storage(error.to_string()))?;
    let Some(row) = row else {
        return Err(ExplorerGatewayContextError::NotFound);
    };

    if row.attempt_id != identity.attempt_id()
        || row.delegation_id != identity.delegation_id()
        || row.package_id != identity.work_package_id()
        || row.lease_attempt_id != row.attempt_id
    {
        return Err(ExplorerGatewayContextError::ScopeDrift);
    }
    if row.attempt_status != "running" || row.delegation_status != "running" {
        return Err(ExplorerGatewayContextError::StaleAttempt);
    }
    if row.lease_status != "active" {
        return Err(ExplorerGatewayContextError::LeaseInactive);
    }
    if row.lease_expires_at <= now_secs || row.capability_expires_at <= now_secs {
        return Err(ExplorerGatewayContextError::Expired);
    }
    if row.package_status != "active"
        || row.task_shape != TaskShape::Explore.as_str()
        || row.worker_profile != WorkerProfile::Explorer.as_str()
        || row.scope_digest.is_empty()
        || row.capability_scope_ref.is_empty()
        || row.plan_scope_digest != row.scope_digest
        || row.plan_policy_version != row.worker_policy_version
        || row.worker_policy_version <= 0
    {
        return Err(ExplorerGatewayContextError::ScopeDrift);
    }
    if binding.capability_ref() != row.capability_scope_ref
        || binding.worker_profile() != WorkerProfile::Explorer
        || binding.task_shape() != TaskShape::Explore
    {
        return Err(ExplorerGatewayContextError::ScopeDrift);
    }
    let Some(ref epoch_payload) = row.epoch_payload else {
        return Err(ExplorerGatewayContextError::StaleAttempt);
    };
    let epoch = serde_json::from_str::<DispatchEpoch>(&epoch_payload)
        .map_err(|_| ExplorerGatewayContextError::StaleAttempt)?
        .epoch;
    if epoch == 0 || epoch != binding.lease_epoch() {
        return Err(ExplorerGatewayContextError::StaleAttempt);
    }

    let lease = lease_from_row(&row)?;
    let scope = WorkPackageScope {
        session_id: row.session_id,
        owner_profile_id: row.owner_profile_id,
        workspace_key: row.workspace_key,
    };
    let package = WorkPackage {
        id: row.package_id,
        scope: scope.clone(),
        task_shape: TaskShape::Explore,
        worker_profile: WorkerProfile::Explorer,
        worker_policy_version: u16::try_from(row.worker_policy_version)
            .map_err(|_| ExplorerGatewayContextError::ScopeDrift)?,
        scope_digest: row.scope_digest,
        capability_scope_ref: row.capability_scope_ref,
        capability_expires_at: row.capability_expires_at,
        candidate_version: row.candidate_version,
        status: WorkPackageStatus::Active,
    };
    let attempt = DelegatedAttempt {
        delegation_id: row.delegation_id,
        attempt_id: row.attempt_id,
        work_package_id: package.id.clone(),
        lease,
        epoch,
    };
    Ok(ExplorerGatewaySnapshot {
        attempt,
        package,
        scope,
        live_lease: true,
    })
}

fn lease_from_row(row: &SnapshotRow) -> Result<CapabilityLease, ExplorerGatewayContextError> {
    let read_roots = parse_json::<Vec<PathBuf>>(&row.read_roots_json)?;
    let write_roots = parse_json::<Vec<PathBuf>>(&row.write_roots_json)?;
    let tool_allowlist = parse_json::<Vec<String>>(&row.tool_allowlist_json)?;
    let network_hosts = parse_json::<Vec<String>>(&row.network_hosts_json)?;
    let status = match row.lease_status.as_str() {
        "active" => LeaseStatus::Active,
        "revoked" => LeaseStatus::Revoked,
        "expired" => LeaseStatus::Expired,
        _ => {
            return Err(ExplorerGatewayContextError::LeaseInactive);
        }
    };
    let expires_at_ms = row.lease_expires_at.checked_mul(1_000).ok_or_else(|| {
        ExplorerGatewayContextError::InvalidRequest("lease expiry overflow".into())
    })?;
    CapabilityLease::from_grant(CapabilityGrant {
        id: row.lease_id.clone(),
        attempt_id: row.lease_attempt_id.clone(),
        status,
        expires_at_ms,
        read_roots,
        write_roots,
        tool_allowlist,
        network_hosts,
    })
    .map_err(|error| {
        ExplorerGatewayContextError::InvalidRequest(format!("invalid lease grant: {error:?}"))
    })
}

fn parse_json<T: serde::de::DeserializeOwned>(
    value: &str,
) -> Result<T, ExplorerGatewayContextError> {
    serde_json::from_str(value)
        .map_err(|_| ExplorerGatewayContextError::InvalidRequest("invalid lease grant JSON".into()))
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchEpoch {
    epoch: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn db() -> SharedDb {
        let connection = Connection::open_in_memory().unwrap();
        crate::db::migrate(&connection).unwrap();
        let db = SharedDb::new(connection);
        db.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO sessions(id,title,created_at,updated_at) VALUES ('s','s',1,1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('m','s','user','m',1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES ('r','s','m','g','running','[]',1,1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO work_packages(id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,candidate_version,created_at,updated_at,status) VALUES ('wp','s',1,'wk','explore','explorer',1,'scope','cap-ref',1000,0,1,1,'active')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO delegations(id,session_id,message_id,parent_run_id,work_package_id,objective,brief_json,status,state_version,created_at,updated_at) VALUES ('d','s','m','r','wp','g','{}','queued',0,1,1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO delegation_attempts(id,delegation_id,attempt_number,status,sandbox_ref,created_at) VALUES ('a','d',1,'queued','sandbox://a',1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO delegation_capability_leases(id,attempt_id,read_roots_json,write_roots_json,tool_allowlist_json,network_hosts_json,budget_json,status,issued_at,expires_at) VALUES ('l','a','[]','[]','[\"network.search\"]','[\"docs.example.com\"]','{}','active',1,100)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO delegation_explorer_plans(id,delegation_id,attempt_id,work_package_id,scope_digest,worker_policy_version,schema_version,plan_json,plan_digest,created_at) VALUES ('ep','d','a','wp','scope',1,1,'{}','digest',1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO delegation_attempt_events(id,attempt_id,sequence,event_type,payload_json,created_at) VALUES ('ev','a',1,'attempt_dispatched','{\"epoch\":1}',1)",
                [],
            )
            .unwrap();
            conn.execute("UPDATE delegation_attempts SET status='running' WHERE id='a'", [])
                .unwrap();
            conn.execute("UPDATE delegations SET status='running' WHERE id='d'", [])
                .unwrap();
        })
        .unwrap();
        db
    }

    fn binding() -> WorkerHostBinding {
        WorkerHostBinding::new(
            "a",
            "d",
            "wp",
            1,
            "cap-ref",
            WorkerProfile::Explorer,
            TaskShape::Explore,
        )
        .unwrap()
    }

    #[test]
    fn loads_live_snapshot_from_shared_db() {
        let loader = ExplorerGatewayContextLoader::new(db());
        let snapshot = loader.load_snapshot(&binding(), 5_000).unwrap();
        assert_eq!(snapshot.attempt.attempt_id, "a");
        assert_eq!(snapshot.attempt.lease.expires_at_ms, 100_000);
        assert_eq!(snapshot.package.scope.workspace_key, "wk");
        assert!(snapshot.live_lease);
    }

    #[test]
    fn missing_binding_fails_closed() {
        let loader = ExplorerGatewayContextLoader::new(db());
        let missing = WorkerHostBinding::new(
            "missing",
            "d",
            "wp",
            1,
            "cap-ref",
            WorkerProfile::Explorer,
            TaskShape::Explore,
        )
        .unwrap();
        assert!(matches!(
            loader.load_snapshot(&missing, 5_000),
            Err(ExplorerGatewayContextError::NotFound)
        ));
    }

    #[test]
    fn stale_epoch_and_revoked_lease_are_rejected() {
        let db = db();
        let loader = ExplorerGatewayContextLoader::new(db.clone());
        let stale = WorkerHostBinding::new(
            "a",
            "d",
            "wp",
            2,
            "cap-ref",
            WorkerProfile::Explorer,
            TaskShape::Explore,
        )
        .unwrap();
        assert!(matches!(
            loader.load_snapshot(&stale, 5_000),
            Err(ExplorerGatewayContextError::StaleAttempt)
        ));
        db.with_conn_mut(|conn| conn.execute("UPDATE delegation_capability_leases SET status='revoked',revoked_at=2 WHERE attempt_id='a'", []).unwrap()).unwrap();
        assert!(matches!(
            loader.load_snapshot(&binding(), 5_000),
            Err(ExplorerGatewayContextError::LeaseInactive)
        ));
    }

    #[test]
    fn expiry_and_scope_drift_are_checked_on_every_operation() {
        let lease_db = db();
        let loader = ExplorerGatewayContextLoader::new(lease_db.clone());
        lease_db
            .with_conn_mut(|conn| {
                conn.execute(
                    "UPDATE delegation_capability_leases SET expires_at=5 WHERE attempt_id='a'",
                    [],
                )
                .unwrap()
            })
            .unwrap();
        assert!(matches!(
            loader.load_snapshot(&binding(), 5_000),
            Err(ExplorerGatewayContextError::Expired)
        ));

        let drift_db = db();
        let drift_loader = ExplorerGatewayContextLoader::new(drift_db.clone());
        drift_db
            .with_conn_mut(|conn| {
                conn.execute(
                    "UPDATE work_packages SET scope_digest='drifted' WHERE id='wp'",
                    [],
                )
                .unwrap()
            })
            .unwrap();
        assert!(matches!(
            drift_loader.load_snapshot(&binding(), 5_000),
            Err(ExplorerGatewayContextError::ScopeDrift)
        ));
    }

    #[test]
    fn restarted_loader_reloads_the_same_live_rows_without_cached_lease() {
        let db = db();
        let first = ExplorerGatewayContextLoader::new(db.clone());
        assert!(first.load_snapshot(&binding(), 5_000).is_ok());
        let restarted = ExplorerGatewayContextLoader::new(db.clone());
        db.with_conn_mut(|conn| {
            conn.execute(
                "UPDATE delegation_capability_leases SET status='expired' WHERE attempt_id='a'",
                [],
            )
            .unwrap()
        })
        .unwrap();
        assert!(matches!(
            restarted.load_snapshot(&binding(), 5_000),
            Err(ExplorerGatewayContextError::LeaseInactive)
        ));
    }
}
