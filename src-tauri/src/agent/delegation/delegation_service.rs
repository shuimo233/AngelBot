//! Composition root for delegated work.
//!
//! This module deliberately contains orchestration policy, not another
//! delegation state machine.  `DelegatedAttemptScheduler` remains the sole
//! owner of outbox CAS, admission capacity, lease recovery, and worker
//! dispatch.  The service only gates its lifecycle and coordinates terminal
//! cleanup after a Main-Agent decision.

use rusqlite::OptionalExtension;

use super::{
    delegated_attempt_scheduler::{
        DelegatedAttemptScheduler, DelegatedSchedulerError, SchedulerReport,
    },
    delegated_delivery_follow_up::DelegatedDeliveryFollowUpQueue,
    delegation_contract::{ContractLimits, DelegationDelivery},
    delegation_runtime::{
        AdmissionPolicy, Clock, DelegationRuntime, DelegationRuntimeError, FixedAdmissionPolicy,
        NoopSandboxController, SandboxController, SystemClock, WorkerAdapter, WorkerDispatch,
    },
    delivery_inbox::{
        DecisionOutcome, DeliveryInbox, DeliveryScope, InboxError, ParentDecision,
        RetryContinuation,
    },
    review_agent::ReviewExecutor,
    review_contract::ReviewSubject,
    review_coordinator::ReviewCoordinator,
    review_store::{ReviewJob, ReviewJobStatus, ReviewStoreError},
    shared_db::SharedDb,
    terminal_cleanup::{FailClosedTerminalCleanup, TerminalCleanupPort},
    workspace_admission::{WorkspaceAdmission, WorkspaceAdmissionError},
};

/// Production currently has no synchronous delegated worker launcher.  The
/// fail-closed adapter keeps startup deterministic: it never claims work and
/// refuses an accidental launch until a controlled async bridge is injected.
#[derive(Default)]
pub struct FailClosedWorker;
impl WorkerAdapter for FailClosedWorker {
    fn start(&mut self, _: WorkerDispatch) -> Result<(), String> {
        Err("delegated worker launcher is not configured".into())
    }
    fn stop(&mut self, _: &str, _: super::delegation_runtime::StopReason) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceState {
    Created,
    Running,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegationServiceError {
    DatabaseMismatch,
    NotStarted,
    Stopped,
    Scheduler,
    Runtime,
    Admission,
    Inbox,
    /// Change deliveries cannot use the generic parent-accept path.  The
    /// review/materialization coordinator must win the change-specific CAS
    /// before the attempt is finalized or its ephemeral worktree is cleaned.
    ChangeReviewNotReady,
    /// The producer delivery is visible as structured pending context, but
    /// the parent decision gate remains closed until its independent Reviewer
    /// has recorded a passed outcome.
    ReviewPending,
    ReviewRejected,
    Review,
    CleanupPending(String),
}

impl std::fmt::Display for DelegationServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DatabaseMismatch => {
                f.write_str("delegation service components use different databases")
            }
            Self::NotStarted => f.write_str("delegation service has not started"),
            Self::Stopped => f.write_str("delegation service is shut down"),
            Self::Scheduler => f.write_str("delegation scheduler failed"),
            Self::Runtime => f.write_str("delegation runtime failed"),
            Self::Admission => f.write_str("workspace admission recovery failed"),
            Self::Inbox => f.write_str("delivery inbox operation failed"),
            Self::ChangeReviewNotReady => {
                f.write_str("change delivery requires the review/materialization coordinator")
            }
            Self::ReviewPending => f.write_str("delivery review is not complete"),
            Self::ReviewRejected => f.write_str("delivery review did not pass"),
            Self::Review => f.write_str("reviewer execution failed"),
            Self::CleanupPending(attempt) => {
                write!(f, "terminal cleanup is pending for attempt {attempt}")
            }
        }
    }
}
impl std::error::Error for DelegationServiceError {}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ServiceReport {
    pub scheduler: SchedulerReport,
    pub admissions_reconciled: usize,
    /// Count of independently reviewed read-only deliveries that were newly
    /// made eligible for the constrained Main-Agent summary path during this
    /// host tick. The host uses this edge signal to wake its foreground queue
    /// without the delegation service depending on application/UI state.
    pub reviewed_delivery_follow_ups_queued: usize,
}

/// Type-erased lifecycle seam for `AppState`.  Tauri should not expose the
/// scheduler's worker/clock/policy generics; all calls remain serialized by
/// this one handle and delegate to the same service instance.
/// Internal lifecycle seam between the pump and its concrete execution loop.
/// Main-Agent callers use `DelegationServiceHandle` rather than the loop.
pub(crate) trait DelegationServiceLoop: Send {
    fn start(&mut self) -> Result<ServiceReport, DelegationServiceError>;
    fn tick(&mut self) -> Result<ServiceReport, DelegationServiceError>;
    fn shutdown(&mut self) -> Result<(), DelegationServiceError>;
    fn poll_workers(&mut self, limits: &ContractLimits) -> Result<usize, DelegationServiceError>;
    fn active_worker_attempts(&self) -> usize;
    fn materialize_change_for_session(
        &mut self,
        _session_id: &str,
        _delivery_id: &str,
        _ttl_secs: i64,
        _limits: &ContractLimits,
    ) -> Result<DecisionOutcome, DelegationServiceError> {
        Err(DelegationServiceError::ChangeReviewNotReady)
    }
    fn decline_change_for_session(
        &mut self,
        _session_id: &str,
        _delivery_id: &str,
        _ttl_secs: i64,
        _limits: &ContractLimits,
    ) -> Result<(), DelegationServiceError> {
        Err(DelegationServiceError::ChangeReviewNotReady)
    }
    fn heartbeat(
        &mut self,
        heartbeat: super::delegation_runtime::Heartbeat,
    ) -> Result<(), DelegationServiceError>;
    fn submit_delivery(
        &mut self,
        delivery: DelegationDelivery,
        limits: &ContractLimits,
    ) -> Result<(), DelegationServiceError>;
    fn decide_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        decision: ParentDecision,
        reason: Option<&str>,
        limits: &ContractLimits,
    ) -> Result<DecisionOutcome, DelegationServiceError> {
        let _ = (session_id, delivery_id, decision, reason, limits);
        Err(DelegationServiceError::Inbox)
    }
    fn retry_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        reason: &str,
        limits: &ContractLimits,
    ) -> Result<RetryContinuation, DelegationServiceError> {
        let _ = (session_id, delivery_id, reason, limits);
        Err(DelegationServiceError::Inbox)
    }
}

pub struct DelegationServiceHandle {
    inner: std::sync::Arc<std::sync::Mutex<Box<dyn DelegationServiceLoop>>>,
}

impl Clone for DelegationServiceHandle {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl DelegationServiceHandle {
    pub(crate) fn new<Svc>(service: Svc) -> Self
    where
        Svc: DelegationServiceLoop + 'static,
    {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(Box::new(service))),
        }
    }

    pub fn start(&self) -> Result<ServiceReport, DelegationServiceError> {
        self.inner
            .lock()
            .map_err(|_| DelegationServiceError::Runtime)?
            .start()
    }

    pub fn tick(&self) -> Result<ServiceReport, DelegationServiceError> {
        self.inner
            .lock()
            .map_err(|_| DelegationServiceError::Runtime)?
            .tick()
    }

    pub fn poll_workers(&self, limits: &ContractLimits) -> Result<usize, DelegationServiceError> {
        self.inner
            .lock()
            .map_err(|_| DelegationServiceError::Runtime)?
            .poll_workers(limits)
    }

    pub fn active_worker_attempts(&self) -> Result<usize, DelegationServiceError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| DelegationServiceError::Runtime)?
            .active_worker_attempts())
    }

    pub fn shutdown(&self) -> Result<(), DelegationServiceError> {
        self.inner
            .lock()
            .map_err(|_| DelegationServiceError::Runtime)?
            .shutdown()
    }

    pub fn heartbeat(
        &self,
        heartbeat: super::delegation_runtime::Heartbeat,
    ) -> Result<(), DelegationServiceError> {
        self.inner
            .lock()
            .map_err(|_| DelegationServiceError::Runtime)?
            .heartbeat(heartbeat)
    }

    pub fn submit_delivery(
        &self,
        delivery: DelegationDelivery,
        limits: &ContractLimits,
    ) -> Result<(), DelegationServiceError> {
        self.inner
            .lock()
            .map_err(|_| DelegationServiceError::Runtime)?
            .submit_delivery(delivery, limits)
    }

    pub fn decide_for_session(
        &self,
        session_id: &str,
        delivery_id: &str,
        decision: ParentDecision,
        reason: Option<&str>,
        limits: &ContractLimits,
    ) -> Result<DecisionOutcome, DelegationServiceError> {
        self.inner
            .lock()
            .map_err(|_| DelegationServiceError::Runtime)?
            .decide_for_session(session_id, delivery_id, decision, reason, limits)
    }

    pub fn retry_for_session(
        &self,
        session_id: &str,
        delivery_id: &str,
        reason: &str,
        limits: &ContractLimits,
    ) -> Result<RetryContinuation, DelegationServiceError> {
        self.inner
            .lock()
            .map_err(|_| DelegationServiceError::Runtime)?
            .retry_for_session(session_id, delivery_id, reason, limits)
    }

    pub fn materialize_change_for_session(
        &self,
        session_id: &str,
        delivery_id: &str,
        ttl_secs: i64,
        limits: &ContractLimits,
    ) -> Result<DecisionOutcome, DelegationServiceError> {
        self.inner
            .lock()
            .map_err(|_| DelegationServiceError::Runtime)?
            .materialize_change_for_session(session_id, delivery_id, ttl_secs, limits)
    }

    pub fn decline_change_for_session(
        &self,
        session_id: &str,
        delivery_id: &str,
        ttl_secs: i64,
        limits: &ContractLimits,
    ) -> Result<(), DelegationServiceError> {
        self.inner
            .lock()
            .map_err(|_| DelegationServiceError::Runtime)?
            .decline_change_for_session(session_id, delivery_id, ttl_secs, limits)
    }

    /// Fail-closed startup adapter used until the async worker bridge is
    /// explicitly wired.  It still runs durable recovery and admission expiry
    /// against the injected SharedDb, but cannot launch queued work.
    pub(crate) fn fail_closed(db: SharedDb) -> Result<Self, DelegationServiceError> {
        let runtime = DelegationRuntime::new_shared(
            db.clone(),
            FailClosedWorker,
            SystemClock,
            FixedAdmissionPolicy {
                max_running_attempts: 0,
            },
            30,
        )
        .map_err(|_| DelegationServiceError::Runtime)?;
        let scheduler = DelegatedAttemptScheduler::with_sandbox(runtime);
        let service = DelegationService::with_cleanup(
            db,
            scheduler,
            WorkspaceAdmission::new(),
            FailClosedTerminalCleanup,
        )?;
        Ok(Self::new(service))
    }
}

/// A single synchronous service instance.  The application owns one value
/// and calls `start` once, then `tick` from its existing scheduler loop.  No
/// background task or second worker loop is created here.
pub struct DelegationService<W, C, P, S = NoopSandboxController, K = FailClosedTerminalCleanup> {
    db: SharedDb,
    scheduler: DelegatedAttemptScheduler<W, C, P, S>,
    admission: WorkspaceAdmission,
    cleanup: K,
    review: Option<ReviewCoordinator>,
    review_executor: Option<Box<dyn ReviewExecutor>>,
    change_pipeline: Option<super::change_pipeline::ChangePipeline>,
    state: ServiceState,
}

impl<W, C, P> DelegationService<W, C, P, NoopSandboxController, FailClosedTerminalCleanup>
where
    W: WorkerAdapter,
    C: Clock,
    P: AdmissionPolicy,
{
    pub fn new(
        db: SharedDb,
        scheduler: DelegatedAttemptScheduler<W, C, P>,
        admission: WorkspaceAdmission,
    ) -> Result<Self, DelegationServiceError> {
        Self::with_cleanup(db, scheduler, admission, FailClosedTerminalCleanup)
    }
}

impl<W, C, P, S, K> DelegationService<W, C, P, S, K>
where
    W: WorkerAdapter,
    C: Clock,
    P: AdmissionPolicy,
    S: SandboxController,
    K: TerminalCleanupPort,
{
    pub fn with_cleanup(
        db: SharedDb,
        scheduler: DelegatedAttemptScheduler<W, C, P, S>,
        admission: WorkspaceAdmission,
        cleanup: K,
    ) -> Result<Self, DelegationServiceError> {
        if !db.same_instance(&scheduler.runtime().shared_db()) {
            return Err(DelegationServiceError::DatabaseMismatch);
        }
        Ok(Self {
            db,
            scheduler,
            admission,
            cleanup,
            review: None,
            review_executor: None,
            change_pipeline: None,
            state: ServiceState::Created,
        })
    }

    /// Add the semantic Reviewer Agent to this same service instance. The
    /// reviewer is polled by DelegationPump; it does not own a second loop or
    /// worker bridge. `ReviewCoordinator` remains persistence/policy-only and
    /// `ReviewExecutor` remains the model execution adapter.
    pub fn with_review(
        mut self,
        coordinator: ReviewCoordinator,
        executor: Box<dyn ReviewExecutor>,
    ) -> Self {
        self.review = Some(coordinator);
        self.review_executor = Some(executor);
        self
    }

    /// Add the Main-Agent-owned Change hand-off. The pipeline is not a worker
    /// and is invoked only after the semantic Reviewer records a passed
    /// outcome; it remains responsible for candidate capture and the later
    /// explicit canonical-write confirmation.
    pub fn with_change_pipeline(
        mut self,
        pipeline: super::change_pipeline::ChangePipeline,
    ) -> Self {
        self.change_pipeline = Some(pipeline);
        self
    }

    pub fn start(&mut self) -> Result<ServiceReport, DelegationServiceError> {
        match self.state {
            ServiceState::Stopped => return Err(DelegationServiceError::Stopped),
            ServiceState::Running => return Ok(ServiceReport::default()),
            ServiceState::Created => {}
        }
        // Delivery persistence and reviewer enqueue use independent durable
        // writes. Repair their crash window before recovery seals workers or
        // releases any change worktree needed for candidate preparation.
        self.recover_missing_review_jobs()?;
        let scheduler = self
            .scheduler
            .start()
            .map_err(|_| DelegationServiceError::Scheduler)?;
        let now = self.scheduler.runtime().now();
        let reconciled = self
            .db
            .with_conn_mut(|conn| self.admission.recover(conn, now))
            .map_err(|_| DelegationServiceError::Admission)?
            .map_err(|_| DelegationServiceError::Admission)?;
        self.release_recovered_delivery_admissions(now)?;
        self.recover_review_jobs(now)?;
        let reviewed_delivery_follow_ups_queued = self.recover_reviewed_delivery_follow_ups(now)?;
        self.recover_terminal_cleanup()?;
        self.state = ServiceState::Running;
        Ok(ServiceReport {
            scheduler,
            admissions_reconciled: reconciled,
            reviewed_delivery_follow_ups_queued,
        })
    }

    pub fn tick(&mut self) -> Result<ServiceReport, DelegationServiceError> {
        self.ensure_running()?;
        let now = self.scheduler.runtime().now();
        let reconciled = self
            .db
            .with_conn_mut(|conn| self.admission.expire(conn, now))
            .map_err(|_| DelegationServiceError::Admission)?
            .map_err(|_| DelegationServiceError::Admission)?;
        let scheduler = self
            .scheduler
            .tick()
            .map_err(|_| DelegationServiceError::Scheduler)?;
        for attempt_id in scheduler.expired.iter().chain(&scheduler.policy_ineligible) {
            match self.finish_attempt(attempt_id) {
                Ok(()) | Err(DelegationServiceError::CleanupPending(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(ServiceReport {
            scheduler,
            admissions_reconciled: reconciled,
            reviewed_delivery_follow_ups_queued: 0,
        })
    }

    /// Drain events from the scheduler's one worker bridge and apply them to
    /// the same runtime/inbox. The bridge is not duplicated by the pump and no
    /// callback re-enters the service mutex while it is held.
    pub fn poll_workers(
        &mut self,
        limits: &ContractLimits,
    ) -> Result<usize, DelegationServiceError> {
        self.ensure_running()?;
        let events = self
            .scheduler
            .runtime_mut()
            .worker_mut()
            .poll_workers()
            .map_err(|_| DelegationServiceError::Scheduler)?;
        let mut first_error = None;
        for event in events {
            // The adapter has drained this batch. Do not discard unrelated
            // events when one event is rejected or one cleanup fails.
            let result = (|| -> Result<(), DelegationServiceError> {
                match event {
                    super::delegation_runtime::WorkerEvent::Heartbeat(heartbeat) => self
                        .scheduler
                        .runtime_mut()
                        .heartbeat(heartbeat)
                        .map_err(|_| DelegationServiceError::Scheduler)?,
                    super::delegation_runtime::WorkerEvent::Delivery { delivery, .. } => {
                        DeliveryInbox::submit(self.scheduler.runtime_mut(), &delivery, limits)
                            .map(|_| ())
                            .map_err(|_| DelegationServiceError::Inbox)?;
                        self.queue_review_for_delivery(&delivery)?;
                    }
                    super::delegation_runtime::WorkerEvent::Terminal { attempt_id, .. } => {
                        self.worker_terminated(&attempt_id)?;
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        let reviewed_delivery_follow_ups_queued = match self.poll_review_events() {
            Ok(count) => count,
            Err(error) => {
                first_error.get_or_insert(error);
                0
            }
        };
        first_error.map_or(Ok(reviewed_delivery_follow_ups_queued), Err)
    }

    fn worker_terminated(&mut self, attempt_id: &str) -> Result<(), DelegationServiceError> {
        // The worker bridge validates the attempt/epoch. Exit is not success:
        // preserve submitted deliveries for review, but stop an abandoned run.
        let abandoned = self
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM delegation_attempts a
             JOIN delegations d ON d.id=a.delegation_id
             WHERE a.id=?1 AND a.status='running' AND d.status='running')",
                    [attempt_id],
                    |row| row.get::<_, bool>(0),
                )
            })
            .map_err(|_| DelegationServiceError::Runtime)?
            .map_err(|_| DelegationServiceError::Runtime)?;
        if abandoned {
            self.scheduler
                .report_failure(attempt_id)
                .map_err(|_| DelegationServiceError::Scheduler)?;
            self.finish_attempt(attempt_id)?;
        }
        Ok(())
    }

    fn queue_review_for_delivery(
        &mut self,
        delivery: &DelegationDelivery,
    ) -> Result<(), DelegationServiceError> {
        let Some(job) = self.enqueue_review_for_delivery(delivery)? else {
            return Ok(());
        };
        self.start_review_job(job)
    }

    fn enqueue_review_for_delivery(
        &self,
        delivery: &DelegationDelivery,
    ) -> Result<Option<ReviewJob>, DelegationServiceError> {
        let Some(coordinator) = self.review.as_ref() else {
            // Fail-closed deployments may still persist a delivery so the
            // foreground can show recovery state, but no accept path can
            // pass without a review coordinator (see review_gate below).
            return Ok(None);
        };
        let attempt_id = delivery.identity.attempt_id.clone();
        let subject_labels = self
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT d.objective,p.task_shape,p.worker_profile
                     FROM delegation_attempts a
                     JOIN delegations d ON d.id=a.delegation_id
                     JOIN work_packages p ON p.id=d.work_package_id
                     WHERE a.id=?1",
                    [&attempt_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .map_err(|_| ReviewStoreError::NotFound)
            })
            .map_err(|_| DelegationServiceError::Review)?
            .map_err(|_| DelegationServiceError::Review)?;
        let (goal, task_shape, worker_profile) = subject_labels;
        let subject = ReviewSubject::from_delivery(delivery, goal, task_shape, worker_profile);
        let job_id = format!(
            "review_{}_{}",
            delivery.delivery_id, delivery.delivery_revision
        );
        let job = coordinator
            .enqueue(
                &job_id,
                &attempt_id,
                &delivery.delivery_id,
                i64::from(delivery.delivery_revision),
                &subject,
                self.scheduler.runtime().now(),
            )
            .map_err(|_| DelegationServiceError::Review)?;
        Ok(Some(job))
    }

    fn recover_missing_review_jobs(&self) -> Result<(), DelegationServiceError> {
        if self.review.is_none() {
            return Ok(());
        }
        let payloads = self
            .db
            .with_conn(|conn| {
                let mut statement = conn.prepare(
                    "SELECT delivery.payload_json
                     FROM delegation_deliveries delivery
                     JOIN delegations delegation ON delegation.id=delivery.delegation_id
                     LEFT JOIN delegation_review_jobs review
                       ON review.delivery_id=delivery.id
                      AND review.delivery_revision=delivery.delivery_version
                     WHERE delivery.acceptance_status='submitted'
                       AND delegation.status='awaiting_summary'
                       AND review.id IS NULL",
                )?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                rows.collect::<Result<Vec<_>, rusqlite::Error>>()
            })
            .map_err(|_| DelegationServiceError::Review)?
            .map_err(|_| DelegationServiceError::Review)?;
        for payload in payloads {
            let delivery = serde_json::from_str::<DelegationDelivery>(&payload)
                .map_err(|_| DelegationServiceError::Review)?;
            self.enqueue_review_for_delivery(&delivery)?;
        }
        Ok(())
    }

    /// Restart-safe recovery remains inside the one service lifecycle. It
    /// resets only abandoned reviewer claims and then reclaims each job with
    /// the normal CAS before any model task is started.
    fn recover_review_jobs(&mut self, now: i64) -> Result<(), DelegationServiceError> {
        let Some(coordinator) = self.review.as_ref() else {
            return Ok(());
        };
        let jobs = coordinator
            .recover_runnable(now)
            .map_err(|_| DelegationServiceError::Review)?;
        for job in jobs {
            self.start_review_job(job)?;
        }
        Ok(())
    }

    /// Repair the small crash window after a Reviewer records `passed` but
    /// before the service has inserted its source-bound foreground follow-up.
    /// The queue is idempotent, so this also safely discovers passed Explorer
    /// reviews written by an earlier application version.
    fn recover_reviewed_delivery_follow_ups(
        &self,
        now: i64,
    ) -> Result<usize, DelegationServiceError> {
        let delivery_ids = self
            .db
            .with_conn(|connection| {
                let mut statement = connection.prepare(
                    "SELECT delivery.id
                     FROM delegation_deliveries delivery
                     JOIN delegations delegation ON delegation.id = delivery.delegation_id
                     JOIN work_packages package ON package.id = delegation.work_package_id
                     JOIN delegation_review_jobs review
                       ON review.delivery_id = delivery.id
                      AND review.delivery_revision = delivery.delivery_version
                      AND review.implementation_attempt_id = delivery.attempt_id
                     WHERE delivery.acceptance_status = 'submitted'
                       AND delegation.status = 'awaiting_summary'
                       AND package.task_shape = 'explore'
                       AND package.worker_profile = 'explorer'
                       AND review.status = 'passed'",
                )?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                rows.collect::<Result<Vec<_>, rusqlite::Error>>()
            })
            .map_err(|_| DelegationServiceError::Review)?
            .map_err(|_| DelegationServiceError::Review)?;
        let queue = DelegatedDeliveryFollowUpQueue::new(self.db.clone());
        let mut queued = 0;
        for delivery_id in delivery_ids {
            if queue
                .enqueue_reviewed_explorer_delivery(&delivery_id, now)
                .map_err(|_| DelegationServiceError::Review)?
                .newly_enqueued()
            {
                queued += 1;
            }
        }
        Ok(queued)
    }

    fn start_review_job(&mut self, job: ReviewJob) -> Result<(), DelegationServiceError> {
        if job.status != ReviewJobStatus::Queued {
            return Ok(());
        }
        let reviewer_attempt_id = format!("reviewer_attempt_{}", job.id);
        let claimed = self
            .review
            .as_ref()
            .ok_or(DelegationServiceError::Review)?
            .claim(
                &job.id,
                &job.implementation_attempt_id,
                &reviewer_attempt_id,
                self.scheduler.runtime().now(),
            )
            .map_err(|_| DelegationServiceError::Review)?;
        let enqueue_result = self
            .review_executor
            .as_mut()
            .ok_or(DelegationServiceError::Review)?
            .enqueue(claimed);
        if let Err(error) = enqueue_result {
            let outcome = super::review_contract::ReviewOutcome::execution_failed(&error);
            self.review
                .as_ref()
                .ok_or(DelegationServiceError::Review)?
                .record_outcome(
                    &job.id,
                    &reviewer_attempt_id,
                    &outcome,
                    self.scheduler.runtime().now(),
                )
                .map_err(|_| DelegationServiceError::Review)?;
        }
        Ok(())
    }

    fn poll_review_events(&mut self) -> Result<usize, DelegationServiceError> {
        let Some(executor) = self.review_executor.as_mut() else {
            return Ok(0);
        };
        let events = executor
            .poll()
            .map_err(|_| DelegationServiceError::Review)?;
        let Some(coordinator) = self.review.as_ref() else {
            return Ok(0);
        };
        let mut first_error = None;
        let mut reviewed_delivery_follow_ups_queued = 0;
        for event in events {
            let result = (|| -> Result<(), DelegationServiceError> {
                let outcome = match event.outcome {
                    Ok(outcome) => outcome,
                    Err(reason) => super::review_contract::ReviewOutcome::execution_failed(&reason),
                };
                match coordinator.record_outcome(
                    &event.job_id,
                    &event.reviewer_attempt_id,
                    &outcome,
                    self.scheduler.runtime().now(),
                ) {
                    Ok(_) => {}
                    // Stale/cancelled claims cannot trigger downstream preparation.
                    Err(ReviewStoreError::InvalidTransition) => return Ok(()),
                    Err(_) => return Err(DelegationServiceError::Review),
                }
                let review_job = coordinator
                    .load(&event.job_id)
                    .map_err(|_| DelegationServiceError::Review)?;
                if outcome.verdict == super::review_contract::SemanticReviewVerdict::Passed {
                    let now = self.scheduler.runtime().now();
                    if self
                        .is_change_attempt(&review_job.implementation_attempt_id)
                        .map_err(|_| DelegationServiceError::Runtime)?
                    {
                        if let Some(pipeline) = self.change_pipeline.as_ref() {
                            if pipeline
                                .prepare_by_delivery(&review_job.delivery_id, now)
                                .is_err()
                            {
                                pipeline.mark_needs_decision(&review_job.delivery_id, now);
                            }
                        }
                    } else if DelegatedDeliveryFollowUpQueue::new(self.db.clone())
                        .enqueue_reviewed_explorer_delivery(&review_job.delivery_id, now)
                        .map_err(|_| DelegationServiceError::Review)?
                        .newly_enqueued()
                    {
                        reviewed_delivery_follow_ups_queued += 1;
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(reviewed_delivery_follow_ups_queued), Err)
    }

    fn review_gate(
        &self,
        delivery_id: &str,
        delivery_revision: i64,
    ) -> Result<(), DelegationServiceError> {
        let Some(coordinator) = self.review.as_ref() else {
            return Err(DelegationServiceError::ReviewPending);
        };
        let job = coordinator
            .load_for_delivery(delivery_id, delivery_revision)
            .map_err(|error| match error {
                ReviewStoreError::NotFound => DelegationServiceError::ReviewPending,
                _ => DelegationServiceError::Review,
            })?;
        match job.status {
            ReviewJobStatus::Passed => Ok(()),
            ReviewJobStatus::Failed | ReviewJobStatus::NeedsDecision => {
                Err(DelegationServiceError::ReviewRejected)
            }
            ReviewJobStatus::Queued | ReviewJobStatus::Running | ReviewJobStatus::Cancelled => {
                Err(DelegationServiceError::ReviewPending)
            }
        }
    }

    pub fn active_worker_attempts(&self) -> usize {
        self.scheduler.runtime().worker().active_worker_attempts()
    }

    pub fn shutdown(&mut self) -> Result<(), DelegationServiceError> {
        if self.state == ServiceState::Stopped {
            return Ok(());
        }
        if self.state == ServiceState::Running {
            if let Some(executor) = self.review_executor.as_mut() {
                executor.stop_all();
            }
            if let Some(coordinator) = self.review.as_ref() {
                coordinator
                    .recover_runnable(self.scheduler.runtime().now())
                    .map_err(|_| DelegationServiceError::Review)?;
            }
            let active = self
                .scheduler
                .runtime()
                .active_attempts()
                .map_err(|_| DelegationServiceError::Runtime)?;
            let mut first_error = None;
            for attempt_id in active {
                if let Err(error) = self.finish_attempt(&attempt_id) {
                    first_error.get_or_insert(error);
                }
            }
            self.state = ServiceState::Stopped;
            if let Some(error) = first_error {
                return Err(error);
            }
        } else {
            self.state = ServiceState::Stopped;
        }
        Ok(())
    }

    fn accept(
        &mut self,
        scope: &DeliveryScope,
        delivery_id: &str,
        limits: &ContractLimits,
    ) -> Result<DecisionOutcome, DelegationServiceError> {
        self.ensure_running()?;
        let attempt_id = self.attempt_for_delivery(scope, delivery_id)?;
        if self.review.is_some() {
            let revision = self.delivery_revision(scope, delivery_id)?;
            self.review_gate(delivery_id, revision)?;
        }
        if self.is_change_attempt(&attempt_id)? {
            // The generic inbox accept path completes the delegation and then
            // releases the worktree.  A change delivery must remain in the
            // review pipeline until capture, independent verification,
            // parent confirmation, and materialization have all completed.
            return Err(DelegationServiceError::ChangeReviewNotReady);
        }
        let outcome =
            DeliveryInbox::accept(self.scheduler.runtime_mut(), scope, delivery_id, limits)
                .map_err(|_| DelegationServiceError::Inbox)?;
        if matches!(
            outcome,
            DecisionOutcome::Accepted(_) | DecisionOutcome::AlreadyAccepted(_)
        ) {
            // Parent acceptance is already durable at this point. A best-effort
            // terminal cleanup may need recovery, but it must not turn that
            // committed decision into a failed tool result and make the Main
            // Agent retry an acknowledgement that has already taken effect.
            match self.finish_attempt(&attempt_id) {
                Ok(()) | Err(DelegationServiceError::CleanupPending(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(outcome)
    }

    /// The only Change terminal path. It is invoked by a Main-Agent tool only
    /// after that tool's normal user confirmation. Generic delivery acceptance
    /// remains intentionally unable to bypass this method.
    fn materialize_change(
        &mut self,
        scope: &DeliveryScope,
        delivery_id: &str,
        approve: bool,
        ttl_secs: i64,
        limits: &ContractLimits,
    ) -> Result<DecisionOutcome, DelegationServiceError> {
        self.ensure_running()?;
        let attempt_id = self.attempt_for_delivery(scope, delivery_id)?;
        if !self
            .is_change_attempt(&attempt_id)
            .map_err(|_| DelegationServiceError::Runtime)?
        {
            return Err(DelegationServiceError::ChangeReviewNotReady);
        }
        let revision = self.delivery_revision(scope, delivery_id)?;
        self.review_gate(delivery_id, revision)?;
        let now = self.scheduler.runtime().now();
        let pipeline = self
            .change_pipeline
            .as_ref()
            .ok_or(DelegationServiceError::ChangeReviewNotReady)?;
        let prepared = pipeline
            .prepare(scope, delivery_id, now)
            .map_err(|_| DelegationServiceError::ChangeReviewNotReady)?;
        if !approve {
            pipeline
                .decline(&prepared, ttl_secs, now)
                .map_err(|_| DelegationServiceError::ChangeReviewNotReady)?;
            self.cancel(&attempt_id)?;
            return Err(DelegationServiceError::ChangeReviewNotReady);
        }
        pipeline
            .confirm_and_materialize(&prepared, true, ttl_secs, now)
            .map_err(|_| DelegationServiceError::ChangeReviewNotReady)?;
        let outcome =
            DeliveryInbox::accept(self.scheduler.runtime_mut(), scope, delivery_id, limits)
                .map_err(|_| DelegationServiceError::Inbox)?;
        if matches!(
            outcome,
            DecisionOutcome::Accepted(_) | DecisionOutcome::AlreadyAccepted(_)
        ) {
            self.finish_attempt(&attempt_id)?;
        }
        Ok(outcome)
    }

    fn needs_decision(
        &mut self,
        scope: &DeliveryScope,
        delivery_id: &str,
        limits: &ContractLimits,
    ) -> Result<DecisionOutcome, DelegationServiceError> {
        self.ensure_running()?;
        let attempt_id = self.attempt_for_delivery(scope, delivery_id)?;
        let outcome =
            DeliveryInbox::needs_decision(self.scheduler.runtime_mut(), scope, delivery_id, limits)
                .map_err(|_| DelegationServiceError::Inbox)?;
        if matches!(
            outcome,
            DecisionOutcome::NeedsDecision(_) | DecisionOutcome::AlreadyNeedsDecision(_)
        ) {
            self.finish_attempt(&attempt_id)?;
        }
        Ok(outcome)
    }

    fn retry(
        &mut self,
        scope: &DeliveryScope,
        delivery_id: &str,
        reason: &str,
        limits: &ContractLimits,
    ) -> Result<RetryContinuation, DelegationServiceError> {
        self.ensure_running()?;
        let attempt_id = self.attempt_for_delivery(scope, delivery_id)?;
        let outcome = DeliveryInbox::retry(
            self.scheduler.runtime_mut(),
            scope,
            delivery_id,
            reason,
            limits,
        )
        .map_err(|_| DelegationServiceError::Inbox)?;
        self.finish_attempt(&attempt_id)?;
        Ok(outcome)
    }

    fn cancel(&mut self, attempt_id: &str) -> Result<(), DelegationServiceError> {
        self.ensure_running()?;
        self.scheduler
            .runtime_mut()
            .cancel(attempt_id)
            .map_err(|_| DelegationServiceError::Runtime)?;
        self.finish_attempt(attempt_id)
    }

    /// Resolve a delivery using only the Main-Agent's trusted session scope.
    /// The parent message/run tuple is read from the durable delegation row;
    /// the model never supplies or widens that scope.
    pub fn decide_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        decision: ParentDecision,
        _reason: Option<&str>,
        limits: &ContractLimits,
    ) -> Result<DecisionOutcome, DelegationServiceError> {
        self.ensure_running()?;
        let scope = self.scope_for_session(session_id, delivery_id)?;
        match decision {
            ParentDecision::Accept => self.accept(&scope, delivery_id, limits),
            ParentDecision::NeedsDecision => self.needs_decision(&scope, delivery_id, limits),
        }
    }

    /// Request a retry using only the Main-Agent's trusted session scope.
    /// The durable delivery lookup supplies the parent message/run tuple, so
    /// callers cannot retry a delivery owned by another foreground session.
    pub fn retry_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        reason: &str,
        limits: &ContractLimits,
    ) -> Result<RetryContinuation, DelegationServiceError> {
        self.ensure_running()?;
        let scope = self.scope_for_session(session_id, delivery_id)?;
        self.retry(&scope, delivery_id, reason, limits)
    }

    pub fn materialize_change_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        ttl_secs: i64,
        limits: &ContractLimits,
    ) -> Result<DecisionOutcome, DelegationServiceError> {
        self.ensure_running()?;
        let scope = self.scope_for_session(session_id, delivery_id)?;
        self.materialize_change(&scope, delivery_id, true, ttl_secs, limits)
    }

    /// Close a reviewed Change handoff when the user's confirmation rejects
    /// canonical materialization.  This is deliberately separate from the
    /// tool handler: a rejected confirmation never executes that handler, but
    /// the temporary worktree, lease, and admission must still reach their
    /// normal terminal cleanup path.
    pub fn decline_change_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        ttl_secs: i64,
        limits: &ContractLimits,
    ) -> Result<(), DelegationServiceError> {
        self.ensure_running()?;
        let scope = self.scope_for_session(session_id, delivery_id)?;
        let attempt_id = self.attempt_for_delivery(&scope, delivery_id)?;
        if !self.is_change_attempt(&attempt_id)? {
            return Err(DelegationServiceError::ChangeReviewNotReady);
        }
        let revision = self.delivery_revision(&scope, delivery_id)?;
        self.review_gate(delivery_id, revision)?;
        let now = self.scheduler.runtime().now();
        let pipeline = self
            .change_pipeline
            .as_ref()
            .ok_or(DelegationServiceError::ChangeReviewNotReady)?;
        let prepared = pipeline
            .prepare(&scope, delivery_id, now)
            .map_err(|_| DelegationServiceError::ChangeReviewNotReady)?;
        pipeline
            .decline(&prepared, ttl_secs, now)
            .map_err(|_| DelegationServiceError::ChangeReviewNotReady)?;
        self.cancel(&attempt_id)
    }

    fn ensure_running(&self) -> Result<(), DelegationServiceError> {
        match self.state {
            ServiceState::Created => Err(DelegationServiceError::NotStarted),
            ServiceState::Running => Ok(()),
            ServiceState::Stopped => Err(DelegationServiceError::Stopped),
        }
    }

    /// Recover delivery scope from durable ownership so callers at the
    /// Main-Agent seam can provide only the trusted session and delivery ids.
    fn scope_for_session(
        &self,
        session_id: &str,
        delivery_id: &str,
    ) -> Result<DeliveryScope, DelegationServiceError> {
        self.scheduler
            .runtime()
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT g.message_id, g.parent_run_id
                     FROM delegation_deliveries d
                     JOIN delegations g ON g.id = d.delegation_id
                     WHERE d.id = ?1 AND g.session_id = ?2",
                    rusqlite::params![delivery_id, session_id],
                    |row| {
                        Ok(DeliveryScope {
                            session_id: session_id.to_string(),
                            message_id: row.get(0)?,
                            parent_run_id: row.get(1)?,
                        })
                    },
                )
                .optional()
                .map_err(|error| DelegationRuntimeError::Storage(error.to_string()))?
                .ok_or_else(|| DelegationRuntimeError::Storage("delivery not found".into()))
            })
            .map_err(|_| DelegationServiceError::Inbox)
    }

    fn attempt_for_delivery(
        &self,
        scope: &DeliveryScope,
        delivery_id: &str,
    ) -> Result<String, DelegationServiceError> {
        self.scheduler
            .runtime()
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT d.attempt_id FROM delegation_deliveries d
                     JOIN delegations g ON g.id=d.delegation_id
                     WHERE d.id=?1 AND g.session_id=?2 AND g.message_id=?3 AND g.parent_run_id=?4",
                    rusqlite::params![
                        delivery_id,
                        scope.session_id,
                        scope.message_id,
                        scope.parent_run_id
                    ],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| DelegationRuntimeError::Storage(error.to_string()))?
                .ok_or_else(|| DelegationRuntimeError::Storage("delivery not found".into()))
            })
            .map_err(|_| DelegationServiceError::Inbox)
    }

    fn delivery_revision(
        &self,
        scope: &DeliveryScope,
        delivery_id: &str,
    ) -> Result<i64, DelegationServiceError> {
        self.scheduler
            .runtime()
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT d.delivery_version
                     FROM delegation_deliveries d
                     JOIN delegations g ON g.id=d.delegation_id
                     WHERE d.id=?1 AND g.session_id=?2 AND g.message_id=?3 AND g.parent_run_id=?4",
                    rusqlite::params![
                        delivery_id,
                        &scope.session_id,
                        &scope.message_id,
                        &scope.parent_run_id
                    ],
                    |row| row.get(0),
                )
                .map_err(|_| DelegationRuntimeError::Storage("delivery not found".into()))
            })
            .map_err(|_| DelegationServiceError::Inbox)
    }

    fn is_change_attempt(&self, attempt_id: &str) -> Result<bool, DelegationServiceError> {
        self.scheduler
            .runtime()
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT p.task_shape
                     FROM delegation_attempts a
                     JOIN delegations d ON d.id = a.delegation_id
                     JOIN work_packages p ON p.id = d.work_package_id
                     WHERE a.id = ?1",
                    [attempt_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map(|shape| shape.as_deref() == Some("change"))
                .map_err(|error| DelegationRuntimeError::Storage(error.to_string()))
            })
            .map_err(|_| DelegationServiceError::Runtime)
    }

    fn finish_attempt(&mut self, attempt_id: &str) -> Result<(), DelegationServiceError> {
        // `finalize_attempt` commits the durable seal before asking the
        // short-lived worker/sandbox adapters to stop.  Those adapters are
        // deliberately best-effort at this boundary: a failure after the
        // durable seal must not keep us from driving the resource-ledger
        // cleanup that can recover it.  A failure before the seal remains a
        // runtime failure because the attempt may still be runnable.
        if self
            .scheduler
            .runtime_mut()
            .finalize_attempt(attempt_id)
            .is_err()
            && !self.attempt_is_durably_terminalized(attempt_id)?
        {
            return Err(DelegationServiceError::Runtime);
        }

        // The resource-ledger cleanup owns sandbox lifecycle and carries the
        // admission CAS token.  Run it before the broad admission fallback so
        // an external sandbox failure is persisted as recoverable cleanup
        // work instead of being hidden behind a generic admission error.
        let cleanup = self.cleanup.cleanup_attempt(attempt_id);
        let now = self.scheduler.runtime().now();
        let admission = self
            .db
            .with_conn_mut(|conn| self.admission.release_attempt(conn, attempt_id, now))
            .map_err(|_| DelegationServiceError::Admission)
            .and_then(|result| result.map_err(|_| DelegationServiceError::Admission));
        if let Err(error) = cleanup {
            self.record_cleanup_pending(attempt_id, &error);
            return Err(DelegationServiceError::CleanupPending(attempt_id.into()));
        }
        if let Err(error) = admission {
            self.record_cleanup_pending(attempt_id, &error.to_string());
            return Err(DelegationServiceError::CleanupPending(attempt_id.into()));
        }
        Ok(())
    }

    fn attempt_is_durably_terminalized(
        &self,
        attempt_id: &str,
    ) -> Result<bool, DelegationServiceError> {
        self.db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT EXISTS(
                         SELECT 1
                         FROM delegation_attempts attempt
                         WHERE attempt.id=?1
                           AND attempt.status='sealed'
                           AND NOT EXISTS (
                               SELECT 1
                               FROM delegation_capability_leases lease
                               WHERE lease.attempt_id=attempt.id
                                 AND lease.status='active'
                           )
                     )",
                    [attempt_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map(|terminalized| terminalized == Some(1))
            })
            .map_err(|_| DelegationServiceError::Runtime)?
            .map_err(|_| DelegationServiceError::Runtime)
    }

    fn recover_terminal_cleanup(&mut self) -> Result<(), DelegationServiceError> {
        // A crash can happen after sealing but before cleanup records an event.
        // Durable bindings, not event history or paths, identify residual resources.
        let attempts = self
            .db
            .with_conn(|conn| {
                let mut statement = conn.prepare(
                    "SELECT DISTINCT a.id FROM delegation_attempts a
                 JOIN delegation_resource_bindings b ON b.attempt_id = a.id
                 WHERE a.status = 'sealed' AND b.cleanup_status != 'cleaned'
                   AND NOT EXISTS (
                     SELECT 1 FROM delegation_deliveries delivery
                     JOIN delegations delegation ON delegation.id=delivery.delegation_id
                     JOIN work_packages package ON package.id=delegation.work_package_id
                     WHERE delivery.attempt_id=a.id
                       AND delivery.acceptance_status='submitted'
                       AND delegation.status='awaiting_summary'
                       AND package.task_shape='change'
                   )",
                )?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                rows.collect::<Result<Vec<_>, rusqlite::Error>>()
            })
            .map_err(|_| DelegationServiceError::Runtime)?
            .map_err(|_| DelegationServiceError::Runtime)?;
        for attempt_id in attempts {
            if let Err(error) = self.cleanup.cleanup_attempt(&attempt_id) {
                // One unavailable resource must not prevent other tasks starting.
                self.record_cleanup_pending(&attempt_id, &error);
            }
        }
        Ok(())
    }

    fn release_recovered_delivery_admissions(
        &self,
        now: i64,
    ) -> Result<(), DelegationServiceError> {
        let attempts = self
            .db
            .with_conn(|conn| {
                let mut statement = conn.prepare(
                    "SELECT DISTINCT attempt.id
                     FROM delegation_attempts attempt
                     JOIN delegation_deliveries delivery ON delivery.attempt_id=attempt.id
                     JOIN delegations delegation ON delegation.id=delivery.delegation_id
                     JOIN workspace_admissions admission ON admission.attempt_id=attempt.id
                     WHERE attempt.status='sealed'
                       AND delivery.acceptance_status='submitted'
                       AND delegation.status='awaiting_summary'
                       AND admission.status='active'",
                )?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                rows.collect::<Result<Vec<_>, rusqlite::Error>>()
            })
            .map_err(|_| DelegationServiceError::Admission)?
            .map_err(|_| DelegationServiceError::Admission)?;
        for attempt_id in attempts {
            self.db
                .with_conn_mut(|conn| self.admission.release_attempt(conn, &attempt_id, now))
                .map_err(|_| DelegationServiceError::Admission)?
                .map_err(|_| DelegationServiceError::Admission)?;
        }
        Ok(())
    }

    fn record_cleanup_pending(&self, attempt_id: &str, reason: &str) {
        let _ = self.scheduler.runtime().with_connection_mut(|conn| {
            let sequence: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(sequence),0)+1 FROM delegation_attempt_events WHERE attempt_id=?1",
                    [attempt_id],
                    |row| row.get(0),
                )
                .map_err(|error| DelegationRuntimeError::Storage(error.to_string()))?;
            conn.execute(
                "INSERT INTO delegation_attempt_events (id, attempt_id, sequence, event_type, payload_json, created_at)
                 VALUES (?1,?2,?3,'cleanup_pending',?4,?5)",
                rusqlite::params![format!("{attempt_id}:{sequence}"), attempt_id, sequence, serde_json::json!({"reason": reason}).to_string(), self.scheduler.runtime().now()],
            )
            .map_err(|error| DelegationRuntimeError::Storage(error.to_string()))?;
            Ok(())
        });
    }
}

impl<W, C, P, S, K> DelegationServiceLoop for DelegationService<W, C, P, S, K>
where
    W: WorkerAdapter + Send,
    C: Clock + Send,
    P: AdmissionPolicy + Send,
    S: SandboxController + Send,
    K: TerminalCleanupPort + Send,
{
    fn start(&mut self) -> Result<ServiceReport, DelegationServiceError> {
        DelegationService::start(self)
    }

    fn tick(&mut self) -> Result<ServiceReport, DelegationServiceError> {
        DelegationService::tick(self)
    }

    fn shutdown(&mut self) -> Result<(), DelegationServiceError> {
        DelegationService::shutdown(self)
    }

    fn poll_workers(&mut self, limits: &ContractLimits) -> Result<usize, DelegationServiceError> {
        DelegationService::poll_workers(self, limits)
    }

    fn active_worker_attempts(&self) -> usize {
        DelegationService::active_worker_attempts(self)
    }

    fn decline_change_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        ttl_secs: i64,
        limits: &ContractLimits,
    ) -> Result<(), DelegationServiceError> {
        DelegationService::decline_change_for_session(
            self,
            session_id,
            delivery_id,
            ttl_secs,
            limits,
        )
    }

    fn heartbeat(
        &mut self,
        heartbeat: super::delegation_runtime::Heartbeat,
    ) -> Result<(), DelegationServiceError> {
        self.ensure_running()?;
        self.scheduler
            .heartbeat(heartbeat)
            .map_err(|_| DelegationServiceError::Scheduler)
    }

    fn submit_delivery(
        &mut self,
        delivery: DelegationDelivery,
        limits: &ContractLimits,
    ) -> Result<(), DelegationServiceError> {
        self.ensure_running()?;
        DeliveryInbox::submit(self.scheduler.runtime_mut(), &delivery, limits)
            .map(|_| ())
            .map_err(|_| DelegationServiceError::Inbox)?;
        self.queue_review_for_delivery(&delivery)
    }

    fn decide_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        decision: ParentDecision,
        reason: Option<&str>,
        limits: &ContractLimits,
    ) -> Result<DecisionOutcome, DelegationServiceError> {
        DelegationService::decide_for_session(
            self,
            session_id,
            delivery_id,
            decision,
            reason,
            limits,
        )
    }

    fn retry_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        reason: &str,
        limits: &ContractLimits,
    ) -> Result<RetryContinuation, DelegationServiceError> {
        DelegationService::retry_for_session(self, session_id, delivery_id, reason, limits)
    }
}

impl From<DelegationRuntimeError> for DelegationServiceError {
    fn from(_: DelegationRuntimeError) -> Self {
        Self::Runtime
    }
}

impl From<WorkspaceAdmissionError> for DelegationServiceError {
    fn from(_: WorkspaceAdmissionError) -> Self {
        Self::Admission
    }
}

impl From<DelegatedSchedulerError> for DelegationServiceError {
    fn from(_: DelegatedSchedulerError) -> Self {
        Self::Scheduler
    }
}

impl From<InboxError> for DelegationServiceError {
    fn from(_: InboxError) -> Self {
        Self::Inbox
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        path::Path,
        process::Command,
        sync::{Arc, Mutex},
    };

    use rusqlite::Connection;
    use serde_json::json;

    use super::*;
    use crate::agent::delegation_contract::{
        CandidateArtifact, Confidence, DelegationBrief, DelegationIdentity, DeliveryStatus,
        KeyFact, UsageFlags, VerificationRecord, VerificationStatus,
    };
    use crate::agent::delegation_runtime::{
        FixedAdmissionPolicy, SandboxController, StopReason, WorkerDispatch,
    };
    use crate::agent::review_contract::{ReviewOutcome, SemanticReviewVerdict};
    use crate::agent::terminal_cleanup::{
        CleanupBinding, CleanupPending, CleanupReport, PersistentResourceCleanup,
    };
    use crate::agent::{
        concrete_sandbox::production_components_with_provider,
        delegated_model_binding::DelegatedModelBindingRequest,
        delegation::{DelegationRepository, NewAttempt, NewCapabilityLease, NewDelegation},
        delegation_issuance::{
            DelegationIssuance, DelegationIssuer, DelegationReceipt, DelegationRequest,
            ForegroundRunIdentity,
        },
        resource_binding::{
            CleanupStatus as ResourceCleanupStatus, LifecycleState, ResourceBindingRepository,
            ResourceKind,
        },
        workspace_admission::{WorkspaceAdmissionMode, WorkspaceAdmissionRequest},
        workspace_isolation::{
            WorktreeCapability, WorktreeFileProvider, WorktreeManifestIdentity, WorktreeOperation,
        },
    };

    #[derive(Default)]
    struct Worker(Vec<super::super::delegation_runtime::WorkerEvent>);
    impl WorkerAdapter for Worker {
        fn start(&mut self, _: WorkerDispatch) -> Result<(), String> {
            Ok(())
        }
        fn stop(&mut self, _: &str, _: StopReason) -> Result<(), String> {
            Ok(())
        }
        fn poll_workers(
            &mut self,
        ) -> Result<Vec<super::super::delegation_runtime::WorkerEvent>, String> {
            Ok(std::mem::take(&mut self.0))
        }
    }
    struct Clock(Cell<i64>);
    impl super::super::delegation_runtime::Clock for Clock {
        fn now(&self) -> i64 {
            self.0.get()
        }
    }
    #[derive(Default)]
    struct Cleanup {
        calls: Vec<String>,
    }
    impl TerminalCleanupPort for Cleanup {
        fn cleanup(&mut self, binding: &CleanupBinding) -> Result<CleanupReport, CleanupPending> {
            self.calls.push(binding.attempt_id.clone());
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
            self.calls.push(attempt_id.into());
            Ok(())
        }
    }

    #[derive(Default)]
    struct CleanupPendingOnce {
        calls: Vec<String>,
    }

    impl TerminalCleanupPort for CleanupPendingOnce {
        fn cleanup(&mut self, binding: &CleanupBinding) -> Result<CleanupReport, CleanupPending> {
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
            self.calls.push(attempt_id.into());
            Err("temporary cleanup failure".into())
        }
    }

    #[derive(Clone)]
    struct FailingSandbox {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl FailingSandbox {
        fn new(calls: Arc<Mutex<Vec<String>>>) -> Self {
            Self { calls }
        }
    }

    impl SandboxController for FailingSandbox {
        fn seal_and_revoke(&mut self, attempt_id: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(attempt_id.into());
            Err("simulated terminal sandbox failure".into())
        }
    }
    #[derive(Default)]
    struct NoopReviewExecutor(Vec<crate::agent::review_agent::ReviewExecutionEvent>);
    impl ReviewExecutor for NoopReviewExecutor {
        fn enqueue(&mut self, _: ReviewJob) -> Result<(), String> {
            Ok(())
        }

        fn poll(
            &mut self,
        ) -> Result<Vec<crate::agent::review_agent::ReviewExecutionEvent>, String> {
            Ok(std::mem::take(&mut self.0))
        }

        fn stop_all(&mut self) {}
    }

    fn service_with_sandbox_and_cleanup<S: SandboxController, K: TerminalCleanupPort>(
        sandbox: S,
        cleanup: K,
    ) -> DelegationService<Worker, Clock, FixedAdmissionPolicy, S, K> {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let db = SharedDb::new(conn);
        let runtime = DelegationRuntime::with_sandbox(
            db.clone(),
            Worker::default(),
            Clock(Cell::new(10)),
            FixedAdmissionPolicy {
                max_running_attempts: 2,
            },
            sandbox,
            30,
        )
        .unwrap();
        let scheduler = DelegatedAttemptScheduler::with_sandbox(runtime);
        DelegationService::with_cleanup(db, scheduler, WorkspaceAdmission::new(), cleanup).unwrap()
    }

    fn service_with_cleanup<K: TerminalCleanupPort>(
        cleanup: K,
    ) -> DelegationService<Worker, Clock, FixedAdmissionPolicy, NoopSandboxController, K> {
        service_with_sandbox_and_cleanup(NoopSandboxController, cleanup)
    }

    fn service(
    ) -> DelegationService<Worker, Clock, FixedAdmissionPolicy, NoopSandboxController, Cleanup>
    {
        service_with_cleanup(Cleanup::default())
    }

    fn seed_queued_explorer_attempt(db: &SharedDb) {
        db.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO sessions(id,title,created_at,updated_at) VALUES ('s','s',1,1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('m','s','user','x',1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES ('r','s','m','x','running','[]',1,1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO work_packages(id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,candidate_version,status,created_at,updated_at) VALUES ('p','s',1,'w','explore','explorer',1,'scope','source',100,0,'active',1,1)",
                [],
            )?;
            let brief = DelegationBrief {
                schema_version: 1,
                identity: DelegationIdentity {
                    session_id: "s".into(),
                    message_id: "m".into(),
                    parent_run_id: "r".into(),
                    delegation_id: "d".into(),
                    attempt_id: "a".into(),
                },
                goal: "x".into(),
                background: vec![],
                constraints: vec![],
                allowed_references: vec!["evidence://test".into()],
                allowed_capabilities: vec![],
                completion_criteria: vec!["done".into()],
            };
            DelegationRepository::create_queued(
                conn,
                &NewDelegation {
                    id: "d".into(),
                    session_id: "s".into(),
                    message_id: "m".into(),
                    parent_run_id: "r".into(),
                    work_package_id: Some("p".into()),
                    idempotency_key: None,
                    objective: "x".into(),
                    brief_json: serde_json::to_string(&brief).unwrap(),
                },
                &NewAttempt {
                    id: "a".into(),
                    attempt_number: 1,
                    sandbox_ref: "sandbox://a".into(),
                    outbox_id: "out_a".into(),
                    dispatch_payload_json: "{}".into(),
                },
                &NewCapabilityLease {
                    id: "lease_a".into(),
                    read_roots_json: "[]".into(),
                    write_roots_json: "[]".into(),
                    tool_allowlist_json: "[]".into(),
                    network_hosts_json: "[]".into(),
                    budget_json: "{}".into(),
                    expires_at: 100,
                },
                1,
            )
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok::<(), rusqlite::Error>(())
        })
        .unwrap()
        .unwrap();
    }

    fn completed_explorer_delivery() -> DelegationDelivery {
        DelegationDelivery {
            schema_version: 1,
            delivery_id: "delivery-a".into(),
            delivery_revision: 1,
            identity: DelegationIdentity {
                session_id: "s".into(),
                message_id: "m".into(),
                parent_run_id: "r".into(),
                delegation_id: "d".into(),
                attempt_id: "a".into(),
            },
            status: DeliveryStatus::Completed,
            executive_summary: "done".into(),
            key_facts: vec![KeyFact {
                statement: "fact".into(),
                confidence: Confidence::High,
                evidence_refs: vec!["evidence://test".into()],
            }],
            milestones: vec![],
            verifications: vec![VerificationRecord {
                item: "goal".into(),
                method: "test".into(),
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
        }
    }

    #[test]
    fn start_is_recovery_first_and_idempotent() {
        let mut service = service();
        assert!(matches!(
            service.tick(),
            Err(DelegationServiceError::NotStarted)
        ));
        let first = service.start().unwrap();
        assert!(first.scheduler.recovered_unknown_effects.is_empty());
        assert_eq!(service.start().unwrap(), ServiceReport::default());
    }

    #[test]
    fn tick_uses_one_loop_and_shutdown_is_idempotent() {
        let mut service = service();
        service.start().unwrap();
        assert!(service.tick().is_ok());
        service.shutdown().unwrap();
        service.shutdown().unwrap();
        assert!(matches!(
            service.tick(),
            Err(DelegationServiceError::Stopped)
        ));
    }

    #[test]
    fn terminal_cancel_invokes_cleanup_for_the_attempt() {
        let mut service = service();
        service
            .db
            .with_conn_mut(|conn| {
                conn.execute("INSERT INTO sessions(id,title,created_at,updated_at) VALUES ('s','s',1,1)", [])?;
                conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('m','s','user','x',1)", [])?;
                conn.execute("INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES ('r','s','m','x','running','[]',1,1)", [])?;
                conn.execute("INSERT INTO work_packages(id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,candidate_version,status,created_at,updated_at) VALUES ('p','s',1,'w','explore','explorer',1,'scope','source',100,0,'active',1,1)", [])?;
                let brief = DelegationBrief {
                    schema_version: 1,
                    identity: DelegationIdentity { session_id: "s".into(), message_id: "m".into(), parent_run_id: "r".into(), delegation_id: "d".into(), attempt_id: "a".into() },
                    goal: "x".into(), background: vec![], constraints: vec![], allowed_references: vec!["evidence://test".into()], allowed_capabilities: vec![], completion_criteria: vec!["done".into()],
                };
                DelegationRepository::create_queued(conn, &NewDelegation { id: "d".into(), session_id: "s".into(), message_id: "m".into(), parent_run_id: "r".into(), work_package_id: Some("p".into()), idempotency_key: None, objective: "x".into(), brief_json: serde_json::to_string(&brief).unwrap() }, &NewAttempt { id: "a".into(), attempt_number: 1, sandbox_ref: "sandbox://a".into(), outbox_id: "out_a".into(), dispatch_payload_json: "{}".into() }, &NewCapabilityLease { id: "lease_a".into(), read_roots_json: "[]".into(), write_roots_json: "[]".into(), tool_allowlist_json: "[]".into(), network_hosts_json: "[]".into(), budget_json: "{}".into(), expires_at: 100 }, 1).map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok::<(), rusqlite::Error>(())
            })
            .unwrap()
            .unwrap();
        service.start().unwrap();
        service.cancel("a").unwrap();
        assert_eq!(service.cleanup.calls, vec!["a"]);
    }

    #[test]
    fn accepting_a_delivery_survives_cleanup_pending_after_the_decision_is_durable() {
        let mut service = service_with_cleanup(CleanupPendingOnce::default());
        service
            .db
            .with_conn_mut(|conn| {
                conn.execute(
                    "INSERT INTO sessions(id,title,created_at,updated_at) VALUES ('s','s',1,1)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('m','s','user','x',1)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES ('r','s','m','x','running','[]',1,1)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO work_packages(id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,candidate_version,status,created_at,updated_at) VALUES ('p','s',1,'w','explore','explorer',1,'scope','source',100,0,'active',1,1)",
                    [],
                )?;
                let brief = DelegationBrief {
                    schema_version: 1,
                    identity: DelegationIdentity {
                        session_id: "s".into(),
                        message_id: "m".into(),
                        parent_run_id: "r".into(),
                        delegation_id: "d".into(),
                        attempt_id: "a".into(),
                    },
                    goal: "x".into(),
                    background: vec![],
                    constraints: vec![],
                    allowed_references: vec!["evidence://test".into()],
                    allowed_capabilities: vec![],
                    completion_criteria: vec!["done".into()],
                };
                DelegationRepository::create_queued(
                    conn,
                    &NewDelegation {
                        id: "d".into(),
                        session_id: "s".into(),
                        message_id: "m".into(),
                        parent_run_id: "r".into(),
                        work_package_id: Some("p".into()),
                        idempotency_key: None,
                        objective: "x".into(),
                        brief_json: serde_json::to_string(&brief).unwrap(),
                    },
                    &NewAttempt {
                        id: "a".into(),
                        attempt_number: 1,
                        sandbox_ref: "sandbox://a".into(),
                        outbox_id: "out_a".into(),
                        dispatch_payload_json: "{}".into(),
                    },
                    &NewCapabilityLease {
                        id: "lease_a".into(),
                        read_roots_json: "[]".into(),
                        write_roots_json: "[]".into(),
                        tool_allowlist_json: "[]".into(),
                        network_hosts_json: "[]".into(),
                        budget_json: "{}".into(),
                        expires_at: 100,
                    },
                    1,
                )
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok::<(), rusqlite::Error>(())
            })
            .unwrap()
            .unwrap();
        service.start().unwrap();
        assert_eq!(
            service.tick().unwrap().scheduler.dispatched,
            vec!["a".to_string()]
        );

        let delivery = DelegationDelivery {
            schema_version: 1,
            delivery_id: "delivery-a".into(),
            delivery_revision: 1,
            identity: DelegationIdentity {
                session_id: "s".into(),
                message_id: "m".into(),
                parent_run_id: "r".into(),
                delegation_id: "d".into(),
                attempt_id: "a".into(),
            },
            status: DeliveryStatus::Completed,
            executive_summary: "done".into(),
            key_facts: vec![KeyFact {
                statement: "fact".into(),
                confidence: Confidence::High,
                evidence_refs: vec!["evidence://test".into()],
            }],
            milestones: vec![],
            verifications: vec![VerificationRecord {
                item: "goal".into(),
                method: "test".into(),
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
        let limits = ContractLimits::default();
        service.submit_delivery(delivery, &limits).unwrap();

        assert!(matches!(
            service
                .decide_for_session("s", "delivery-a", ParentDecision::Accept, None, &limits)
                .unwrap(),
            DecisionOutcome::Accepted(_)
        ));
        assert_eq!(service.cleanup.calls, vec!["a"]);
        service
            .db
            .with_conn(|conn| {
                let statuses = conn.query_row(
                    "SELECT delivery.acceptance_status, delegation.status
                     FROM delegation_deliveries delivery
                     JOIN delegations delegation ON delegation.id = delivery.delegation_id
                     WHERE delivery.id = 'delivery-a'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )?;
                assert_eq!(statuses, ("accepted".into(), "completed".into()));
                Ok::<(), rusqlite::Error>(())
            })
            .unwrap()
            .unwrap();
    }

    #[test]
    fn accepting_a_delivery_survives_sandbox_failure_after_durable_terminalization() {
        let sandbox_calls = Arc::new(Mutex::new(Vec::new()));
        let mut service = service_with_sandbox_and_cleanup(
            FailingSandbox::new(sandbox_calls.clone()),
            Cleanup::default(),
        );
        seed_queued_explorer_attempt(&service.db);

        service.start().unwrap();
        assert_eq!(
            service.tick().unwrap().scheduler.dispatched,
            vec!["a".to_string()]
        );
        let admission = service.admission;
        service
            .db
            .with_conn_mut(|conn| {
                admission
                    .acquire(
                        conn,
                        WorkspaceAdmissionRequest::new(
                            "w",
                            "p",
                            "a",
                            "lease_a",
                            WorkspaceAdmissionMode::Read,
                            100,
                        ),
                        10,
                    )
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok::<(), rusqlite::Error>(())
            })
            .unwrap()
            .unwrap();

        let limits = ContractLimits::default();
        service
            .submit_delivery(completed_explorer_delivery(), &limits)
            .unwrap();
        assert!(matches!(
            service
                .decide_for_session("s", "delivery-a", ParentDecision::Accept, None, &limits)
                .unwrap(),
            DecisionOutcome::Accepted(_)
        ));

        assert_eq!(
            *sandbox_calls.lock().unwrap(),
            vec!["a".to_string()],
            "the injected terminal sandbox failure must happen after the durable seal"
        );
        assert_eq!(service.cleanup.calls, vec!["a".to_string()]);
        service
            .db
            .with_conn(|conn| {
                let statuses = conn.query_row(
                    "SELECT attempt.status, lease.status, admission.status,
                            delivery.acceptance_status, delegation.status
                     FROM delegation_attempts attempt
                     JOIN delegation_capability_leases lease ON lease.attempt_id = attempt.id
                     JOIN workspace_admissions admission ON admission.attempt_id = attempt.id
                     JOIN delegation_deliveries delivery ON delivery.attempt_id = attempt.id
                     JOIN delegations delegation ON delegation.id = attempt.delegation_id
                     WHERE attempt.id = 'a'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )?;
                assert_eq!(
                    statuses,
                    (
                        "sealed".to_string(),
                        "revoked".to_string(),
                        "released".to_string(),
                        "accepted".to_string(),
                        "completed".to_string(),
                    )
                );
                Ok::<(), rusqlite::Error>(())
            })
            .unwrap()
            .unwrap();
    }

    fn initialize_git_workspace(path: &Path) {
        for args in [
            vec!["init"],
            vec!["config", "user.email", "angelbot-test@example.invalid"],
            vec!["config", "user.name", "AngelBot Test"],
            vec!["commit", "--allow-empty", "-m", "initial"],
        ] {
            assert!(Command::new("git")
                .args(args)
                .current_dir(path)
                .status()
                .unwrap()
                .success());
        }
    }

    struct RealDelegationFixture {
        db: SharedDb,
        identity: ForegroundRunIdentity,
        workspace: tempfile::TempDir,
        app_data: tempfile::TempDir,
    }

    fn real_delegation_fixture(session_id: &str) -> RealDelegationFixture {
        let app_data = tempfile::tempdir().unwrap();
        let conn = Connection::open(app_data.path().join("test.sqlite")).unwrap();
        crate::db::migrate(&conn).unwrap();
        let db = SharedDb::new(conn);
        let workspace = tempfile::tempdir().unwrap();
        initialize_git_workspace(workspace.path());
        let message_id = format!("message-{session_id}");
        db.with_conn_mut(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO profile (id,updated_at) VALUES (1,1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO sessions (id,title,work_dir,created_at,updated_at)
                 VALUES (?1,'cleanup',?2,1,1)",
                rusqlite::params![session_id, workspace.path().to_string_lossy().to_string()],
            )?;
            conn.execute(
                "INSERT INTO projects (id,name,path,created_at,active_session_id)
                 VALUES (?1,'cleanup-project',?2,1,?1)",
                rusqlite::params![session_id, workspace.path().to_string_lossy().to_string()],
            )?;
            conn.execute(
                "INSERT INTO messages (id,session_id,role,content,is_provisional,created_at)
                 VALUES (?1,?2,'assistant','',1,1)",
                rusqlite::params![message_id, session_id],
            )?;
            conn.execute(
                "INSERT INTO task_runs
                    (id,session_id,message_id,goal,status,plan,created_at,updated_at)
                 VALUES (?1,?2,?1,'cleanup','running','[]',1,1)",
                rusqlite::params![message_id, session_id],
            )?;
            Ok::<(), rusqlite::Error>(())
        })
        .unwrap()
        .unwrap();
        let identity = db
            .with_conn(|conn| {
                ForegroundRunIdentity::prepare(
                    conn,
                    session_id,
                    &message_id,
                    workspace.path().to_path_buf(),
                )
            })
            .unwrap()
            .unwrap();
        RealDelegationFixture {
            db,
            identity,
            workspace,
            app_data,
        }
    }

    fn issue_local_delegation(
        fixture: &RealDelegationFixture,
        call_id: &str,
        task_shape: &str,
        worker_profile: &str,
    ) -> DelegationReceipt {
        let request: DelegationRequest = serde_json::from_value(json!({
            "goal": "perform bounded local work",
            "background_refs": ["evidence://cleanup"],
            "task_shape": task_shape,
            "worker_profile": worker_profile,
            "explorer_operations": []
        }))
        .unwrap();
        DelegationIssuance::new(
            fixture.db.arc(),
            fixture.app_data.path().join("delegated").join("sandboxes"),
            DelegatedModelBindingRequest::new("provider:test", "model:test", "keychain:test", 1)
                .unwrap(),
        )
        .issue(&fixture.identity, call_id, request, None)
        .unwrap()
    }

    #[test]
    fn terminal_cancel_cleans_persisted_sandbox_and_git_worktree() {
        for termination in ["cancel", "interrupted", "expired", "disconnected"] {
            let fixture = real_delegation_fixture("cleanup-session");
            let receipt = issue_local_delegation(&fixture, "cleanup-call", "explore", "explorer");
            let RealDelegationFixture {
                db,
                workspace: _workspace,
                app_data: storage,
                ..
            } = fixture;
            let app_data = storage.path();

            let repository = ResourceBindingRepository::new(db.clone());
            let sandbox_before = repository
                .load_for_attempt(&receipt.attempt_id, ResourceKind::Sandbox)
                .unwrap();
            let worktree_before = repository
                .load_for_attempt(&receipt.attempt_id, ResourceKind::Worktree)
                .unwrap();
            assert_eq!(sandbox_before.cleanup_status, ResourceCleanupStatus::None);
            assert_eq!(worktree_before.cleanup_status, ResourceCleanupStatus::None);
            let sandbox_active = app_data.join("delegated/sandboxes/active");
            let worktree_active = app_data.join("delegated/worktrees/active");
            assert_eq!(std::fs::read_dir(&sandbox_active).unwrap().count(), 1);
            assert_eq!(std::fs::read_dir(&worktree_active).unwrap().count(), 1);

            let (sandbox, cleanup_effects, _) =
                production_components_with_provider(db.clone(), app_data).unwrap();
            let now = chrono::Utc::now().timestamp();
            let runtime = DelegationRuntime::with_sandbox(
                db.clone(),
                Worker::default(),
                Clock(Cell::new(now)),
                FixedAdmissionPolicy {
                    max_running_attempts: 2,
                },
                sandbox,
                30,
            )
            .unwrap();
            let scheduler = DelegatedAttemptScheduler::with_sandbox(runtime);
            let cleanup = PersistentResourceCleanup::new(
                ResourceBindingRepository::new(db.clone()),
                cleanup_effects,
            );
            let mut service = DelegationService::with_cleanup(
                db.clone(),
                scheduler,
                WorkspaceAdmission::new(),
                cleanup,
            )
            .unwrap();
            if termination == "interrupted" {
                // Persist the crash window: sealed, but no cleanup event or effects.
                service
                    .scheduler
                    .runtime_mut()
                    .finalize_attempt(&receipt.attempt_id)
                    .unwrap();
            }
            service.start().unwrap();
            if termination == "cancel" {
                service.cancel(&receipt.attempt_id).unwrap();
            } else if termination == "expired" || termination == "disconnected" {
                assert_eq!(
                    service.tick().unwrap().scheduler.dispatched,
                    vec![receipt.attempt_id.clone()]
                );
                if termination == "expired" {
                    db.with_conn(|conn| {
                    conn.execute(
                        "UPDATE delegation_capability_leases SET issued_at=0, expires_at=1 WHERE attempt_id=?1",
                        [&receipt.attempt_id],
                    )
                })
                .unwrap()
                .unwrap();
                    assert_eq!(
                        service.tick().unwrap().scheduler.expired,
                        vec![receipt.attempt_id.clone()]
                    );
                } else {
                    use super::super::delegation_runtime::{Heartbeat, WorkerEvent};
                    service.scheduler.runtime_mut().worker_mut().0 = vec![
                        WorkerEvent::Heartbeat(Heartbeat {
                            attempt_id: "missing-attempt".into(),
                            epoch: 1,
                            stage: "work".into(),
                            progress_percent: 0,
                            budget_remaining: "ok".into(),
                            evidence_refs: vec![],
                        }),
                        WorkerEvent::Terminal {
                            attempt_id: receipt.attempt_id.clone(),
                            lease_epoch: 1,
                            reason: "disconnected".into(),
                        },
                    ];
                    assert!(service.poll_workers(&ContractLimits::default()).is_err());
                    service.poll_workers(&ContractLimits::default()).unwrap();
                }
                assert!(service.tick().unwrap().scheduler.dispatched.is_empty());
                db.with_conn(|conn| {
                    let status: String = conn
                        .query_row(
                            "SELECT status FROM delegations WHERE id=?1",
                            [&receipt.delegation_id],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(status, "needs_decision");
                })
                .unwrap();
            }

            let sandbox_after = repository
                .load_for_attempt(&receipt.attempt_id, ResourceKind::Sandbox)
                .unwrap();
            let worktree_after = repository
                .load_for_attempt(&receipt.attempt_id, ResourceKind::Worktree)
                .unwrap();
            assert_eq!(sandbox_after.cleanup_status, ResourceCleanupStatus::Cleaned);
            assert_eq!(sandbox_after.lifecycle_state, LifecycleState::Quarantined);
            assert_eq!(
                worktree_after.cleanup_status,
                ResourceCleanupStatus::Cleaned
            );
            assert_eq!(std::fs::read_dir(&sandbox_active).unwrap().count(), 0);
            assert_eq!(std::fs::read_dir(&worktree_active).unwrap().count(), 0);
            db.with_conn(|conn| {
                let attempt_status: String = conn
                    .query_row(
                        "SELECT status FROM delegation_attempts WHERE id=?1",
                        [&receipt.attempt_id],
                        |row| row.get(0),
                    )
                    .unwrap();
                let lease_status: String = conn
                    .query_row(
                        "SELECT status FROM delegation_capability_leases WHERE attempt_id=?1",
                        [&receipt.attempt_id],
                        |row| row.get(0),
                    )
                    .unwrap();
                let admission_status: String = conn
                    .query_row(
                        "SELECT status FROM workspace_admissions WHERE attempt_id=?1",
                        [&receipt.attempt_id],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(attempt_status, "sealed");
                assert_eq!(lease_status, "revoked");
                assert_eq!(admission_status, "released");
            })
            .unwrap();

            // Drop every connection/provider before replaying cleanup, as on restart.
            drop(service);
            drop(repository);
            drop(db);
            let reopened = SharedDb::new(Connection::open(app_data.join("test.sqlite")).unwrap());
            let (_, effects, _) =
                production_components_with_provider(reopened.clone(), app_data).unwrap();
            let repository = ResourceBindingRepository::new(reopened.clone());
            let mut cleanup = PersistentResourceCleanup::new(repository.clone(), effects);
            cleanup.cleanup_attempt(&receipt.attempt_id).unwrap();
            for kind in [ResourceKind::Sandbox, ResourceKind::Worktree] {
                assert_eq!(
                    repository
                        .load_for_attempt(&receipt.attempt_id, kind)
                        .unwrap()
                        .cleanup_status,
                    ResourceCleanupStatus::Cleaned
                );
            }
            assert_eq!(std::fs::read_dir(&sandbox_active).unwrap().count(), 0);
            assert_eq!(std::fs::read_dir(&worktree_active).unwrap().count(), 0);
            reopened
                .with_conn(|conn| {
                    let violation: Option<String> = conn
                        .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
                        .optional()
                        .unwrap();
                    assert_eq!(violation, None);
                })
                .unwrap();
        }
    }

    #[test]
    fn declined_reviewed_change_uses_the_same_persistent_terminal_cleanup() {
        let fixture = real_delegation_fixture("decline-session");
        let db = fixture.db.clone();
        let receipt = issue_local_delegation(&fixture, "decline-call", "change", "implementer");
        let repository = ResourceBindingRepository::new(db.clone());
        let worktree_binding = repository
            .load_for_attempt(&receipt.attempt_id, ResourceKind::Worktree)
            .unwrap();
        let worktree_identity = WorktreeManifestIdentity::from_binding(&worktree_binding).unwrap();
        let (sandbox, cleanup_effects, provider) =
            production_components_with_provider(db.clone(), fixture.app_data.path()).unwrap();
        let access = provider
            .open_file_access(
                &worktree_identity,
                WorktreeCapability {
                    read_roots: vec![".".into()],
                    write_roots: vec![".".into()],
                    operation: WorktreeOperation::Write,
                    lease_epoch: u64::try_from(worktree_binding.identity.lease_epoch).unwrap(),
                    collaboration_key: fixture.identity.workspace_key.clone(),
                },
            )
            .unwrap();
        provider
            .write_candidate(
                &access,
                "src/feature.txt",
                b"candidate remains isolated\n",
                1024,
            )
            .unwrap();

        let now = chrono::Utc::now().timestamp();
        let runtime = DelegationRuntime::with_sandbox(
            db.clone(),
            Worker::default(),
            Clock(Cell::new(now)),
            FixedAdmissionPolicy {
                max_running_attempts: 2,
            },
            sandbox,
            30,
        )
        .unwrap();
        let scheduler = DelegatedAttemptScheduler::with_sandbox(runtime);
        let cleanup = PersistentResourceCleanup::new(repository.clone(), cleanup_effects);
        let reviews = ReviewCoordinator::new(db.clone());
        let mut service = DelegationService::with_cleanup(
            db.clone(),
            scheduler,
            WorkspaceAdmission::new(),
            cleanup,
        )
        .unwrap()
        .with_review(reviews.clone(), Box::new(NoopReviewExecutor::default()))
        .with_change_pipeline(crate::agent::change_pipeline::ChangePipeline::new(
            db.clone(),
            provider,
        ));
        service.start().unwrap();
        service.tick().unwrap();

        let delivery_id = "delivery-real-decline";
        let delivery = DelegationDelivery {
            schema_version: 1,
            delivery_id: delivery_id.into(),
            delivery_revision: 1,
            identity: DelegationIdentity {
                session_id: fixture.identity.session_id.clone(),
                message_id: fixture.identity.message_id.clone(),
                parent_run_id: fixture.identity.parent_run_id.clone(),
                delegation_id: receipt.delegation_id.clone(),
                attempt_id: receipt.attempt_id.clone(),
            },
            status: DeliveryStatus::Completed,
            executive_summary: "Prepared one isolated candidate file.".into(),
            key_facts: vec![KeyFact {
                statement: "Only src/feature.txt changed in the temporary worktree.".into(),
                confidence: Confidence::High,
                evidence_refs: vec!["evidence://change/feature".into()],
            }],
            milestones: Vec::new(),
            verifications: vec![VerificationRecord {
                item: "candidate scope".into(),
                method: "deterministic fixture".into(),
                status: VerificationStatus::Passed,
                conclusion: "The candidate is ready for review.".into(),
                evidence_ref: "evidence://change/feature".into(),
            }],
            candidate_artifacts: vec![CandidateArtifact {
                kind: "source".into(),
                relative_ref: "src/feature.txt".into(),
                description: "Isolated candidate content.".into(),
                evidence_ref: Some("evidence://change/feature".into()),
            }],
            open_questions: Vec::new(),
            risks: Vec::new(),
            evidence_refs: vec!["evidence://change/feature".into()],
            usage: UsageFlags {
                context_truncated: false,
                output_truncated: false,
                budget_exhausted: false,
            },
        };
        let limits = ContractLimits::default();
        // Reproduce a process loss after the delivery commit and before the
        // separate reviewer enqueue. The new service must repair that gap
        // before it seals/reclaims any producer resources.
        DeliveryInbox::submit(service.scheduler.runtime_mut(), &delivery, &limits).unwrap();
        assert!(matches!(
            reviews.load_for_delivery(delivery_id, 1),
            Err(ReviewStoreError::NotFound)
        ));
        drop(service);

        let (sandbox, cleanup_effects, provider) =
            production_components_with_provider(db.clone(), fixture.app_data.path()).unwrap();
        let runtime = DelegationRuntime::with_sandbox(
            db.clone(),
            Worker::default(),
            Clock(Cell::new(now + 1)),
            FixedAdmissionPolicy {
                max_running_attempts: 2,
            },
            sandbox,
            30,
        )
        .unwrap();
        let scheduler = DelegatedAttemptScheduler::with_sandbox(runtime);
        let cleanup = PersistentResourceCleanup::new(repository.clone(), cleanup_effects);
        let reviews = ReviewCoordinator::new(db.clone());
        let mut service = DelegationService::with_cleanup(
            db.clone(),
            scheduler,
            WorkspaceAdmission::new(),
            cleanup,
        )
        .unwrap()
        .with_review(reviews.clone(), Box::new(NoopReviewExecutor::default()))
        .with_change_pipeline(crate::agent::change_pipeline::ChangePipeline::new(
            db.clone(),
            provider,
        ));
        service.start().unwrap();
        db.with_conn(|conn| {
            let attempt_status: String = conn
                .query_row(
                    "SELECT status FROM delegation_attempts WHERE id=?1",
                    [&receipt.attempt_id],
                    |row| row.get(0),
                )
                .unwrap();
            let delegation_status: String = conn
                .query_row(
                    "SELECT status FROM delegations WHERE id=?1",
                    [&receipt.delegation_id],
                    |row| row.get(0),
                )
                .unwrap();
            let admission_status: String = conn
                .query_row(
                    "SELECT status FROM workspace_admissions WHERE attempt_id=?1",
                    [&receipt.attempt_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(attempt_status, "sealed");
            assert_eq!(delegation_status, "awaiting_summary");
            assert_eq!(admission_status, "released");
        })
        .unwrap();
        assert_eq!(
            repository
                .load_for_attempt(&receipt.attempt_id, ResourceKind::Worktree)
                .unwrap()
                .cleanup_status,
            ResourceCleanupStatus::None
        );
        let review = reviews.load_for_delivery(delivery_id, 1).unwrap();
        let reviewer_attempt_id = review.reviewer_attempt_id.clone().unwrap();
        let valid = crate::agent::review_agent::ReviewExecutionEvent {
            job_id: review.id.clone(),
            reviewer_attempt_id,
            outcome: Ok(ReviewOutcome {
                verdict: SemanticReviewVerdict::Passed,
                summary: "The candidate matches its bounded delivery.".into(),
                findings: Vec::new(),
                missing_evidence: Vec::new(),
                evidence_refs: vec!["evidence://change/feature".into()],
            }),
        };
        let mut stale = valid.clone();
        stale.job_id = "missing-review-job".into();
        let mut invalid = valid.clone();
        invalid.outcome.as_mut().unwrap().summary.clear();
        service.review_executor = Some(Box::new(NoopReviewExecutor(vec![stale, invalid, valid])));
        assert!(service.poll_review_events().is_err());
        assert_eq!(
            reviews.load(&review.id).unwrap().status,
            ReviewJobStatus::Passed
        );
        service.poll_review_events().unwrap();

        service
            .decline_change_for_session(&fixture.identity.session_id, delivery_id, 3600, &limits)
            .unwrap();

        // Replay the table rebuild with populated, linked rows. Empty-schema
        // migration tests cannot detect lost artifacts or broken handoff FKs.
        let artifact_id: String = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT artifact_id FROM delegation_change_handoffs WHERE delivery_id=?1",
                    [delivery_id],
                    |row| row.get(0),
                )
            })
            .unwrap()
            .unwrap();
        let artifacts = crate::agent::review_artifact_store::ReviewArtifactStore::new(db.clone());
        let before = artifacts.load(&artifact_id).unwrap();
        db.with_conn_mut(|conn| {
            let tx = conn.transaction().unwrap();
            tx.execute_batch(include_str!(
                "../../migrations/070_review_execution_identity.sql"
            ))
            .unwrap();
            let violation: Option<String> = tx
                .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
                .optional()
                .unwrap();
            assert_eq!(violation, None);
            tx.commit().unwrap();
        })
        .unwrap();
        assert_eq!(artifacts.load(&artifact_id).unwrap(), before);

        assert!(
            !fixture.workspace.path().join("src/feature.txt").exists(),
            "declining a candidate must never write the canonical workspace"
        );
        assert_eq!(
            repository
                .load_for_attempt(&receipt.attempt_id, ResourceKind::Sandbox)
                .unwrap()
                .cleanup_status,
            ResourceCleanupStatus::Cleaned
        );
        assert_eq!(
            repository
                .load_for_attempt(&receipt.attempt_id, ResourceKind::Worktree)
                .unwrap()
                .cleanup_status,
            ResourceCleanupStatus::Cleaned
        );
        db.with_conn(|conn| {
            let package_status: String = conn
                .query_row(
                    "SELECT p.status FROM work_packages p
                     JOIN delegations d ON d.work_package_id=p.id
                     WHERE d.id=?1",
                    [&receipt.delegation_id],
                    |row| row.get(0),
                )
                .unwrap();
            let handoff_status: String = conn
                .query_row(
                    "SELECT status FROM delegation_change_handoffs WHERE delivery_id=?1",
                    [delivery_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(package_status, "declined");
            assert_eq!(handoff_status, "cancelled");
        })
        .unwrap();
    }

    #[test]
    fn mismatched_shared_database_is_rejected() {
        let mut other = service();
        let db = SharedDb::new(Connection::open_in_memory().unwrap());
        let scheduler = std::mem::replace(&mut other.scheduler, {
            let runtime = DelegationRuntime::new_shared(
                other.db.clone(),
                Worker::default(),
                Clock(Cell::new(10)),
                FixedAdmissionPolicy {
                    max_running_attempts: 1,
                },
                30,
            )
            .unwrap();
            DelegatedAttemptScheduler::with_sandbox(runtime)
        });
        let _ = scheduler;
        assert!(!db.same_instance(&other.db));
    }

    #[test]
    fn fail_closed_handle_has_one_recoverable_loop() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let db = SharedDb::new(conn);
        let handle = DelegationServiceHandle::fail_closed(db).unwrap();
        let first = handle.start().unwrap();
        assert!(first.scheduler.recovered_unknown_effects.is_empty());
        assert_eq!(handle.start().unwrap(), ServiceReport::default());
        assert!(handle.tick().is_ok());
        handle.shutdown().unwrap();
        handle.shutdown().unwrap();
    }
}
