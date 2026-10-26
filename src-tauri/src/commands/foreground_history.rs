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
    let calls: Vec<LlmToolCall> = parse_inline_tool_calls(json)?
        .into_iter()
        .filter_map(|call| {
            Some(LlmToolCall {
                id: call.id,
                name: call.name,
                arguments: serde_json::from_str(&call.arguments).ok()?,
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
    // Legacy transcript rows can share a timestamp. Preserve their append
    // order; role priority or random IDs can put replies before user input.
    let mut stmt = conn
        .prepare(&format!(
            "SELECT role, content, tool_calls, tool_call_id, metadata, created_at FROM (
               SELECT role, content, tool_calls, tool_call_id, metadata, created_at, rowid AS insertion_order
               FROM messages
               WHERE session_id = ?1 AND created_at < ?2{visible}
                 AND (COALESCE(metadata, '') NOT LIKE '%\"compressed\": true%'
                      OR metadata LIKE '%\"privateProtocolTranscript\"%')
               ORDER BY created_at DESC, insertion_order DESC
               LIMIT CASE WHEN EXISTS (
                 SELECT 1 FROM messages
                 WHERE session_id = ?1 AND created_at < ?2{visible}
                   AND role = 'assistant'
                   AND metadata LIKE '%\"privateProtocolTranscript\"%'
               ) THEN -1 ELSE 100 END
             )
             ORDER BY created_at ASC, insertion_order ASC"
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
    // Decode from the SQLite iterator and release each raw metadata string.
    // Keeping every encrypted JSON row beside its decoded ledger doubles the
    // retained payload before the replay projection is even assembled.
    let mut decoded_rows = Vec::new();
    let mut private_turn_times = BTreeSet::new();
    for row in rows {
        let (role, content, tool_calls, tool_call_id, metadata, created_at) =
            row.map_err(|e| e.to_string())?;
        let ledger = if role == "assistant" {
            private_transcript_from_metadata(metadata.as_deref())?
        } else {
            None
        };
        if ledger.is_some() {
            private_turn_times.insert(created_at);
        }
        decoded_rows.push((
            (role, content, tool_calls, tool_call_id, created_at),
            ledger,
        ));
    }
    let mut history = Vec::new();
    for ((role, content, tool_calls, tool_call_id, created_at), ledger) in decoded_rows {
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

fn has_unknown_result(message: &Message) -> bool {
    message.tool_results.iter().flatten().any(|result| {
        crate::agent::ToolResult::is_result_unknown(
            result.success,
            result.error.as_deref().unwrap_or(&result.output),
        )
    })
}

/// Count proven repairs, without cloning tool output bodies or trusting an
/// empty/stale TaskFacts record. Missing/ambiguous call bindings stay failed.
fn repaired_failure_count(calls: &[ToolCallInfo], results: &[ToolResultInfo]) -> usize {
    if results.iter().all(|result| result.success) {
        return 0;
    }
    let by_id: std::collections::BTreeMap<_, _> =
        calls.iter().map(|call| (call.id.as_str(), call)).collect();
    if by_id.len() != calls.len()
        || calls.iter().any(|call| call.id.is_empty())
        || results
            .iter()
            .map(|result| &result.call_id)
            .collect::<BTreeSet<_>>()
            .len()
            != results.len()
    {
        return 0;
    }
    let arguments = |result: &ToolResultInfo| -> Option<serde_json::Value> {
        let call = by_id.get(result.call_id.as_str())?;
        if call.name != result.tool_name {
            return None;
        }
        let args: serde_json::Value = serde_json::from_str(&call.arguments).ok()?;
        args.is_object().then_some(args)
    };
    results
        .iter()
        .enumerate()
        .filter(|(index, failed)| {
            if failed.success || failed.confirmation_status.as_deref() == Some("pending") {
                return false;
            }
            let Some(failed_args) = arguments(failed) else {
                return false;
            };
            results[index + 1..].iter().any(|succeeded| {
                succeeded.success
                    && succeeded.tool_name == failed.tool_name
                    && arguments(succeeded).is_some_and(|args| {
                        crate::agent::operation_repair_matches(
                            &failed.tool_name,
                            &failed.call_id,
                            &failed_args,
                            &failed.output,
                            &succeeded.tool_name,
                            &args,
                        )
                    })
            })
        })
        .count()
}

fn receipt_run_state(
    calls: &[ToolCallInfo],
    results: &[ToolResultInfo],
    explicit_continue: bool,
) -> crate::agent::run_state::DerivedRunState {
    let repaired = repaired_failure_count(calls, results);
    let unknown = results
        .iter()
        .any(|result| crate::agent::ToolResult::is_result_unknown(result.success, &result.output));
    derive_run_state(RunStateInput {
        // A repaired rejection/failed attempt is settled, not a fake success.
        // Public counts and receipts retain the complete, truthful attempt log.
        total_steps: calls.len().saturating_sub(repaired),
        completed_steps: results.iter().filter(|result| result.success).count(),
        has_pending_confirmation: results.iter().any(|result| {
            result.confirmation_required && result.confirmation_status.as_deref() == Some("pending")
        }),
        has_rejected_confirmation: results
            .iter()
            .any(|result| result.confirmation_status.as_deref() == Some("rejected")),
        has_approved_confirmation: results
            .iter()
            .any(|result| result.confirmation_status.as_deref() == Some("approved")),
        has_failure: unknown || results.iter().filter(|result| !result.success).count() > repaired,
        has_unknown_result: unknown,
        provider_terminal_failure: results
            .iter()
            .any(|result| is_provider_terminal_failure(&result.tool_name, &result.output)),
        explicit_continue_after_approval: explicit_continue,
    })
}

pub(crate) fn summarize_task_run(message: &Message, goal: String) -> Option<AgentTaskRun> {
    let stopped = message.task_facts.as_ref().is_some_and(|facts| {
        facts.terminal_reason == Some(crate::agent::task_facts::TaskTerminalReason::Stopped)
    });
    let tool_calls = message.tool_calls.as_deref().unwrap_or(&[]);
    if tool_calls.is_empty() && !stopped {
        return None;
    }
    let results = message.tool_results.as_deref().unwrap_or(&[]);
    let has_unknown_result = has_unknown_result(message);
    let completed_step_count = results.iter().filter(|r| r.success).count();
    let run_state = receipt_run_state(tool_calls, results, false);
    let (status, confirmation_state, resumable) = if stopped {
        (
            crate::agent::AgentRunStatus::Stopped.as_str().to_string(),
            crate::agent::ConfirmationState::None.as_str().to_string(),
            false,
        )
    } else if is_soft_paused_agent_response(&message.content) {
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
        resumable: resumable && !has_unknown_result,
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
        if let Some(mut stored) = stored {
            // Older rows can carry a stale Continue bit. The actual receipt
            // still governs projection, just as it does continuation admission.
            stored.resumable &= !has_unknown_result(message);
            message.task_run = Some(stored);
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
    fn corrected_rejection_stays_completed_after_refresh_and_reopen() {
        let rejection = serde_json::json!({"code":"HOST_ARGUMENTS_REJECTED","dispatched":false,
            "call_id":"bad","tool_name":"snapshot","null_corrections":["depth"],"message":"not executed"}).to_string();
        for (output, expected) in [
            (rejection.clone(), "completed"),
            (format!("Error: {rejection}"), "needs_attention"),
            (
                "Error: {\"code\":\"RESULT_UNKNOWN\"}".into(),
                "needs_attention",
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("data.db");
            let conn = Connection::open(&path).unwrap();
            crate::db::migrate(&conn).unwrap();
            conn.execute_batch("INSERT INTO sessions(id,title,created_at,updated_at) VALUES('s','test',1,1);
                INSERT INTO messages(id,session_id,role,content,created_at) VALUES('a','s','assistant','Actual state checked',2);").unwrap();
            let calls = vec![
                ToolCallInfo {
                    id: "bad".into(),
                    name: "snapshot".into(),
                    arguments: r#"{"target":"a","depth":null}"#.into(),
                },
                ToolCallInfo {
                    id: "good".into(),
                    name: "snapshot".into(),
                    arguments: r#"{"target":"a","depth":10}"#.into(),
                },
            ];
            let results = vec![
                ToolResultInfo {
                    call_id: "bad".into(),
                    tool_name: "snapshot".into(),
                    success: false,
                    output,
                    error: None,
                    confirmation_required: false,
                    confirmation_status: None,
                },
                ToolResultInfo {
                    call_id: "good".into(),
                    tool_name: "snapshot".into(),
                    success: true,
                    output: "fresh observed state".into(),
                    error: None,
                    confirmation_required: false,
                    confirmation_status: None,
                },
            ];
            let mut message = Message {
                id: "a".into(),
                session_id: "s".into(),
                role: "assistant".into(),
                content: "Actual state checked".into(),
                text_attachments: vec![],
                created_at: 2,
                parent_id: None,
                is_deleted: None,
                tool_calls: Some(calls),
                tool_results: Some(results),
                task_run: None,
                task_facts: Some(TaskFacts::new("observe target a")),
            };
            let run = summarize_task_run(&message, "observe target a".into()).unwrap();
            assert_eq!(run.status, expected);
            assert_eq!(
                (run.step_count, run.completed_step_count),
                (2, 1),
                "a rejected attempt must not become a fake successful operation"
            );
            message.task_run = Some(run);
            persist_tool_results(
                &conn,
                "s",
                2,
                message.tool_calls.as_deref(),
                message.tool_results.as_deref().unwrap(),
            )
            .unwrap();
            persist_task_run(&conn, &message).unwrap();
            refresh_persisted_task_run_from_steps(&conn, "s", "a", 2).unwrap();
            drop(conn);
            let conn = Connection::open(path).unwrap();
            message.task_run = None;
            let mut history = vec![message];
            hydrate_task_runs_from_db(&conn, &mut history).unwrap();
            assert_eq!(history[0].task_run.as_ref().unwrap().status, expected);
            assert_eq!(
                conn.query_row(
                    "SELECT success FROM agent_steps WHERE call_id='bad'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
    }

    #[test]
    fn unknown_tool_outcomes_are_not_resumable_in_current_or_legacy_history() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute_batch("INSERT INTO sessions(id,title,work_dir,created_at,updated_at) VALUES('s','test','.',1,1);
            INSERT INTO messages(id,session_id,role,content,created_at) VALUES('a','s','assistant','Inspect the target first',2);").unwrap();
        let mut message = Message {
            id: "a".into(),
            session_id: "s".into(),
            role: "assistant".into(),
            text_attachments: vec![],
            content: "Inspect the target first".into(),
            created_at: 2,
            parent_id: None,
            is_deleted: None,
            task_run: None,
            task_facts: None,
            tool_calls: Some(vec![ToolCallInfo {
                id: "call".into(),
                name: "mcp_owned_write".into(),
                arguments: "{}".into(),
            }]),
            tool_results: Some(vec![ToolResultInfo {
                call_id: "call".into(),
                tool_name: "mcp_owned_write".into(),
                success: false,
                output: r#"Error: {"code":"RESULT_UNKNOWN","message":"inspect first"}"#.into(),
                error: None,
                confirmation_required: false,
                confirmation_status: None,
            }]),
        };
        let run = summarize_task_run(&message, "Save draft".into()).unwrap();
        assert_eq!(run.status, "needs_attention");
        assert!(
            !run.resumable,
            "live unknown results must not offer Continue"
        );
        message.task_run = Some(run);
        persist_tool_results(
            &conn,
            "s",
            2,
            message.tool_calls.as_deref(),
            message.tool_results.as_deref().unwrap(),
        )
        .unwrap();
        persist_task_run(&conn, &message).unwrap();
        refresh_persisted_task_run_from_steps(&conn, "s", "a", 2).unwrap();
        assert_eq!(
            conn.query_row("SELECT resumable FROM task_runs WHERE id='a'", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );

        // Existing installations may contain the old resumable bit. Project the
        // actual receipt without rewriting history or changing schema.
        conn.execute("UPDATE task_runs SET resumable=1 WHERE id='a'", [])
            .unwrap();
        message.task_run = None;
        let mut history = vec![message];
        hydrate_task_runs_from_db(&conn, &mut history).unwrap();
        assert!(!history[0].task_run.as_ref().unwrap().resumable);

        let mut message = history.remove(0);
        for (success, output, expected) in [
            (false, "Temporary failure before dispatch", true),
            (true, r#"{"code":"RESULT_UNKNOWN"}"#, false),
        ] {
            let result = &mut message.tool_results.as_mut().unwrap()[0];
            result.success = success;
            result.output = output.into();
            let run = summarize_task_run(&message, "Save draft".into()).unwrap();
            assert_eq!(run.resumable, expected);
        }
    }

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
        assert_eq!(
            history[1].tool_calls.as_ref().unwrap()[0].arguments,
            serde_json::json!({}),
            "frontend JSON-string arguments must be restored as structured model arguments"
        );
        assert_eq!(history[2].role, "tool");
        assert_eq!(history[2].tool_call_id.as_deref(), Some("complete"));

        let expected = serde_json::json!({ "app_id": "observer", "mode": "image" });
        for arguments in [serde_json::json!(expected.to_string()), expected.clone()] {
            conn.execute(
                "UPDATE messages SET tool_calls = ?1 WHERE id = 'a'",
                [serde_json::json!([{
                    "id": "complete",
                    "name": "delegate_work",
                    "arguments": arguments,
                }])
                .to_string()],
            )
            .unwrap();
            let history = hydrate_llm_history(&conn, "s", 10).unwrap();
            assert_eq!(history.len(), 3);
            assert_eq!(
                history[1].tool_calls.as_ref().unwrap()[0].arguments,
                expected
            );
        }

        conn.execute(
            "UPDATE messages SET tool_calls = ?1 WHERE id = 'a'",
            [serde_json::json!([{
                "id": "complete",
                "name": "delegate_work",
                "arguments": "{invalid",
            }])
            .to_string()],
        )
        .unwrap();
        let history = hydrate_llm_history(&conn, "s", 10).unwrap();
        assert_eq!(history.len(), 2);
        assert!(history[1].tool_calls.is_none());
        assert!(history.iter().all(|message| message.role != "tool"));
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
            "SELECT COALESCE(call_id, id), tool_name, COALESCE(tool_input, 'null'), COALESCE(tool_output, ''), success
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
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i32>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let steps = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if steps.is_empty() {
        return Ok(());
    }
    let (calls, results): (Vec<_>, Vec<_>) = steps
        .into_iter()
        .map(|(id, name, input, output, success)| {
            let pending = success != 1 && output.contains("Confirmation required");
            let status = if pending {
                Some("pending")
            } else if output.contains("Confirmation rejected") {
                Some("rejected")
            } else if success == 1 && output.contains("Confirmation approved") {
                Some("approved")
            } else {
                None
            };
            (
                ToolCallInfo {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: input,
                },
                ToolResultInfo {
                    call_id: id,
                    tool_name: name,
                    success: success == 1,
                    output,
                    error: None,
                    confirmation_required: pending,
                    confirmation_status: status.map(str::to_owned),
                },
            )
        })
        .unzip();
    let plan: Vec<_> = calls.iter().map(|call| call.name.clone()).collect();
    let step_count = calls.len() as i64;
    let completed_step_count = results.iter().filter(|result| result.success).count() as i64;
    let run_state = receipt_run_state(&calls, &results, true);
    let plan_json = serde_json::to_string(&plan).map_err(|e| e.to_string())?;
    let continuation_context = results
        .iter()
        .map(|result| {
            let output = result.output.chars().take(600).collect::<String>();
            let state = if result.success {
                "已完成"
            } else {
                "未成功"
            };
            format!("{state} {}: {output}", result.tool_name)
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
