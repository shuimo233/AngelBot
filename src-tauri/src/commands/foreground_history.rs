//! Foreground transcript history and task protocol conversion.
//!
//! This module owns the DB boundary for reconstructing provider-native history
//! and the legacy task/run projections used by the chat timeline.  It does not
//! own a transaction: callers such as `ForegroundRunStore` provide the
//! connection and decide the surrounding atomicity.

use crate::agent::task_facts::{
    persist_task_facts, PlanStepFact, PlanStepStatus, TaskFacts, TaskTerminalReason,
};
use crate::agent::{derive_run_state, RunStateInput};
use crate::commands::foreground_message_contracts::{
    AgentTaskRun, Message, ToolCallInfo, ToolResultInfo,
};
use crate::commands::foreground_text_attachments;
use crate::llm::{Message as LlmMessage, ToolCall as LlmToolCall};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::BTreeSet;
use uuid::Uuid;

const PRIVATE_TRANSCRIPT_KEY: &str = "privateProtocolTranscript";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivateProtocolTranscript {
    version: u8,
    messages: Vec<LlmMessage>,
}

/// The sole backend-private replay ledger. Frontend `Message` has no metadata
/// field, and history must not reconstruct this data from UI tool aggregates.
pub(crate) fn private_transcript_metadata(
    messages: &[LlmMessage],
) -> Result<Option<String>, String> {
    if messages.is_empty() {
        return Ok(None);
    }
    validate_private_transcript(messages)?;
    let value = serde_json::json!({PRIVATE_TRANSCRIPT_KEY: {
        "version": 1, "messages": messages,
    }});
    let encoded =
        serde_json::to_string(&value).map_err(|_| "Invalid private protocol transcript")?;
    if encoded.len() > 16 * 1024 * 1024 {
        return Err("Private protocol transcript exceeded storage limit".into());
    }
    Ok(Some(encoded))
}

fn validate_private_transcript(messages: &[LlmMessage]) -> Result<(), String> {
    if messages.len() > 1024 {
        return Err("Private protocol transcript exceeded storage limit".into());
    }
    for message in messages {
        if !matches!(message.role.as_str(), "assistant" | "tool" | "user")
            || (message.protocol_state.is_some() && message.role != "assistant")
            || (message.tool_calls.is_some() && message.role != "assistant")
            || (message.tool_call_id.is_some() && message.role != "tool")
        {
            return Err("Invalid private protocol transcript".into());
        }
        if let Some(state) = &message.protocol_state {
            state.validate()?;
        }
    }
    Ok(())
}

fn private_transcript_from_metadata(
    metadata: Option<&str>,
) -> Result<Option<Vec<LlmMessage>>, String> {
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    if metadata.len() > 16 * 1024 * 1024 {
        return Err("Private protocol transcript exceeded storage limit".into());
    }
    let value: serde_json::Value =
        serde_json::from_str(metadata).map_err(|_| "Invalid history metadata")?;
    let Some(value) = value.get(PRIVATE_TRANSCRIPT_KEY) else {
        return Ok(None);
    };
    let ledger: PrivateProtocolTranscript =
        serde_json::from_value(value.clone()).map_err(|_| "Invalid private protocol transcript")?;
    if ledger.version != 1 {
        return Err("Unsupported private protocol transcript version".into());
    }
    validate_private_transcript(&ledger.messages)?;
    Ok(Some(ledger.messages))
}

/// Keep the native tool receipt in sync when a durable confirmation settles.
/// The caller's transaction also owns the step/UI result update. Opaque
/// assistant output items are never rewritten, exposed, or reaggregated.
pub(crate) fn refresh_private_transcript_tool_result(
    conn: &Connection,
    session_id: &str,
    message_id: &str,
    call_id: &str,
    output: &str,
) -> Result<(), String> {
    let has_metadata: bool = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM pragma_table_info('messages') WHERE name='metadata')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| "Unable to load private protocol transcript")?;
    if !has_metadata {
        return Ok(());
    }
    let metadata: Option<String> = conn
        .query_row(
            "SELECT metadata FROM messages WHERE id=?1 AND session_id=?2 AND role='assistant'",
            params![message_id, session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "Unable to load private protocol transcript")?
        .flatten();
    let Some(mut messages) = private_transcript_from_metadata(metadata.as_deref())? else {
        return Ok(());
    };
    let matching: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            (message.role == "tool" && message.tool_call_id.as_deref() == Some(call_id))
                .then_some(index)
        })
        .collect();
    let [index] = matching.as_slice() else {
        return Err("Private protocol transcript has no unique tool result".into());
    };
    messages[*index].content = output.to_owned();
    // Preserve unrelated metadata, such as the compressed marker, while
    // validating the complete replay payload under its original schema.
    let updated = private_transcript_metadata(&messages)?
        .ok_or("Private protocol transcript has no tool result")?;
    let updated: serde_json::Value =
        serde_json::from_str(&updated).map_err(|_| "Invalid private protocol transcript")?;
    let mut metadata: serde_json::Value = serde_json::from_str(metadata.as_deref().unwrap_or("{}"))
        .map_err(|_| "Invalid history metadata")?;
    metadata[PRIVATE_TRANSCRIPT_KEY] = updated[PRIVATE_TRANSCRIPT_KEY].clone();
    let encoded =
        serde_json::to_string(&metadata).map_err(|_| "Invalid private protocol transcript")?;
    if encoded.len() > 16 * 1024 * 1024 {
        return Err("Private protocol transcript exceeded storage limit".into());
    }
    let changed = conn
        .execute(
            "UPDATE messages SET metadata=?1 WHERE id=?2 AND session_id=?3 AND role='assistant'",
            params![encoded, message_id, session_id],
        )
        .map_err(|_| "Unable to persist private protocol transcript")?;
    if changed != 1 {
        return Err("Private protocol transcript is no longer available".into());
    }
    Ok(())
}

/// Decode a frontend-facing tool call from the inline protocol JSON column.
pub(crate) fn parse_inline_tool_calls(json: Option<&str>) -> Option<Vec<ToolCallInfo>> {
    let value: serde_json::Value = serde_json::from_str(json?).ok()?;
    let calls: Vec<ToolCallInfo> = value
        .as_array()?
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.to_string();
            let name = item.get("name")?.as_str()?.to_string();
            let arguments = item.get("arguments")?;
            Some(ToolCallInfo {
                id,
                name,
                arguments: match arguments {
                    serde_json::Value::String(value) => value.clone(),
                    other => other.to_string(),
                },
            })
        })
        .collect();
    (!calls.is_empty()).then_some(calls)
}

/// Decode the same inline column into provider-native tool calls for replay.
fn parse_inline_llm_tool_calls(json: Option<&str>) -> Option<Vec<LlmToolCall>> {
    let value: serde_json::Value = serde_json::from_str(json?).ok()?;
    let calls: Vec<LlmToolCall> = value
        .as_array()?
        .iter()
        .filter_map(|item| {
            Some(LlmToolCall {
                id: item.get("id")?.as_str()?.to_string(),
                name: item.get("name")?.as_str()?.to_string(),
                arguments: item.get("arguments")?.clone(),
            })
        })
        .collect();
    (!calls.is_empty()).then_some(calls)
}

/// Rebuild missing frontend tool protocol from durable `agent_steps` rows.
///
/// New assistant rows keep provider tool calls inline, but results still live
/// in the tool/step rows.  Hydration therefore has to fill either half of the
/// protocol independently.  Returning early merely because calls are already
/// present leaves successful work looking permanently in progress after a
/// reload.
pub(crate) fn hydrate_agent_steps(
    conn: &Connection,
    messages: &mut [Message],
) -> Result<(), String> {
    if !messages.iter().any(|message| {
        message.role == "assistant"
            && (message.tool_calls.is_none() || message.tool_results.is_none())
    }) {
        return Ok(());
    }

    for message in messages.iter_mut().filter(|message| {
        message.role == "assistant"
            && (message.tool_calls.is_none() || message.tool_results.is_none())
    }) {
        let mut stmt = conn
            .prepare(
                "SELECT COALESCE(call_id, id), tool_name, tool_input, tool_output, success
                 FROM agent_steps
                 WHERE session_id = ?1 AND created_at = ?2
                 ORDER BY seq ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![&message.session_id, message.created_at], |row| {
                let id: String = row.get(0)?;
                let tool_name: String = row.get(1)?;
                let output = row.get::<_, Option<String>>(3)?.unwrap_or_default();
                let success: i32 = row.get(4)?;
                let confirmation_required =
                    success != 1 && output.contains("Confirmation required");
                let confirmation_status = if confirmation_required {
                    Some("pending".to_string())
                } else if output.contains("Confirmation rejected") {
                    Some("rejected".to_string())
                } else if output.contains("Confirmation cancelled") {
                    Some("cancelled".to_string())
                } else if output.contains("Confirmation approved") {
                    Some("approved".to_string())
                } else {
                    None
                };
                Ok((
                    ToolCallInfo {
                        id: id.clone(),
                        name: tool_name.clone(),
                        arguments: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    },
                    ToolResultInfo {
                        call_id: id,
                        tool_name,
                        success: success == 1,
                        output: output.clone(),
                        error: (success != 1).then_some(output),
                        confirmation_required,
                        confirmation_status,
                    },
                ))
            })
            .map_err(|e| e.to_string())?;
        let steps = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        if !steps.is_empty() {
            let (calls, results): (Vec<_>, Vec<_>) = steps.into_iter().unzip();
            if message.tool_calls.is_none() {
                message.tool_calls = Some(calls);
            }
            if message.tool_results.is_none() {
                message.tool_results = Some(results);
            }
        }
    }
    Ok(())
}

/// Recreate provider-native assistant/tool pairs from the inline transcript.
pub(crate) fn hydrate_llm_history(
    conn: &Connection,
    session_id: &str,
    before_created_at: i64,
) -> Result<Vec<LlmMessage>, String> {
    let has_provisional = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM pragma_table_info('messages') WHERE name = 'is_provisional')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        != 0;
    let visible = if has_provisional {
        " AND is_provisional = 0"
    } else {
        ""
    };
    let mut stmt = conn
        .prepare(&format!(
            "SELECT role, content, tool_calls, tool_call_id, metadata, created_at FROM (
               SELECT role, content, tool_calls, tool_call_id, metadata, created_at, id
               FROM messages
               WHERE session_id = ?1 AND created_at < ?2{visible}
                 AND (COALESCE(metadata, '') NOT LIKE '%\"compressed\": true%'
                      OR metadata LIKE '%\"privateProtocolTranscript\"%')
               ORDER BY created_at DESC, id DESC
               LIMIT CASE WHEN EXISTS (
                 SELECT 1 FROM messages
                 WHERE session_id = ?1 AND created_at < ?2{visible}
                   AND role = 'assistant'
                   AND metadata LIKE '%\"privateProtocolTranscript\"%'
               ) THEN -1 ELSE 100 END
             )
             ORDER BY created_at ASC,
                      CASE WHEN role = 'assistant' THEN 0
                           WHEN role = 'tool' THEN 1
                           ELSE 2 END,
                      id ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![session_id, before_created_at], |row| {
            let role: String = row.get(0)?;
            let content: String = row.get(1)?;
            let metadata: Option<String> = row.get(4)?;
            let content =
                foreground_text_attachments::render_stored(&content, &role, metadata.as_deref())
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Text,
                            error.into(),
                        )
                    })?;
            Ok((
                role,
                content,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                metadata,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let rows = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut decoded_rows = Vec::with_capacity(rows.len());
    let mut private_turn_times = BTreeSet::new();
    for row in rows {
        let ledger = if row.0 == "assistant" {
            private_transcript_from_metadata(row.4.as_deref())?
        } else {
            None
        };
        if ledger.is_some() {
            private_turn_times.insert(row.5);
        }
        decoded_rows.push((row, ledger));
    }
    let mut history = Vec::new();
    for ((role, content, tool_calls, tool_call_id, _, created_at), ledger) in decoded_rows {
        if let Some(ledger) = ledger {
            history.extend(ledger);
        } else if role != "tool" || !private_turn_times.contains(&created_at) {
            let protocol_role = match role.as_str() {
                "assistant" => "assistant",
                "tool" => "tool",
                _ => "user",
            };
            history.push(LlmMessage {
                role: protocol_role.into(),
                content,
                tool_calls: (protocol_role == "assistant")
                    .then(|| parse_inline_llm_tool_calls(tool_calls.as_deref()))
                    .flatten(),
                tool_call_id: (protocol_role == "tool").then_some(tool_call_id).flatten(),
                tool_images: Vec::new(),
                protocol_state: None,
            });
        }
    }
    // Providers require assistant tool calls and tool results to be an exact
    // pair.  Historical data can contain an interrupted turn, so sanitize the
    // replay projection instead of passing an invalid transcript back to the
    // provider.  This does not mutate the user-visible audit history.
    let result_ids: BTreeSet<String> = history
        .iter()
        .filter_map(|message| {
            (message.role == "tool")
                .then_some(message.tool_call_id.as_deref())
                .flatten()
        })
        .map(str::to_owned)
        .collect();
    for message in &history {
        if message.protocol_state.is_some()
            && message
                .tool_calls
                .as_ref()
                .is_some_and(|calls| calls.iter().any(|call| !result_ids.contains(&call.id)))
        {
            // Do not edit a subset of calls while retaining the complete opaque
            // output items: the two would contradict each other.
            return Err("Private protocol transcript contains an unsettled tool batch".into());
        }
    }
    let retained_call_ids: BTreeSet<String> = history
        .iter()
        .filter(|message| message.role == "assistant")
        .flat_map(|message| message.tool_calls.clone().unwrap_or_default())
        .filter(|call| result_ids.contains(&call.id))
        .map(|call| call.id)
        .collect();

    Ok(history
        .into_iter()
        .filter_map(|mut message| {
            if message.role == "tool"
                && !message
                    .tool_call_id
                    .as_deref()
                    .is_some_and(|call_id| retained_call_ids.contains(call_id))
            {
                return None;
            }
            if message.protocol_state.is_none() {
                message.tool_calls = message
                    .tool_calls
                    .map(|calls| {
                        calls
                            .into_iter()
                            .filter(|call| retained_call_ids.contains(&call.id))
                            .collect::<Vec<_>>()
                    })
                    .filter(|calls| !calls.is_empty());
            }
            Some(message)
        })
        .collect())
}

fn is_provider_terminal_failure(tool_name: &str, output: &str) -> bool {
    tool_name == "agent_loop"
        && (output.starts_with("模型连续") || output.starts_with("模型请求未完成"))
}

fn is_soft_paused_agent_response(content: &str) -> bool {
    content.starts_with("已暂停，等待用户介入（")
}

pub(crate) fn summarize_task_run(message: &Message, goal: String) -> Option<AgentTaskRun> {
    let tool_calls = message.tool_calls.as_ref()?;
    if tool_calls.is_empty() {
        return None;
    }
    let results = message.tool_results.as_deref().unwrap_or(&[]);
    let has_pending_confirmation = results
        .iter()
        .any(|r| r.confirmation_required && r.confirmation_status.as_deref() == Some("pending"));
    let has_failure = results.iter().any(|r| !r.success);
    let has_provider_terminal_failure = results
        .iter()
        .any(|r| is_provider_terminal_failure(&r.tool_name, &r.output));
    let completed_step_count = results.iter().filter(|r| r.success).count();
    let run_state = derive_run_state(RunStateInput {
        total_steps: tool_calls.len(),
        completed_steps: completed_step_count,
        has_pending_confirmation,
        has_rejected_confirmation: results
            .iter()
            .any(|r| r.confirmation_status.as_deref() == Some("rejected")),
        has_approved_confirmation: results
            .iter()
            .any(|r| r.confirmation_status.as_deref() == Some("approved")),
        has_failure,
        provider_terminal_failure: has_provider_terminal_failure,
        explicit_continue_after_approval: false,
    });
    let (status, confirmation_state, resumable) = if is_soft_paused_agent_response(&message.content)
    {
        (
            crate::agent::AgentRunStatus::NeedsAttention
                .as_str()
                .to_string(),
            crate::agent::ConfirmationState::None.as_str().to_string(),
            true,
        )
    } else {
        (
            run_state.status.as_str().to_string(),
            run_state.confirmation.as_str().to_string(),
            run_state.resumable,
        )
    };
    Some(AgentTaskRun {
        id: message.id.clone(),
        goal,
        status,
        plan: tool_calls.iter().map(|call| call.name.clone()).collect(),
        confirmation_state,
        resumable,
        step_count: tool_calls.len(),
        completed_step_count,
    })
}

pub(crate) fn hydrate_task_runs(messages: &mut [Message]) {
    let mut last_user_goal: Option<String> = None;
    for message in messages {
        if message.role == "user" {
            last_user_goal = Some(message.content.clone());
            continue;
        }
        if message.role != "assistant" || message.task_run.is_some() {
            continue;
        }
        if let Some(goal) = last_user_goal.clone() {
            message.task_run = summarize_task_run(message, goal);
        }
    }
}

pub(crate) fn table_exists(conn: &Connection, table_name: &str) -> Result<bool, String> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
            params![table_name],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(exists > 0)
}

pub(crate) fn hydrate_task_runs_from_db(
    conn: &Connection,
    messages: &mut [Message],
) -> Result<(), String> {
    if !table_exists(conn, "task_runs")? {
        hydrate_task_runs(messages);
        return Ok(());
    }
    for message in messages.iter_mut() {
        let stored = conn
            .query_row(
                "SELECT id, goal, status, plan, confirmation_state, resumable, step_count, completed_step_count
                 FROM task_runs WHERE message_id = ?1",
                params![&message.id],
                |row| {
                    let plan = serde_json::from_str::<Vec<String>>(&row.get::<_, String>(3)?)
                        .unwrap_or_default();
                    Ok(AgentTaskRun {
                        id: row.get(0)?,
                        goal: row.get(1)?,
                        status: row.get(2)?,
                        plan,
                        confirmation_state: row.get(4)?,
                        resumable: row.get::<_, i32>(5)? != 0,
                        step_count: row.get::<_, i64>(6)? as usize,
                        completed_step_count: row.get::<_, i64>(7)? as usize,
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if stored.is_some() {
            message.task_run = stored;
        }
    }
    hydrate_task_runs(messages);
    Ok(())
}

pub(crate) fn hydrate_task_facts_from_db(
    conn: &Connection,
    messages: &mut [Message],
) -> Result<(), String> {
    if !table_exists(conn, "task_run_facts")? {
        return Ok(());
    }
    for message in messages.iter_mut().filter(|m| m.role == "assistant") {
        let json: Option<String> = conn
            .query_row(
                "SELECT facts_json FROM task_run_facts WHERE message_id = ?1",
                params![&message.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        message.task_facts = json
            .map(|text| serde_json::from_str::<TaskFacts>(&text))
            .transpose()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn legacy_task_facts(task_run: &AgentTaskRun) -> TaskFacts {
    let mut facts = TaskFacts::new(task_run.goal.clone());
    facts.plan = task_run
        .plan
        .iter()
        .enumerate()
        .map(|(index, description)| PlanStepFact {
            id: format!("legacy-plan-{}", index + 1),
            description: description.clone(),
            depends_on: Vec::new(),
            status: if index < task_run.completed_step_count {
                PlanStepStatus::Completed
            } else {
                PlanStepStatus::Pending
            },
        })
        .collect();
    facts.completed_steps = facts
        .plan
        .iter()
        .filter(|step| step.status == PlanStepStatus::Completed)
        .map(|step| step.id.clone())
        .collect();
    facts.terminal_reason = match task_run.status.as_str() {
        "completed" => Some(TaskTerminalReason::Completed),
        "awaiting_confirmation" => Some(TaskTerminalReason::AwaitingConfirmation),
        "needs_attention" | "continue_suggested" | "provider_unavailable" => {
            Some(TaskTerminalReason::NeedsAttention)
        }
        "stopped" => Some(TaskTerminalReason::Stopped),
        _ => None,
    };
    facts
}

fn persist_legacy_task_facts(
    conn: &Connection,
    task_run: &AgentTaskRun,
    message: &Message,
    now: i64,
) -> Result<(), String> {
    if !table_exists(conn, "task_run_facts")? {
        return Ok(());
    }
    persist_task_facts(
        conn,
        &task_run.id,
        &message.session_id,
        &message.id,
        &legacy_task_facts(task_run),
        now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_replays_only_complete_tool_call_pairs() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, work_dir, created_at, updated_at)
             VALUES ('s', 'test', '.', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES ('u', 's', 'user', 'inspect', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, tool_calls, created_at)
             VALUES ('a', 's', 'assistant', 'queued', ?1, 2)",
            [r#"[{"id":"complete","name":"delegate_work","arguments":"{}"},{"id":"orphan","name":"read_file","arguments":"{}"}]"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, tool_call_id, tool_name, created_at)
             VALUES ('t', 's', 'tool', 'queued receipt', 'complete', 'delegate_work', 2)",
            [],
        )
        .unwrap();

        let history = hydrate_llm_history(&conn, "s", 10).unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[1].role, "assistant");
        assert_eq!(history[1].tool_calls.as_ref().unwrap().len(), 1);
        assert_eq!(history[1].tool_calls.as_ref().unwrap()[0].id, "complete");
        assert_eq!(history[2].role, "tool");
        assert_eq!(history[2].tool_call_id.as_deref(), Some("complete"));
    }

    #[test]
    fn plain_conversation_cannot_create_an_agent_step() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();

        let error = persist_tool_results(
            &conn,
            "session",
            1,
            None,
            &[ToolResultInfo {
                call_id: "not-a-tool-call".to_string(),
                tool_name: "conversation".to_string(),
                success: true,
                output: "A supportive reply".to_string(),
                error: None,
                confirmation_required: false,
                confirmation_status: None,
            }],
        )
        .unwrap_err();

        assert!(error.contains("matching provider tool calls"));
        let persisted: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_steps", [], |row| row.get(0))
            .unwrap();
        assert_eq!(persisted, 0);
    }
}

pub(crate) fn persist_task_run(conn: &Connection, message: &Message) -> Result<(), String> {
    let Some(task_run) = message.task_run.as_ref() else {
        return Ok(());
    };
    if !table_exists(conn, "task_runs")? {
        return Ok(());
    }
    let plan = serde_json::to_string(&task_run.plan).map_err(|e| e.to_string())?;
    let now = Utc::now().timestamp();
    // `REPLACE` is a delete followed by an insert in SQLite.  A foreground
    // run can be the durable parent of delegated work, so deleting it would
    // cascade-delete its queued delegation during normal reply promotion.
    conn.execute(
        "INSERT INTO task_runs (
            id, session_id, message_id, goal, status, plan, confirmation_state,
            resumable, step_count, completed_step_count, continuation_context,
            created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, '',
            ?11, ?11)
         ON CONFLICT(id) DO UPDATE SET
            session_id=excluded.session_id,
            message_id=excluded.message_id,
            goal=excluded.goal,
            status=excluded.status,
            plan=excluded.plan,
            confirmation_state=excluded.confirmation_state,
            resumable=excluded.resumable,
            step_count=excluded.step_count,
            completed_step_count=excluded.completed_step_count,
            continuation_context=excluded.continuation_context,
            updated_at=excluded.updated_at",
        params![
            &task_run.id,
            &message.session_id,
            &message.id,
            &task_run.goal,
            &task_run.status,
            plan,
            &task_run.confirmation_state,
            if task_run.resumable { 1 } else { 0 },
            task_run.step_count as i64,
            task_run.completed_step_count as i64,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;
    persist_legacy_task_facts(conn, task_run, message, now)
}

/// Update a durable task summary after a confirmation continuation changes a
/// step row.  This deliberately remains connection-scoped so the caller can
/// update the step and summary under one transaction boundary if needed.
pub(crate) fn refresh_persisted_task_run_from_steps(
    conn: &Connection,
    session_id: &str,
    message_id: &str,
    message_created_at: i64,
) -> Result<(), String> {
    if !table_exists(conn, "task_runs")? {
        return Ok(());
    }
    let mut stmt = conn
        .prepare(
            "SELECT tool_name, COALESCE(tool_output, ''), success
             FROM agent_steps
             WHERE session_id = ?1 AND created_at = ?2
             ORDER BY seq ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![session_id, message_created_at], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i32>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let steps = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if steps.is_empty() {
        return Ok(());
    }
    let plan: Vec<String> = steps.iter().map(|(name, _, _)| name.clone()).collect();
    let step_count = steps.len() as i64;
    let completed_step_count = steps.iter().filter(|(_, _, success)| *success == 1).count() as i64;
    let run_state = derive_run_state(RunStateInput {
        total_steps: step_count as usize,
        completed_steps: completed_step_count as usize,
        has_pending_confirmation: steps
            .iter()
            .any(|(_, output, success)| *success != 1 && output.contains("Confirmation required")),
        has_rejected_confirmation: steps
            .iter()
            .any(|(_, output, _)| output.contains("Confirmation rejected")),
        has_approved_confirmation: steps
            .iter()
            .any(|(_, output, success)| *success == 1 && output.contains("Confirmation approved")),
        has_failure: steps.iter().any(|(_, _, success)| *success != 1),
        provider_terminal_failure: steps
            .iter()
            .any(|(name, output, _)| is_provider_terminal_failure(name, output)),
        explicit_continue_after_approval: true,
    });
    let plan_json = serde_json::to_string(&plan).map_err(|e| e.to_string())?;
    let continuation_context = steps
        .iter()
        .map(|(name, output, success)| {
            let output = output.chars().take(600).collect::<String>();
            let state = if *success != 0 {
                "宸插畬鎴?"
            } else {
                "鏈畬鎴?"
            };
            format!("{state} {name}: {output}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    conn.execute(
        "UPDATE task_runs SET status=?1, plan=?2, confirmation_state=?3,
             resumable=?4, step_count=?5, completed_step_count=?6,
             continuation_context=?7, updated_at=?8
         WHERE session_id=?9 AND message_id=?10",
        params![
            run_state.status.as_str(),
            plan_json,
            run_state.confirmation.as_str(),
            if run_state.resumable { 1 } else { 0 },
            step_count,
            completed_step_count,
            continuation_context,
            Utc::now().timestamp(),
            session_id,
            message_id,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Persist provider tool results and their audit rows with a visible assistant
/// reply. This is deliberately a tool-only boundary: plain user/assistant
/// conversation cannot produce an `agent_steps` row. Every row must prove its
/// identity by matching a provider-issued tool call. The caller owns the
/// transaction.
pub(crate) fn persist_tool_results(
    conn: &Connection,
    session_id: &str,
    created_at: i64,
    tool_calls: Option<&[ToolCallInfo]>,
    steps: &[ToolResultInfo],
) -> Result<(), String> {
    persist_tool_results_with_mcp_revisions(
        conn,
        session_id,
        created_at,
        tool_calls,
        steps,
        &crate::agent::handlers::McpToolRevisionCatalog::new(),
    )
}

/// Persist tool results and bind pending MCP confirmations to the exact
/// definition revision the Main Agent saw while preparing this turn. The
/// catalog is created alongside the foreground registry, never refreshed here.
pub(crate) fn persist_tool_results_with_mcp_revisions(
    conn: &Connection,
    session_id: &str,
    created_at: i64,
    tool_calls: Option<&[ToolCallInfo]>,
    steps: &[ToolResultInfo],
    mcp_tool_revisions: &crate::agent::handlers::McpToolRevisionCatalog,
) -> Result<(), String> {
    if steps.is_empty() {
        return Ok(());
    }
    let calls = tool_calls.ok_or_else(|| {
        "refusing to persist agent_steps without matching provider tool calls".to_string()
    })?;
    let matched_calls = steps
        .iter()
        .map(|step| {
            if step.call_id.trim().is_empty() || step.tool_name.trim().is_empty() {
                return Err(
                    "refusing to persist an agent_step without a tool call identity".to_string(),
                );
            }
            calls
                .iter()
                .find(|call| call.id == step.call_id && call.name == step.tool_name)
                .ok_or_else(|| {
                    format!(
                        "refusing to persist agent_step {} without a matching provider tool call",
                        step.call_id
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (index, (step, call)) in steps.iter().zip(matched_calls).enumerate() {
        let tool_output = if step.success {
            step.output.clone()
        } else {
            step.error.clone().unwrap_or_else(|| step.output.clone())
        };
        conn.execute(
            "INSERT OR IGNORE INTO messages
               (id, session_id, role, content, tool_call_id, tool_name, created_at)
             VALUES (?1, ?2, 'tool', ?3, ?4, ?5, ?6)",
            params![
                Uuid::new_v4().to_string(),
                session_id,
                tool_output,
                step.call_id,
                step.tool_name,
                created_at,
            ],
        )
        .map_err(|e| format!("persist tool message {}: {}", step.call_id, e))?;
        let mcp_schema_version = step
            .confirmation_required
            .then(|| mcp_tool_revisions.get(&step.tool_name))
            .flatten();
        conn.execute(
            "INSERT INTO agent_steps (id, call_id, session_id, tool_name, tool_input, tool_output, mcp_schema_version, success, seq, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                Uuid::new_v4().to_string(),
                &step.call_id,
                session_id,
                &step.tool_name,
                call.arguments,
                tool_output,
                mcp_schema_version,
                step.success,
                (index + 1) as i32,
                created_at,
            ],
        )
        .map_err(|e| format!("persist agent_step {}: {}", step.call_id, e))?;
    }
    Ok(())
}
