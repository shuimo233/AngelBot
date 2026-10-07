//! Main-Agent-controlled independent reviewer boundary.
//!
//! This module does not run a model or expose a worktree path.  A concrete
//! verifier host supplies bounded evidence through `complete`; this gate
//! proves that the verifier is a distinct attempt with read-only authority
//! before handing the evidence to the durable one-shot store.

use super::{
    review_artifact_store::{ReviewArtifactStore, ReviewArtifactStoreError},
    workspace_isolation::{ReviewArtifact, ReviewEvidence, ReviewVerdict},
};

const MAX_EVIDENCE_REFS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewerAttemptRequest {
    pub artifact_id: String,
    pub implementation_attempt_id: String,
    pub verifier_attempt_id: String,
    pub scope_digest: String,
    /// This is a trusted capability fact resolved by the service, not a model
    /// field.  A verifier with any write grant is rejected before review.
    pub read_only: bool,
    pub reviewed_commit_sha: String,
    pub reviewed_diff_hash: String,
    pub verdict: ReviewVerdict,
    pub evidence_refs: Vec<String>,
    pub reviewed_at: i64,
    pub now: i64,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ReviewerAttemptError {
    #[error("reviewer request is invalid")]
    InvalidRequest,
    #[error("reviewer capability is not read-only")]
    WriteCapability,
    #[error("reviewer attempt must be distinct")]
    SameAttempt,
    #[error("artifact scope does not match reviewer scope")]
    ScopeMismatch,
    #[error("durable review store rejected evidence: {0}")]
    Store(ReviewArtifactStoreError),
}

pub struct ReviewerAttemptService {
    store: ReviewArtifactStore,
}

impl ReviewerAttemptService {
    pub fn new(store: ReviewArtifactStore) -> Self {
        Self { store }
    }

    pub fn complete(
        &self,
        request: ReviewerAttemptRequest,
    ) -> Result<ReviewArtifact, ReviewerAttemptError> {
        if !request.read_only {
            return Err(ReviewerAttemptError::WriteCapability);
        }
        if request.verifier_attempt_id.is_empty()
            || request.implementation_attempt_id.is_empty()
            || request.verifier_attempt_id == request.implementation_attempt_id
            || request.artifact_id.is_empty()
            || request.scope_digest.is_empty()
            || request.reviewed_at <= 0
            || request.evidence_refs.len() > MAX_EVIDENCE_REFS
            || request.evidence_refs.iter().any(|value| {
                value.is_empty() || value.len() > 512 || value.contains(['\0', '\r', '\n'])
            })
        {
            return Err(
                if request.verifier_attempt_id == request.implementation_attempt_id {
                    ReviewerAttemptError::SameAttempt
                } else {
                    ReviewerAttemptError::InvalidRequest
                },
            );
        }
        let artifact = self
            .store
            .load(&request.artifact_id)
            .map_err(ReviewerAttemptError::Store)?;
        if artifact.scope_digest != request.scope_digest
            || artifact.implementation_attempt_id != request.implementation_attempt_id
        {
            return Err(ReviewerAttemptError::ScopeMismatch);
        }
        self.store
            .record_review(
                &request.artifact_id,
                &ReviewEvidence {
                    verifier_attempt_id: request.verifier_attempt_id,
                    verdict: request.verdict,
                    reviewed_commit_sha: request.reviewed_commit_sha,
                    reviewed_diff_hash: request.reviewed_diff_hash,
                    evidence_refs: request.evidence_refs,
                    reviewed_at: request.reviewed_at,
                },
                request.now,
            )
            .map_err(ReviewerAttemptError::Store)
    }
}
