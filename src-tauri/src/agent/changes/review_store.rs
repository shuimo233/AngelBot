//! Durable storage for one semantic review job per producer delivery.
//!
//! This module owns SQLite and idempotency only.  It does not execute models,
//! schedule workers, or decide whether a parent accepts a delivery.

use rusqlite::{params, Connection};

use super::{
    review_contract::{ReviewOutcome, ReviewSubject},
    shared_db::SharedDb,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewJobStatus {
    Queued,
    Running,
    Passed,
    Failed,
    NeedsDecision,
    Cancelled,
}

impl ReviewJobStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::NeedsDecision => "needs_decision",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "passed" => Self::Passed,
            "failed" => Self::Failed,
            "needs_decision" => Self::NeedsDecision,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewJob {
    pub id: String,
    pub implementation_attempt_id: String,
    pub delivery_id: String,
    pub delivery_revision: i64,
    pub subject: ReviewSubject,
    pub status: ReviewJobStatus,
    pub reviewer_attempt_id: Option<String>,
    pub outcome: Option<ReviewOutcome>,
    pub created_at: i64,
    pub updated_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ReviewStoreError {
    #[error("review subject is invalid")]
    InvalidSubject,
    #[error("review job was not found")]
    NotFound,
    #[error("review job identity conflicts with an existing job")]
    Conflict,
    #[error("review job transition is invalid")]
    InvalidTransition,
    #[error("review job storage failed")]
    Storage,
}

#[derive(Clone)]
pub struct ReviewJobStore {
    db: SharedDb,
}

impl ReviewJobStore {
    pub fn new(db: SharedDb) -> Self {
        Self { db }
    }

    pub fn enqueue(
        &self,
        id: &str,
        implementation_attempt_id: &str,
        delivery_id: &str,
        delivery_revision: i64,
        subject: &ReviewSubject,
        now: i64,
    ) -> Result<ReviewJob, ReviewStoreError> {
        if id.is_empty()
            || implementation_attempt_id.is_empty()
            || delivery_id.is_empty()
            || delivery_revision <= 0
        {
            return Err(ReviewStoreError::InvalidSubject);
        }
        subject
            .validate()
            .map_err(|_| ReviewStoreError::InvalidSubject)?;
        let subject_json =
            serde_json::to_string(subject).map_err(|_| ReviewStoreError::InvalidSubject)?;
        self.db
            .with_conn_mut(|conn| {
                conn.execute(
                    "INSERT INTO delegation_review_jobs
                     (id,implementation_attempt_id,delivery_id,delivery_revision,subject_json,status,created_at,updated_at)
                     VALUES (?1,?2,?3,?4,?5,'queued',?6,?6)
                     ON CONFLICT(delivery_id,delivery_revision) DO NOTHING",
                    params![
                        id,
                        implementation_attempt_id,
                        delivery_id,
                        delivery_revision,
                        subject_json,
                        now
                    ],
                )
                .map_err(|_| ReviewStoreError::Storage)
            })
            .map_err(|_| ReviewStoreError::Storage)??;
        let stored = self.load_by_delivery(delivery_id, delivery_revision)?;
        if stored.id != id
            || stored.implementation_attempt_id != implementation_attempt_id
            || stored.subject != *subject
        {
            return Err(ReviewStoreError::Conflict);
        }
        Ok(stored)
    }

    pub fn claim(
        &self,
        job_id: &str,
        reviewer_attempt_id: &str,
        now: i64,
    ) -> Result<ReviewJob, ReviewStoreError> {
        if reviewer_attempt_id.is_empty() {
            return Err(ReviewStoreError::InvalidTransition);
        }
        self.db
            .with_conn_mut(|conn| {
                let changed = conn
                    .execute(
                        "UPDATE delegation_review_jobs
                         SET status='running', reviewer_attempt_id=?1, started_at=?2, updated_at=?2
                         WHERE id=?3 AND status='queued'",
                        params![reviewer_attempt_id, now, job_id],
                    )
                    .map_err(|_| ReviewStoreError::Storage)?;
                if changed != 1 {
                    return Err(ReviewStoreError::InvalidTransition);
                }
                Ok(())
            })
            .map_err(|_| ReviewStoreError::Storage)??;
        self.load(job_id)
    }

    pub fn record_outcome(
        &self,
        job_id: &str,
        reviewer_attempt_id: &str,
        outcome: &ReviewOutcome,
        now: i64,
    ) -> Result<ReviewJob, ReviewStoreError> {
        if reviewer_attempt_id.is_empty() {
            return Err(ReviewStoreError::InvalidTransition);
        }
        outcome
            .validate()
            .map_err(|_| ReviewStoreError::InvalidSubject)?;
        let status = match outcome.verdict {
            super::review_contract::SemanticReviewVerdict::Passed => ReviewJobStatus::Passed,
            super::review_contract::SemanticReviewVerdict::Failed => ReviewJobStatus::Failed,
            super::review_contract::SemanticReviewVerdict::NeedsDecision => {
                ReviewJobStatus::NeedsDecision
            }
        };
        let outcome_json =
            serde_json::to_string(outcome).map_err(|_| ReviewStoreError::InvalidSubject)?;
        self.db
            .with_conn_mut(|conn| {
                let changed = conn
                    .execute(
                        "UPDATE delegation_review_jobs
                         SET status=?1, outcome_json=?2, ended_at=?3, updated_at=?3
                         WHERE id=?4 AND reviewer_attempt_id=?5 AND status='running'",
                        params![
                            status.as_str(),
                            outcome_json,
                            now,
                            job_id,
                            reviewer_attempt_id
                        ],
                    )
                    .map_err(|_| ReviewStoreError::Storage)?;
                if changed != 1 {
                    return Err(ReviewStoreError::InvalidTransition);
                }
                Ok(())
            })
            .map_err(|_| ReviewStoreError::Storage)??;
        self.load(job_id)
    }

    /// A reviewer process is ephemeral. After a host restart (or orderly
    /// shutdown that cancels it), clear only its claim so the immutable
    /// producer subject can be reviewed again.
    pub fn requeue_running(&self, now: i64) -> Result<usize, ReviewStoreError> {
        self.db
            .with_conn_mut(|conn| {
                conn.execute(
                    "UPDATE delegation_review_jobs
                     SET status='queued', reviewer_attempt_id=NULL,
                         started_at=NULL, updated_at=?1
                     WHERE status='running'",
                    params![now],
                )
                .map_err(|_| ReviewStoreError::Storage)
            })
            .map_err(|_| ReviewStoreError::Storage)?
    }

    /// Return durable jobs that have no verdict. Each still needs a CAS claim
    /// before a reviewer model may start.
    pub fn list_queued(&self) -> Result<Vec<ReviewJob>, ReviewStoreError> {
        self.db
            .with_conn(|conn| {
                let mut statement = conn
                    .prepare(
                        "SELECT id,implementation_attempt_id,delivery_id,delivery_revision,
                                subject_json,status,reviewer_attempt_id,outcome_json,
                                created_at,updated_at,started_at,ended_at
                         FROM delegation_review_jobs
                         WHERE status='queued'
                         ORDER BY created_at,id",
                    )
                    .map_err(|_| ReviewStoreError::Storage)?;
                let rows = statement
                    .query_map([], map_job_row)
                    .map_err(|_| ReviewStoreError::Storage)?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|_| ReviewStoreError::Storage)
            })
            .map_err(|_| ReviewStoreError::Storage)?
    }

    pub fn load(&self, job_id: &str) -> Result<ReviewJob, ReviewStoreError> {
        self.db
            .with_conn(|conn| load_job(conn, Some(job_id), None))
            .map_err(|_| ReviewStoreError::Storage)?
            .map_err(|_| ReviewStoreError::NotFound)
    }

    pub(crate) fn load_by_delivery(
        &self,
        delivery_id: &str,
        delivery_revision: i64,
    ) -> Result<ReviewJob, ReviewStoreError> {
        self.db
            .with_conn(|conn| load_job(conn, None, Some((delivery_id, delivery_revision))))
            .map_err(|_| ReviewStoreError::Storage)?
            .map_err(|_| ReviewStoreError::NotFound)
    }
}

fn load_job(
    conn: &Connection,
    id: Option<&str>,
    delivery: Option<(&str, i64)>,
) -> Result<ReviewJob, rusqlite::Error> {
    let (predicate, values): (&str, Vec<Box<dyn rusqlite::ToSql>>) = match (id, delivery) {
        (Some(id), None) => ("j.id=?1", vec![Box::new(id.to_string())]),
        (None, Some((delivery_id, revision))) => (
            "j.delivery_id=?1 AND j.delivery_revision=?2",
            vec![Box::new(delivery_id.to_string()), Box::new(revision)],
        ),
        _ => return Err(rusqlite::Error::InvalidParameterName("lookup".into())),
    };
    let sql = format!(
        "SELECT j.id,j.implementation_attempt_id,j.delivery_id,j.delivery_revision,
                j.subject_json,j.status,j.reviewer_attempt_id,j.outcome_json,
                j.created_at,j.updated_at,j.started_at,j.ended_at
         FROM delegation_review_jobs j WHERE {predicate}"
    );
    conn.query_row(&sql, rusqlite::params_from_iter(values.iter()), map_job_row)
}

fn map_job_row(row: &rusqlite::Row<'_>) -> Result<ReviewJob, rusqlite::Error> {
    let status: String = row.get(5)?;
    let outcome_json: Option<String> = row.get(7)?;
    Ok(ReviewJob {
        id: row.get(0)?,
        implementation_attempt_id: row.get(1)?,
        delivery_id: row.get(2)?,
        delivery_revision: row.get(3)?,
        subject: serde_json::from_str(&row.get::<_, String>(4)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        status: ReviewJobStatus::parse(&status).ok_or(rusqlite::Error::InvalidQuery)?,
        reviewer_attempt_id: row.get(6)?,
        outcome: outcome_json
            .map(|value| serde_json::from_str(&value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        started_at: row.get(10)?,
        ended_at: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrate;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    fn setup() -> (SharedDb, ReviewSubject) {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO sessions(id,title,created_at,updated_at) VALUES ('s','s',1,1);
             INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('m','s','user','m',1);
             INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES ('r','s','m','g','running','[]',1,1);
             INSERT INTO work_packages(id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,candidate_version,created_at,updated_at,status) VALUES ('wp','s',1,'wk','explore','explorer',1,'scope','approved',1000,0,1,1,'active');
             INSERT INTO delegations(id,session_id,message_id,parent_run_id,work_package_id,objective,brief_json,status,state_version,created_at,updated_at) VALUES ('d','s','m','r','wp','g','{}','running',0,1,1);
             INSERT INTO delegation_attempts(id,delegation_id,attempt_number,status,sandbox_ref,created_at,started_at) VALUES ('a','d',1,'running','sandbox://a',1,1);
             INSERT INTO delegation_deliveries(id,delegation_id,attempt_id,delivery_version,schema_version,payload_json,created_at) VALUES ('delivery','d','a',1,1,'{}',1);",
        ).unwrap();
        (
            SharedDb::from_arc(Arc::new(Mutex::new(conn))),
            ReviewSubject {
                delivery_id: "delivery".into(),
                implementation_attempt_id: "a".into(),
                task_shape: "explore".into(),
                worker_profile: "explorer".into(),
                goal: "g".into(),
                summary: "done".into(),
                key_facts: vec![],
                milestones: vec![],
                verifications: vec![],
                artifact_refs: vec!["evidence://one".into()],
                risks: vec![],
                open_questions: vec![],
            },
        )
    }

    #[test]
    fn enqueue_is_idempotent_and_claim_requires_unique_job() {
        let (db, subject) = setup();
        let store = ReviewJobStore::new(db);
        let first = store
            .enqueue("job", "a", "delivery", 1, &subject, 2)
            .unwrap();
        let again = store
            .enqueue("job", "a", "delivery", 1, &subject, 3)
            .unwrap();
        assert_eq!(first.id, again.id);
        assert_eq!(
            store.claim("job", "review-a", 4).unwrap().status,
            ReviewJobStatus::Running
        );
        assert_eq!(
            store.claim("job", "review-b", 5),
            Err(ReviewStoreError::InvalidTransition)
        );
        assert_eq!(store.requeue_running(6).unwrap(), 1);
        let recovered = store.list_queued().unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].reviewer_attempt_id, None);
        assert_eq!(
            store.claim("job", "review-b", 7).unwrap().status,
            ReviewJobStatus::Running
        );
    }

    #[test]
    fn outcome_is_only_recorded_for_the_claiming_reviewer() {
        let (db, subject) = setup();
        let store = ReviewJobStore::new(db);
        store
            .enqueue("job", "a", "delivery", 1, &subject, 2)
            .unwrap();
        store.claim("job", "review-a", 3).unwrap();
        let outcome = ReviewOutcome {
            verdict: super::super::review_contract::SemanticReviewVerdict::Passed,
            summary: "looks complete".into(),
            findings: vec![],
            missing_evidence: vec![],
            evidence_refs: vec!["evidence://review".into()],
        };
        assert_eq!(
            store.record_outcome("job", "review-b", &outcome, 4),
            Err(ReviewStoreError::InvalidTransition)
        );
        assert_eq!(
            store
                .record_outcome("job", "review-a", &outcome, 5)
                .unwrap()
                .status,
            ReviewJobStatus::Passed
        );
    }
}
