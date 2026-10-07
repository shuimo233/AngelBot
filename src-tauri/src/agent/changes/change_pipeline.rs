//! Main-Agent-owned Change hand-off after a producer delivery has passed its
//! semantic Reviewer.
//!
//! This module is deliberately outside the scheduler and worker hosts.  It
//! resolves an opaque worktree binding, captures an immutable artifact, creates
//! a candidate/change-set, records the already-distinct semantic Reviewer as
//! the independent review fact, and exposes a separate confirmed materialize
//! operation.  No child receives a canonical workspace path or confirmation
//! authority.

use std::sync::Arc;

use super::{
    change_handoff_store::{ChangeHandoffStatus, ChangeHandoffStore},
    delegation_contract::{DelegationDelivery, DeliveryStatus},
    delivery_inbox::{DeliveryInbox, DeliveryScope},
    materializer::{
        CanonicalWorkspaceLocator, ManifestCanonicalWorkspaceLocator, MaterializationError,
        MaterializationReceipt, Materializer,
    },
    resource_binding::{ResourceBindingRepository, ResourceKind},
    review_artifact_store::{ReviewArtifactStore, ReviewArtifactStoreError},
    review_contract::SemanticReviewVerdict,
    review_coordinator::ReviewCoordinator,
    reviewer_attempt::{ReviewerAttemptError, ReviewerAttemptRequest, ReviewerAttemptService},
    shared_db::SharedDb,
    work_package::{
        CandidateArtifact, ChangeSet, Confirmation, WorkPackageError, WorkPackageRepository,
        WorkPackageScope, WorkPackageStatus,
    },
    workspace_isolation::{
        GitWorktreeProvider, ProcessGitCommandRunner, ReviewArtifact, ReviewCaptureRequest,
        ReviewVerdict, WorktreeCapability, WorktreeFileProvider, WorktreeManifestIdentity,
        WorktreeManifestProvider, WorktreeOperation,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedChange {
    pub delivery_id: String,
    pub attempt_id: String,
    pub work_package_id: String,
    pub scope: WorkPackageScope,
    pub change_set_id: String,
    pub artifact_id: String,
    pub base_revision: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ChangePipelineError {
    #[error("delegated Change delivery is unavailable")]
    DeliveryUnavailable,
    #[error("delegated Change review has not passed")]
    ReviewNotPassed,
    #[error("delegated Change worktree is unavailable")]
    WorktreeUnavailable,
    #[error("delegated Change cannot be materialized")]
    MaterializationUnavailable,
    #[error("delegated Change storage failed")]
    Storage,
    #[error("delegated Change workflow rejected the request")]
    Workflow,
}

/// Private workflow implementation for `ChangePipeline`.
///
/// Its operations are intentionally not a separate Module: the pipeline is
/// the only caller and owns the external seam for reviewed delegated changes.
/// Keeping the persistence choreography here preserves locality without
/// exposing another interface for callers to coordinate.
struct ChangeWorkflow {
    db: SharedDb,
    artifacts: ReviewArtifactStore,
    reviewer: ReviewerAttemptService,
}

#[derive(Debug, thiserror::Error)]
enum ChangeWorkflowError {
    #[error("delivery is not a semantically reviewed completed change delivery")]
    DeliveryNotReviewed,
    #[error("delivery does not belong to the requested package")]
    DeliveryScopeMismatch,
    #[error("delivery contains no candidate artifacts")]
    NoCandidates,
    #[error("work-package operation failed: {0}")]
    WorkPackage(WorkPackageError),
    #[error("review artifact operation failed: {0}")]
    ReviewStore(ReviewArtifactStoreError),
    #[error("reviewer attempt failed: {0}")]
    Reviewer(ReviewerAttemptError),
    #[error("canonical materialization failed: {0}")]
    Materialization(#[from] MaterializationError),
    #[error("database error")]
    Storage,
}

impl ChangeWorkflow {
    fn new(db: SharedDb) -> Self {
        let artifacts = ReviewArtifactStore::new(db.clone());
        Self {
            reviewer: ReviewerAttemptService::new(artifacts.clone()),
            db,
            artifacts,
        }
    }

    fn reviewed_delivery_to_change_set(
        &self,
        scope: &WorkPackageScope,
        package_id: &str,
        delivery: &DelegationDelivery,
        expected_candidate_version: i64,
        base_revision: &str,
    ) -> Result<ChangeSet, ChangeWorkflowError> {
        let trusted_delivery = self
            .db
            .with_conn(|conn| {
                let scope = DeliveryScope {
                    session_id: delivery.identity.session_id.clone(),
                    message_id: delivery.identity.message_id.clone(),
                    parent_run_id: delivery.identity.parent_run_id.clone(),
                };
                DeliveryInbox::pending_from_connection(
                    conn,
                    &scope,
                    &super::delegation_contract::ContractLimits::default(),
                )
                .map_err(|_| ())?
                .into_iter()
                .find(|candidate| candidate.delivery.delivery_id == delivery.delivery_id)
                .map(|candidate| candidate.delivery)
                .ok_or(())
            })
            .map_err(|_| ChangeWorkflowError::Storage)?
            .map_err(|_| ChangeWorkflowError::DeliveryNotReviewed)?;
        if trusted_delivery.delivery_id != delivery.delivery_id
            || trusted_delivery.delivery_revision != delivery.delivery_revision
            || trusted_delivery.identity != delivery.identity
        {
            return Err(ChangeWorkflowError::DeliveryScopeMismatch);
        }
        if trusted_delivery.status != DeliveryStatus::Completed
            || trusted_delivery.candidate_artifacts.is_empty()
        {
            return Err(ChangeWorkflowError::NoCandidates);
        }
        let reviewer_passed = self
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT status FROM delegation_review_jobs
                     WHERE delivery_id=?1 AND delivery_revision=?2",
                    [
                        &trusted_delivery.delivery_id,
                        &trusted_delivery.delivery_revision.to_string(),
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map(|status| status == "passed")
                .map_err(|_| ())
            })
            .map_err(|_| ChangeWorkflowError::Storage)?
            .map_err(|_| ChangeWorkflowError::DeliveryNotReviewed)?;
        if !reviewer_passed {
            return Err(ChangeWorkflowError::DeliveryNotReviewed);
        }
        let accepted_package = self
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT d.work_package_id
                     FROM delegations d
                     WHERE d.id=?1",
                    [&trusted_delivery.identity.delegation_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|_| ())
            })
            .map_err(|_| ChangeWorkflowError::Storage)?
            .map_err(|_| ChangeWorkflowError::DeliveryNotReviewed)?;
        if accepted_package != package_id {
            return Err(ChangeWorkflowError::DeliveryScopeMismatch);
        }
        let artifacts = trusted_delivery
            .candidate_artifacts
            .iter()
            .map(|artifact| CandidateArtifact {
                relative_ref: artifact.relative_ref.clone(),
                evidence_refs: artifact
                    .evidence_ref
                    .as_ref()
                    .map(|reference| vec![reference.clone()])
                    .unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        let evidence = trusted_delivery.evidence_refs.clone();
        self.db
            .with_conn_mut(|conn| {
                let tx = conn
                    .transaction()
                    .map_err(|_| ChangeWorkflowError::Storage)?;
                let candidate = WorkPackageRepository::replace_candidates_in_tx(
                    &tx,
                    scope,
                    package_id,
                    expected_candidate_version,
                    artifacts,
                )
                .map_err(ChangeWorkflowError::WorkPackage)?;
                let change = WorkPackageRepository::create_change_set_in_tx(
                    &tx,
                    scope,
                    package_id,
                    &candidate.id,
                    base_revision,
                    evidence,
                )
                .map_err(ChangeWorkflowError::WorkPackage)?;
                tx.commit().map_err(|_| ChangeWorkflowError::Storage)?;
                Ok(change)
            })
            .map_err(|_| ChangeWorkflowError::Storage)?
    }

    fn capture_artifact(
        &self,
        artifact: &ReviewArtifact,
    ) -> Result<ReviewArtifact, ChangeWorkflowError> {
        self.artifacts
            .capture(artifact)
            .map_err(ChangeWorkflowError::ReviewStore)
    }

    fn bind_artifact_change_set(
        &self,
        artifact_id: &str,
        change_set: &ChangeSet,
        now: i64,
    ) -> Result<ReviewArtifact, ChangeWorkflowError> {
        self.artifacts
            .bind_change_set(artifact_id, change_set, now)
            .map_err(ChangeWorkflowError::ReviewStore)
    }

    fn record_independent_review(
        &self,
        request: ReviewerAttemptRequest,
    ) -> Result<ReviewArtifact, ChangeWorkflowError> {
        self.reviewer
            .complete(request)
            .map_err(ChangeWorkflowError::Reviewer)
    }

    fn confirm_change(
        &self,
        scope: &WorkPackageScope,
        change_set_id: &str,
        approve: bool,
        ttl_secs: i64,
        now: i64,
    ) -> Result<Confirmation, ChangeWorkflowError> {
        self.db
            .with_conn_mut(|conn| {
                WorkPackageRepository::confirm(conn, scope, change_set_id, approve, ttl_secs, now)
                    .map_err(ChangeWorkflowError::WorkPackage)
            })
            .map_err(|_| ChangeWorkflowError::Storage)?
    }

    fn materialize<R, L>(
        &self,
        materializer: &Materializer<R, L>,
        scope: WorkPackageScope,
        confirmation_id: String,
        artifact_id: &str,
        observed_base_revision: String,
        now: i64,
    ) -> Result<MaterializationReceipt, ChangeWorkflowError>
    where
        R: super::workspace_isolation::GitCommandRunner + 'static,
        L: CanonicalWorkspaceLocator + 'static,
    {
        materializer
            .materialize_stored(
                &self.artifacts,
                scope,
                confirmation_id,
                artifact_id,
                observed_base_revision,
                now,
            )
            .map_err(ChangeWorkflowError::Materialization)
    }
}

/// One deep Main-Agent composition root for a reviewed Change delivery.  The
/// provider owns all VCS paths and validates its manifest at every boundary.
pub struct ChangePipeline {
    db: SharedDb,
    provider: Arc<GitWorktreeProvider>,
    workflow: ChangeWorkflow,
    reviews: ReviewCoordinator,
    handoffs: ChangeHandoffStore,
}

impl ChangePipeline {
    pub fn new(db: SharedDb, provider: Arc<GitWorktreeProvider>) -> Self {
        Self {
            workflow: ChangeWorkflow::new(db.clone()),
            reviews: ReviewCoordinator::new(db.clone()),
            handoffs: ChangeHandoffStore::new(db.clone()),
            db,
            provider,
        }
    }

    /// Recover the trusted parent scope from durable delivery ownership. This
    /// is used by the pump after a semantic reviewer passes; no scheduler or
    /// worker supplies session/message/run identifiers here.
    pub fn prepare_by_delivery(
        &self,
        delivery_id: &str,
        now: i64,
    ) -> Result<PreparedChange, ChangePipelineError> {
        let scope = self
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT g.session_id,g.message_id,g.parent_run_id
                     FROM delegation_deliveries d
                     JOIN delegations g ON g.id=d.delegation_id
                     WHERE d.id=?1 AND d.acceptance_status='submitted'
                       AND g.status='awaiting_summary'",
                    [delivery_id],
                    |row| {
                        Ok(DeliveryScope {
                            session_id: row.get(0)?,
                            message_id: row.get(1)?,
                            parent_run_id: row.get(2)?,
                        })
                    },
                )
                .map_err(|_| ())
            })
            .map_err(|_| ChangePipelineError::Storage)?
            .map_err(|_| ChangePipelineError::DeliveryUnavailable)?;
        self.prepare(&scope, delivery_id, now)
    }

    /// A preparation failure is made durable rather than retried against an
    /// already-mutated temporary worktree. The parent Agent receives this as a
    /// structured decision state and may explicitly request a fresh attempt.
    pub fn mark_needs_decision(&self, delivery_id: &str, now: i64) {
        let _ = self.handoffs.mark_needs_decision(delivery_id, now);
    }

    /// Record an explicit parent decline without touching the canonical
    /// workspace. The caller then performs the normal terminal cleanup.
    pub fn decline(
        &self,
        prepared: &PreparedChange,
        ttl_secs: i64,
        now: i64,
    ) -> Result<(), ChangePipelineError> {
        self.workflow
            .confirm_change(
                &prepared.scope,
                &prepared.change_set_id,
                false,
                ttl_secs,
                now,
            )
            .map_err(|_| ChangePipelineError::Workflow)?;
        self.handoffs
            .mark_cancelled(&prepared.delivery_id, now)
            .map_err(|_| ChangePipelineError::Storage)
    }

    /// Prepare a candidate after the independent semantic Reviewer passed.
    /// This does not write the canonical workspace and does not complete the
    /// delegation, so the temporary worktree remains available until the
    /// parent confirmation/materialization terminal path runs.
    pub fn prepare(
        &self,
        scope: &DeliveryScope,
        delivery_id: &str,
        now: i64,
    ) -> Result<PreparedChange, ChangePipelineError> {
        let delivery = self
            .db
            .with_conn(|conn| {
                DeliveryInbox::pending_from_connection(
                    conn,
                    scope,
                    &super::delegation_contract::ContractLimits::default(),
                )
                .map_err(|_| ())?
                .into_iter()
                .find(|row| row.delivery.delivery_id == delivery_id)
                .map(|row| row.delivery)
                .ok_or(())
            })
            .map_err(|_| ChangePipelineError::Storage)?
            .map_err(|_| ChangePipelineError::DeliveryUnavailable)?;

        let review = self
            .reviews
            .load_for_delivery(&delivery.delivery_id, i64::from(delivery.delivery_revision))
            .map_err(|_| ChangePipelineError::ReviewNotPassed)?;
        if review.status != super::review_store::ReviewJobStatus::Passed
            || review
                .outcome
                .as_ref()
                .is_none_or(|outcome| outcome.verdict != SemanticReviewVerdict::Passed)
        {
            return Err(ChangePipelineError::ReviewNotPassed);
        }
        let reviewer_attempt_id = review
            .reviewer_attempt_id
            .ok_or(ChangePipelineError::ReviewNotPassed)?;

        let package = self.load_change_package(scope, &delivery.identity.delegation_id)?;
        let handoff = self
            .handoffs
            .begin(
                &delivery.delivery_id,
                i64::from(delivery.delivery_revision),
                &delivery.identity.attempt_id,
                &package.id,
                &review.id,
                now,
            )
            .map_err(|_| ChangePipelineError::Storage)?;
        match handoff.status {
            ChangeHandoffStatus::Ready | ChangeHandoffStatus::ConfirmationPending => {
                return Ok(PreparedChange {
                    delivery_id: handoff.delivery_id,
                    attempt_id: handoff.implementation_attempt_id,
                    work_package_id: handoff.work_package_id,
                    scope: package.scope,
                    change_set_id: handoff.change_set_id.ok_or(ChangePipelineError::Storage)?,
                    artifact_id: handoff.artifact_id.ok_or(ChangePipelineError::Storage)?,
                    base_revision: handoff.base_revision.ok_or(ChangePipelineError::Storage)?,
                });
            }
            ChangeHandoffStatus::Preparing
            | ChangeHandoffStatus::NeedsDecision
            | ChangeHandoffStatus::Cancelled
            | ChangeHandoffStatus::Materialized => {}
        }
        if handoff.status != ChangeHandoffStatus::Preparing {
            return Err(ChangePipelineError::Workflow);
        }
        let worktree = self.worktree_identity(&delivery.identity.attempt_id)?;
        let verified = self
            .provider
            .resolve_manifest(&worktree)
            .map_err(|_| ChangePipelineError::WorktreeUnavailable)?
            .ok_or(ChangePipelineError::WorktreeUnavailable)?;
        let capability = WorktreeCapability {
            read_roots: vec![".".into()],
            write_roots: vec![".".into()],
            operation: WorktreeOperation::Write,
            lease_epoch: worktree.lease_epoch,
            collaboration_key: package.scope.workspace_key.clone(),
        };
        let access = self
            .provider
            .open_file_access(&worktree, capability)
            .map_err(|_| ChangePipelineError::WorktreeUnavailable)?;
        let artifact = self
            .provider
            .capture_verified(
                &verified,
                &access,
                &ReviewCaptureRequest {
                    change_set_id: None,
                    allowed_relative_paths: delivery
                        .candidate_artifacts
                        .iter()
                        .map(|artifact| artifact.relative_ref.clone())
                        .collect(),
                    scope_digest: package.scope_digest.clone(),
                    commit_message: format!("AngelBot delegated change {}", delivery.delivery_id),
                    created_at: now,
                },
            )
            .map_err(|_| ChangePipelineError::WorktreeUnavailable)?;
        let artifact = self
            .workflow
            .capture_artifact(&artifact)
            .map_err(|_| ChangePipelineError::Workflow)?;
        let change = self
            .workflow
            .reviewed_delivery_to_change_set(
                &package.scope,
                &package.id,
                &delivery,
                package.candidate_version,
                &artifact.base_revision,
            )
            .map_err(map_workflow_error)?;
        let artifact = self
            .workflow
            .bind_artifact_change_set(&artifact.id, &change, now)
            .map_err(|_| ChangePipelineError::Workflow)?;
        let evidence_refs = review
            .outcome
            .as_ref()
            .map(|outcome| outcome.evidence_refs.clone())
            .unwrap_or_default();
        self.workflow
            .record_independent_review(ReviewerAttemptRequest {
                artifact_id: artifact.id.clone(),
                implementation_attempt_id: delivery.identity.attempt_id.clone(),
                verifier_attempt_id: reviewer_attempt_id,
                scope_digest: package.scope_digest.clone(),
                read_only: true,
                reviewed_commit_sha: artifact.commit_sha.clone(),
                reviewed_diff_hash: artifact.diff_hash.clone(),
                verdict: ReviewVerdict::Passed,
                evidence_refs,
                reviewed_at: now,
                now,
            })
            .map_err(|_| ChangePipelineError::Workflow)?;
        self.handoffs
            .mark_ready(
                &delivery.delivery_id,
                &artifact.id,
                &change.id,
                &artifact.base_revision,
                now,
            )
            .map_err(|_| ChangePipelineError::Storage)?;
        Ok(PreparedChange {
            delivery_id: delivery.delivery_id,
            attempt_id: delivery.identity.attempt_id,
            work_package_id: package.id,
            scope: package.scope,
            change_set_id: change.id,
            artifact_id: artifact.id,
            base_revision: artifact.base_revision,
        })
    }

    fn load_change_package(
        &self,
        scope: &DeliveryScope,
        delegation_id: &str,
    ) -> Result<super::work_package::WorkPackage, ChangePipelineError> {
        self.db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT p.id,p.owner_profile_id,p.workspace_key,p.task_shape,p.worker_profile,
                            p.worker_policy_version,p.scope_digest,p.capability_scope_ref,
                            p.capability_expires_at,p.candidate_version,p.status
                     FROM delegations d JOIN work_packages p ON p.id=d.work_package_id
                     WHERE d.id=?1 AND d.session_id=?2 AND d.message_id=?3 AND d.parent_run_id=?4",
                    [
                        delegation_id,
                        &scope.session_id,
                        &scope.message_id,
                        &scope.parent_run_id,
                    ],
                    |row| {
                        let shape = row.get::<_, String>(3)?;
                        let profile = row.get::<_, String>(4)?;
                        let status = row.get::<_, String>(10)?;
                        Ok(super::work_package::WorkPackage {
                            id: row.get(0)?,
                            scope: WorkPackageScope {
                                session_id: scope.session_id.clone(),
                                owner_profile_id: row.get(1)?,
                                workspace_key: row.get(2)?,
                            },
                            task_shape: super::worker_policy::TaskShape::from_str(&shape)
                                .ok_or(rusqlite::Error::InvalidQuery)?,
                            worker_profile: super::worker_policy::WorkerProfile::from_str(&profile)
                                .ok_or(rusqlite::Error::InvalidQuery)?,
                            worker_policy_version: row.get(5)?,
                            scope_digest: row.get(6)?,
                            capability_scope_ref: row.get(7)?,
                            capability_expires_at: row.get(8)?,
                            candidate_version: row.get(9)?,
                            status: WorkPackageStatus::from_str(&status)
                                .ok_or(rusqlite::Error::InvalidQuery)?,
                        })
                    },
                )
                .map_err(|_| ())
            })
            .map_err(|_| ChangePipelineError::Storage)?
            .map_err(|_| ChangePipelineError::DeliveryUnavailable)
    }

    fn worktree_identity(
        &self,
        attempt_id: &str,
    ) -> Result<WorktreeManifestIdentity, ChangePipelineError> {
        let record = ResourceBindingRepository::new(self.db.clone())
            .load_for_attempt(attempt_id, ResourceKind::Worktree)
            .map_err(|_| ChangePipelineError::WorktreeUnavailable)?;
        WorktreeManifestIdentity::from_binding(&record)
            .map_err(|_| ChangePipelineError::WorktreeUnavailable)
    }

    /// The only canonical write entry point. A parent-confirmed call creates
    /// the durable confirmation then materializes the same verified worktree
    /// artifact; callers must finalize/cleanup the attempt only after success
    /// or a durable terminal refusal.
    pub fn confirm_and_materialize(
        &self,
        prepared: &PreparedChange,
        approve: bool,
        ttl_secs: i64,
        now: i64,
    ) -> Result<MaterializationReceipt, ChangePipelineError> {
        let confirmation = self
            .workflow
            .confirm_change(
                &prepared.scope,
                &prepared.change_set_id,
                approve,
                ttl_secs,
                now,
            )
            .map_err(|_| ChangePipelineError::Workflow)?;
        if !approve {
            return Err(ChangePipelineError::MaterializationUnavailable);
        }
        self.handoffs
            .mark_confirmation_pending(&prepared.delivery_id, &confirmation.id, now)
            .map_err(|_| ChangePipelineError::Storage)?;
        let identity = self.worktree_identity(&prepared.attempt_id)?;
        let locator = Arc::new(ManifestCanonicalWorkspaceLocator::new(
            self.provider.clone(),
            prepared.scope.clone(),
            identity,
        ));
        let materializer =
            Materializer::new(self.db.clone(), Arc::new(ProcessGitCommandRunner), locator);
        let receipt = self
            .workflow
            .materialize(
                &materializer,
                prepared.scope.clone(),
                confirmation.id,
                &prepared.artifact_id,
                prepared.base_revision.clone(),
                now,
            )
            .map_err(|_| ChangePipelineError::MaterializationUnavailable)?;
        self.handoffs
            .mark_materialized(&prepared.delivery_id, &receipt.id, now)
            .map_err(|_| ChangePipelineError::Storage)?;
        Ok(receipt)
    }
}

fn map_workflow_error(error: ChangeWorkflowError) -> ChangePipelineError {
    match error {
        ChangeWorkflowError::DeliveryNotReviewed => ChangePipelineError::ReviewNotPassed,
        ChangeWorkflowError::Storage => ChangePipelineError::Storage,
        _ => ChangePipelineError::Workflow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        delegation::{DelegationRepository, NewAttempt, NewCapabilityLease, NewDelegation},
        delegation_contract::{
            CandidateArtifact, Confidence, DelegationBrief, DelegationDelivery, DelegationIdentity,
            DeliveryStatus, KeyFact, UsageFlags, VerificationRecord, VerificationStatus,
        },
        materializer::MaterializationStatus,
        resource_binding::{NewResourceBinding, ResourceBindingIdentity, ResourceKind},
        review_contract::{ReviewOutcome, ReviewSubject, SemanticReviewVerdict},
        workspace_isolation::{WorkspaceProvider, WorkspaceRequest},
    };
    use rusqlite::Connection;
    use std::{fs, path::Path, process::Command};
    use tempfile::tempdir;

    const NOW: i64 = 10;

    fn git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .status()
            .expect("git must be available for the deterministic worktree test");
        assert!(status.success(), "git command failed: {args:?}");
    }

    fn db() -> SharedDb {
        let connection = Connection::open_in_memory().unwrap();
        crate::db::migrate(&connection).unwrap();
        SharedDb::new(connection)
    }

    fn scope() -> WorkPackageScope {
        WorkPackageScope {
            session_id: "session_change".into(),
            owner_profile_id: 1,
            workspace_key: "workspace_change".into(),
        }
    }

    fn delivery() -> DelegationDelivery {
        DelegationDelivery {
            schema_version: 1,
            delivery_id: "delivery_change".into(),
            delivery_revision: 1,
            identity: DelegationIdentity {
                session_id: "session_change".into(),
                message_id: "message_change".into(),
                parent_run_id: "run_change".into(),
                delegation_id: "delegation_change".into(),
                attempt_id: "attempt_change".into(),
            },
            status: DeliveryStatus::Completed,
            executive_summary: "Implemented the requested isolated change.".into(),
            key_facts: vec![KeyFact {
                statement: "The candidate only changes src/feature.txt.".into(),
                confidence: Confidence::High,
                evidence_refs: vec!["evidence://change/feature".into()],
            }],
            milestones: vec![],
            verifications: vec![VerificationRecord {
                item: "candidate content".into(),
                method: "deterministic fixture".into(),
                status: VerificationStatus::Passed,
                conclusion: "Candidate was written in the isolated worktree.".into(),
                evidence_ref: "evidence://change/feature".into(),
            }],
            candidate_artifacts: vec![CandidateArtifact {
                kind: "source".into(),
                relative_ref: "src/feature.txt".into(),
                description: "Updated feature text.".into(),
                evidence_ref: Some("evidence://change/feature".into()),
            }],
            open_questions: vec![],
            risks: vec![],
            evidence_refs: vec!["evidence://change/feature".into()],
            usage: UsageFlags {
                context_truncated: false,
                output_truncated: false,
                budget_exhausted: false,
            },
        }
    }

    fn brief() -> DelegationBrief {
        DelegationBrief {
            schema_version: 1,
            identity: delivery().identity,
            goal: "make a bounded change".into(),
            background: vec![],
            constraints: vec!["Only update src/feature.txt".into()],
            allowed_references: vec!["evidence://change/feature".into()],
            allowed_capabilities: vec![],
            completion_criteria: vec!["Candidate is ready for independent review".into()],
        }
    }

    fn seed_change_delivery(db: &SharedDb) {
        let package_scope = scope();
        db.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO sessions(id,title,created_at,updated_at) VALUES ('session_change','session',1,1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('message_change','session_change','user','change',1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES ('run_change','session_change','message_change','change','running','[]',1,1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO work_packages(id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,candidate_version,created_at,updated_at,status) VALUES ('package_change',?1,?2,?3,'change','implementer',1,'scope_change','approved',1000,0,1,1,'active')",
                rusqlite::params![package_scope.session_id, package_scope.owner_profile_id, package_scope.workspace_key],
            )?;
            DelegationRepository::create_queued(
                conn,
                &NewDelegation {
                    id: "delegation_change".into(),
                    session_id: "session_change".into(),
                    message_id: "message_change".into(),
                    parent_run_id: "run_change".into(),
                    work_package_id: Some("package_change".into()),
                    idempotency_key: Some("change-test".into()),
                    objective: "make a bounded change".into(),
                    brief_json: serde_json::to_string(&brief()).unwrap(),
                },
                &NewAttempt {
                    id: "attempt_change".into(),
                    attempt_number: 1,
                    sandbox_ref: "sandbox://attempt_change".into(),
                    outbox_id: "outbox_change".into(),
                    dispatch_payload_json: "opaque".into(),
                },
                &NewCapabilityLease {
                    id: "lease_change".into(),
                    read_roots_json: "[\".\"]".into(),
                    write_roots_json: "[\".\"]".into(),
                    tool_allowlist_json: "[]".into(),
                    network_hosts_json: "[]".into(),
                    budget_json: "{}".into(),
                    expires_at: 1000,
                },
                NOW,
            )
            .map_err(rusqlite::Error::InvalidParameterName)?;
            conn.execute(
                "UPDATE delegation_attempts SET status='running',started_at=?1 WHERE id='attempt_change'",
                [NOW],
            )?;
            conn.execute(
                "UPDATE delegations SET status='awaiting_summary',state_version=2,updated_at=?1 WHERE id='delegation_change'",
                [NOW],
            )?;
            let value = serde_json::to_string(&delivery()).unwrap();
            DelegationRepository::submit_delivery(
                conn,
                "delivery_change",
                "delegation_change",
                "attempt_change",
                1,
                1,
                &value,
                NOW,
            )
            .map_err(rusqlite::Error::InvalidParameterName)?;
            Ok::<(), rusqlite::Error>(())
        })
        .unwrap()
        .unwrap();
    }

    #[test]
    fn declined_change_records_terminal_handoff_without_materialization_authority() {
        let db = db();
        seed_change_delivery(&db);
        let storage = tempdir().unwrap();
        let provider = Arc::new(
            GitWorktreeProvider::new(storage.path(), storage.path().join("manifests")).unwrap(),
        );
        let pipeline = ChangePipeline::new(db.clone(), provider);
        let package_scope = scope();
        let candidate = db
            .with_conn_mut(|conn| {
                WorkPackageRepository::replace_candidates(
                    conn,
                    &package_scope,
                    "package_change",
                    0,
                    vec![crate::agent::work_package::CandidateArtifact {
                        relative_ref: "src/feature.txt".into(),
                        evidence_refs: vec!["evidence://change/feature".into()],
                    }],
                )
            })
            .unwrap()
            .unwrap();
        let change = db
            .with_conn_mut(|conn| {
                WorkPackageRepository::create_change_set(
                    conn,
                    &package_scope,
                    "package_change",
                    &candidate.id,
                    "base_change",
                    vec!["evidence://change/feature".into()],
                )
            })
            .unwrap()
            .unwrap();
        db.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO delegation_review_jobs (id,implementation_attempt_id,delivery_id,delivery_revision,subject_json,status,outcome_json,created_at,updated_at,ended_at) VALUES ('review_change','attempt_change','delivery_change',1,'{}','passed','{}',?1,?1,?1)",
                [NOW],
            )
        })
        .unwrap()
        .unwrap();
        db.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO delegation_review_artifacts (id,schema_version,work_package_id,change_set_id,implementation_attempt_id,scope_digest,base_revision,base_commit,branch,audit_ref,commit_sha,diff_hash,changed_paths_json,allowed_relative_paths_json,created_at,updated_at) VALUES ('artifact_change',1,'package_change',?1,'attempt_change','scope_change','base_change',?2,'candidate/change','audit://change',?2,'diff_change','[\"src/feature.txt\"]','[\"src/feature.txt\"]',?3,?3)",
                rusqlite::params![change.id, "a".repeat(40), NOW],
            )
        })
        .unwrap()
        .unwrap();
        let handoffs = ChangeHandoffStore::new(db.clone());
        handoffs
            .begin(
                "delivery_change",
                1,
                "attempt_change",
                "package_change",
                "review_change",
                NOW,
            )
            .unwrap();
        handoffs
            .mark_ready(
                "delivery_change",
                "artifact_change",
                &change.id,
                "base_change",
                NOW,
            )
            .unwrap();

        let prepared = PreparedChange {
            delivery_id: "delivery_change".into(),
            attempt_id: "attempt_change".into(),
            work_package_id: "package_change".into(),
            scope: package_scope,
            change_set_id: change.id,
            artifact_id: "artifact_change".into(),
            base_revision: "base_change".into(),
        };
        pipeline.decline(&prepared, 3600, NOW).unwrap();

        let confirmation = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT status FROM work_package_confirmations WHERE change_set_id=?1",
                    [&prepared.change_set_id],
                    |row| row.get::<_, String>(0),
                )
            })
            .unwrap()
            .unwrap();
        assert_eq!(confirmation, "declined");
        let package_status = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT status FROM work_packages WHERE id='package_change'",
                    [],
                    |row| row.get::<_, String>(0),
                )
            })
            .unwrap()
            .unwrap();
        assert_eq!(package_status, "declined");
        assert_eq!(
            handoffs.load("delivery_change").unwrap().unwrap().status,
            ChangeHandoffStatus::Cancelled
        );
    }

    #[test]
    fn reviewed_change_requires_parent_confirmation_before_exact_materialization() {
        let canonical = tempdir().unwrap();
        fs::create_dir_all(canonical.path().join("src")).unwrap();
        fs::write(canonical.path().join("src/feature.txt"), "original\n").unwrap();
        git(canonical.path(), &["init"]);
        git(
            canonical.path(),
            &["config", "user.email", "angelbot-test@example.invalid"],
        );
        git(canonical.path(), &["config", "user.name", "AngelBot Test"]);
        git(canonical.path(), &["add", "."]);
        git(canonical.path(), &["commit", "-m", "base"]);

        let storage = tempdir().unwrap();
        let provider = Arc::new(
            GitWorktreeProvider::new(storage.path(), storage.path().join("manifests")).unwrap(),
        );
        let request = WorkspaceRequest {
            scope: scope(),
            work_package_id: "package_change".into(),
            attempt_id: "attempt_change".into(),
            canonical_workspace: canonical.path().into(),
            base_revision: "HEAD".into(),
            lease_epoch: 1,
        };
        let handle = provider.prepare(&request).unwrap();
        let identity = WorktreeManifestIdentity::for_handle(&handle, "scope_change").unwrap();
        provider.persist_manifest(&handle, &identity).unwrap();
        let access = provider
            .open_file_access(
                &identity,
                WorktreeCapability {
                    read_roots: vec![".".into()],
                    write_roots: vec![".".into()],
                    operation: WorktreeOperation::Write,
                    lease_epoch: 1,
                    collaboration_key: "workspace_change".into(),
                },
            )
            .unwrap();
        provider
            .write_candidate(&access, "src/feature.txt", b"updated\n", 1024)
            .unwrap();

        let db = db();
        seed_change_delivery(&db);
        ResourceBindingRepository::new(db.clone())
            .create(&NewResourceBinding {
                identity: ResourceBindingIdentity {
                    id: "binding_change".into(),
                    delegation_id: "delegation_change".into(),
                    work_package_id: "package_change".into(),
                    attempt_id: "attempt_change".into(),
                    lease_id: "lease_change".into(),
                    resource_kind: ResourceKind::Worktree,
                    provider_kind: identity.provider_kind.clone(),
                    resource_ref: identity.handle_id.clone(),
                    manifest_locator: identity.manifest_locator.clone(),
                    manifest_version: 1,
                    manifest_nonce: identity.manifest_nonce.clone(),
                    manifest_digest: identity.manifest_digest.clone(),
                    scope_digest: identity.scope_digest.clone(),
                    lease_epoch: 1,
                    admission: None,
                },
                created_at: NOW,
            })
            .unwrap();

        let pipeline = ChangePipeline::new(db.clone(), provider);
        let parent_scope = DeliveryScope {
            session_id: "session_change".into(),
            message_id: "message_change".into(),
            parent_run_id: "run_change".into(),
        };
        db.with_conn(|conn| {
            let state: (String, String, i64, String) = conn.query_row(
                "SELECT g.status,d.acceptance_status,d.delivery_version,d.payload_json FROM delegation_deliveries d JOIN delegations g ON g.id=d.delegation_id WHERE d.id='delivery_change'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            assert_eq!(state.0, "awaiting_summary");
            assert_eq!(state.1, "submitted");
            assert_eq!(state.2, 1);
            assert!(state.3.contains("schema_version"), "payload={}", state.3);
            Ok::<(), rusqlite::Error>(())
        })
        .unwrap()
        .unwrap();
        let pending = db.with_conn(|conn| {
            DeliveryInbox::pending_from_connection(
                conn,
                &parent_scope,
                &super::super::delegation_contract::ContractLimits::default(),
            )
        });
        let pending = pending.unwrap().unwrap();
        assert_eq!(pending.len(), 1);
        let before_review = pipeline.prepare(&parent_scope, "delivery_change", NOW);
        assert!(
            matches!(before_review, Err(ChangePipelineError::ReviewNotPassed)),
            "unexpected pre-review pipeline result: {before_review:?}"
        );
        assert_eq!(
            fs::read_to_string(canonical.path().join("src/feature.txt")).unwrap(),
            "original\n"
        );

        let subject = ReviewSubject::from_delivery(
            &delivery(),
            "make a bounded change",
            "change",
            "implementer",
        );
        db.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO delegation_attempts(id,delegation_id,attempt_number,status,sandbox_ref,created_at,started_at,ended_at) VALUES ('reviewer_change','delegation_change',2,'sealed','sandbox://reviewer_change',?1,?1,?1)",
                [NOW],
            )?;
            Ok::<(), rusqlite::Error>(())
        })
        .unwrap()
        .unwrap();
        let reviews = ReviewCoordinator::new(db.clone());
        reviews
            .enqueue(
                "review_change",
                "attempt_change",
                "delivery_change",
                1,
                &subject,
                NOW,
            )
            .unwrap();
        reviews
            .claim("review_change", "attempt_change", "reviewer_change", NOW)
            .unwrap();
        reviews
            .record_outcome(
                "review_change",
                "reviewer_change",
                &ReviewOutcome {
                    verdict: SemanticReviewVerdict::Passed,
                    summary: "Independent reviewer accepted the bounded change.".into(),
                    findings: vec![],
                    missing_evidence: vec![],
                    evidence_refs: vec!["evidence://review/change".into()],
                },
                NOW,
            )
            .unwrap();

        let prepared = pipeline
            .prepare(&parent_scope, "delivery_change", NOW)
            .expect("a passed independent review must prepare a change handoff");
        assert_eq!(
            fs::read_to_string(canonical.path().join("src/feature.txt")).unwrap(),
            "original\n",
            "preparing a reviewed candidate must not write the canonical workspace"
        );
        let receipt = pipeline
            .confirm_and_materialize(&prepared, true, 3600, NOW)
            .expect("only a parent-confirmed reviewed artifact may materialize");
        assert_eq!(receipt.status, MaterializationStatus::Applied);
        assert_eq!(
            fs::read_to_string(canonical.path().join("src/feature.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "updated\n"
        );
    }
}
