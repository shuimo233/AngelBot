//! Durable confirmation boundary for one user-visible unit of work.
//!
//! This module is intentionally a no-privilege domain contract: it neither
//! starts workers nor writes the real workspace.  A later Materializer must
//! accept an *approved and still-current* `ChangeSet` from this facade.

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub use super::worker_policy::TaskShape;
use super::worker_policy::{CapabilityPolicy, WorkerProfile, WORKER_POLICY_VERSION};

pub const WORK_PACKAGE_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkPackageScope {
    pub session_id: String,
    pub owner_profile_id: i64,
    pub workspace_key: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorkPackage {
    pub scope: WorkPackageScope,
    pub task_shape: TaskShape,
    pub worker_profile: WorkerProfile,
    pub scope_digest: String,
    pub capability_scope_ref: String,
    pub capability_expires_at: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkPackage {
    pub id: String,
    pub scope: WorkPackageScope,
    pub task_shape: TaskShape,
    pub worker_profile: WorkerProfile,
    pub worker_policy_version: u16,
    pub scope_digest: String,
    pub capability_scope_ref: String,
    pub capability_expires_at: i64,
    pub candidate_version: i64,
    pub status: WorkPackageStatus,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkPackageStatus {
    Draft,
    Active,
    Frozen,
    Accepted,
    Declined,
    Cancelled,
    Expired,
}
impl WorkPackageStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Frozen => "frozen",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
    pub(crate) fn from_str(v: &str) -> Option<Self> {
        match v {
            "draft" => Some(Self::Draft),
            "active" => Some(Self::Active),
            "frozen" => Some(Self::Frozen),
            "accepted" => Some(Self::Accepted),
            "declined" => Some(Self::Declined),
            "cancelled" => Some(Self::Cancelled),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateArtifact {
    pub relative_ref: String,
    pub evidence_refs: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateSet {
    pub id: String,
    pub package_id: String,
    pub version: i64,
    pub artifacts: Vec<CandidateArtifact>,
    pub candidate_hash: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    pub id: String,
    pub package_id: String,
    pub candidate_set_id: String,
    pub scope_digest: String,
    pub candidate_hash: String,
    pub base_revision: String,
    pub summary_refs: Vec<String>,
    pub change_set_hash: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationStatus {
    Approved,
    Declined,
    Expired,
    Invalidated,
}
impl ConfirmationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Declined => "declined",
            Self::Expired => "expired",
            Self::Invalidated => "invalidated",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    pub id: String,
    pub change_set_id: String,
    pub status: ConfirmationStatus,
    pub expires_at: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkPackageError {
    #[error("work package not found in this scope")]
    NotFound,
    #[error("only change work packages can be confirmed")]
    ExploreCannotConfirm,
    #[error("candidate set version conflict")]
    VersionConflict,
    #[error("confirmation binding has drifted or expired")]
    BindingInvalid,
    #[error("invalid contract: {0}")]
    Invalid(String),
    #[error("storage: {0}")]
    Storage(String),
}

/// Small facade for Main Agent command handling and a future Materializer.
pub struct WorkPackageRepository;
impl WorkPackageRepository {
    /// Loads an immutable package through the caller's trusted workspace
    /// scope.  Issuance retries use this narrow read seam instead of
    /// reconstructing a package from model-provided fields.
    pub fn load(
        conn: &Connection,
        scope: &WorkPackageScope,
        package_id: &str,
    ) -> Result<WorkPackage, WorkPackageError> {
        load_package(conn, scope, package_id)
    }

    pub fn cancel(
        conn: &mut Connection,
        scope: &WorkPackageScope,
        package_id: &str,
        now: i64,
    ) -> Result<(), WorkPackageError> {
        let tx = conn.transaction().map_err(storage)?;
        let p = load_package_tx(&tx, scope, package_id)?;
        transition(&tx, &p.id, p.status, WorkPackageStatus::Cancelled, now)?;
        tx.commit().map_err(storage)
    }
    pub fn expire(
        conn: &mut Connection,
        scope: &WorkPackageScope,
        package_id: &str,
        now: i64,
    ) -> Result<(), WorkPackageError> {
        let tx = conn.transaction().map_err(storage)?;
        let p = load_package_tx(&tx, scope, package_id)?;
        transition(&tx, &p.id, p.status, WorkPackageStatus::Expired, now)?;
        tx.commit().map_err(storage)
    }
    pub fn create(
        conn: &mut Connection,
        new: NewWorkPackage,
    ) -> Result<WorkPackage, WorkPackageError> {
        validate_new(&new)?;
        let tx = conn.transaction().map_err(storage)?;
        ensure_session(&tx, &new.scope)?;
        let id = format!("wp_{}", Uuid::new_v4().simple());
        let now = Utc::now().timestamp();
        let policy = CapabilityPolicy::compile(new.task_shape, new.worker_profile);
        policy
            .validate()
            .map_err(|error| WorkPackageError::Invalid(error.to_string()))?;
        tx.execute("INSERT INTO work_packages (id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11)", params![id,new.scope.session_id,new.scope.owner_profile_id,new.scope.workspace_key,new.task_shape.as_str(),new.worker_profile.as_str(),WORKER_POLICY_VERSION,new.scope_digest,new.capability_scope_ref,new.capability_expires_at,now]).map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(WorkPackage {
            id,
            scope: new.scope,
            task_shape: new.task_shape,
            worker_profile: new.worker_profile,
            worker_policy_version: WORKER_POLICY_VERSION,
            scope_digest: new.scope_digest,
            capability_scope_ref: new.capability_scope_ref,
            capability_expires_at: new.capability_expires_at,
            candidate_version: 0,
            status: WorkPackageStatus::Active,
        })
    }

    /// Atomically replaces the package's current candidate aggregate.  A new
    /// aggregate invalidates every prior approval for this package.
    pub fn replace_candidates(
        conn: &mut Connection,
        scope: &WorkPackageScope,
        package_id: &str,
        expected_version: i64,
        artifacts: Vec<CandidateArtifact>,
    ) -> Result<CandidateSet, WorkPackageError> {
        validate_artifacts(&artifacts)?;
        let tx = conn.transaction().map_err(storage)?;
        let candidate =
            Self::replace_candidates_in_tx(&tx, scope, package_id, expected_version, artifacts)?;
        tx.commit().map_err(storage)?;
        Ok(candidate)
    }

    pub(crate) fn replace_candidates_in_tx(
        tx: &Transaction<'_>,
        scope: &WorkPackageScope,
        package_id: &str,
        expected_version: i64,
        artifacts: Vec<CandidateArtifact>,
    ) -> Result<CandidateSet, WorkPackageError> {
        validate_artifacts(&artifacts)?;
        let package = load_package_tx(tx, scope, package_id)?;
        ensure_active(&package)?;
        if package.candidate_version != expected_version {
            return Err(WorkPackageError::VersionConflict);
        }
        let version = expected_version + 1;
        let manifest = canonical_artifacts(&artifacts)?;
        let candidate_hash = digest(&manifest);
        let id = format!("cset_{}", Uuid::new_v4().simple());
        let now = Utc::now().timestamp();
        let moved = tx.execute("UPDATE work_packages SET candidate_version=?1,updated_at=?2 WHERE id=?3 AND candidate_version=?4",params![version,now,package_id,expected_version]).map_err(storage)?;
        if moved != 1 {
            return Err(WorkPackageError::VersionConflict);
        }
        tx.execute("INSERT INTO work_package_candidate_sets (id,work_package_id,version,manifest_json,candidate_hash,created_at) VALUES (?1,?2,?3,?4,?5,?6)",params![id,package_id,version,manifest,candidate_hash,now]).map_err(storage)?;
        tx.execute("UPDATE work_package_confirmations SET status='invalidated',updated_at=?1 WHERE work_package_id=?2 AND status='approved'",params![now,package_id]).map_err(storage)?;
        Ok(CandidateSet {
            id,
            package_id: package_id.into(),
            version,
            artifacts,
            candidate_hash,
        })
    }

    pub fn create_change_set(
        conn: &mut Connection,
        scope: &WorkPackageScope,
        package_id: &str,
        candidate_set_id: &str,
        base_revision: &str,
        summary_refs: Vec<String>,
    ) -> Result<ChangeSet, WorkPackageError> {
        if !short(base_revision) || summary_refs.iter().any(|r| !short(r)) {
            return Err(WorkPackageError::Invalid(
                "invalid base revision or summary reference".into(),
            ));
        }
        let tx = conn.transaction().map_err(storage)?;
        let change = Self::create_change_set_in_tx(
            &tx,
            scope,
            package_id,
            candidate_set_id,
            base_revision,
            summary_refs,
        )?;
        tx.commit().map_err(storage)?;
        Ok(change)
    }

    pub(crate) fn create_change_set_in_tx(
        tx: &Transaction<'_>,
        scope: &WorkPackageScope,
        package_id: &str,
        candidate_set_id: &str,
        base_revision: &str,
        summary_refs: Vec<String>,
    ) -> Result<ChangeSet, WorkPackageError> {
        if !short(base_revision) || summary_refs.iter().any(|r| !short(r)) {
            return Err(WorkPackageError::Invalid(
                "invalid base revision or summary reference".into(),
            ));
        }
        let package = load_package_tx(tx, scope, package_id)?;
        ensure_active(&package)?;
        let (version, candidate_hash):(i64,String)=tx.query_row("SELECT version,candidate_hash FROM work_package_candidate_sets WHERE id=?1 AND work_package_id=?2",params![candidate_set_id,package_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(storage)?.ok_or(WorkPackageError::NotFound)?;
        if version != package.candidate_version {
            return Err(WorkPackageError::BindingInvalid);
        }
        let summary_json = canonical_refs(&summary_refs)?;
        let binding=format!("v={WORK_PACKAGE_VERSION}|scope={}|candidate={candidate_hash}|base={base_revision}|summary={summary_json}",package.scope_digest);
        let change_set_hash = digest(&binding);
        let id = format!("csetchg_{}", Uuid::new_v4().simple());
        let now = Utc::now().timestamp();
        tx.execute("INSERT INTO work_package_change_sets (id,work_package_id,candidate_set_id,scope_digest,candidate_hash,base_revision,summary_refs_json,change_set_hash,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![id,package_id,candidate_set_id,package.scope_digest,candidate_hash,base_revision,summary_json,change_set_hash,now]).map_err(storage)?;
        transition(
            &tx,
            &package.id,
            WorkPackageStatus::Active,
            WorkPackageStatus::Frozen,
            now,
        )?;
        Ok(ChangeSet {
            id,
            package_id: package_id.into(),
            candidate_set_id: candidate_set_id.into(),
            scope_digest: package.scope_digest,
            candidate_hash,
            base_revision: base_revision.into(),
            summary_refs,
            change_set_hash,
        })
    }

    /// Records a user decision only when every immutable binding is still
    /// current.  `approved` is deliberately not materialization authority.
    pub fn confirm(
        conn: &mut Connection,
        scope: &WorkPackageScope,
        change_set_id: &str,
        approve: bool,
        ttl_secs: i64,
        now: i64,
    ) -> Result<Confirmation, WorkPackageError> {
        if ttl_secs <= 0 {
            return Err(WorkPackageError::Invalid(
                "confirmation TTL must be positive".into(),
            ));
        }
        let tx = conn.transaction().map_err(storage)?;
        let (package, change) = load_change_tx(&tx, scope, change_set_id)?;
        if package.status != WorkPackageStatus::Frozen {
            return Err(WorkPackageError::BindingInvalid);
        }
        if package.task_shape == TaskShape::Explore {
            return Err(WorkPackageError::ExploreCannotConfirm);
        }
        let current: Option<String>=tx.query_row("SELECT candidate_hash FROM work_package_candidate_sets WHERE work_package_id=?1 AND version=?2",params![package.id,package.candidate_version],|r|r.get(0)).optional().map_err(storage)?;
        let valid = current.as_deref() == Some(&change.candidate_hash)
            && package.scope_digest == change.scope_digest;
        let status = if !valid {
            ConfirmationStatus::Invalidated
        } else if approve {
            ConfirmationStatus::Approved
        } else {
            ConfirmationStatus::Declined
        };
        let id = format!("confirm_{}", Uuid::new_v4().simple());
        let expires_at = now + ttl_secs;
        tx.execute("INSERT INTO work_package_confirmations (id,work_package_id,change_set_id,status,scope_digest,candidate_hash,base_revision,change_set_hash,expires_at,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10)",params![id,package.id,change_set_id,status.as_str(),change.scope_digest,change.candidate_hash,change.base_revision,change.change_set_hash,expires_at,now]).map_err(storage)?;
        transition(
            &tx,
            &package.id,
            WorkPackageStatus::Frozen,
            if approve {
                WorkPackageStatus::Accepted
            } else {
                WorkPackageStatus::Declined
            },
            now,
        )?;
        tx.commit().map_err(storage)?;
        Ok(Confirmation {
            id,
            change_set_id: change_set_id.into(),
            status,
            expires_at,
        })
    }

    /// A future Materializer calls this immediately before writing. It makes
    /// an approval unusable if time elapsed or candidate/scope bindings drift.
    pub fn approved_for_materialization(
        conn: &mut Connection,
        scope: &WorkPackageScope,
        confirmation_id: &str,
        observed_base_revision: &str,
        now: i64,
    ) -> Result<ChangeSet, WorkPackageError> {
        let tx = conn.transaction().map_err(storage)?;
        let (change_id,status,expires):(String,String,i64)=tx.query_row("SELECT c.change_set_id,c.status,c.expires_at FROM work_package_confirmations c JOIN work_packages p ON p.id=c.work_package_id WHERE c.id=?1 AND p.session_id=?2 AND p.owner_profile_id=?3 AND p.workspace_key=?4",params![confirmation_id,scope.session_id,scope.owner_profile_id,scope.workspace_key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(storage)?.ok_or(WorkPackageError::NotFound)?;
        if status != "approved" || expires <= now {
            if status == "approved" {
                tx.execute("UPDATE work_package_confirmations SET status='expired',updated_at=?1 WHERE id=?2",params![now,confirmation_id]).map_err(storage)?;
                tx.commit().map_err(storage)?;
            }
            return Err(WorkPackageError::BindingInvalid);
        }
        let (package, change) = load_change_tx(&tx, scope, &change_id)?;
        let current:Option<String>=tx.query_row("SELECT candidate_hash FROM work_package_candidate_sets WHERE work_package_id=?1 AND version=?2",params![package.id,package.candidate_version],|r|r.get(0)).optional().map_err(storage)?;
        if package.status != WorkPackageStatus::Accepted
            || package.task_shape != TaskShape::Change
            || package.scope_digest != change.scope_digest
            || current.as_deref() != Some(&change.candidate_hash)
            || observed_base_revision != change.base_revision
        {
            tx.execute("UPDATE work_package_confirmations SET status='invalidated',updated_at=?1 WHERE id=?2",params![now,confirmation_id]).map_err(storage)?;
            tx.commit().map_err(storage)?;
            return Err(WorkPackageError::BindingInvalid);
        }
        tx.commit().map_err(storage)?;
        Ok(change)
    }
}

fn ensure_session(tx: &Transaction<'_>, scope: &WorkPackageScope) -> Result<(), WorkPackageError> {
    let found: Option<String> = tx
        .query_row(
            "SELECT id FROM sessions WHERE id=?1",
            [&scope.session_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage)?;
    if found.is_none() {
        return Err(WorkPackageError::NotFound);
    }
    Ok(())
}
fn ensure_active(p: &WorkPackage) -> Result<(), WorkPackageError> {
    if p.status == WorkPackageStatus::Active {
        Ok(())
    } else {
        Err(WorkPackageError::BindingInvalid)
    }
}
fn transition(
    tx: &Transaction<'_>,
    id: &str,
    from: WorkPackageStatus,
    to: WorkPackageStatus,
    now: i64,
) -> Result<(), WorkPackageError> {
    let ok = matches!(
        (from, to),
        (
            WorkPackageStatus::Draft,
            WorkPackageStatus::Active | WorkPackageStatus::Cancelled | WorkPackageStatus::Expired
        ) | (
            WorkPackageStatus::Active,
            WorkPackageStatus::Frozen | WorkPackageStatus::Cancelled | WorkPackageStatus::Expired
        ) | (
            WorkPackageStatus::Frozen,
            WorkPackageStatus::Accepted
                | WorkPackageStatus::Declined
                | WorkPackageStatus::Cancelled
                | WorkPackageStatus::Expired
        )
    );
    if !ok {
        return Err(WorkPackageError::BindingInvalid);
    }
    let n = tx
        .execute(
            "UPDATE work_packages SET status=?1,updated_at=?2 WHERE id=?3 AND status=?4",
            params![to.as_str(), now, id, from.as_str()],
        )
        .map_err(storage)?;
    if n == 1 {
        Ok(())
    } else {
        Err(WorkPackageError::BindingInvalid)
    }
}
fn load_package_tx(
    tx: &Transaction<'_>,
    s: &WorkPackageScope,
    id: &str,
) -> Result<WorkPackage, WorkPackageError> {
    load_package(tx, s, id)
}

fn load_package(
    conn: &Connection,
    s: &WorkPackageScope,
    id: &str,
) -> Result<WorkPackage, WorkPackageError> {
    conn.query_row("SELECT id,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,candidate_version,status FROM work_packages WHERE id=?1 AND session_id=?2 AND owner_profile_id=?3 AND workspace_key=?4",params![id,s.session_id,s.owner_profile_id,s.workspace_key],|r|{let shape:String=r.get(1)?;let profile:String=r.get(2)?;let version:i64=r.get(3)?;let status:String=r.get(8)?; let task_shape=TaskShape::from_str(&shape).ok_or_else(||rusqlite::Error::InvalidQuery)?;let worker_profile=WorkerProfile::from_str(&profile).ok_or_else(||rusqlite::Error::InvalidQuery)?; let worker_policy_version=u16::try_from(version).map_err(|_|rusqlite::Error::InvalidQuery)?;let status=WorkPackageStatus::from_str(&status).ok_or_else(||rusqlite::Error::InvalidQuery)?;Ok(WorkPackage{id:r.get(0)?,scope:s.clone(),task_shape,worker_profile,worker_policy_version,scope_digest:r.get(4)?,capability_scope_ref:r.get(5)?,capability_expires_at:r.get(6)?,candidate_version:r.get(7)?,status})}).optional().map_err(storage)?.ok_or(WorkPackageError::NotFound)
}
fn load_change_tx(
    tx: &Transaction<'_>,
    s: &WorkPackageScope,
    id: &str,
) -> Result<(WorkPackage, ChangeSet), WorkPackageError> {
    let p=tx.query_row("SELECT p.id,p.task_shape,p.worker_profile,p.worker_policy_version,p.scope_digest,p.capability_scope_ref,p.capability_expires_at,p.candidate_version,p.status,c.id,c.candidate_set_id,c.candidate_hash,c.base_revision,c.summary_refs_json,c.change_set_hash FROM work_package_change_sets c JOIN work_packages p ON p.id=c.work_package_id WHERE c.id=?1 AND p.session_id=?2 AND p.owner_profile_id=?3 AND p.workspace_key=?4",params![id,s.session_id,s.owner_profile_id,s.workspace_key],|r|{let shape:String=r.get(1)?;let profile:String=r.get(2)?;let version:i64=r.get(3)?;let status:String=r.get(8)?;let refs:String=r.get(13)?;let task_shape=TaskShape::from_str(&shape).ok_or_else(||rusqlite::Error::InvalidQuery)?;let worker_profile=WorkerProfile::from_str(&profile).ok_or_else(||rusqlite::Error::InvalidQuery)?;let worker_policy_version=u16::try_from(version).map_err(|_|rusqlite::Error::InvalidQuery)?;let status=WorkPackageStatus::from_str(&status).ok_or_else(||rusqlite::Error::InvalidQuery)?;let package=WorkPackage{id:r.get(0)?,scope:s.clone(),task_shape,worker_profile,worker_policy_version,scope_digest:r.get(4)?,capability_scope_ref:r.get(5)?,capability_expires_at:r.get(6)?,candidate_version:r.get(7)?,status}; let c=ChangeSet{id:id.into(),package_id:package.id.clone(),candidate_set_id:r.get(10)?,scope_digest:package.scope_digest.clone(),candidate_hash:r.get(11)?,base_revision:r.get(12)?,summary_refs:serde_json::from_str(&refs).map_err(json_error)?,change_set_hash:r.get(14)?};Ok((package,c))}).optional().map_err(storage)?.ok_or(WorkPackageError::NotFound)?;
    Ok(p)
}
fn validate_new(v: &NewWorkPackage) -> Result<(), WorkPackageError> {
    if !short(&v.scope.session_id)
        || v.scope.owner_profile_id <= 0
        || !short(&v.scope.workspace_key)
        || !short(&v.scope_digest)
        || !short(&v.capability_scope_ref)
        || v.capability_expires_at <= 0
    {
        return Err(WorkPackageError::Invalid(
            "invalid work package scope".into(),
        ));
    }
    Ok(())
}
fn validate_artifacts(v: &[CandidateArtifact]) -> Result<(), WorkPackageError> {
    if v.len() > 128
        || v.iter().any(|a| {
            !short(&a.relative_ref)
                || a.evidence_refs.len() > 8
                || a.evidence_refs.iter().any(|r| !short(r))
        })
    {
        return Err(WorkPackageError::Invalid(
            "invalid candidate artifact".into(),
        ));
    }
    Ok(())
}
fn canonical_artifacts(v: &[CandidateArtifact]) -> Result<String, WorkPackageError> {
    let mut x = v.to_vec();
    x.sort_by(|a, b| a.relative_ref.cmp(&b.relative_ref));
    for a in &mut x {
        a.evidence_refs.sort();
        a.evidence_refs.dedup()
    }
    serde_json::to_string(&x).map_err(|e| WorkPackageError::Storage(e.to_string()))
}
fn canonical_refs(v: &[String]) -> Result<String, WorkPackageError> {
    let mut x = v.to_vec();
    x.sort();
    x.dedup();
    serde_json::to_string(&x).map_err(|e| WorkPackageError::Storage(e.to_string()))
}
fn digest(v: &str) -> String {
    let mut h = Sha256::new();
    h.update(v.as_bytes());
    format!("sha256:{}", hex::encode(h.finalize()))
}
fn short(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 256
        && v.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':' | b'.' | b'/' | b'@')
        })
}
fn storage(e: rusqlite::Error) -> WorkPackageError {
    WorkPackageError::Storage(e.to_string())
}
fn json_error(e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    fn scope() -> WorkPackageScope {
        WorkPackageScope {
            session_id: "s".into(),
            owner_profile_id: 1,
            workspace_key: "workspace".into(),
        }
    }
    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::db::migrate(&c).unwrap();
        c.execute(
            "INSERT INTO sessions (id,title,created_at,updated_at) VALUES ('s','s',0,0)",
            [],
        )
        .unwrap();
        c
    }
    fn package(c: &mut Connection, shape: TaskShape) -> WorkPackage {
        WorkPackageRepository::create(
            c,
            NewWorkPackage {
                scope: scope(),
                task_shape: shape,
                worker_profile: WorkerProfile::Implementer,
                scope_digest: "scope_a".into(),
                capability_scope_ref: "cap_a".into(),
                capability_expires_at: 9999999999,
            },
        )
        .unwrap()
    }
    fn candidate(c: &mut Connection, p: &WorkPackage) -> CandidateSet {
        WorkPackageRepository::replace_candidates(
            c,
            &scope(),
            &p.id,
            0,
            vec![CandidateArtifact {
                relative_ref: "src/a.rs".into(),
                evidence_refs: vec!["evidence:a".into()],
            }],
        )
        .unwrap()
    }
    #[test]
    fn explore_cannot_confirm() {
        let mut c = db();
        let p = package(&mut c, TaskShape::Explore);
        let d = candidate(&mut c, &p);
        let ch = WorkPackageRepository::create_change_set(
            &mut c,
            &scope(),
            &p.id,
            &d.id,
            "base1",
            vec!["summary:a".into()],
        )
        .unwrap();
        assert!(matches!(
            WorkPackageRepository::confirm(&mut c, &scope(), &ch.id, true, 10, 100),
            Err(WorkPackageError::ExploreCannotConfirm)
        ));
    }
    #[test]
    fn approval_requires_unchanged_bindings_and_expiry() {
        let mut c = db();
        let p = package(&mut c, TaskShape::Change);
        let d = candidate(&mut c, &p);
        let ch = WorkPackageRepository::create_change_set(
            &mut c,
            &scope(),
            &p.id,
            &d.id,
            "base1",
            vec![],
        )
        .unwrap();
        let ok = WorkPackageRepository::confirm(&mut c, &scope(), &ch.id, true, 10, 100).unwrap();
        assert!(WorkPackageRepository::approved_for_materialization(
            &mut c,
            &scope(),
            &ok.id,
            "base1",
            109
        )
        .is_ok());
        assert!(matches!(
            WorkPackageRepository::approved_for_materialization(
                &mut c,
                &scope(),
                &ok.id,
                "base1",
                110
            ),
            Err(WorkPackageError::BindingInvalid)
        ));
    }
    #[test]
    fn base_revision_drift_invalidates_approval() {
        let mut c = db();
        let p = package(&mut c, TaskShape::Change);
        let d = candidate(&mut c, &p);
        let ch = WorkPackageRepository::create_change_set(
            &mut c,
            &scope(),
            &p.id,
            &d.id,
            "base1",
            vec![],
        )
        .unwrap();
        let ok = WorkPackageRepository::confirm(&mut c, &scope(), &ch.id, true, 99, 100).unwrap();
        assert!(matches!(
            WorkPackageRepository::approved_for_materialization(
                &mut c,
                &scope(),
                &ok.id,
                "base2",
                101
            ),
            Err(WorkPackageError::BindingInvalid)
        ));
    }
    #[test]
    fn frozen_package_rejects_candidate_drift() {
        let mut c = db();
        let p = package(&mut c, TaskShape::Change);
        let d = candidate(&mut c, &p);
        let _ = WorkPackageRepository::create_change_set(
            &mut c,
            &scope(),
            &p.id,
            &d.id,
            "base1",
            vec![],
        )
        .unwrap();
        assert!(matches!(
            WorkPackageRepository::replace_candidates(
                &mut c,
                &scope(),
                &p.id,
                1,
                vec![CandidateArtifact {
                    relative_ref: "src/b.rs".into(),
                    evidence_refs: vec![]
                }]
            ),
            Err(WorkPackageError::BindingInvalid)
        ));
    }
    #[test]
    fn ownership_and_version_conflicts_are_isolated() {
        let mut c = db();
        let p = package(&mut c, TaskShape::Change);
        let bad = WorkPackageScope {
            owner_profile_id: 2,
            ..scope()
        };
        assert!(matches!(
            WorkPackageRepository::replace_candidates(&mut c, &bad, &p.id, 0, vec![]),
            Err(WorkPackageError::NotFound)
        ));
        candidate(&mut c, &p);
        assert!(matches!(
            WorkPackageRepository::replace_candidates(&mut c, &scope(), &p.id, 0, vec![]),
            Err(WorkPackageError::VersionConflict)
        ));
    }
    #[test]
    fn selected_profile_and_policy_version_are_frozen_for_recovery() {
        let mut c = db();
        let p = package(&mut c, TaskShape::Change);
        assert_eq!(p.worker_profile, WorkerProfile::Implementer);
        assert_eq!(p.worker_policy_version, WORKER_POLICY_VERSION);
        let row: (String, i64) = c
            .query_row(
                "SELECT worker_profile,worker_policy_version FROM work_packages WHERE id=?1",
                [&p.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            ("implementer".into(), i64::from(WORKER_POLICY_VERSION))
        );
    }
}
