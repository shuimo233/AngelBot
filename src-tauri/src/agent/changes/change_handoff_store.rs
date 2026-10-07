//! Durable state for the Main-Agent-owned Change hand-off.
//!
//! This store deliberately records only identities and immutable artifact facts.
//! Worktree paths, model transcripts and raw worker output remain owned by their
//! respective providers and are never reintroduced at this seam.

use rusqlite::{params, OptionalExtension};

use super::shared_db::SharedDb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeHandoffStatus {
    Preparing,
    Ready,
    ConfirmationPending,
    Materialized,
    NeedsDecision,
    Cancelled,
}

impl ChangeHandoffStatus {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "preparing" => Some(Self::Preparing),
            "ready" => Some(Self::Ready),
            "confirmation_pending" => Some(Self::ConfirmationPending),
            "materialized" => Some(Self::Materialized),
            "needs_decision" => Some(Self::NeedsDecision),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Ready => "ready",
            Self::ConfirmationPending => "confirmation_pending",
            Self::Materialized => "materialized",
            Self::NeedsDecision => "needs_decision",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeHandoff {
    pub delivery_id: String,
    pub delivery_revision: i64,
    pub implementation_attempt_id: String,
    pub work_package_id: String,
    pub semantic_review_job_id: String,
    pub status: ChangeHandoffStatus,
    pub artifact_id: Option<String>,
    pub change_set_id: Option<String>,
    pub base_revision: Option<String>,
    pub confirmation_id: Option<String>,
    pub materialization_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ChangeHandoffStoreError {
    #[error("change handoff storage failed")]
    Storage,
    #[error("change handoff transition is invalid")]
    InvalidTransition,
}

#[derive(Clone)]
pub struct ChangeHandoffStore {
    db: SharedDb,
}

impl ChangeHandoffStore {
    pub fn new(db: SharedDb) -> Self {
        Self { db }
    }

    pub fn begin(
        &self,
        delivery_id: &str,
        delivery_revision: i64,
        implementation_attempt_id: &str,
        work_package_id: &str,
        semantic_review_job_id: &str,
        now: i64,
    ) -> Result<ChangeHandoff, ChangeHandoffStoreError> {
        self.db
            .with_conn_mut(|conn| {
                conn.execute(
                    "INSERT OR IGNORE INTO delegation_change_handoffs (
                        delivery_id,delivery_revision,implementation_attempt_id,work_package_id,
                        semantic_review_job_id,status,created_at,updated_at
                     ) VALUES (?1,?2,?3,?4,?5,'preparing',?6,?6)",
                    params![
                        delivery_id,
                        delivery_revision,
                        implementation_attempt_id,
                        work_package_id,
                        semantic_review_job_id,
                        now
                    ],
                )
                .map_err(|_| ChangeHandoffStoreError::Storage)?;
                Self::load_from(conn, delivery_id)
            })
            .map_err(|_| ChangeHandoffStoreError::Storage)?
    }

    pub fn load(
        &self,
        delivery_id: &str,
    ) -> Result<Option<ChangeHandoff>, ChangeHandoffStoreError> {
        self.db
            .with_conn(|conn| Self::load_from_optional(conn, delivery_id))
            .map_err(|_| ChangeHandoffStoreError::Storage)?
    }

    pub fn mark_ready(
        &self,
        delivery_id: &str,
        artifact_id: &str,
        change_set_id: &str,
        base_revision: &str,
        now: i64,
    ) -> Result<ChangeHandoff, ChangeHandoffStoreError> {
        self.transition(
            delivery_id,
            ChangeHandoffStatus::Preparing,
            ChangeHandoffStatus::Ready,
            artifact_id,
            change_set_id,
            base_revision,
            None,
            None,
            now,
        )
    }

    pub fn mark_confirmation_pending(
        &self,
        delivery_id: &str,
        confirmation_id: &str,
        now: i64,
    ) -> Result<ChangeHandoff, ChangeHandoffStoreError> {
        self.transition(
            delivery_id,
            ChangeHandoffStatus::Ready,
            ChangeHandoffStatus::ConfirmationPending,
            "",
            "",
            "",
            Some(confirmation_id),
            None,
            now,
        )
    }

    pub fn mark_materialized(
        &self,
        delivery_id: &str,
        materialization_id: &str,
        now: i64,
    ) -> Result<ChangeHandoff, ChangeHandoffStoreError> {
        self.transition(
            delivery_id,
            ChangeHandoffStatus::ConfirmationPending,
            ChangeHandoffStatus::Materialized,
            "",
            "",
            "",
            None,
            Some(materialization_id),
            now,
        )
    }

    pub fn mark_needs_decision(
        &self,
        delivery_id: &str,
        now: i64,
    ) -> Result<(), ChangeHandoffStoreError> {
        self.db
            .with_conn_mut(|conn| {
                let changed = conn
                    .execute(
                        "UPDATE delegation_change_handoffs
                         SET status='needs_decision', updated_at=?2
                         WHERE delivery_id=?1 AND status='preparing'",
                        params![delivery_id, now],
                    )
                    .map_err(|_| ChangeHandoffStoreError::Storage)?;
                if changed == 1 {
                    Ok(())
                } else {
                    Err(ChangeHandoffStoreError::InvalidTransition)
                }
            })
            .map_err(|_| ChangeHandoffStoreError::Storage)?
    }

    pub fn mark_cancelled(
        &self,
        delivery_id: &str,
        now: i64,
    ) -> Result<(), ChangeHandoffStoreError> {
        self.db
            .with_conn_mut(|conn| {
                let changed = conn
                    .execute(
                        "UPDATE delegation_change_handoffs
                         SET status='cancelled', updated_at=?2
                         WHERE delivery_id=?1 AND status IN ('preparing','ready','confirmation_pending')",
                        params![delivery_id, now],
                    )
                    .map_err(|_| ChangeHandoffStoreError::Storage)?;
                if changed == 1 {
                    Ok(())
                } else {
                    Err(ChangeHandoffStoreError::InvalidTransition)
                }
            })
            .map_err(|_| ChangeHandoffStoreError::Storage)?
    }

    #[allow(clippy::too_many_arguments)]
    fn transition(
        &self,
        delivery_id: &str,
        from: ChangeHandoffStatus,
        to: ChangeHandoffStatus,
        artifact_id: &str,
        change_set_id: &str,
        base_revision: &str,
        confirmation_id: Option<&str>,
        materialization_id: Option<&str>,
        now: i64,
    ) -> Result<ChangeHandoff, ChangeHandoffStoreError> {
        self.db
            .with_conn_mut(|conn| {
                let changed = conn
                    .execute(
                        "UPDATE delegation_change_handoffs SET
                             status=?2,
                             artifact_id=CASE WHEN ?3='' THEN artifact_id ELSE ?3 END,
                             change_set_id=CASE WHEN ?4='' THEN change_set_id ELSE ?4 END,
                             base_revision=CASE WHEN ?5='' THEN base_revision ELSE ?5 END,
                             confirmation_id=COALESCE(?6, confirmation_id),
                             materialization_id=COALESCE(?7, materialization_id),
                             updated_at=?8
                         WHERE delivery_id=?1 AND status=?9",
                        params![
                            delivery_id,
                            to.as_str(),
                            artifact_id,
                            change_set_id,
                            base_revision,
                            confirmation_id,
                            materialization_id,
                            now,
                            from.as_str(),
                        ],
                    )
                    .map_err(|_| ChangeHandoffStoreError::Storage)?;
                if changed != 1 {
                    return Err(ChangeHandoffStoreError::InvalidTransition);
                }
                Self::load_from(conn, delivery_id)
            })
            .map_err(|_| ChangeHandoffStoreError::Storage)?
    }

    fn load_from(
        conn: &rusqlite::Connection,
        delivery_id: &str,
    ) -> Result<ChangeHandoff, ChangeHandoffStoreError> {
        Self::load_from_optional(conn, delivery_id)?.ok_or(ChangeHandoffStoreError::Storage)
    }

    fn load_from_optional(
        conn: &rusqlite::Connection,
        delivery_id: &str,
    ) -> Result<Option<ChangeHandoff>, ChangeHandoffStoreError> {
        conn.query_row(
            "SELECT delivery_id,delivery_revision,implementation_attempt_id,work_package_id,
                    semantic_review_job_id,status,artifact_id,change_set_id,base_revision,
                    confirmation_id,materialization_id
             FROM delegation_change_handoffs WHERE delivery_id=?1",
            [delivery_id],
            |row| {
                let status: String = row.get(5)?;
                Ok(ChangeHandoff {
                    delivery_id: row.get(0)?,
                    delivery_revision: row.get(1)?,
                    implementation_attempt_id: row.get(2)?,
                    work_package_id: row.get(3)?,
                    semantic_review_job_id: row.get(4)?,
                    status: ChangeHandoffStatus::parse(&status)
                        .ok_or(rusqlite::Error::InvalidQuery)?,
                    artifact_id: row.get(6)?,
                    change_set_id: row.get(7)?,
                    base_revision: row.get(8)?,
                    confirmation_id: row.get(9)?,
                    materialization_id: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(|_| ChangeHandoffStoreError::Storage)
    }
}
