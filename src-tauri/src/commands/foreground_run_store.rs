//! Durable transcript and foreground-run protocol.
//!
//! This is deliberately not a generic message repository.  Its small
//! interface owns the relational invariants of one foreground turn: hidden
//! provisional parents never appear in either projection, tool protocol rows
//! are committed with the visible reply, and a failed run is never left marked
//! as running.

use crate::agent::task_facts::{persist_task_facts, PlanStepStatus, TaskFacts, TaskTerminalReason};
use crate::commands::foreground_history::{
    hydrate_agent_steps, hydrate_task_facts_from_db, hydrate_task_runs_from_db,
    parse_inline_tool_calls, persist_task_run, persist_tool_results_with_mcp_revisions,
    private_transcript_metadata, refresh_private_transcript_tool_result, summarize_task_run,
    table_exists,
};
use crate::commands::foreground_message_contracts::{Message, ToolCallInfo, ToolResultInfo};
use crate::commands::foreground_response_projector::PersistedTurnOutcome;
use crate::commands::foreground_text_attachments::{self, TextAttachment};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(crate) struct ForegroundRunStore {
    db: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone)]
pub(crate) struct BeginForegroundTurn {
    pub session_id: String,
    pub user_message_id: String,
    pub reply_id: String,
    pub role: String,
    pub content: String,
    pub text_attachments: Vec<TextAttachment>,
    pub persist_user_message: bool,
    pub now: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct ForegroundRun {
    pub session_id: String,
    /// The durable ancestor which the visible assistant reply must retain.
    /// For normal turns this is the user message just persisted; for internal
    /// Main-Agent follow-ups it is the session's pre-existing leaf.
    pub parent_message_id: Option<String>,
    pub reply_id: String,
    pub now: i64,
}

/// A caller-owned admission decision made while the foreground reservation
/// transaction is still open.  Keeping this small and storage-level avoids
/// coupling the foreground protocol to any particular scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForegroundRunAdmission {
    Start,
    Skip,
}

/// An owned, one-shot admission hook for callers that need a policy decision
/// at the precise reservation seam without teaching this module that policy.
pub(crate) type ForegroundRunAdmissionGuard = Box<
    dyn for<'connection> FnOnce(&Transaction<'connection>) -> Result<ForegroundRunAdmission, String>
        + Send,
>;

/// The result of atomically attempting to reserve a foreground turn.
/// `Skipped` means the admission hook declined before any transcript or
/// `task_runs` mutation was made.
#[derive(Debug, Clone)]
pub(crate) enum ForegroundRunBeginResult {
    Started(ForegroundRun),
    Skipped,
}

impl ForegroundRunStore {
    pub(crate) fn new(db: Arc<Mutex<Connection>>) -> Self {
        Self { db }
    }

    /// Atomically reserve the hidden assistant parent/run before any internal
    /// tool (including `delegate_work`) can require a durable foreground FK.
    pub(crate) fn begin(&self, input: BeginForegroundTurn) -> Result<ForegroundRun, String> {
        match self.begin_with_admission(input, |_| Ok(ForegroundRunAdmission::Start))? {
            ForegroundRunBeginResult::Started(run) => Ok(run),
            ForegroundRunBeginResult::Skipped => {
                unreachable!("the default foreground admission always starts")
            }
        }
    }

    /// Reserve a foreground turn only if `admission` approves it inside the
    /// same SQLite transaction.  The hook runs after the stable read snapshot
    /// is available but before the first transcript mutation, so `Skip` is a
    /// true no-op for messages, `task_runs`, and the session leaf.
    pub(crate) fn begin_with_admission<F>(
        &self,
        input: BeginForegroundTurn,
        admission: F,
    ) -> Result<ForegroundRunBeginResult, String>
    where
        F: FnOnce(&Transaction<'_>) -> Result<ForegroundRunAdmission, String>,
    {
        // Direct storage callers cannot bypass foreground admission validation.
        foreground_text_attachments::validate_turn(
            &input.role,
            &input.content,
            &input.text_attachments,
        )?;
        let conn = self.db.lock().map_err(|e| e.to_string())?;
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let last_created_at = tx
            .query_row(
                "SELECT MAX(created_at) FROM messages WHERE session_id = ?1",
                params![&input.session_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(|e| e.to_string())?;
        // SQLite timestamps use seconds. Two quick turns can otherwise share
        // the same user/reply timestamps and fall back to random UUID order on
        // reload. Keep each turn monotonically after the current transcript.
        let now = last_created_at
            .map(|last| input.now.max(last.saturating_add(1)))
            .unwrap_or(input.now);

        let previous_leaf_id: Option<String> = tx
            .query_row(
                "SELECT leaf_message_id FROM sessions WHERE id = ?1",
                params![&input.session_id],
                |row| row.get(0),
            )
            .unwrap_or(None);

        match admission(&tx)? {
            ForegroundRunAdmission::Start => {}
            ForegroundRunAdmission::Skip => {
                tx.commit().map_err(|e| e.to_string())?;
                return Ok(ForegroundRunBeginResult::Skipped);
            }
        }

        // A normal user turn replaces the future of the current conversation
        // leaf.  If that leaf is waiting for approval, make the unapproved
        // operation terminal before the new user message becomes visible.  This
        // is deliberately part of the same transaction: hiding the old card in
        // the UI alone must never leave a delayed confirmation able to execute.
        if input.persist_user_message {
            supersede_pending_leaf_confirmation(
                &tx,
                &input.session_id,
                previous_leaf_id.as_deref(),
                now,
            )?;
        }

        if input.persist_user_message {
            tx.execute(
                "INSERT INTO messages (id, session_id, role, content, parent_id, created_at, metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    &input.user_message_id,
                    &input.session_id,
                    &input.role,
                    &input.content,
                    previous_leaf_id.as_deref(),
                    now,
                    foreground_text_attachments::metadata(&input.text_attachments),
                ],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "UPDATE sessions SET leaf_message_id = ?1, updated_at = ?2 WHERE id = ?3",
                params![&input.user_message_id, now, &input.session_id],
            )
            .map_err(|e| e.to_string())?;
        }

        let assistant_parent = if input.persist_user_message {
            Some(input.user_message_id.clone())
        } else {
            previous_leaf_id
        };
        tx.execute(
            "INSERT INTO messages (id, session_id, role, content, parent_id, is_provisional, created_at)
             VALUES (?1, ?2, 'assistant', '', ?3, 1, ?4)",
            params![
                &input.reply_id,
                &input.session_id,
                assistant_parent.as_deref(),
                now,
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO task_runs (id, session_id, message_id, goal, status, plan, created_at, updated_at)
             VALUES (?1, ?2, ?1, ?3, 'running', '[]', ?4, ?4)",
            params![&input.reply_id, &input.session_id, &input.content, now],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;

        Ok(ForegroundRunBeginResult::Started(ForegroundRun {
            session_id: input.session_id,
            parent_message_id: assistant_parent,
            reply_id: input.reply_id,
            now,
        }))
    }

    /// Promote a prepared assistant reply and commit its entire replay/task
    /// protocol as one SQLite transaction.  This intentionally does *not*
    /// emit terminal events; `ForegroundLifecycleControl` remains the only
    /// audit-before-visible-event barrier.
    pub(crate) fn complete(
        &self,
        run: &ForegroundRun,
        outcome: PersistedTurnOutcome,
    ) -> Result<Message, String> {
        self.complete_with_mcp_revisions(
            run,
            outcome,
            &crate::agent::handlers::McpToolRevisionCatalog::new(),
        )
    }

    /// Commit one completed foreground turn, preserving any MCP contract
    /// revisions captured while this Main-Agent registry was assembled.
    pub(crate) fn complete_with_mcp_revisions(
        &self,
        run: &ForegroundRun,
        outcome: PersistedTurnOutcome,
        mcp_tool_revisions: &crate::agent::handlers::McpToolRevisionCatalog,
    ) -> Result<Message, String> {
        let protocol_metadata = outcome
            .protocol_transcript
            .as_deref()
            .map(private_transcript_metadata)
            .transpose()?
            .flatten();
        let mut reply = Message {
            id: run.reply_id.clone(),
            session_id: run.session_id.clone(),
            role: "assistant".to_string(),
            content: outcome.content.clone(),
            text_attachments: Vec::new(),
            created_at: run.now + 1,
            parent_id: run.parent_message_id.clone(),
            is_deleted: None,
            tool_calls: outcome.tool_calls.clone(),
            tool_results: outcome.tool_results.clone(),
            task_run: None,
            task_facts: outcome.task_facts.clone(),
        };
        let goal = outcome.goal;
        reply.task_run = summarize_task_run(&reply, goal.clone());
        let tool_calls_json = reply
            .tool_calls
            .as_ref()
            .and_then(|calls| {
                (!calls.is_empty()).then(|| {
                    serde_json::to_string(
                        &calls
                            .iter()
                            .map(|call| {
                                serde_json::json!({
                                    "id": call.id,
                                    "name": call.name,
                                    "arguments": call.arguments,
                                })
                            })
                            .collect::<Vec<_>>(),
                    )
                    .map_err(|e| e.to_string())
                })
            })
            .transpose()?;

        let conn = self.db.lock().map_err(|e| e.to_string())?;
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let finalized = tx.execute(
            "UPDATE messages SET content=?1, parent_id=?2, tool_calls=?3, is_provisional=0, created_at=?4, metadata=?7
             WHERE id=?5 AND session_id=?6 AND role='assistant' AND is_provisional=1",
            params![
                &reply.content,
                reply.parent_id.as_deref(),
                tool_calls_json,
                reply.created_at,
                &run.reply_id,
                &run.session_id,
                protocol_metadata,
            ],
        ).map_err(|e| e.to_string())?;
        if finalized != 1 {
            return Err(
                "foreground provisional assistant reply is missing or already finalized"
                    .to_string(),
            );
        }
        tx.execute(
            "UPDATE sessions SET leaf_message_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![&run.reply_id, run.now, &run.session_id],
        )
        .map_err(|e| e.to_string())?;
        if let Some(steps) = reply.tool_results.as_deref() {
            persist_tool_results_with_mcp_revisions(
                &tx,
                &run.session_id,
                reply.created_at,
                reply.tool_calls.as_deref(),
                steps,
                mcp_tool_revisions,
            )?;
        }
        persist_task_run(&tx, &reply)?;
        if reply.task_run.is_none() {
            // Every foreground turn reserves a durable run before model
            // preparation. A text-only reply has no UI task card, but its
            // reservation must still leave `running`; automation recovery and
            // run history rely on this terminal record.
            let changed = tx
                .execute(
                    "UPDATE task_runs
                     SET goal = ?1, status = 'completed', plan = '[]',
                         confirmation_state = 'none', resumable = 0,
                         step_count = 0, completed_step_count = 0,
                         continuation_context = '', updated_at = ?2
                     WHERE id = ?3 AND session_id = ?4",
                    params![&goal, reply.created_at, &run.reply_id, &run.session_id],
                )
                .map_err(|e| e.to_string())?;
            if changed != 1 {
                return Err("foreground task run is missing while completing a text reply".into());
            }
        }
        if let (Some(facts), Some(task_run)) = (reply.task_facts.as_ref(), reply.task_run.as_ref())
        {
            persist_task_facts(
                &tx,
                &task_run.id,
                &reply.session_id,
                &reply.id,
                facts,
                reply.created_at,
            )?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(reply)
    }

    /// A failed foreground run becomes one visible, resumable Main-Agent
    /// state. Leaving a hidden provisional reply behind makes the active
    /// branch look terminal even though the task requires user attention.
    pub(crate) fn mark_needs_attention(&self, run: &ForegroundRun) -> Result<(), String> {
        if !self.recover_interrupted_run(run, Utc::now().timestamp())? {
            return Err(
                "foreground task run is missing while recording recoverable failure".to_string(),
            );
        }
        Ok(())
    }

    /// Promote one known provisional reply without sweeping other concurrent
    /// foreground turns. Internal automation dispatch uses this after a
    /// failure that occurred after reservation, and the ordinary message
    /// pipeline uses the same invariant for its own preparation failures.
    pub(crate) fn recover_interrupted_run(
        &self,
        run: &ForegroundRun,
        now: i64,
    ) -> Result<bool, String> {
        self.recover_known_provisional_run(&run.session_id, &run.reply_id, now)
    }

    pub(crate) fn recover_known_provisional_run(
        &self,
        session_id: &str,
        reply_id: &str,
        now: i64,
    ) -> Result<bool, String> {
        let mut conn = self.db.lock().map_err(|e| e.to_string())?;
        recover_interrupted_run_in_conn(&mut conn, session_id, reply_id, now)
    }

    /// Converts foreground turns that were interrupted by a process exit into
    /// one visible, resumable Main-Agent state.  A hidden provisional reply
    /// must never leave the user at a terminal-looking user message with the
    /// composer silently waiting forever after restart.
    ///
    /// This does not touch delegated attempts: the delegation pump owns their
    /// independent recovery.  It only restores the foreground transcript and
    /// preserves the run as a resumable parent for later continuation.
    pub(crate) fn recover_interrupted_runs(&self, now: i64) -> Result<usize, String> {
        let mut conn = self.db.lock().map_err(|e| e.to_string())?;
        recover_interrupted_runs_in_conn(&mut conn, now)
    }
}

const SUPERSEDED_CONFIRMATION_OUTPUT: &str =
    "Confirmation cancelled: a newer user message superseded this action. The operation was not run.";

/// Make a pending side-effect request terminal inside the caller's transaction.
///
/// The task summary, fact record, executable step, and model-facing `tool`
/// message must agree.  Keeping this as one transaction-level seam lets a new
/// user turn and an explicit stop apply the same safety rule without each
/// inventing a partial cancellation projection.
pub(crate) fn terminalize_pending_confirmation_in_tx(
    tx: &Transaction<'_>,
    session_id: &str,
    message_id: &str,
    now: i64,
    cancellation_output: &str,
) -> Result<bool, String> {
    if !table_exists(tx, "task_runs")? {
        return Ok(false);
    }

    let pending_run: Option<(String, i64)> = tx
        .query_row(
            "SELECT task_runs.id, messages.created_at
             FROM task_runs
             JOIN messages ON messages.id = task_runs.message_id
             WHERE task_runs.session_id = ?1
               AND task_runs.message_id = ?2
               AND task_runs.status = 'awaiting_confirmation'",
            params![session_id, message_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((task_run_id, message_created_at)) = pending_run else {
        return Ok(false);
    };

    let mut statement = tx
        .prepare(
            "SELECT COALESCE(call_id, id)
             FROM agent_steps
             WHERE session_id = ?1
               AND created_at = ?2
               AND success != 1
               AND COALESCE(tool_output, '') LIKE '%Confirmation required%'",
        )
        .map_err(|e| e.to_string())?;
    let superseded_call_ids = statement
        .query_map(params![session_id, message_created_at], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(statement);

    tx.execute(
        "UPDATE agent_steps
         SET tool_output = ?1, success = 0
         WHERE session_id = ?2
           AND created_at = ?3
           AND success != 1
           AND COALESCE(tool_output, '') LIKE '%Confirmation required%'",
        params![cancellation_output, session_id, message_created_at],
    )
    .map_err(|e| e.to_string())?;
    // The foreground model rehydrates its conversation from durable `tool`
    // messages, not from the task projection.  Keep that source in sync so a
    // redirected turn cannot inherit a stale "Confirmation required" result.
    tx.execute(
        "UPDATE messages
         SET content = ?1
         WHERE session_id = ?2
           AND role = 'tool'
           AND created_at = ?3
           AND tool_call_id IN (
                SELECT COALESCE(call_id, id)
                FROM agent_steps
                WHERE session_id = ?2
                  AND created_at = ?3
                  AND tool_output = ?1
           )",
        params![cancellation_output, session_id, message_created_at],
    )
    .map_err(|e| e.to_string())?;
    for call_id in &superseded_call_ids {
        refresh_private_transcript_tool_result(
            tx,
            session_id,
            message_id,
            call_id,
            cancellation_output,
        )?;
    }
    tx.execute(
        "UPDATE task_runs
         SET status = 'stopped', confirmation_state = 'none', resumable = 0, updated_at = ?1
         WHERE id = ?2",
        params![now, task_run_id],
    )
    .map_err(|e| e.to_string())?;

    if table_exists(tx, "task_run_facts")? {
        let stored_facts: Option<String> = tx
            .query_row(
                "SELECT facts_json FROM task_run_facts WHERE task_run_id = ?1",
                params![task_run_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(facts_json) = stored_facts {
            let mut facts: TaskFacts = serde_json::from_str(&facts_json)
                .map_err(|e| format!("stored task facts are invalid: {e}"))?;
            facts.pending_confirmation = None;
            facts.terminal_reason = Some(TaskTerminalReason::Stopped);
            for step in &mut facts.plan {
                if superseded_call_ids.contains(&step.id) {
                    step.status = PlanStepStatus::Skipped;
                }
            }
            persist_task_facts(tx, &task_run_id, session_id, message_id, &facts, now)?;
        }
    }

    Ok(true)
}

/// Stop an unapproved side effect attached to the leaf that a new user turn is
/// about to replace.  A redirect must make the old action non-executable, not
/// merely hide its confirmation card.
fn supersede_pending_leaf_confirmation(
    tx: &Transaction<'_>,
    session_id: &str,
    leaf_message_id: Option<&str>,
    now: i64,
) -> Result<(), String> {
    let Some(message_id) = leaf_message_id else {
        return Ok(());
    };
    terminalize_pending_confirmation_in_tx(
        tx,
        session_id,
        message_id,
        now,
        SUPERSEDED_CONFIRMATION_OUTPUT,
    )?;
    Ok(())
}

pub(crate) fn recover_interrupted_runs_in_conn(
    conn: &mut Connection,
    now: i64,
) -> Result<usize, String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let interrupted = {
        let mut statement = tx
            .prepare(
                "SELECT r.id, r.session_id, m.parent_id
                     FROM task_runs r
                     JOIN messages m ON m.id = r.message_id AND m.session_id = r.session_id
                     WHERE r.status = 'running'
                       AND m.role = 'assistant'
                       AND m.is_provisional = 1",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };

    for (run_id, session_id, parent_id) in &interrupted {
        promote_interrupted_run(&tx, run_id, session_id, parent_id.as_deref(), now)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(interrupted.len())
}

fn recover_interrupted_run_in_conn(
    conn: &mut Connection,
    session_id: &str,
    reply_id: &str,
    now: i64,
) -> Result<bool, String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let interrupted = tx
        .query_row(
            "SELECT r.id, r.session_id, m.parent_id
             FROM task_runs r
             JOIN messages m ON m.id = r.message_id AND m.session_id = r.session_id
             WHERE r.id = ?1 AND r.session_id = ?2
               AND m.role = 'assistant' AND m.is_provisional = 1",
            params![reply_id, session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((run_id, stored_session_id, parent_id)) = interrupted else {
        tx.commit().map_err(|e| e.to_string())?;
        return Ok(false);
    };
    promote_interrupted_run(&tx, &run_id, &stored_session_id, parent_id.as_deref(), now)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(true)
}

fn promote_interrupted_run(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
    session_id: &str,
    parent_id: Option<&str>,
    now: i64,
) -> Result<(), String> {
    // Project only the trusted event kind/code, never its diagnostic message.
    // Older databases without a journal retain the interruption fallback.
    let model_failed = if table_exists(tx, "agent_run_events")? {
        tx.query_row(
            "SELECT CASE WHEN json_valid(payload) THEN
                 CASE WHEN json_extract(payload, '$.type') = 'Error'
                       AND json_extract(payload, '$.data.code') = 'LlmError'
                      THEN 1 ELSE 0 END
                 ELSE 0 END
             FROM agent_run_events
             WHERE run_id = ?1 AND session_id = ?2 AND event_type = 'Error'
             ORDER BY sequence DESC LIMIT 1",
            params![run_id, session_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .unwrap_or(false)
    } else {
        false
    };
    let content = if model_failed {
        "模型请求未能完成。任务状态已安全保留，请检查模型连接后继续。"
    } else {
        "主 Agent 本轮未能完成。任务状态已安全保留，你可以继续此会话。"
    };
    tx.execute(
        "UPDATE task_runs
             SET status = 'needs_attention', resumable = 1, updated_at = ?1
             WHERE id = ?2 AND session_id = ?3",
        params![now, run_id, session_id],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE messages
             SET content = ?1, is_provisional = 0, created_at = ?2
             WHERE id = ?3 AND session_id = ?4 AND role = 'assistant' AND is_provisional = 1",
        params![content, now, run_id, session_id],
    )
    .map_err(|e| e.to_string())?;
    if let Some(parent_id) = parent_id {
        // Never steal a branch that advanced after this run was reserved. The
        // common send-message path points the leaf at its parent until the
        // reply is promoted.
        tx.execute(
            "UPDATE sessions
                 SET leaf_message_id = ?1, updated_at = ?2
                 WHERE id = ?3 AND leaf_message_id = ?4",
            params![run_id, now, session_id, parent_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl ForegroundRunStore {
    /// UI-compatible active-branch projection. Historical fixtures without
    /// `is_provisional` remain readable; modern databases never expose hidden
    /// parents.
    pub(crate) fn project_session(&self, session_id: &str) -> Result<Vec<Message>, String> {
        let conn = self.db.lock().map_err(|e| e.to_string())?;
        project_session_from_conn(&conn, session_id)
    }
}

fn project_session_from_conn(conn: &Connection, session_id: &str) -> Result<Vec<Message>, String> {
    let leaf_id: Option<String> = conn
        .query_row(
            "SELECT leaf_message_id FROM sessions WHERE id = ?1",
            params![session_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let Some(leaf_id) = leaf_id else {
        return Ok(Vec::new());
    };
    let has_provisional = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('messages') WHERE name='is_provisional')", [], |r| r.get::<_, i64>(0)
    ).unwrap_or(0) != 0;
    let visible = if has_provisional {
        " AND is_provisional = 0"
    } else {
        ""
    };
    let visible_m = if has_provisional {
        " AND m.is_provisional = 0"
    } else {
        ""
    };
    let mut stmt = conn.prepare(&format!(
        "WITH RECURSIVE ancestors AS (
            SELECT id, session_id, role, content, parent_id, created_at, tool_calls, tool_call_id, tool_name, metadata
            FROM messages WHERE id = ?1 AND role != 'tool'{visible}
            UNION ALL
            SELECT m.id, m.session_id, m.role, m.content, m.parent_id, m.created_at, m.tool_calls, m.tool_call_id, m.tool_name, m.metadata
            FROM messages m JOIN ancestors a ON m.id = a.parent_id
            WHERE m.role != 'tool'{visible_m}
        )
        SELECT id, session_id, role, content, created_at, parent_id, tool_calls, metadata
        FROM ancestors ORDER BY created_at ASC, id ASC LIMIT 500"
    )).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![leaf_id], |r| {
            Ok(Message {
                id: r.get(0)?,
                session_id: r.get(1)?,
                role: r.get(2)?,
                content: r.get(3)?,
                text_attachments: foreground_text_attachments::from_metadata(
                    r.get::<_, Option<String>>(7)?.as_deref(),
                )
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        7,
                        rusqlite::types::Type::Text,
                        error.into(),
                    )
                })?,
                created_at: r.get(4)?,
                parent_id: r.get(5)?,
                is_deleted: None,
                tool_calls: parse_inline_tool_calls(r.get::<_, Option<String>>(6)?.as_deref()),
                tool_results: None,
                task_run: None,
                task_facts: None,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut messages = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    hydrate_agent_steps(conn, &mut messages)?;
    hydrate_task_runs_from_db(conn, &mut messages)?;
    hydrate_task_facts_from_db(conn, &mut messages)?;
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn private_messages() -> Vec<crate::llm::Message> {
        let state = crate::llm::ProtocolContinuation {
            protocol: crate::llm::ModelProtocol::OpenaiResponses,
            model: "test-model".into(),
            credential_ref: "test-profile".into(),
            output_items: vec![
                serde_json::json!({"type":"reasoning","id":"rs_1","encrypted_content":"encrypted-marker"}),
                serde_json::json!({"type":"function_call","id":"fc_1","call_id":"call-1","name":"read_file","arguments":"{}"}),
            ],
        };
        vec![
            crate::llm::Message {
                role: "assistant".into(),
                content: "Inspecting".into(),
                tool_calls: Some(vec![crate::llm::ToolCall {
                    id: "call-1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({}),
                }]),
                tool_call_id: None,
                tool_images: Vec::new(),
                protocol_state: Some(state),
            },
            crate::llm::Message {
                role: "tool".into(),
                content: "read-result".into(),
                tool_calls: None,
                tool_call_id: Some("call-1".into()),
                tool_images: Vec::new(),
                protocol_state: None,
            },
            crate::llm::Message {
                role: "assistant".into(),
                content: "done".into(),
                tool_calls: None,
                tool_call_id: None,
                tool_images: Vec::new(),
                protocol_state: Some(crate::llm::ProtocolContinuation {
                    protocol: crate::llm::ModelProtocol::OpenaiResponses,
                    model: "test-model".into(),
                    credential_ref: "test-profile".into(),
                    output_items: vec![
                        serde_json::json!({"type":"message","id":"msg_2","phase":"final_answer","content":[{"type":"output_text","text":"done"}]}),
                    ],
                }),
            },
        ]
    }

    #[test]
    fn private_protocol_survives_shutdown_reopen_without_ui_payload_or_tool_aggregation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private-history.sqlite");
        let conn = Connection::open(&path).unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO sessions(id,title,created_at,updated_at) VALUES('s','test',1,1)",
            [],
        )
        .unwrap();
        let store = ForegroundRunStore::new(Arc::new(Mutex::new(conn)));
        let run = store
            .begin(BeginForegroundTurn {
                session_id: "s".into(),
                user_message_id: "u".into(),
                reply_id: "a".into(),
                role: "user".into(),
                content: "inspect".into(),
                text_attachments: vec![],
                persist_user_message: true,
                now: 10,
            })
            .unwrap();
        let reply = store
            .complete(
                &run,
                PersistedTurnOutcome {
                    content: "done".into(),
                    goal: "inspect".into(),
                    // UI is a lossy projection deliberately unrelated to native calls.
                    tool_calls: None,
                    tool_results: None,
                    task_facts: None,
                    protocol_transcript: Some(private_messages()),
                },
            )
            .unwrap();
        let projected = serde_json::to_string(&reply).unwrap();
        assert!(!projected.contains("encrypted-marker"));
        assert!(!projected.contains("protocol_state"));
        drop(store);

        let conn = Connection::open(&path).unwrap();
        let restored =
            crate::commands::foreground_history::hydrate_llm_history(&conn, "s", 20).unwrap();
        assert_eq!(restored.len(), 4);
        assert_eq!(restored[1].tool_calls.as_ref().unwrap()[0].id, "call-1");
        assert_eq!(restored[2].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(
            restored[1].protocol_state.as_ref().unwrap().output_items[0]["encrypted_content"],
            "encrypted-marker"
        );
        assert_eq!(
            restored[3].protocol_state.as_ref().unwrap().output_items[0]["phase"],
            "final_answer"
        );
        assert!(!format!("{restored:?}").contains("encrypted-marker"));
        // Old text compaction flags must not destroy the private ledger.
        let metadata: String = conn
            .query_row("SELECT metadata FROM messages WHERE id='a'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let compressed = foreground_text_attachments::compressed_metadata(Some(&metadata));
        conn.execute("UPDATE messages SET metadata=?1 WHERE id='a'", [compressed])
            .unwrap();
        let compressed_history =
            crate::commands::foreground_history::hydrate_llm_history(&conn, "s", 20).unwrap();
        assert_eq!(
            compressed_history[1]
                .protocol_state
                .as_ref()
                .unwrap()
                .output_items[0]["encrypted_content"],
            "encrypted-marker"
        );
        // A cutoff before this assistant excludes its entire native turn.
        let cutoff =
            crate::commands::foreground_history::hydrate_llm_history(&conn, "s", 11).unwrap();
        assert_eq!(cutoff.len(), 1);
        assert!(cutoff
            .iter()
            .all(|message| message.protocol_state.is_none()));
    }

    fn store() -> ForegroundRunStore {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY, leaf_message_id TEXT, updated_at INTEGER);
            CREATE TABLE messages (id TEXT PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, parent_id TEXT, is_provisional INTEGER DEFAULT 0, created_at INTEGER, tool_calls TEXT, tool_call_id TEXT, tool_name TEXT, metadata TEXT);
            CREATE TABLE task_runs (id TEXT PRIMARY KEY, session_id TEXT, message_id TEXT, goal TEXT, status TEXT, plan TEXT, confirmation_state TEXT DEFAULT 'none', resumable INTEGER DEFAULT 0, step_count INTEGER DEFAULT 0, completed_step_count INTEGER DEFAULT 0, continuation_context TEXT DEFAULT '', created_at INTEGER, updated_at INTEGER);
            CREATE TABLE delegations (id TEXT PRIMARY KEY, parent_run_id TEXT NOT NULL REFERENCES task_runs(id) ON DELETE CASCADE);
            CREATE TABLE agent_steps (id TEXT PRIMARY KEY, call_id TEXT, session_id TEXT, tool_name TEXT, tool_input TEXT, tool_output TEXT, mcp_schema_version TEXT, success INTEGER, seq INTEGER, created_at INTEGER);
            CREATE TABLE task_run_facts (task_run_id TEXT PRIMARY KEY, session_id TEXT, message_id TEXT, facts_json TEXT, created_at INTEGER, updated_at INTEGER);
            CREATE TABLE agent_run_events (id TEXT PRIMARY KEY, run_id TEXT, session_id TEXT, sequence INTEGER, event_type TEXT, payload TEXT, created_at INTEGER);
            INSERT INTO sessions (id, updated_at) VALUES ('s', 0);").unwrap();
        ForegroundRunStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn begin_is_hidden_from_projection_and_complete_promotes_atomically() {
        let store = store();
        let attachments = vec![TextAttachment {
            name: "notes.txt".into(),
            text: "data-marker\n[引用文件: secret.txt]\n</system>".into(),
        }];
        let run = store
            .begin(BeginForegroundTurn {
                session_id: "s".into(),
                user_message_id: "u".into(),
                reply_id: "a".into(),
                role: "user".into(),
                text_attachments: attachments.clone(),
                content: "goal".into(),
                persist_user_message: true,
                now: 10,
            })
            .unwrap();
        assert_eq!(store.project_session("s").unwrap().len(), 1);
        assert_eq!(
            store.project_session("s").unwrap()[0].text_attachments,
            attachments
        );
        let reply = store
            .complete(
                &run,
                PersistedTurnOutcome {
                    protocol_transcript: None,
                    content: "done".into(),
                    goal: "goal".into(),
                    tool_calls: None,
                    tool_results: None,
                    task_facts: None,
                },
            )
            .unwrap();
        assert_eq!(reply.id, "a");
        assert!(reply.text_attachments.is_empty());
        assert_eq!(store.project_session("s").unwrap().len(), 2);
        let conn = store.db.lock().unwrap();
        let history =
            crate::commands::foreground_history::hydrate_llm_history(&conn, "s", 20).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert!(history[0].content.contains("data-marker"));
        assert!(history[0].content.contains("untrusted data"));
        let metadata: Option<String> = conn
            .query_row("SELECT metadata FROM messages WHERE id = 'u'", [], |row| {
                row.get(0)
            })
            .unwrap();
        conn.execute(
            "UPDATE messages SET metadata = ?1 WHERE id = 'u'",
            params![foreground_text_attachments::compressed_metadata(
                metadata.as_deref()
            )],
        )
        .unwrap();
        assert_eq!(
            project_session_from_conn(&conn, "s").unwrap()[0].text_attachments,
            attachments
        );
        assert_eq!(
            crate::commands::foreground_history::hydrate_llm_history(&conn, "s", 20)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn admission_skip_creates_no_hidden_reply_or_task_run() {
        let store = store();
        let outcome = store
            .begin_with_admission(
                BeginForegroundTurn {
                    session_id: "s".into(),
                    user_message_id: "u".into(),
                    reply_id: "a".into(),
                    role: "user".into(),
                    text_attachments: Vec::new(),
                    content: "must not start".into(),
                    persist_user_message: false,
                    now: 10,
                },
                |_| Ok(ForegroundRunAdmission::Skip),
            )
            .unwrap();

        assert!(matches!(outcome, ForegroundRunBeginResult::Skipped));
        assert!(store.project_session("s").unwrap().is_empty());
        let conn = store.db.lock().unwrap();
        let task_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM task_runs WHERE id = 'a'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(task_count, 0);
    }

    #[test]
    fn rapid_turns_keep_conversation_order_when_wall_clock_seconds_match() {
        let store = store();
        for (user_id, reply_id, goal) in [("u1", "a1", "first"), ("u2", "a2", "second")] {
            let run = store
                .begin(BeginForegroundTurn {
                    session_id: "s".into(),
                    user_message_id: user_id.into(),
                    reply_id: reply_id.into(),
                    role: "user".into(),
                    text_attachments: if user_id == "u1" {
                        vec![TextAttachment {
                            name: "first.txt".into(),
                            text: "first-file-marker".into(),
                        }]
                    } else {
                        Vec::new()
                    },
                    content: goal.into(),
                    persist_user_message: true,
                    now: 10,
                })
                .unwrap();
            if user_id == "u2" {
                let conn = store.db.lock().unwrap();
                let replay =
                    crate::commands::foreground_history::hydrate_llm_history(&conn, "s", run.now)
                        .unwrap();
                assert_eq!(replay.len(), 2);
                assert!(replay[0].content.contains("first-file-marker"));
                assert!(replay
                    .iter()
                    .all(|message| !message.content.contains("second")));
            }
            store
                .complete(
                    &run,
                    PersistedTurnOutcome {
                        protocol_transcript: None,
                        content: format!("{goal} done"),
                        goal: goal.into(),
                        tool_calls: None,
                        tool_results: None,
                        task_facts: None,
                    },
                )
                .unwrap();
        }

        let messages = store.project_session("s").unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["u1", "a1", "u2", "a2"],
        );
        assert!(messages
            .windows(2)
            .all(|pair| pair[0].created_at < pair[1].created_at));
    }

    #[test]
    fn internal_follow_up_extends_the_existing_branch_without_a_user_message() {
        let store = store();
        let initial = store
            .begin(BeginForegroundTurn {
                session_id: "s".into(),
                user_message_id: "u".into(),
                reply_id: "a".into(),
                role: "user".into(),
                text_attachments: Vec::new(),
                content: "initial request".into(),
                persist_user_message: true,
                now: 10,
            })
            .unwrap();
        store
            .complete(
                &initial,
                PersistedTurnOutcome {
                    protocol_transcript: None,
                    content: "initial response".into(),
                    goal: "initial request".into(),
                    tool_calls: None,
                    tool_results: None,
                    task_facts: None,
                },
            )
            .unwrap();

        let follow_up = store
            .begin(BeginForegroundTurn {
                session_id: "s".into(),
                user_message_id: "unused".into(),
                reply_id: "auto".into(),
                role: "user".into(),
                text_attachments: Vec::new(),
                content: "scheduled follow-up".into(),
                persist_user_message: false,
                now: 20,
            })
            .unwrap();
        store
            .complete(
                &follow_up,
                PersistedTurnOutcome {
                    protocol_transcript: None,
                    content: "automation response".into(),
                    goal: "scheduled follow-up".into(),
                    tool_calls: None,
                    tool_results: None,
                    task_facts: None,
                },
            )
            .unwrap();

        let messages = store.project_session("s").unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["u", "a", "auto"]
        );
        assert_eq!(messages.last().unwrap().parent_id.as_deref(), Some("a"));
    }

    #[test]
    fn text_only_reply_finishes_its_durable_run() {
        let store = store();
        let run = store
            .begin(BeginForegroundTurn {
                session_id: "s".into(),
                user_message_id: "u".into(),
                reply_id: "a".into(),
                role: "user".into(),
                text_attachments: Vec::new(),
                content: "say hello".into(),
                persist_user_message: true,
                now: 10,
            })
            .unwrap();
        store
            .complete(
                &run,
                PersistedTurnOutcome {
                    protocol_transcript: None,
                    content: "hello".into(),
                    goal: "say hello".into(),
                    tool_calls: None,
                    tool_results: None,
                    task_facts: None,
                },
            )
            .unwrap();

        let conn = store.db.lock().unwrap();
        let state: (String, i64, i64) = conn
            .query_row(
                "SELECT status, step_count, completed_step_count FROM task_runs WHERE id = 'a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, ("completed".into(), 0, 0));
    }

    #[test]
    fn completion_preserves_delegations_owned_by_the_provisional_run() {
        let store = store();
        let run = store
            .begin(BeginForegroundTurn {
                session_id: "s".into(),
                user_message_id: "u".into(),
                reply_id: "a".into(),
                role: "user".into(),
                text_attachments: Vec::new(),
                content: "goal".into(),
                persist_user_message: true,
                now: 10,
            })
            .unwrap();
        {
            let conn = store.db.lock().unwrap();
            conn.execute(
                "INSERT INTO delegations (id, parent_run_id) VALUES ('delegation', 'a')",
                [],
            )
            .unwrap();
        }

        store
            .complete(
                &run,
                PersistedTurnOutcome {
                    protocol_transcript: None,
                    content: "done".into(),
                    goal: "goal".into(),
                    tool_calls: None,
                    tool_results: None,
                    task_facts: None,
                },
            )
            .unwrap();

        let conn = store.db.lock().unwrap();
        let surviving: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM delegations WHERE id = 'delegation'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(surviving, 1);
    }

    #[test]
    fn interrupted_provisional_run_becomes_visible_and_resumable_on_restart() {
        let store = store();
        store
            .begin(BeginForegroundTurn {
                session_id: "s".into(),
                user_message_id: "u".into(),
                reply_id: "a".into(),
                role: "user".into(),
                text_attachments: Vec::new(),
                content: "inspect the project".into(),
                persist_user_message: true,
                now: 10,
            })
            .unwrap();

        assert_eq!(store.recover_interrupted_runs(20).unwrap(), 1);
        let messages = store.project_session("s").unwrap();
        assert_eq!(
            messages.last().map(|message| message.id.as_str()),
            Some("a")
        );
        assert!(messages
            .last()
            .unwrap()
            .content
            .contains("任务状态已安全保留"));
        let conn = store.db.lock().unwrap();
        let state: (String, i64) = conn
            .query_row(
                "SELECT status, resumable FROM task_runs WHERE id = 'a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, ("needs_attention".into(), 1));
    }

    #[test]
    fn persisted_model_failure_remains_visible_after_recovery() {
        for persist_user_message in [true, false] {
            let store = store();
            if !persist_user_message {
                store
                    .db
                    .lock()
                    .unwrap()
                    .execute_batch(
                        "INSERT INTO messages (id, session_id, role, content, created_at)
                     VALUES ('u', 's', 'user', 'existing request', 1);
                     UPDATE sessions SET leaf_message_id = 'u' WHERE id = 's';",
                    )
                    .unwrap();
            }
            let run = store
                .begin(BeginForegroundTurn {
                    session_id: "s".into(),
                    user_message_id: "u".into(),
                    reply_id: "a".into(),
                    role: "user".into(),
                    text_attachments: Vec::new(),
                    content: "inspect the project".into(),
                    persist_user_message,
                    now: 10,
                })
                .unwrap();
            let journal =
                crate::agent::event::AgentRunJournal::new(store.db.clone(), run.reply_id.clone());
            journal
                .append(
                    &run.session_id,
                    &crate::agent::event::AgentEvent::Error {
                        code: crate::agent::event::ErrorCode::LlmError,
                        message: "Bearer private-test-key https://private.example/?input=secret"
                            .into(),
                        recoverable: true,
                        turn_id: None,
                    },
                )
                .unwrap();

            if persist_user_message {
                store.mark_needs_attention(&run).unwrap();
            } else {
                assert_eq!(store.recover_interrupted_runs(20).unwrap(), 1);
            }
            let messages = store.project_session("s").unwrap();
            assert_eq!(messages.len(), 2);
            let reply = messages.last().unwrap();
            assert_eq!(
                reply.content,
                "模型请求未能完成。任务状态已安全保留，请检查模型连接后继续。"
            );
            assert!(!reply.content.contains("private-test-key"));
            assert!(!reply.content.contains("private.example"));
            let task = reply.task_run.as_ref().unwrap();
            assert_eq!(task.status, "needs_attention");
            assert!(task.resumable);
            assert_eq!(task.step_count, 0);

            // Repeated catch/restart recovery cannot replace an already visible reason.
            journal
                .append(
                    &run.session_id,
                    &crate::agent::event::AgentEvent::Error {
                        code: crate::agent::event::ErrorCode::Unknown,
                        message: "must not replace the saved projection".into(),
                        recoverable: true,
                        turn_id: None,
                    },
                )
                .unwrap();
            assert!(!store.recover_interrupted_run(&run, 21).unwrap());
            assert_eq!(store.recover_interrupted_runs(22).unwrap(), 0);
            let recovered = store.project_session("s").unwrap();
            assert_eq!(recovered.last().unwrap().content, reply.content);
            assert_eq!(recovered.last().unwrap().created_at, reply.created_at);
        }
    }

    #[test]
    fn recovery_never_uses_another_run_or_untrusted_error_detail() {
        let llm_error =
            r#"{"type":"Error","data":{"code":"LlmError","message":"Bearer private-test-key"}}"#;
        for (event_run, event_session, event_type, payload) in [
            ("another-run", "s", "Error", llm_error),
            ("a", "another-session", "Error", llm_error),
            ("a", "s", "MessageEnd", llm_error),
            (
                "a",
                "s",
                "Error",
                r#"{"type":"MessageEnd","data":{"code":"LlmError"}}"#,
            ),
            (
                "a",
                "s",
                "Error",
                r#"{"type":"Error","data":{"code":"ToolError","message":"private-test-key"}}"#,
            ),
            (
                "a",
                "s",
                "Error",
                r#"{"type":"Error","data":{"code":"Unknown","message":"LlmError private-test-key"}}"#,
            ),
            (
                "a",
                "s",
                "Error",
                r#"{"type":"Error","data":{"code":"untrusted-new-code","message":"LlmError private-test-key"}}"#,
            ),
            ("a", "s", "Error", "malformed private-test-key"),
        ] {
            let store = store();
            let run = store
                .begin(BeginForegroundTurn {
                    session_id: "s".into(),
                    user_message_id: "u".into(),
                    reply_id: "a".into(),
                    role: "user".into(),
                    text_attachments: Vec::new(),
                    content: "inspect the project".into(),
                    persist_user_message: true,
                    now: 10,
                })
                .unwrap();
            store
                .db
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO agent_run_events
                 (id, run_id, session_id, sequence, event_type, payload, created_at)
                 VALUES ('event', ?1, ?2, 0, ?3, ?4, 11)",
                    params![event_run, event_session, event_type, payload],
                )
                .unwrap();

            assert!(store.recover_interrupted_run(&run, 20).unwrap());
            let messages = store.project_session("s").unwrap();
            assert_eq!(
                messages.last().unwrap().content,
                "主 Agent 本轮未能完成。任务状态已安全保留，你可以继续此会话。",
                "scope={event_run}/{event_session}, event={event_type}"
            );
        }
    }

    #[test]
    fn targeted_recovery_leaves_other_provisional_turns_untouched() {
        let store = store();
        {
            let conn = store.db.lock().unwrap();
            conn.execute("INSERT INTO sessions (id, updated_at) VALUES ('s2', 0)", [])
                .unwrap();
        }
        let first = store
            .begin(BeginForegroundTurn {
                session_id: "s".into(),
                user_message_id: "u1".into(),
                reply_id: "a1".into(),
                role: "user".into(),
                text_attachments: Vec::new(),
                content: "first".into(),
                persist_user_message: true,
                now: 10,
            })
            .unwrap();
        let _second = store
            .begin(BeginForegroundTurn {
                session_id: "s2".into(),
                user_message_id: "u2".into(),
                reply_id: "a2".into(),
                role: "user".into(),
                text_attachments: Vec::new(),
                content: "second".into(),
                persist_user_message: true,
                now: 10,
            })
            .unwrap();

        assert!(store.recover_interrupted_run(&first, 20).unwrap());
        assert!(!store.recover_interrupted_run(&first, 21).unwrap());
        assert_eq!(store.project_session("s").unwrap().last().unwrap().id, "a1");
        let conn = store.db.lock().unwrap();
        let untouched: (String, i64) = conn
            .query_row(
                "SELECT r.status, m.is_provisional
                 FROM task_runs r JOIN messages m ON m.id = r.message_id
                 WHERE r.id = 'a2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(untouched, ("running".into(), 1));
    }

    #[test]
    fn new_user_turn_supersedes_the_pending_confirmation_on_its_leaf() {
        use crate::agent::task_facts::{PendingConfirmationFact, PlanStepFact};

        let store = store();
        let mut facts = TaskFacts::new("open sound settings");
        facts.plan = vec![PlanStepFact {
            id: "call-1".into(),
            description: "open_windows_setting".into(),
            depends_on: Vec::new(),
            status: PlanStepStatus::Pending,
        }];
        facts.pending_confirmation = Some(PendingConfirmationFact {
            call_id: "call-1".into(),
            tool_name: "open_windows_setting".into(),
            arguments: serde_json::json!({"page": "sound"}),
            requested_at: 2,
        });
        facts.terminal_reason = Some(TaskTerminalReason::AwaitingConfirmation);
        {
            let conn = store.db.lock().unwrap();
            conn.execute_batch(
                "INSERT INTO messages (id, session_id, role, content, is_provisional, created_at)
                 VALUES ('u1', 's', 'user', 'open sound settings', 0, 1);
                 INSERT INTO messages (id, session_id, role, content, parent_id, is_provisional, created_at)
                 VALUES ('a1', 's', 'assistant', 'waiting for your approval', 'u1', 0, 2);
                 INSERT INTO messages (id, session_id, role, content, tool_call_id, is_provisional, created_at)
                 VALUES ('tool-1', 's', 'tool', 'Confirmation required before executing side-effect tool.', 'call-1', 0, 2);
                 UPDATE sessions SET leaf_message_id = 'a1' WHERE id = 's';
                 INSERT INTO task_runs (
                    id, session_id, message_id, goal, status, plan, confirmation_state,
                    resumable, step_count, completed_step_count, created_at, updated_at
                 ) VALUES (
                    'a1', 's', 'a1', 'open sound settings', 'awaiting_confirmation',
                    '[\"open_windows_setting\"]', 'pending', 1, 1, 0, 2, 2
                 );
                 INSERT INTO agent_steps (
                    id, call_id, session_id, tool_name, tool_input, tool_output, success, seq, created_at
                 ) VALUES (
                    'call-1', 'call-1', 's', 'open_windows_setting', '{\"page\":\"sound\"}',
                    'Confirmation required before executing side-effect tool.', 0, 1, 2
                 );",
            )
            .unwrap();
            conn.execute(
                "UPDATE messages SET tool_calls = ?1 WHERE id = 'a1'",
                params![serde_json::json!([{
                    "id": "call-1",
                    "name": "open_windows_setting",
                    "arguments": { "page": "sound" },
                }])
                .to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO task_run_facts (task_run_id, session_id, message_id, facts_json, created_at)
                 VALUES ('a1', 's', 'a1', ?1, 2)",
                params![serde_json::to_string(&facts).unwrap()],
            )
            .unwrap();
        }

        let redirected = store
            .begin(BeginForegroundTurn {
                session_id: "s".into(),
                user_message_id: "u2".into(),
                reply_id: "a2".into(),
                role: "user".into(),
                text_attachments: Vec::new(),
                content: "do not open settings; summarize the next step instead".into(),
                persist_user_message: true,
                now: 3,
            })
            .unwrap();
        assert_eq!(redirected.parent_message_id.as_deref(), Some("u2"));

        let conn = store.db.lock().unwrap();
        let state: (String, String, i64) = conn
            .query_row(
                "SELECT status, confirmation_state, resumable FROM task_runs WHERE id = 'a1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, ("stopped".into(), "none".into(), 0));
        let output: String = conn
            .query_row(
                "SELECT tool_output FROM agent_steps WHERE id = 'call-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(output, SUPERSEDED_CONFIRMATION_OUTPUT);
        let history_output: String = conn
            .query_row(
                "SELECT content FROM messages WHERE id = 'tool-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(history_output, SUPERSEDED_CONFIRMATION_OUTPUT);
        let model_history =
            crate::commands::foreground_history::hydrate_llm_history(&conn, "s", 4).unwrap();
        let tool_history = model_history
            .iter()
            .find(|message| message.role == "tool")
            .expect("the original tool result remains part of model history");
        assert_eq!(tool_history.content, SUPERSEDED_CONFIRMATION_OUTPUT);
        assert!(!tool_history.content.contains("Confirmation required"));
        let stored_facts: TaskFacts = conn
            .query_row(
                "SELECT facts_json FROM task_run_facts WHERE task_run_id = 'a1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|json| serde_json::from_str(&json).unwrap())
            .unwrap();
        assert!(stored_facts.pending_confirmation.is_none());
        assert_eq!(
            stored_facts.terminal_reason,
            Some(TaskTerminalReason::Stopped)
        );
        assert_eq!(stored_facts.plan[0].status, PlanStepStatus::Skipped);
    }

    #[test]
    fn failed_run_is_not_left_running() {
        let store = store();
        let run = store
            .begin(BeginForegroundTurn {
                session_id: "s".into(),
                user_message_id: "u".into(),
                reply_id: "a".into(),
                role: "user".into(),
                text_attachments: Vec::new(),
                content: "goal".into(),
                persist_user_message: true,
                now: 10,
            })
            .unwrap();
        store.mark_needs_attention(&run).unwrap();
        let conn = store.db.lock().unwrap();
        let status: String = conn
            .query_row("SELECT status FROM task_runs WHERE id='a'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "needs_attention");
    }

    #[test]
    fn rejected_tool_protocol_rolls_back_reply_promotion() {
        let store = store();
        let run = store
            .begin(BeginForegroundTurn {
                session_id: "s".into(),
                user_message_id: "u".into(),
                reply_id: "a".into(),
                role: "user".into(),
                text_attachments: Vec::new(),
                content: "goal".into(),
                persist_user_message: true,
                now: 10,
            })
            .unwrap();
        let error = store
            .complete(
                &run,
                PersistedTurnOutcome {
                    protocol_transcript: None,
                    content: "must not appear".into(),
                    goal: "goal".into(),
                    tool_calls: None,
                    tool_results: Some(vec![ToolResultInfo {
                        call_id: "call".into(),
                        tool_name: "read_file".into(),
                        success: true,
                        output: "x".into(),
                        error: None,
                        confirmation_required: false,
                        confirmation_status: None,
                    }]),
                    task_facts: None,
                },
            )
            .unwrap_err();
        assert!(error.contains("matching provider tool calls"));
        assert_eq!(store.project_session("s").unwrap().len(), 1);
        let conn = store.db.lock().unwrap();
        let provisional: i64 = conn
            .query_row(
                "SELECT is_provisional FROM messages WHERE id='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(provisional, 1);
    }
}
