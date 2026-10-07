//! Durable opaque identities for delegated temporary resources.
//!
//! This module is the small persistence seam between issuance/scheduling and
//! provider cleanup.  It never accepts or returns a filesystem path.  A
//! provider owns the meaning of `resource_ref` and `manifest_locator`; the
//! repository only compares those values with the immutable manifest tuple
//! recorded by the Main Agent.

use std::{fmt, str::FromStr};

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::shared_db::SharedDb;

const MAX_OPAQUE_LEN: usize = 256;
const MAX_DIGEST_LEN: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Sandbox,
    Worktree,
}

impl ResourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Worktree => "worktree",
        }
    }
}

impl FromStr for ResourceKind {
    type Err = ResourceBindingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "sandbox" => Ok(Self::Sandbox),
            "worktree" => Ok(Self::Worktree),
            _ => Err(ResourceBindingError::UnknownResource),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    Preparing,
    Ready,
    Sealed,
    Revoked,
    Quarantined,
    Unknown,
}

impl LifecycleState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Ready => "ready",
            Self::Sealed => "sealed",
            Self::Revoked => "revoked",
            Self::Quarantined => "quarantined",
            Self::Unknown => "unknown",
        }
    }
}

impl FromStr for LifecycleState {
    type Err = ResourceBindingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "preparing" => Ok(Self::Preparing),
            "ready" => Ok(Self::Ready),
            "sealed" => Ok(Self::Sealed),
            "revoked" => Ok(Self::Revoked),
            "quarantined" => Ok(Self::Quarantined),
            "unknown" => Ok(Self::Unknown),
            _ => Err(ResourceBindingError::Storage(format!(
                "invalid resource lifecycle state: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupStatus {
    None,
    Pending,
    Retry,
    Cleaned,
    Blocked,
    Retained,
}

impl CleanupStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pending => "pending",
            Self::Retry => "retry",
            Self::Cleaned => "cleaned",
            Self::Blocked => "blocked",
            Self::Retained => "retained",
        }
    }
}

impl FromStr for CleanupStatus {
    type Err = ResourceBindingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "none" => Ok(Self::None),
            "pending" => Ok(Self::Pending),
            "retry" => Ok(Self::Retry),
            "cleaned" => Ok(Self::Cleaned),
            "blocked" => Ok(Self::Blocked),
            "retained" => Ok(Self::Retained),
            _ => Err(ResourceBindingError::Storage(format!(
                "invalid resource cleanup status: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionCapture {
    pub id: String,
    pub state_version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceBindingIdentity {
    pub id: String,
    pub delegation_id: String,
    pub work_package_id: String,
    pub attempt_id: String,
    pub lease_id: String,
    pub resource_kind: ResourceKind,
    pub provider_kind: String,
    pub resource_ref: String,
    pub manifest_locator: String,
    pub manifest_version: i64,
    pub manifest_nonce: String,
    pub manifest_digest: String,
    pub scope_digest: String,
    pub lease_epoch: i64,
    pub admission: Option<AdmissionCapture>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewResourceBinding {
    pub identity: ResourceBindingIdentity,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceBinding {
    pub identity: ResourceBindingIdentity,
    pub lifecycle_state: LifecycleState,
    pub cleanup_status: CleanupStatus,
    pub cleanup_step: Option<String>,
    pub cleanup_attempts: i64,
    pub next_cleanup_at: Option<i64>,
    pub quarantine_until: Option<i64>,
    pub last_error_class: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub sealed_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub cleaned_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceBindingError {
    Storage(String),
    InvalidOpaqueIdentity(&'static str),
    InvalidDigest(&'static str),
    UnknownResource,
    NotFound,
    IdentityMismatch(&'static str),
    InvalidTransition,
    StaleCleanup,
}

impl fmt::Display for ResourceBindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => write!(f, "resource binding storage error: {error}"),
            Self::InvalidOpaqueIdentity(field) => {
                write!(f, "invalid opaque resource identity: {field}")
            }
            Self::InvalidDigest(field) => write!(f, "invalid resource binding digest: {field}"),
            Self::UnknownResource => f.write_str("unknown resource binding"),
            Self::NotFound => f.write_str("resource binding not found"),
            Self::IdentityMismatch(field) => write!(f, "resource binding mismatch: {field}"),
            Self::InvalidTransition => f.write_str("invalid resource cleanup transition"),
            Self::StaleCleanup => f.write_str("stale resource cleanup transition"),
        }
    }
}

impl std::error::Error for ResourceBindingError {}

impl From<rusqlite::Error> for ResourceBindingError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

fn valid_opaque(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_OPAQUE_LEN
        && !value.contains(['/', '\\', '\0', '\r', '\n', ':'])
        && !value.contains("..")
        && !value.starts_with('.')
}

fn validate_identity(identity: &ResourceBindingIdentity) -> Result<(), ResourceBindingError> {
    for (field, value) in [
        ("id", identity.id.as_str()),
        ("delegation_id", identity.delegation_id.as_str()),
        ("work_package_id", identity.work_package_id.as_str()),
        ("attempt_id", identity.attempt_id.as_str()),
        ("lease_id", identity.lease_id.as_str()),
        ("provider_kind", identity.provider_kind.as_str()),
        ("resource_ref", identity.resource_ref.as_str()),
        ("manifest_locator", identity.manifest_locator.as_str()),
        ("manifest_nonce", identity.manifest_nonce.as_str()),
    ] {
        if !valid_opaque(value) {
            return Err(ResourceBindingError::InvalidOpaqueIdentity(field));
        }
    }
    if identity.manifest_version <= 0 || identity.lease_epoch <= 0 {
        return Err(ResourceBindingError::InvalidDigest("version_or_epoch"));
    }
    for (field, value) in [
        ("manifest_digest", identity.manifest_digest.as_str()),
        ("scope_digest", identity.scope_digest.as_str()),
    ] {
        if value.is_empty()
            || value.len() > MAX_DIGEST_LEN
            || value.contains(['/', '\\', '\0', '\r', '\n'])
        {
            return Err(ResourceBindingError::InvalidDigest(field));
        }
    }
    if let Some(admission) = &identity.admission {
        if !valid_opaque(&admission.id) || admission.state_version < 0 {
            return Err(ResourceBindingError::InvalidOpaqueIdentity("admission"));
        }
    }
    Ok(())
}

fn storage<T>(
    result: Result<T, super::shared_db::SharedDbError>,
) -> Result<T, ResourceBindingError> {
    result.map_err(|error| ResourceBindingError::Storage(error.to_string()))
}

/// Short-lock repository.  A caller that already owns a transaction uses
/// [`ResourceBindingRepository::create_in_tx`]; all standalone calls release
/// the shared DB mutex before returning.
#[derive(Clone)]
pub struct ResourceBindingRepository {
    db: SharedDb,
    now: fn() -> i64,
}

impl ResourceBindingRepository {
    pub fn new(db: SharedDb) -> Self {
        Self {
            db,
            now: || chrono::Utc::now().timestamp(),
        }
    }

    pub fn with_clock(db: SharedDb, now: fn() -> i64) -> Self {
        Self { db, now }
    }

    pub fn db(&self) -> &SharedDb {
        &self.db
    }

    pub fn create(&self, record: &NewResourceBinding) -> Result<(), ResourceBindingError> {
        validate_identity(&record.identity)?;
        storage(self.db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            Self::create_in_tx(&tx, record)?;
            tx.commit()?;
            Ok::<_, ResourceBindingError>(())
        }))?
    }

    /// Atomic issuance seam.  The caller owns the surrounding transaction and
    /// inserts delegation/attempt/lease rows before invoking this function.
    pub fn create_in_tx(
        tx: &Transaction<'_>,
        record: &NewResourceBinding,
    ) -> Result<(), ResourceBindingError> {
        validate_identity(&record.identity)?;
        let i = &record.identity;
        tx.execute(
            "INSERT INTO delegation_resource_bindings
                (id, delegation_id, work_package_id, attempt_id, lease_id,
                 resource_kind, provider_kind, resource_ref, manifest_locator,
                 manifest_version, manifest_nonce, manifest_digest, scope_digest,
                 lease_epoch, admission_id, admission_state_version,
                 lifecycle_state, cleanup_status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, ?14, ?15, ?16, 'preparing', 'none', ?17, ?17)",
            params![
                i.id,
                i.delegation_id,
                i.work_package_id,
                i.attempt_id,
                i.lease_id,
                i.resource_kind.as_str(),
                i.provider_kind,
                i.resource_ref,
                i.manifest_locator,
                i.manifest_version,
                i.manifest_nonce,
                i.manifest_digest,
                i.scope_digest,
                i.lease_epoch,
                i.admission.as_ref().map(|value| value.id.as_str()),
                i.admission.as_ref().map(|value| value.state_version),
                record.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<ResourceBinding, ResourceBindingError> {
        if !valid_opaque(id) {
            return Err(ResourceBindingError::InvalidOpaqueIdentity("id"));
        }
        storage(self.db.with_conn(|conn| load_by_id(conn, id)))??
            .ok_or(ResourceBindingError::NotFound)
    }

    pub fn load_for_attempt(
        &self,
        attempt_id: &str,
        resource_kind: ResourceKind,
    ) -> Result<ResourceBinding, ResourceBindingError> {
        if !valid_opaque(attempt_id) {
            return Err(ResourceBindingError::InvalidOpaqueIdentity("attempt_id"));
        }
        storage(self.db.with_conn(|conn| {
            conn.query_row(
                "SELECT id FROM delegation_resource_bindings WHERE attempt_id = ?1 AND resource_kind = ?2",
                params![attempt_id, resource_kind.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(ResourceBindingError::from)
            .and_then(|id| match id {
                Some(id) => load_by_id(conn, &id),
                None => Ok(None),
            })
        }))??
        .ok_or(ResourceBindingError::NotFound)
    }

    /// CAS cleanup progress.  No provider call is made while the DB mutex is
    /// held; the caller invokes the provider first and records one idempotent
    /// transition afterward.
    pub fn transition_cleanup(
        &self,
        id: &str,
        expected: CleanupStatus,
        next: CleanupStatus,
        lifecycle_state: Option<LifecycleState>,
        cleanup_step: Option<&str>,
        next_cleanup_at: Option<i64>,
        quarantine_until: Option<i64>,
        error_class: Option<&str>,
    ) -> Result<bool, ResourceBindingError> {
        if !valid_opaque(id) {
            return Err(ResourceBindingError::InvalidOpaqueIdentity("id"));
        }
        let now = (self.now)();
        storage(self.db.with_conn_mut(|conn| {
            let changed = conn.execute(
                "UPDATE delegation_resource_bindings
                    SET lifecycle_state = COALESCE(?1, lifecycle_state),
                        cleanup_status = ?2,
                        cleanup_step = ?3,
                        cleanup_attempts = cleanup_attempts + 1,
                        next_cleanup_at = ?4,
                        quarantine_until = ?5,
                        last_error_class = ?6,
                        updated_at = ?7,
                        sealed_at = CASE WHEN ?1 = 'sealed' AND sealed_at IS NULL THEN ?7 ELSE sealed_at END,
                        revoked_at = CASE WHEN ?1 = 'revoked' AND revoked_at IS NULL THEN ?7 ELSE revoked_at END,
                        cleaned_at = CASE WHEN ?2 = 'cleaned' AND cleaned_at IS NULL THEN ?7 ELSE cleaned_at END
                  WHERE id = ?8 AND cleanup_status = ?9",
                params![
                    lifecycle_state.map(LifecycleState::as_str),
                    next.as_str(),
                    cleanup_step,
                    next_cleanup_at,
                    quarantine_until,
                    error_class,
                    now,
                    id,
                    expected.as_str(),
                ],
            )?;
            Ok(changed == 1)
        }))?
    }

    /// Return known rows that need reconciliation after a process restart.
    /// Unknown provider residuals do not appear here and are intentionally
    /// retained by the provider scanner until an operator resolves them.
    pub fn scan_restart(&self, now: i64) -> Result<Vec<ResourceBinding>, ResourceBindingError> {
        storage(self.db.with_conn(|conn| {
            let mut statement = conn.prepare(
                "SELECT id FROM delegation_resource_bindings
                  WHERE cleanup_status IN ('pending', 'retry')
                     OR lifecycle_state IN ('sealed', 'revoked', 'quarantined', 'unknown')
                  ORDER BY updated_at ASC, id ASC",
            )?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let mut result = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(binding) = load_by_id(conn, &id)? {
                    if binding.quarantine_until.map(|at| at <= now).unwrap_or(true)
                        || matches!(
                            binding.cleanup_status,
                            CleanupStatus::Pending | CleanupStatus::Retry
                        )
                    {
                        result.push(binding);
                    }
                }
            }
            Ok(result)
        }))?
    }
}

/// Resolver that compares a provider manifest identity with the durable row.
/// It has no path fallback and never guesses an attempt for an unknown ref.
#[derive(Clone)]
pub struct ResourceBindingResolver {
    repository: ResourceBindingRepository,
}

impl ResourceBindingResolver {
    pub fn new(repository: ResourceBindingRepository) -> Self {
        Self { repository }
    }

    pub fn repository(&self) -> &ResourceBindingRepository {
        &self.repository
    }

    pub fn resolve(
        &self,
        attempt_id: &str,
        resource_kind: ResourceKind,
        manifest: &ResourceBindingIdentity,
    ) -> Result<ResourceBinding, ResourceBindingError> {
        validate_identity(manifest)?;
        let binding = self
            .repository
            .load_for_attempt(attempt_id, resource_kind)?;
        if binding.identity.attempt_id != attempt_id {
            return Err(ResourceBindingError::IdentityMismatch("attempt_id"));
        }
        compare_identity(&binding.identity, manifest)?;
        Ok(binding)
    }
}

fn compare_identity(
    durable: &ResourceBindingIdentity,
    candidate: &ResourceBindingIdentity,
) -> Result<(), ResourceBindingError> {
    for (field, left, right) in [
        ("id", durable.id.as_str(), candidate.id.as_str()),
        (
            "delegation_id",
            durable.delegation_id.as_str(),
            candidate.delegation_id.as_str(),
        ),
        (
            "work_package_id",
            durable.work_package_id.as_str(),
            candidate.work_package_id.as_str(),
        ),
        (
            "attempt_id",
            durable.attempt_id.as_str(),
            candidate.attempt_id.as_str(),
        ),
        (
            "lease_id",
            durable.lease_id.as_str(),
            candidate.lease_id.as_str(),
        ),
        (
            "provider_kind",
            durable.provider_kind.as_str(),
            candidate.provider_kind.as_str(),
        ),
        (
            "resource_ref",
            durable.resource_ref.as_str(),
            candidate.resource_ref.as_str(),
        ),
        (
            "manifest_locator",
            durable.manifest_locator.as_str(),
            candidate.manifest_locator.as_str(),
        ),
        (
            "manifest_nonce",
            durable.manifest_nonce.as_str(),
            candidate.manifest_nonce.as_str(),
        ),
        (
            "manifest_digest",
            durable.manifest_digest.as_str(),
            candidate.manifest_digest.as_str(),
        ),
        (
            "scope_digest",
            durable.scope_digest.as_str(),
            candidate.scope_digest.as_str(),
        ),
    ] {
        if left != right {
            return Err(ResourceBindingError::IdentityMismatch(field));
        }
    }
    if durable.resource_kind != candidate.resource_kind
        || durable.manifest_version != candidate.manifest_version
        || durable.lease_epoch != candidate.lease_epoch
    {
        return Err(ResourceBindingError::IdentityMismatch("resource_tuple"));
    }
    if durable.admission != candidate.admission {
        return Err(ResourceBindingError::IdentityMismatch("admission"));
    }
    Ok(())
}

fn load_by_id(
    conn: &Connection,
    id: &str,
) -> Result<Option<ResourceBinding>, ResourceBindingError> {
    conn.query_row(
        "SELECT id, delegation_id, work_package_id, attempt_id, lease_id,
                resource_kind, provider_kind, resource_ref, manifest_locator,
                manifest_version, manifest_nonce, manifest_digest, scope_digest,
                lease_epoch, admission_id, admission_state_version,
                lifecycle_state, cleanup_status, cleanup_step, cleanup_attempts,
                next_cleanup_at, quarantine_until, last_error_class,
                created_at, updated_at, sealed_at, revoked_at, cleaned_at
           FROM delegation_resource_bindings WHERE id = ?1",
        [id],
        |row| {
            let admission_id: Option<String> = row.get(14)?;
            let admission_version: Option<i64> = row.get(15)?;
            let admission = match (admission_id, admission_version) {
                (None, None) => None,
                (Some(id), Some(state_version)) => Some(AdmissionCapture { id, state_version }),
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            Ok(ResourceBinding {
                identity: ResourceBindingIdentity {
                    id: row.get(0)?,
                    delegation_id: row.get(1)?,
                    work_package_id: row.get(2)?,
                    attempt_id: row.get(3)?,
                    lease_id: row.get(4)?,
                    resource_kind: ResourceKind::from_str(row.get::<_, String>(5)?.as_str())
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    provider_kind: row.get(6)?,
                    resource_ref: row.get(7)?,
                    manifest_locator: row.get(8)?,
                    manifest_version: row.get(9)?,
                    manifest_nonce: row.get(10)?,
                    manifest_digest: row.get(11)?,
                    scope_digest: row.get(12)?,
                    lease_epoch: row.get(13)?,
                    admission,
                },
                lifecycle_state: LifecycleState::from_str(row.get::<_, String>(16)?.as_str())
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                cleanup_status: CleanupStatus::from_str(row.get::<_, String>(17)?.as_str())
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                cleanup_step: row.get(18)?,
                cleanup_attempts: row.get(19)?,
                next_cleanup_at: row.get(20)?,
                quarantine_until: row.get(21)?,
                last_error_class: row.get(22)?,
                created_at: row.get(23)?,
                updated_at: row.get(24)?,
                sealed_at: row.get(25)?,
                revoked_at: row.get(26)?,
                cleaned_at: row.get(27)?,
            })
        },
    )
    .optional()
    .map_err(ResourceBindingError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::shared_db::SharedDb;
    use rusqlite::Connection;

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
            conn.execute("INSERT INTO delegation_attempts(id,delegation_id,attempt_number,status,sandbox_ref,created_at) VALUES ('a','d',1,'queued','legacy-ref',1)", []).unwrap();
            conn.execute("INSERT INTO delegation_capability_leases(id,attempt_id,read_roots_json,write_roots_json,tool_allowlist_json,network_hosts_json,budget_json,status,issued_at,expires_at) VALUES ('l','a','[]','[]','[]','[]','{}','active',1,1000)", []).unwrap();
        }).unwrap();
    }

    fn record() -> NewResourceBinding {
        NewResourceBinding {
            identity: ResourceBindingIdentity {
                id: "rb".into(),
                delegation_id: "d".into(),
                work_package_id: "wp".into(),
                attempt_id: "a".into(),
                lease_id: "l".into(),
                resource_kind: ResourceKind::Sandbox,
                provider_kind: "sandbox-v1".into(),
                resource_ref: "sandbox-opaque".into(),
                manifest_locator: "manifest-opaque".into(),
                manifest_version: 1,
                manifest_nonce: "nonce-1".into(),
                manifest_digest: "digest-1".into(),
                scope_digest: "scope".into(),
                lease_epoch: 1,
                admission: None,
            },
            created_at: 2,
        }
    }

    #[test]
    fn opaque_identity_round_trips_after_shared_db_restart() {
        let db = db();
        seed(&db);
        let repository = ResourceBindingRepository::with_clock(db.clone(), || 3);
        repository.create(&record()).unwrap();
        let restarted = ResourceBindingRepository::with_clock(db.clone(), || 4);
        let binding = restarted.load("rb").unwrap();
        assert_eq!(binding.identity.resource_ref, "sandbox-opaque");
        assert_eq!(binding.cleanup_status, CleanupStatus::None);
    }

    #[test]
    fn path_like_resource_identity_is_rejected_before_sql() {
        let mut record = record();
        record.identity.resource_ref = r"C:\\Users\\secret".into();
        assert!(matches!(
            validate_identity(&record.identity),
            Err(ResourceBindingError::InvalidOpaqueIdentity("resource_ref"))
        ));
    }

    #[test]
    fn resolver_rejects_unknown_and_identity_drift() {
        let db = db();
        seed(&db);
        let repository = ResourceBindingRepository::new(db.clone());
        let resolver = ResourceBindingResolver::new(repository.clone());
        assert_eq!(
            resolver.resolve("missing", ResourceKind::Sandbox, &record().identity),
            Err(ResourceBindingError::NotFound)
        );
        repository.create(&record()).unwrap();
        let mut forged = record().identity;
        forged.lease_epoch = 2;
        assert_eq!(
            resolver.resolve("a", ResourceKind::Sandbox, &forged),
            Err(ResourceBindingError::IdentityMismatch("resource_tuple"))
        );
    }

    #[test]
    fn cleanup_transition_is_cas_and_restart_scan_is_bounded() {
        let db = db();
        seed(&db);
        let repository = ResourceBindingRepository::with_clock(db.clone(), || 10);
        repository.create(&record()).unwrap();
        assert!(repository
            .transition_cleanup(
                "rb",
                CleanupStatus::None,
                CleanupStatus::Pending,
                Some(LifecycleState::Revoked),
                Some("revoke"),
                None,
                Some(20),
                None,
            )
            .unwrap());
        assert!(!repository
            .transition_cleanup(
                "rb",
                CleanupStatus::None,
                CleanupStatus::Retry,
                Some(LifecycleState::Unknown),
                Some("retry"),
                Some(30),
                Some(20),
                Some("locked"),
            )
            .unwrap());
        let restarted = repository.scan_restart(20).unwrap();
        assert_eq!(restarted.len(), 1);
        assert_eq!(restarted[0].cleanup_status, CleanupStatus::Pending);
    }

    #[test]
    fn unknown_provider_resource_is_never_guessed_from_legacy_sandbox_ref() {
        let db = db();
        seed(&db);
        let repository = ResourceBindingRepository::new(db.clone());
        assert_eq!(
            repository.load_for_attempt("a", ResourceKind::Worktree),
            Err(ResourceBindingError::NotFound)
        );
        assert!(repository.load(r"C:\\tmp\\legacy-ref").is_err());
    }
}
