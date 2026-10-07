//! Transitional adapter from durable delegation records to Workspace activity.
//!
//! This is deliberately an adapter, not another delegation state machine.
//! The Supervisor owns the Workspace-facing interface; this module reads the
//! existing delegation/delivery records until they are fully migrated.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::agent::delegation_contract::{
    ContractLimits, DelegationBrief, DelegationDelivery, ParentContext, ParentContextAssembler,
};

const MAX_DELEGATIONS: i64 = 12;
const MAX_FACTS: usize = 4;
const MAX_TEXT_CHARS: usize = 240;
const MAX_QUESTIONS: usize = 3;
const MAX_RISKS: usize = 3;
// Explorer keeps this reference in the durable delivery to prove which plan
// produced the result. It is not an external source a person can inspect, so
// the compact Workspace activity count must not present it as user evidence.
const INTERNAL_EXPLORER_PLAN_EVIDENCE_PREFIX: &str = "evidence://explorer/plan/";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ForegroundAgentProjection {
    pub session_id: String,
    pub cursor: i64,
    pub delegations: Vec<DelegationProjection>,
    pub pending_decisions: Vec<PendingDecisionProjection>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DelegationProjection {
    pub id: String,
    pub goal: String,
    pub status: String,
    pub summary: Option<String>,
    pub key_facts: Vec<String>,
    pub milestones_completed: usize,
    pub milestones_total: usize,
    pub verifications_passed: usize,
    pub verifications_total: usize,
    pub evidence_count: usize,
    /// Bounded worker stage. This is a progress label, never raw tool output.
    pub activity: Option<String>,
    /// Independent semantic-review status, when the producer has delivered.
    pub reviewer_status: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PendingDecisionProjection {
    pub delegation_id: String,
    pub goal: String,
    pub questions: Vec<String>,
    pub risks: Vec<String>,
    pub updated_at: i64,
}

pub fn get_for_session(
    conn: &Connection,
    session_id: &str,
) -> Result<ForegroundAgentProjection, String> {
    let session_id = bounded_session_id(session_id)?;
    let mut statement = conn
        .prepare(
            "SELECT id, objective, status, brief_json, updated_at
             FROM delegations
             WHERE session_id = ?1
             ORDER BY updated_at DESC
             LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![session_id, MAX_DELEGATIONS], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|error| error.to_string())?;

    let mut delegations = Vec::new();
    let mut pending_decisions = Vec::new();
    let mut cursor = 0_i64;
    for row in rows {
        let (id, objective, status, brief_json, updated_at) =
            row.map_err(|error| error.to_string())?;
        cursor = cursor.max(updated_at);
        let latest = latest_delivery(conn, &id)?;
        let heartbeat = latest_heartbeat(conn, &id)?;
        let reviewer = latest_reviewer_status(conn, &id)?;
        let activity = heartbeat
            .as_ref()
            .map(|value| activity_label(&value.stage))
            .or_else(|| (status == "queued").then(|| "等待分配".to_owned()))
            .or_else(|| (status == "running").then(|| "正在执行".to_owned()));
        let progress_updated_at = heartbeat
            .as_ref()
            .map(|value| value.created_at)
            .unwrap_or(updated_at)
            .max(
                reviewer
                    .as_ref()
                    .map(|value| value.updated_at)
                    .unwrap_or(updated_at),
            );
        cursor = cursor.max(progress_updated_at);
        let context = latest
            .as_ref()
            .and_then(|payload| decode_parent_context(&brief_json, payload));

        let projection = DelegationProjection {
            id: id.clone(),
            goal: bounded_text(&objective),
            status: status.clone(),
            summary: context.as_ref().map(|value| bounded_text(&value.summary)),
            key_facts: context
                .as_ref()
                .map(|value| {
                    value
                        .facts
                        .iter()
                        .take(MAX_FACTS)
                        .map(|fact| bounded_text(&fact.statement))
                        .collect()
                })
                .unwrap_or_default(),
            milestones_completed: context
                .as_ref()
                .map(|value| {
                    value
                        .milestones
                        .iter()
                        .filter(|milestone| !milestone.outcome.trim().is_empty())
                        .count()
                })
                .unwrap_or_default(),
            milestones_total: context
                .as_ref()
                .map(|value| value.milestones.len())
                .unwrap_or_default(),
            verifications_passed: context
                .as_ref()
                .map(|value| {
                    value
                        .verifications
                        .iter()
                        .filter(|verification| {
                            matches!(
                                verification.status,
                                crate::agent::delegation_contract::VerificationStatus::Passed
                            )
                        })
                        .count()
                })
                .unwrap_or_default(),
            verifications_total: context
                .as_ref()
                .map(|value| value.verifications.len())
                .unwrap_or_default(),
            evidence_count: context
                .as_ref()
                .map(|value| user_visible_evidence_count(&value.evidence_refs))
                .unwrap_or_default(),
            activity,
            reviewer_status: reviewer.map(|value| value.status),
            updated_at: progress_updated_at,
        };

        if status == "needs_decision"
            || context
                .as_ref()
                .is_some_and(|value| !value.open_questions.is_empty())
        {
            let questions = context
                .as_ref()
                .map(|value| {
                    value
                        .open_questions
                        .iter()
                        .take(MAX_QUESTIONS)
                        .map(|question| bounded_text(&question.question))
                        .collect()
                })
                .unwrap_or_default();
            let risks = context
                .as_ref()
                .map(|value| {
                    value
                        .risks
                        .iter()
                        .take(MAX_RISKS)
                        .map(|risk| bounded_text(risk))
                        .collect()
                })
                .unwrap_or_default();
            pending_decisions.push(PendingDecisionProjection {
                delegation_id: id,
                goal: bounded_text(&objective),
                questions,
                risks,
                updated_at,
            });
        }

        delegations.push(projection);
    }

    Ok(ForegroundAgentProjection {
        session_id: session_id.to_owned(),
        cursor,
        delegations,
        pending_decisions,
    })
}

#[derive(Debug, Clone)]
struct HeartbeatProjection {
    stage: String,
    created_at: i64,
}

#[derive(Debug, Clone)]
struct ReviewerProjection {
    status: String,
    updated_at: i64,
}

fn latest_heartbeat(
    conn: &Connection,
    delegation_id: &str,
) -> Result<Option<HeartbeatProjection>, String> {
    if !table_exists(conn, "delegation_attempt_events")?
        || !table_exists(conn, "delegation_attempts")?
    {
        return Ok(None);
    }
    let row = conn
        .query_row(
            "SELECT e.payload_json, e.created_at
             FROM delegation_attempt_events e
             JOIN delegation_attempts a ON a.id = e.attempt_id
             WHERE a.delegation_id = ?1 AND e.event_type = 'heartbeat'
             ORDER BY e.created_at DESC, e.sequence DESC
             LIMIT 1",
            params![delegation_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((payload, created_at)) = row else {
        return Ok(None);
    };
    let value = serde_json::from_str::<serde_json::Value>(&payload).ok();
    let stage = value
        .as_ref()
        .and_then(|value| value.get("stage"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Ok(stage.map(|stage| HeartbeatProjection { stage, created_at }))
}

fn latest_reviewer_status(
    conn: &Connection,
    delegation_id: &str,
) -> Result<Option<ReviewerProjection>, String> {
    if !table_exists(conn, "delegation_review_jobs")? {
        return Ok(None);
    }
    conn.query_row(
        "SELECT j.status, j.updated_at FROM delegation_review_jobs j
         JOIN delegation_deliveries d ON d.id = j.delivery_id
         WHERE d.delegation_id = ?1 ORDER BY j.updated_at DESC, j.id DESC LIMIT 1",
        params![delegation_id],
        |row| {
            Ok(ReviewerProjection {
                status: row.get(0)?,
                updated_at: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(|error| error.to_string())
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        params![name],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(|error| error.to_string())
}

fn latest_delivery(conn: &Connection, delegation_id: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT payload_json
         FROM delegation_deliveries
         WHERE delegation_id = ?1
         ORDER BY created_at DESC, delivery_version DESC
         LIMIT 1",
        params![delegation_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|error| error.to_string())
}

fn decode_parent_context(brief_json: &str, payload_json: &str) -> Option<ParentContext> {
    let brief = serde_json::from_str::<DelegationBrief>(brief_json).ok()?;
    let delivery = serde_json::from_str::<DelegationDelivery>(payload_json).ok()?;
    ParentContextAssembler::assemble(&delivery, &brief, &ContractLimits::default()).ok()
}

fn bounded_session_id(value: &str) -> Result<&str, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 200 || trimmed.contains(['\r', '\n', '\0']) {
        return Err("invalid session id".to_owned());
    }
    Ok(trimmed)
}

fn bounded_text(value: &str) -> String {
    value.chars().take(MAX_TEXT_CHARS).collect()
}

/// Only project a small, Main-Agent-owned vocabulary from worker heartbeats.
/// Heartbeats prove liveness; their raw `stage` field is not a user-facing
/// transcript and must not become an accidental child-agent log surface.
fn activity_label(stage: &str) -> String {
    match stage.trim() {
        "explore" | "explorer" => "正在分析".to_owned(),
        "delivery" => "正在整理结果".to_owned(),
        "model" => "正在处理".to_owned(),
        _ => "正在执行".to_owned(),
    }
}

fn user_visible_evidence_count(evidence_refs: &[String]) -> usize {
    evidence_refs
        .iter()
        .filter(|evidence_ref| !evidence_ref.starts_with(INTERNAL_EXPLORER_PLAN_EVIDENCE_PREFIX))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_is_session_scoped_and_bounded() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"CREATE TABLE delegations (
                id TEXT PRIMARY KEY, session_id TEXT, objective TEXT,
                status TEXT, brief_json TEXT, updated_at INTEGER
             );
             CREATE TABLE delegation_deliveries (
                id TEXT, delegation_id TEXT, payload_json TEXT,
                created_at INTEGER, delivery_version INTEGER
             );
             CREATE TABLE delegation_attempts (id TEXT, delegation_id TEXT);
             CREATE TABLE delegation_attempt_events (
                attempt_id TEXT, sequence INTEGER, event_type TEXT, payload_json TEXT, created_at INTEGER
             );
             CREATE TABLE delegation_review_jobs (
                id TEXT, delivery_id TEXT, status TEXT, updated_at INTEGER
             );
             INSERT INTO delegations VALUES
                ('d1', 's1', 'first', 'running', '{}', 10),
                ('d2', 's2', 'other', 'completed', '{}', 20);
             INSERT INTO delegation_attempts VALUES ('a1', 'd1');
             INSERT INTO delegation_attempt_events VALUES
                ('a1', 1, 'heartbeat', '{"stage":"检查结构；内部工具日志不应显示"}', 15);
             INSERT INTO delegation_deliveries VALUES ('delivery-1', 'd1', '{}', 12, 1);
             INSERT INTO delegation_review_jobs VALUES ('review-1', 'delivery-1', 'running', 16);"#,
        )
        .unwrap();

        let projection = get_for_session(&conn, "s1").unwrap();
        assert_eq!(projection.session_id, "s1");
        assert_eq!(projection.delegations.len(), 1);
        assert_eq!(projection.delegations[0].id, "d1");
        assert_eq!(
            projection.delegations[0].activity.as_deref(),
            Some("正在执行")
        );
        assert_eq!(
            projection.delegations[0].reviewer_status.as_deref(),
            Some("running")
        );
        assert_eq!(projection.cursor, 16);
    }

    #[test]
    fn user_visible_evidence_excludes_internal_explorer_plan_reference() {
        let evidence_refs = vec![
            "evidence://explorer/plan/immutable-plan-digest".to_owned(),
            "evidence://network/search/result-digest/0".to_owned(),
        ];

        assert_eq!(user_visible_evidence_count(&evidence_refs), 1);
    }
}
