//! Semantic review lifecycle boundary.
//!
//! This coordinator owns review policy and identity checks, while
//! `ReviewJobStore` owns SQLite.  Model execution is intentionally not here;
//! a future Pump adapter can claim a job, run a Reviewer Agent, and record the
//! bounded outcome through this narrow interface.

use super::{
    review_contract::{ReviewOutcome, ReviewSubject},
    review_store::{ReviewJob, ReviewJobStore, ReviewStoreError},
    shared_db::SharedDb,
};

#[derive(Clone)]
pub struct ReviewCoordinator {
    store: ReviewJobStore,
}

impl ReviewCoordinator {
    pub fn new(db: SharedDb) -> Self {
        Self {
            store: ReviewJobStore::new(db),
        }
    }

    /// Queue at most one review for a producer delivery revision.  The
    /// delivery remains parent-incomplete until a terminal review outcome is
    /// recorded by the execution adapter.
    pub fn enqueue(
        &self,
        job_id: &str,
        producer_attempt_id: &str,
        delivery_id: &str,
        delivery_revision: i64,
        subject: &ReviewSubject,
        now: i64,
    ) -> Result<ReviewJob, ReviewStoreError> {
        if subject.implementation_attempt_id != producer_attempt_id
            || subject.delivery_id != delivery_id
        {
            return Err(ReviewStoreError::Conflict);
        }
        self.store.enqueue(
            job_id,
            producer_attempt_id,
            delivery_id,
            delivery_revision,
            subject,
            now,
        )
    }

    /// Claim a queued job for a distinct internal Reviewer identity.  This
    /// identity is not a retry and may never equal the producer attempt.
    pub fn claim(
        &self,
        job_id: &str,
        producer_attempt_id: &str,
        reviewer_attempt_id: &str,
        now: i64,
    ) -> Result<ReviewJob, ReviewStoreError> {
        if producer_attempt_id == reviewer_attempt_id {
            return Err(ReviewStoreError::Conflict);
        }
        let job = self.store.load(job_id)?;
        if job.implementation_attempt_id != producer_attempt_id {
            return Err(ReviewStoreError::Conflict);
        }
        self.store.claim(job_id, reviewer_attempt_id, now)
    }

    /// Record only the result from the Reviewer that claimed this job.  The
    /// coordinator does not turn a pass into parent acceptance; that remains a
    /// separate Main-Agent decision.
    pub fn record_outcome(
        &self,
        job_id: &str,
        reviewer_attempt_id: &str,
        outcome: &ReviewOutcome,
        now: i64,
    ) -> Result<ReviewJob, ReviewStoreError> {
        self.store
            .record_outcome(job_id, reviewer_attempt_id, outcome, now)
    }

    pub fn load(&self, job_id: &str) -> Result<ReviewJob, ReviewStoreError> {
        self.store.load(job_id)
    }

    pub fn load_for_delivery(
        &self,
        delivery_id: &str,
        delivery_revision: i64,
    ) -> Result<ReviewJob, ReviewStoreError> {
        self.store.load_by_delivery(delivery_id, delivery_revision)
    }

    pub fn is_passed(
        &self,
        delivery_id: &str,
        delivery_revision: i64,
    ) -> Result<bool, ReviewStoreError> {
        Ok(matches!(
            self.load_for_delivery(delivery_id, delivery_revision)?
                .status,
            super::review_store::ReviewJobStatus::Passed
        ))
    }

    /// Recover only ephemeral reviewer claims. Producer identity and finished
    /// verdicts remain immutable across a host restart.
    pub fn recover_runnable(&self, now: i64) -> Result<Vec<ReviewJob>, ReviewStoreError> {
        self.store.requeue_running(now)?;
        self.store.list_queued()
    }
}
