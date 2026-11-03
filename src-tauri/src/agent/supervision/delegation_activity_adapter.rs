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

/// Host facts only: no worker/model diagnostics or synthetic delivery.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RuntimeFailureProjection {
    pub delegation_id: String,
    pub attempt_id: String,
    pub reason: &'static str,
    pub summary: &'static str,
    pub next_step: &'static str,
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
        let materialization = unresolved_materialization(conn, &id)?;
        let failure = if materialization.is_some() {
            materialization
        } else if status == "needs_decision" {
            latest_runtime_failure(conn, &id)?
        } else {
            None
        };
        let activity = matches!(status.as_str(), "queued" | "running")
            .then(|| {
                heartbeat
                    .as_ref()
                    .map(|value| activity_label(&value.stage))
                    .or_else(|| (status == "queued").then(|| "等待分配".to_owned()))
                    .or_else(|| (status == "running").then(|| "正在执行".to_owned()))
            })
            .flatten();
        let progress_updated_at = heartbeat
            .as_ref()
            .map(|value| value.created_at)
            .unwrap_or(updated_at)
            .max(
                reviewer
                    .as_ref()
                    .map(|value| value.updated_at)
                    .unwrap_or(updated_at),
            )
            .max(updated_at)
            .max(failure.as_ref().map(|value| value.updated_at).unwrap_or(0));
        cursor = cursor.max(progress_updated_at);
        let context = latest
            .as_ref()
            .and_then(|payload| decode_parent_context(&brief_json, payload));

        let projection = DelegationProjection {
            id: id.clone(),
            goal: bounded_text(&objective),
            status: if failure
                .as_ref()
                .is_some_and(|value| value.reason == "materialization_unknown")
            {
                "needs_decision".into()
            } else {
                status.clone()
            },
            summary: failure
                .as_ref()
                .filter(|value| value.reason == "materialization_unknown")
                .map(|value| value.summary.to_owned())
                .or_else(|| {
                    context
                        .as_ref()
                        .map(|value| bounded_text(&value.summary))
                        .or_else(|| failure.as_ref().map(|value| value.summary.to_owned()))
                }),
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
            || failure.is_some()
            || context
                .as_ref()
                .is_some_and(|value| !value.open_questions.is_empty())
        {
            let questions = if let Some(value) = failure
                .as_ref()
                .filter(|value| value.reason == "materialization_unknown")
            {
                vec![value.summary.to_owned(), value.next_step.to_owned()]
            } else {
                context
                    .as_ref()
                    .map(|value| {
                        value
                            .open_questions
                            .iter()
                            .take(MAX_QUESTIONS)
                            .map(|question| bounded_text(&question.question))
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        failure
                            .as_ref()
                            .map(|value| vec![value.summary.to_owned(), value.next_step.to_owned()])
                            .unwrap_or_default()
                    })
            };
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
                updated_at: progress_updated_at,
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

/// The foreground assembler reuses exactly the same safe failure facts as
/// the Main conversation's activity projection, bound to its current session.
pub(crate) fn runtime_failures_for_session(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<RuntimeFailureProjection>, String> {
    let session_id = bounded_session_id(session_id)?;
    let mut statement = conn
        .prepare(
            "SELECT id FROM delegations WHERE session_id=?1 AND (status='needs_decision'
             OR EXISTS(SELECT 1 FROM delegation_materialization_receipts receipt
                       WHERE receipt.work_package_id=delegations.work_package_id AND receipt.status='unknown'))
         ORDER BY updated_at DESC, id LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let ids = statement
        .query_map(params![session_id, MAX_DELEGATIONS], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| error.to_string())?;
    let mut failures = Vec::new();
    for id in ids {
        let id = id.map_err(|error| error.to_string())?;
        if let Some(failure) =
            unresolved_materialization(conn, &id)?.or(latest_runtime_failure(conn, &id)?)
        {
            failures.push(failure);
        }
    }
    Ok(failures)
}

fn unresolved_materialization(
    conn: &Connection,
    delegation_id: &str,
) -> Result<Option<RuntimeFailureProjection>, String> {
    conn.query_row(
        "SELECT a.id,receipt.updated_at FROM delegations d
         JOIN delegation_materialization_receipts receipt ON receipt.work_package_id=d.work_package_id
         JOIN delegation_attempts a ON a.delegation_id=d.id
         WHERE d.id=?1 AND receipt.status='unknown'
           AND a.id=(SELECT id FROM delegation_attempts WHERE delegation_id=d.id ORDER BY attempt_number DESC LIMIT 1)
         ORDER BY receipt.updated_at DESC LIMIT 1", [delegation_id],
        |row| Ok(RuntimeFailureProjection {
            delegation_id: delegation_id.to_owned(), attempt_id: row.get(0)?,
            reason: "materialization_unknown",
            summary: "正式项目写入结果尚未核验，可能已写入部分或全部修改；不能把子代理交付视为正式落盘成功。",
            next_step: "请先检查项目当前文件与 Git 状态，再决定如何处理；没有自动重试或撤销，候选与恢复记录已保留。",
            updated_at: row.get(1)?,
        }),
    ).optional().map_err(|error| error.to_string())
}

fn latest_runtime_failure(
    conn: &Connection,
    delegation_id: &str,
) -> Result<Option<RuntimeFailureProjection>, String> {
    let row = conn.query_row(
        "SELECT a.id, e.event_type, e.payload_json, e.created_at
         FROM delegation_attempts a JOIN delegation_attempt_events e ON e.attempt_id=a.id
         WHERE a.delegation_id=?1 AND a.status IN ('sealed','failed')
           AND a.id=(SELECT id FROM delegation_attempts WHERE delegation_id=?1
                     ORDER BY attempt_number DESC LIMIT 1)
           AND NOT EXISTS (SELECT 1 FROM delegation_deliveries WHERE attempt_id=a.id)
           AND e.event_type IN ('worker_start_rejected','worker_failed','lease_expired','attempt_policy_ineligible','recovery_unknown_effects')
         ORDER BY e.sequence DESC LIMIT 1",
        [delegation_id],
        |row| Ok((row.get::<_,String>(0)?, row.get::<_,String>(1)?, row.get::<_,String>(2)?, row.get::<_,i64>(3)?)),
    ).optional().map_err(|error| error.to_string())?;
    let Some((attempt_id, event, payload, updated_at)) = row else {
        return Ok(None);
    };
    let class = if event == "worker_start_rejected" && payload.len() <= 8192 {
        serde_json::from_str::<serde_json::Value>(&payload)
            .ok()
            .and_then(|value| {
                value
                    .get("class")
                    .and_then(|class| class.as_str())
                    .map(str::to_owned)
            })
    } else {
        None
    };
    let (reason, summary) = match event.as_str() {
        "worker_start_rejected" => match class.as_deref() {
            Some("model_unavailable") => (
                "model_unavailable",
                "委派未能启动：此次任务的模型连接不可用，尚未形成可交付结果。",
            ),
            Some("worktree_unavailable") => (
                "worktree_unavailable",
                "委派未能启动：隔离工作目录不可用，尚未形成可交付结果。",
            ),
            Some("binding_unavailable") => (
                "binding_unavailable",
                "委派未能启动：任务配置或授权已失效，尚未形成可交付结果。",
            ),
            Some("host_capability_denied") => (
                "host_capability_denied",
                "委派未能启动：所需能力未获授权，尚未形成可交付结果。",
            ),
            Some("host_unavailable") => (
                "host_unavailable",
                "委派未能启动：执行环境暂不可用，尚未形成可交付结果。",
            ),
            _ => ("launch_unavailable", "委派未能启动，尚未形成可交付结果。"),
        },
        "worker_failed" => (
            "execution_ended",
            "委派执行在提交结果前结束，目前没有可交付结论；任务完成情况尚未核验。",
        ),
        "lease_expired" => (
            "lease_expired",
            "委派授权已到期，目前没有可交付结论；任务完成情况尚未核验。",
        ),
        "recovery_unknown_effects" => (
            "execution_interrupted",
            "应用在委派提交结果前中断，执行尝试已停止；实际操作结果与任务完成情况尚未核验。",
        ),
        _ => (
            "unclassified_stop",
            "委派执行已中止，目前没有可交付结论；具体原因需要进一步核对。",
        ),
    };
    Ok(Some(RuntimeFailureProjection {
        delegation_id: delegation_id.to_owned(),
        attempt_id,
        reason,
        summary,
        next_step: if event == "recovery_unknown_effects" {
            "请先核对已有操作结果，再决定是否重新派发；没有自动重试。"
        } else {
            "请核对任务状态与相关配置后，再决定是否重新派发；没有自动重试。"
        },
        updated_at,
    }))
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
               AND a.id=(SELECT id FROM delegation_attempts WHERE delegation_id=?1 ORDER BY attempt_number DESC LIMIT 1)
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
         WHERE d.delegation_id = ?1
           AND d.attempt_id=(SELECT id FROM delegation_attempts WHERE delegation_id=?1 ORDER BY attempt_number DESC LIMIT 1)
           AND d.delivery_version=(SELECT MAX(delivery_version) FROM delegation_deliveries WHERE attempt_id=d.attempt_id)
         ORDER BY j.updated_at DESC, j.id DESC LIMIT 1",
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
           AND attempt_id=(SELECT id FROM delegation_attempts WHERE delegation_id=?1 ORDER BY attempt_number DESC LIMIT 1)
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
                created_at INTEGER, delivery_version INTEGER, attempt_id TEXT
             );
             CREATE TABLE delegation_attempts (
                id TEXT, delegation_id TEXT, attempt_number INTEGER DEFAULT 1,
                status TEXT DEFAULT 'running'
             );
             CREATE TABLE delegation_attempt_events (
                attempt_id TEXT, sequence INTEGER, event_type TEXT, payload_json TEXT, created_at INTEGER
             );
             CREATE TABLE delegation_review_jobs (
                id TEXT, delivery_id TEXT, status TEXT, updated_at INTEGER
             );
             INSERT INTO delegations VALUES
                ('d1', 's1', 'first', 'running', '{}', 10),
                ('d2', 's2', 'other', 'completed', '{}', 20);
             INSERT INTO delegation_attempts (id, delegation_id) VALUES ('a1', 'd1');
             INSERT INTO delegation_attempt_events VALUES
                ('a1', 1, 'heartbeat', '{"stage":"检查结构；内部工具日志不应显示"}', 15);
             INSERT INTO delegation_deliveries VALUES ('delivery-1', 'd1', '{}', 12, 1, 'a1');
             INSERT INTO delegation_review_jobs VALUES ('review-1', 'delivery-1', 'running', 16);
             ALTER TABLE delegations ADD COLUMN work_package_id TEXT;
             UPDATE delegations SET work_package_id='p1' WHERE id='d1';
             CREATE TABLE delegation_materialization_receipts (work_package_id TEXT,status TEXT,updated_at INTEGER);"#,
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

        conn.execute(
            "INSERT INTO delegation_deliveries VALUES('delivery-2','d1','{}',17,2,'a1')",
            [],
        )
        .unwrap();
        assert!(
            get_for_session(&conn, "s1").unwrap().delegations[0]
                .reviewer_status
                .is_none(),
            "a new delivery without a review must not inherit the old revision's reviewer"
        );
        conn.execute_batch("INSERT INTO delegation_review_jobs VALUES('review-2','delivery-2','running',18);
            UPDATE delegation_review_jobs SET status='cancelled',updated_at=1000 WHERE id='review-1';").unwrap();
        assert_eq!(
            get_for_session(&conn, "s1").unwrap().delegations[0]
                .reviewer_status
                .as_deref(),
            Some("running"),
            "late cancellation of an older revision must not hide the current reviewer"
        );

        conn.execute_batch(
            "DELETE FROM delegation_deliveries;
             UPDATE delegations SET status='needs_decision', updated_at=20 WHERE id='d1';
             UPDATE delegation_attempts SET status='sealed' WHERE id='a1';
             INSERT INTO delegation_attempt_events VALUES
                ('a1', 2, 'worker_start_rejected', '{\"class\":\"model_unavailable\",\"raw\":\"PRIVATE_CANARY\"}', 20),
                ('a1', 3, 'worker_start_failed', '{}', 20);",
        ).unwrap();
        let failed = get_for_session(&conn, "s1").unwrap();
        assert_eq!(
            failed.delegations[0].activity, None,
            "a terminal task must not look active"
        );
        assert!(failed.delegations[0]
            .summary
            .as_deref()
            .unwrap_or("")
            .contains("模型"));
        assert!(!failed.pending_decisions[0].questions.is_empty());
        assert!(!serde_json::to_string(&failed)
            .unwrap()
            .contains("PRIVATE_CANARY"));
        assert_eq!(failed.cursor, 20);
        assert_eq!(runtime_failures_for_session(&conn, "s2").unwrap().len(), 0);
        for (class, expected) in [
            ("worktree_unavailable", "隔离工作目录"),
            ("binding_unavailable", "配置或授权"),
            ("host_capability_denied", "未获授权"),
            ("host_unavailable", "执行环境"),
            ("PRIVATE_CANARY", "未能启动"),
        ] {
            conn.execute("UPDATE delegation_attempt_events SET payload_json=?1 WHERE event_type='worker_start_rejected'",
                [serde_json::json!({"class":class}).to_string()]).unwrap();
            let facts = runtime_failures_for_session(&conn, "s1").unwrap();
            assert_eq!(facts.len(), 1);
            assert!(facts[0].summary.contains(expected));
            assert!(!serde_json::to_string(&facts)
                .unwrap()
                .contains("PRIVATE_CANARY"));
        }
        // Older workers shared this event with policy rejection. Its name
        // alone cannot prove the cause, even when the attempt is terminal.
        conn.execute_batch("INSERT INTO delegation_attempt_events VALUES ('a1',4,'attempt_policy_ineligible','{}',20);").unwrap();
        let legacy = runtime_failures_for_session(&conn, "s1").unwrap();
        assert_eq!(legacy[0].reason, "unclassified_stop");
        assert!(legacy[0].summary.contains("原因需要进一步核对"));
        conn.execute_batch("INSERT INTO delegation_attempt_events VALUES
            ('a1',5,'recovery_unknown_effects','{\"reason\":\"Recovery\",\"raw\":\"PRIVATE_CANARY\"}',21);").unwrap();
        let recovered = runtime_failures_for_session(&conn, "s1").unwrap();
        assert_eq!(recovered[0].reason, "execution_interrupted");
        assert!(recovered[0].summary.contains("中断"));
        assert!(recovered[0].summary.contains("尚未核验"));
        assert!(recovered[0].next_step.contains("没有自动重试"));
        let visible = get_for_session(&conn, "s1").unwrap();
        assert_eq!(
            visible.delegations[0].summary.as_deref(),
            Some(recovered[0].summary)
        );
        assert!(!visible.pending_decisions[0].questions.is_empty());
        assert_eq!(visible.cursor, 21);
        assert!(!serde_json::to_string(&visible)
            .unwrap()
            .contains("PRIVATE_CANARY"));
        // A retry must not inherit prior execution stages, failures or reviews.
        conn.execute("UPDATE delegation_attempt_events SET payload_json='{\"stage\":\"explore\"}' WHERE event_type='heartbeat'", []).unwrap();
        conn.execute_batch(
            "INSERT INTO delegation_attempts VALUES ('a2','d1',2,'running');
            UPDATE delegations SET status='running',updated_at=21 WHERE id='d1';
            INSERT INTO delegation_deliveries VALUES ('old-delivery','d1','{}',22,2,'a1');
            INSERT INTO delegation_review_jobs VALUES ('old-review','old-delivery','running',22);",
        )
        .unwrap();
        let retry = get_for_session(&conn, "s1").unwrap();
        assert!(retry.delegations[0].summary.is_none());
        assert!(retry.delegations[0].reviewer_status.is_none());
        assert_eq!(retry.delegations[0].activity.as_deref(), Some("正在执行"));
        assert!(latest_delivery(&conn, "d1").unwrap().is_none());
        assert!(runtime_failures_for_session(&conn, "s1")
            .unwrap()
            .is_empty());
        conn.execute_batch("UPDATE delegations SET status='cancelled' WHERE id='d1';")
            .unwrap();
        assert!(get_for_session(&conn, "s1").unwrap().delegations[0]
            .activity
            .is_none());
        conn.execute_batch(
            "UPDATE delegations SET status='awaiting_summary' WHERE id='d1';
            INSERT INTO delegation_materialization_receipts VALUES ('p1','unknown',30);",
        )
        .unwrap();
        let uncertain = get_for_session(&conn, "s1").unwrap();
        assert_eq!(uncertain.delegations[0].status, "needs_decision");
        assert!(uncertain.delegations[0]
            .summary
            .as_deref()
            .unwrap()
            .contains("正式项目"));
        assert!(!uncertain.pending_decisions[0].questions.is_empty());
        assert_eq!(uncertain.cursor, 30);
        let facts = runtime_failures_for_session(&conn, "s1").unwrap();
        assert_eq!(facts[0].reason, "materialization_unknown");
        assert!(runtime_failures_for_session(&conn, "s2")
            .unwrap()
            .is_empty());
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
