//! Durable store for implementation artifacts and independent review facts.
//!
//! The store is intentionally separate from the Git/worktree provider.  It
//! persists only immutable commit identity and bounded evidence references;
//! provider paths remain opaque and provider-owned.  Review binding is a
//! one-shot CAS, so a verifier cannot replace an earlier verdict after a
//! restart or race.

use rusqlite::{params, Connection};

use super::{
    shared_db::SharedDb,
    work_package::ChangeSet,
    workspace_isolation::{ReviewArtifact, ReviewArtifactError, ReviewEvidence, ReviewVerdict},
};

const MAX_PATHS: usize = 128;
const MAX_REFS: usize = 32;
const MAX_REF_BYTES: usize = 512;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ReviewArtifactStoreError {
    #[error("review artifact is invalid")]
    Invalid,
    #[error("review artifact not found")]
    NotFound,
    #[error("review artifact identity already exists with different facts")]
    IdentityConflict,
    #[error("change-set binding is invalid")]
    BindingInvalid,
    #[error("review is already recorded")]
    ReviewAlreadyRecorded,
    #[error("review is invalid")]
    ReviewInvalid,
    #[error("database error")]
    Storage,
}

#[derive(Clone)]
pub struct ReviewArtifactStore {
    db: SharedDb,
}

impl ReviewArtifactStore {
    pub fn new(db: SharedDb) -> Self {
        Self { db }
    }

    /// Persist the capture exactly once.  A retry with the same identity is
    /// idempotent; a retry with changed commit/path facts is rejected.
    pub fn capture(
        &self,
        artifact: &ReviewArtifact,
    ) -> Result<ReviewArtifact, ReviewArtifactStoreError> {
        validate_capture(artifact)?;
        let changed = serde_json::to_string(&artifact.changed_paths)
            .map_err(|_| ReviewArtifactStoreError::Invalid)?;
        let allowed = serde_json::to_string(&artifact.allowed_relative_paths)
            .map_err(|_| ReviewArtifactStoreError::Invalid)?;
        self.db
            .with_conn_mut(|conn| {
                conn.execute(
                    "INSERT INTO delegation_review_artifacts
                     (id,schema_version,work_package_id,change_set_id,implementation_attempt_id,
                      scope_digest,base_revision,base_commit,branch,audit_ref,commit_sha,diff_hash,
                      changed_paths_json,allowed_relative_paths_json,created_at,updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?15)
                     ON CONFLICT(id) DO NOTHING",
                    params![
                        artifact.id,
                        i64::from(artifact.schema_version),
                        artifact.work_package_id,
                        artifact.change_set_id,
                        artifact.implementation_attempt_id,
                        artifact.scope_digest,
                        artifact.base_revision,
                        artifact.base_commit,
                        artifact.branch,
                        artifact.audit_ref,
                        artifact.commit_sha,
                        artifact.diff_hash,
                        changed,
                        allowed,
                        artifact.created_at,
                    ],
                )
                .map_err(|_| ReviewArtifactStoreError::Storage)
            })
            .map_err(|_| ReviewArtifactStoreError::Storage)??;
        let stored = self.load(&artifact.id)?;
        if same_capture_identity(&stored, artifact) {
            Ok(stored)
        } else {
            Err(ReviewArtifactStoreError::IdentityConflict)
        }
    }

    /// Bind a ChangeSet exactly once.  The durable row is the authority used
    /// by restart/review/materialization paths.
    pub fn bind_change_set(
        &self,
        artifact_id: &str,
        change_set: &ChangeSet,
        now: i64,
    ) -> Result<ReviewArtifact, ReviewArtifactStoreError> {
        let artifact = self.load(artifact_id)?;
        let _candidate = artifact
            .clone()
            .bind_change_set(change_set)
            .map_err(map_artifact_error)?;
        self.db
            .with_conn_mut(|conn| {
                conn.execute(
                    "UPDATE delegation_review_artifacts
                     SET change_set_id=?1,updated_at=?2
                     WHERE id=?3 AND change_set_id IS NULL",
                    params![change_set.id, now, artifact_id],
                )
                .map_err(|_| ReviewArtifactStoreError::Storage)
            })
            .map_err(|_| ReviewArtifactStoreError::Storage)??;
        let stored = self.load(artifact_id)?;
        if stored.change_set_id.as_deref() == Some(change_set.id.as_str()) {
            Ok(candidate_with_binding(stored, change_set))
        } else {
            Err(ReviewArtifactStoreError::BindingInvalid)
        }
    }

    /// Record independent verifier evidence with a one-shot compare-and-set.
    pub fn record_review(
        &self,
        artifact_id: &str,
        review: &ReviewEvidence,
        now: i64,
    ) -> Result<ReviewArtifact, ReviewArtifactStoreError> {
        let artifact = self.load(artifact_id)?;
        if artifact.change_set_id.is_none() {
            return Err(ReviewArtifactStoreError::BindingInvalid);
        }
        let candidate = artifact
            .clone()
            .bind_review(review.clone())
            .map_err(map_artifact_error)?;
        let refs = serde_json::to_string(&review.evidence_refs)
            .map_err(|_| ReviewArtifactStoreError::ReviewInvalid)?;
        let changed = self
            .db
            .with_conn_mut(|conn| {
                conn.execute(
                    "UPDATE delegation_review_artifacts
                     SET verifier_attempt_id=?1,verdict=?2,reviewed_commit_sha=?3,
                         reviewed_diff_hash=?4,evidence_refs_json=?5,reviewed_at=?6,updated_at=?7
                     WHERE id=?8 AND verdict IS NULL",
                    params![
                        review.verifier_attempt_id,
                        verdict_str(review.verdict),
                        review.reviewed_commit_sha,
                        review.reviewed_diff_hash,
                        refs,
                        review.reviewed_at,
                        now,
                        artifact_id,
                    ],
                )
                .map_err(|_| ReviewArtifactStoreError::Storage)
            })
            .map_err(|_| ReviewArtifactStoreError::Storage)??;
        if changed != 1 {
            return Err(ReviewArtifactStoreError::ReviewAlreadyRecorded);
        }
        let stored = self.load(artifact_id)?;
        if stored.review == candidate.review {
            Ok(stored)
        } else {
            Err(ReviewArtifactStoreError::ReviewInvalid)
        }
    }

    pub fn load(&self, artifact_id: &str) -> Result<ReviewArtifact, ReviewArtifactStoreError> {
        self.db
            .with_conn(|conn| load_artifact(conn, artifact_id))
            .map_err(|_| ReviewArtifactStoreError::Storage)?
            .map_err(|_| ReviewArtifactStoreError::NotFound)
    }

    pub fn load_ready(
        &self,
        artifact_id: &str,
    ) -> Result<ReviewArtifact, ReviewArtifactStoreError> {
        let artifact = self.load(artifact_id)?;
        artifact
            .ready_for_materialization()
            .map_err(|_| ReviewArtifactStoreError::ReviewInvalid)?;
        Ok(artifact)
    }
}

fn load_artifact(conn: &Connection, id: &str) -> Result<ReviewArtifact, rusqlite::Error> {
    conn.query_row(
        "SELECT id,schema_version,work_package_id,change_set_id,implementation_attempt_id,
                scope_digest,base_revision,base_commit,branch,audit_ref,commit_sha,diff_hash,
                changed_paths_json,allowed_relative_paths_json,created_at,verifier_attempt_id,
                verdict,reviewed_commit_sha,reviewed_diff_hash,evidence_refs_json,reviewed_at
         FROM delegation_review_artifacts WHERE id=?1",
        [id],
        |row| {
            let verdict: Option<String> = row.get(16)?;
            let evidence_refs: Option<String> = row.get(19)?;
            let review = match (
                verdict,
                row.get::<_, Option<String>>(15)?,
                row.get(17)?,
                row.get(18)?,
                evidence_refs,
                row.get(20)?,
            ) {
                (
                    Some(verdict),
                    Some(verifier),
                    Some(commit),
                    Some(diff),
                    Some(refs),
                    Some(reviewed_at),
                ) => Some(ReviewEvidence {
                    verifier_attempt_id: verifier,
                    verdict: parse_verdict(&verdict).ok_or(rusqlite::Error::InvalidQuery)?,
                    reviewed_commit_sha: commit,
                    reviewed_diff_hash: diff,
                    evidence_refs: serde_json::from_str(&refs)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    reviewed_at,
                }),
                (None, None, None, None, None, None) => None,
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            Ok(ReviewArtifact {
                id: row.get(0)?,
                schema_version: row.get::<_, i64>(1)? as u16,
                work_package_id: row.get(2)?,
                change_set_id: row.get(3)?,
                implementation_attempt_id: row.get(4)?,
                scope_digest: row.get(5)?,
                base_revision: row.get(6)?,
                base_commit: row.get(7)?,
                branch: row.get(8)?,
                audit_ref: row.get(9)?,
                commit_sha: row.get(10)?,
                diff_hash: row.get(11)?,
                changed_paths: serde_json::from_str(&row.get::<_, String>(12)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                allowed_relative_paths: serde_json::from_str(&row.get::<_, String>(13)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                created_at: row.get(14)?,
                review,
            })
        },
    )
}

fn validate_capture(artifact: &ReviewArtifact) -> Result<(), ReviewArtifactStoreError> {
    if artifact.id.is_empty()
        || artifact.work_package_id.is_empty()
        || artifact.implementation_attempt_id.is_empty()
        || artifact.scope_digest.is_empty()
        || artifact.base_revision.is_empty()
        || artifact.base_commit.len() < 40
        || artifact.commit_sha.len() < 40
        || artifact.branch.is_empty()
        || artifact.audit_ref.is_empty()
        || artifact.review.is_some()
        || artifact.changed_paths.len() > MAX_PATHS
        || artifact.allowed_relative_paths.len() > MAX_PATHS
        || artifact.changed_paths.iter().any(|path| !safe_path(path))
        || artifact
            .allowed_relative_paths
            .iter()
            .any(|path| !safe_path(path))
    {
        return Err(ReviewArtifactStoreError::Invalid);
    }
    Ok(())
}

fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 512
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && !path.contains(['\\', '\0', '\r', '\n'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn same_capture_identity(left: &ReviewArtifact, right: &ReviewArtifact) -> bool {
    left.schema_version == right.schema_version
        && left.work_package_id == right.work_package_id
        && left.change_set_id == right.change_set_id
        && left.implementation_attempt_id == right.implementation_attempt_id
        && left.scope_digest == right.scope_digest
        && left.base_revision == right.base_revision
        && left.base_commit == right.base_commit
        && left.branch == right.branch
        && left.audit_ref == right.audit_ref
        && left.commit_sha == right.commit_sha
        && left.diff_hash == right.diff_hash
        && left.changed_paths == right.changed_paths
        && left.allowed_relative_paths == right.allowed_relative_paths
        && left.created_at == right.created_at
}

fn candidate_with_binding(mut stored: ReviewArtifact, change_set: &ChangeSet) -> ReviewArtifact {
    stored.change_set_id = Some(change_set.id.clone());
    stored
}

fn map_artifact_error(error: ReviewArtifactError) -> ReviewArtifactStoreError {
    match error {
        ReviewArtifactError::ChangeSetAlreadyBound | ReviewArtifactError::ScopeDigestMismatch => {
            ReviewArtifactStoreError::BindingInvalid
        }
        ReviewArtifactError::ReviewAlreadyBound => ReviewArtifactStoreError::ReviewAlreadyRecorded,
        _ => ReviewArtifactStoreError::ReviewInvalid,
    }
}

fn verdict_str(verdict: ReviewVerdict) -> &'static str {
    match verdict {
        ReviewVerdict::Passed => "passed",
        ReviewVerdict::Failed => "failed",
        ReviewVerdict::NeedsDecision => "needs_decision",
    }
}

fn parse_verdict(value: &str) -> Option<ReviewVerdict> {
    match value {
        "passed" => Some(ReviewVerdict::Passed),
        "failed" => Some(ReviewVerdict::Failed),
        "needs_decision" => Some(ReviewVerdict::NeedsDecision),
        _ => None,
    }
}
