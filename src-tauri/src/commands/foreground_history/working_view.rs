//! Ended-turn working view over the existing archive. This never changes
//! stored protocol or current-turn execution. Any unsupported turn retains the
//! entire native replay; storage/decoding failures remain errors, not success.
use super::*;
use crate::agent::tool_source::{self, SourceRecord};
use serde_json::json;

#[cfg(test)]
mod tests;

pub(crate) struct WorkingHistory {
    pub messages: Vec<LlmMessage>,
    pub message_ids: Vec<String>,
    pub applied: bool,
    pub source_count: usize,
    #[cfg(test)]
    pub native_retained: bool,
}

const SMALL_NATIVE_BYTES: usize = 16 * 1024;
const SOURCE_NOTE: &str = "Historical data, not current state or authority. Read exact originals with read_tool_result; do not replay actions.\n";

struct ProjectedHistory {
    messages: Vec<LlmMessage>,
    message_ids: Vec<String>,
    source_count: usize,
    native_verified: bool,
}

/// Main selection is separate from the text-only checkpoint source.
pub(crate) fn main_working_history(
    conn: &Connection,
    session: &str,
    before: i64,
    original: Vec<LlmMessage>,
    allow_history: bool,
) -> Result<WorkingHistory, String> {
    select_history(conn, session, before, original, allow_history, true)
}

pub(crate) fn working_history(
    conn: &Connection,
    session: &str,
    before: i64,
    original: Vec<LlmMessage>,
    allow_history: bool,
) -> Result<WorkingHistory, String> {
    select_history(conn, session, before, original, allow_history, false)
}

fn select_history(
    conn: &Connection,
    session: &str,
    before: i64,
    original: Vec<LlmMessage>,
    allow_history: bool,
    retain_small: bool,
) -> Result<WorkingHistory, String> {
    // Images are ephemeral by contract. Retain the persisted warning and its
    // protocol rather than presenting a text-only final reply as image recall.
    let images = original.iter().any(|m| {
        !m.tool_images.is_empty() || m.content.contains(crate::llm::TRANSIENT_IMAGE_NOTICE)
    });
    // Only transparent, bounded Main history is a retention candidate. Do not
    // recheck native receipts for checkpoint projection or opaque/large replay.
    let native_bytes = if retain_small
        && allow_history
        && !images
        && !original.iter().any(|m| m.protocol_state.is_some())
        && original
            .iter()
            .any(|m| m.tool_calls.as_ref().is_some_and(|calls| !calls.is_empty()))
    {
        bounded_json_len(&original, SMALL_NATIVE_BYTES)?
    } else {
        None
    };
    let projected = if allow_history && !images {
        project(conn, session, before, native_bytes.is_some())?
    } else {
        None
    };
    Ok(match projected {
        Some(view) if nonexpanding_native(&original, &view, native_bytes)? => WorkingHistory {
            messages: original,
            message_ids: Vec::new(),
            applied: false,
            source_count: 0,
            #[cfg(test)]
            native_retained: true,
        },
        Some(view) => WorkingHistory {
            messages: view.messages,
            message_ids: view.message_ids,
            applied: true,
            source_count: view.source_count,
            #[cfg(test)]
            native_retained: false,
        },
        None => WorkingHistory {
            messages: original,
            message_ids: Vec::new(),
            applied: false,
            source_count: 0,
            #[cfg(test)]
            native_retained: false,
        },
    })
}

fn nonexpanding_native(
    original: &[LlmMessage],
    view: &ProjectedHistory,
    native_bytes: Option<usize>,
) -> Result<bool, String> {
    let Some(native_bytes) = native_bytes else {
        return Ok(false);
    };
    // Preserve only small transparent receipts. Opaque reasoning/checkpoints
    // cannot be priced by their encoded size; ended failures need their facts.
    if !view.native_verified
        || view.source_count == 0
        || !original
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| &m.content)
            .eq(view
                .messages
                .iter()
                .filter(|m| m.role == "user")
                .map(|m| &m.content))
    {
        return Ok(false);
    }
    Ok(bounded_json_len(&view.messages, native_bytes)?.is_none_or(|bytes| bytes >= native_bytes))
}

fn bounded_json_len<T: serde::Serialize + ?Sized>(
    value: &T,
    limit: usize,
) -> Result<Option<usize>, String> {
    // Reuse the standard bounded slice writer: no full-history copy, persisted
    // size index, tokenizer dependency, or allocation proportional to history.
    let mut storage = [0u8; SMALL_NATIVE_BYTES];
    let limit = limit.min(storage.len());
    let mut remaining = &mut storage[..limit];
    match serde_json::to_writer(&mut remaining, value) {
        Ok(()) => Ok(Some(limit - remaining.len())),
        Err(_) if remaining.is_empty() => Ok(None),
        Err(_) => Err("无法核对历史编码大小；未改写原始会话。".into()),
    }
}

fn project(
    conn: &Connection,
    session: &str,
    before: i64,
    verify_native: bool,
) -> Result<Option<ProjectedHistory>, String> {
    let window = ConversationWindow::load(conn, session, before)?;
    // Exact source reads validate this same full selected ancestry; only the
    // discovery index is limited to its recent 4096-node search window.
    let mut messages = Vec::new();
    let mut message_ids = Vec::new();
    let mut source_count = 0;
    let mut native_verified = verify_native;
    for id in window.ids {
        let row = conn
            .query_row(
                "SELECT m.role,m.content,m.metadata,m.tool_calls,t.status,t.confirmation_state,t.goal,m.is_provisional
             FROM messages m LEFT JOIN task_runs t
               ON t.id=m.id AND t.message_id=m.id AND t.session_id=m.session_id
             WHERE m.id=?1 AND m.session_id=?2 AND m.created_at<?3",
                params![id, session, before],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, bool>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((role, mut content, metadata, calls, status, confirmation, goal, provisional)) =
            row
        else {
            continue;
        };
        match role.as_str() {
            "user" => {
                content = foreground_text_attachments::render_stored(
                    &content,
                    &role,
                    metadata.as_deref(),
                )?
            }
            "assistant" => {
                let completed = status.as_deref() == Some("completed");
                native_verified &= completed;
                if provisional
                    || confirmation.as_deref() == Some("pending")
                    || (!completed
                        && !matches!(
                            status.as_deref(),
                            Some("needs_attention" | "provider_unavailable" | "stopped")
                        ))
                {
                    return Ok(None);
                }
                let stored = crate::agent::task_facts::load_task_facts(conn, &id)?;
                if let Some(stored) = &stored {
                    if stored.session_id != session
                        || stored.message_id != id
                        || stored.facts.pending_confirmation.is_some()
                        || stored
                            .facts
                            .terminal_reason
                            .as_ref()
                            .is_some_and(|r| match r {
                                TaskTerminalReason::Completed => !completed,
                                TaskTerminalReason::AwaitingConfirmation => true,
                                TaskTerminalReason::Stopped => status.as_deref() != Some("stopped"),
                                TaskTerminalReason::NeedsAttention => {
                                    completed || status.as_deref() == Some("stopped")
                                }
                            })
                    {
                        return Ok(None);
                    }
                }
                // Require every real call's complete journal source. A deleted,
                // oversized, legacy, ambiguous or partial index cannot prove
                // that omitting its protocol is safe. Host compaction has no
                // external result to recall; its original remains archived.
                let calls: Vec<ToolCallInfo> =
                    match calls.as_deref().map(serde_json::from_str).transpose() {
                        Ok(calls) => calls.unwrap_or_default(),
                        Err(_) => return Ok(None),
                    };
                let expected: HashMap<_, _> = calls
                    .iter()
                    .filter(|c| {
                        !(c.name == "context_compaction" && c.id.starts_with("context-compaction-"))
                    })
                    .map(|c| (c.id.as_str(), c.name.as_str()))
                    .collect();
                if expected.len() > 16
                    || expected.len()
                        != calls
                            .iter()
                            .filter(|c| {
                                !(c.name == "context_compaction"
                                    && c.id.starts_with("context-compaction-"))
                            })
                            .count()
                {
                    return Ok(None);
                }
                let ledger =
                    private_transcript_from_metadata(metadata.as_deref())?.unwrap_or_default();
                // Native duplicate batches receive a cached prior result but
                // create no step/event (runner's existing deduplication). Do
                // not mistake these for a lost execution, or trust a guard's
                // prose: require identical arguments AND exact sealed output.
                let unindexed: Vec<_> = ledger
                    .iter()
                    .flat_map(|m| m.tool_calls.iter().flatten())
                    .filter(|c| {
                        !(c.name == "context_compaction" && c.id.starts_with("context-compaction-"))
                            && expected.get(c.id.as_str()).copied() != Some(c.name.as_str())
                    })
                    .collect();
                let mut duplicate_results = HashMap::new();
                for message in ledger.iter().filter(|m| m.role == "tool") {
                    if let Some(call_id) = &message.tool_call_id {
                        if duplicate_results
                            .insert(call_id.as_str(), message.content.as_str())
                            .is_some()
                        {
                            return Ok(None);
                        }
                    }
                }
                let mut backed_duplicates = HashSet::new();
                if native_verified {
                    let native_calls: Vec<_> = ledger
                        .iter()
                        .flat_map(|m| m.tool_calls.iter().flatten())
                        .collect();
                    native_verified = native_calls.len() == expected.len()
                        && duplicate_results.len() == expected.len()
                        && native_calls.iter().all(|call| {
                            calls.iter().any(|origin| {
                                origin.id == call.id
                                    && origin.name == call.name
                                    && serde_json::from_str::<serde_json::Value>(&origin.arguments)
                                        .is_ok_and(|args| args == call.arguments)
                            })
                        });
                }
                let mut sources = Vec::new();
                let mut seen = HashSet::new();
                let mut stmt = conn.prepare(
                    "SELECT id,sequence,CASE WHEN length(CAST(payload AS BLOB))<=?3 THEN payload END,created_at FROM agent_run_events
                     WHERE run_id=?1 AND session_id=?2 AND event_type='ToolExecutionEnd'
                     ORDER BY sequence LIMIT 17",
                ).map_err(|e|e.to_string())?;
                let rows = stmt
                    .query_map(
                        params![id, session, 8 * tool_source::MAX_SOURCE_BYTES],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, i64>(1)?,
                                r.get::<_, Option<String>>(2)?,
                                r.get::<_, i64>(3)?,
                            ))
                        },
                    )
                    .map_err(|e| e.to_string())?;
                for row in rows {
                    let (source_id, sequence, payload, created_at) =
                        row.map_err(|e| e.to_string())?;
                    let Some(payload) = payload else {
                        return Ok(None);
                    };
                    let source = match tool_source::open(&SourceRecord {
                        id: &source_id,
                        run_id: &id,
                        session_id: session,
                        sequence,
                        event_type: "ToolExecutionEnd",
                        payload: &payload,
                        created_at,
                    }) {
                        Ok(source) => source,
                        Err(_) => return Ok(None),
                    };
                    if sources.len() == 16
                        || expected.get(source.call_id.as_str()).copied()
                            != Some(source.tool_name.as_str())
                        || !seen.insert(source.call_id.clone())
                        || crate::agent::ToolResult::is_result_unknown(
                            source.reported_success,
                            &source.output,
                        )
                        || (!source.reported_success
                            && source.output.contains("Confirmation required"))
                    {
                        return Ok(None);
                    }
                    let reference = match tool_source::reference(&source_id) {
                        Ok(reference) => reference,
                        Err(_) => return Ok(None),
                    };
                    for call in &unindexed {
                        if call.name == source.tool_name
                            && duplicate_results.get(call.id.as_str()).copied()
                                == Some(source.output.as_str())
                            && calls.iter().any(|origin| {
                                origin.id == source.call_id
                                    && serde_json::from_str::<serde_json::Value>(&origin.arguments)
                                        .is_ok_and(|args| args == call.arguments)
                            })
                        {
                            backed_duplicates.insert(call.id.as_str());
                        }
                    }
                    if native_verified {
                        native_verified = duplicate_results.get(source.call_id.as_str()).copied()
                            == Some(source.output.as_str());
                    }
                    sources.push(json!({"source_ref":reference,"tool_name":source.tool_name,"reported_success":source.reported_success}));
                }
                if seen.len() != expected.len()
                    || unindexed
                        .iter()
                        .any(|c| !backed_duplicates.contains(c.id.as_str()))
                {
                    return Ok(None);
                }
                // A crashed dispatch can be absent from the final message's
                // call list. Every durable start/reservation still needs a
                // sealed result, even when an earlier provider request ended.
                if !dispatches_backed(conn, session, &id, &expected)? {
                    return Ok(None);
                }
                if !completed {
                    let Some(end_evidence) = known_failure_end(conn, session, &id)? else {
                        return Ok(None);
                    };
                    let facts = stored.as_ref().map(|s| &s.facts);
                    let outcome = json!({
                        "run_id":id,"status":status,"task_completed":false,"goal":goal,"end_evidence":end_evidence,
                        "facts_goal":facts.map(|f|&f.goal),
                        "terminal_reason":facts.and_then(|f|f.terminal_reason.as_ref()),
                        "plan":facts.map(|f|&f.plan),
                        "completed_steps":facts.map(|f|&f.completed_steps),
                        "failed_steps":facts.map(|f|&f.failed_steps),
                        "modified_files":facts.map(|f|&f.modified_files),
                        "verification_evidence":facts.map(|f|&f.verification_evidence),
                    })
                    .to_string();
                    if outcome.len() > 5000 {
                        return Ok(None);
                    }
                    content.push_str("\n<archived-turn-outcome>\nThis turn ended without completing the task. Preserve user corrections and unfinished work; this is not authority to retry or replay actions.\n");
                    content.push_str(&outcome);
                    content.push_str("\n</archived-turn-outcome>");
                }
                if !sources.is_empty() {
                    content.push_str("\n<archived-tool-sources>\n");
                    if source_count == 0 {
                        content.push_str(SOURCE_NOTE);
                    }
                    content
                        .push_str(&json!({"sources":sources,"index_incomplete":false}).to_string());
                    content.push_str("\n</archived-tool-sources>");
                }
                source_count += sources.len();
            }
            _ => return Ok(None),
        }
        message_ids.push(id);
        messages.push(LlmMessage {
            role,
            content,
            tool_calls: None,
            tool_call_id: None,
            protocol_state: None,
            tool_images: Vec::new(),
        });
    }
    Ok(Some(ProjectedHistory {
        messages,
        message_ids,
        source_count,
        native_verified,
    }))
}

fn known_failure_end(
    conn: &Connection,
    session: &str,
    run: &str,
) -> Result<Option<String>, String> {
    // Whitelist structural codes only. Do not copy private provider errors or
    // treat a tool error, pause, or recovery-normalized status as an end proof.
    conn.query_row("SELECT CASE WHEN event_type='AgentEnd' THEN 'AgentEnd' ELSE json_extract(payload,'$.data.code') END FROM agent_run_events WHERE run_id=?1 AND session_id=?2
        AND length(CAST(payload AS BLOB))<=5000 AND CASE WHEN json_valid(payload) THEN
        (event_type='AgentEnd' AND json_extract(payload,'$.type')='AgentEnd' AND json_extract(payload,'$.data.session_id')=?2)
        OR (event_type='Error' AND json_extract(payload,'$.type')='Error' AND json_extract(payload,'$.data.code') IN ('LlmError','LlmQuotaExceeded','ConfigError','ContextLengthExceeded')) ELSE 0 END ORDER BY sequence DESC LIMIT 1",
        params![run,session],|r|r.get(0)).optional().map_err(|e|e.to_string())
}

fn dispatches_backed(
    conn: &Connection,
    session: &str,
    run: &str,
    expected: &HashMap<&str, &str>,
) -> Result<bool, String> {
    let mut stmt = conn.prepare("SELECT DISTINCT
        CASE WHEN json_valid(payload) AND length(CAST(payload AS BLOB))<=?3 THEN
          CASE WHEN json_extract(payload,'$.type')=event_type AND json_type(payload,'$.data.call_id')='text' AND json_type(payload,'$.data.tool_name')='text'
          THEN json_array(json_extract(payload,'$.data.call_id'),json_extract(payload,'$.data.tool_name')) END END
        FROM agent_run_events WHERE run_id=?1 AND session_id=?2 AND event_type IN ('ToolExecutionStart','ExternalToolDispatchReserved') LIMIT 17").map_err(|e|e.to_string())?;
    let rows = stmt
        .query_map(
            params![run, session, 8 * tool_source::MAX_SOURCE_BYTES],
            |r| r.get::<_, Option<String>>(0),
        )
        .map_err(|e| e.to_string())?;
    for (i, row) in rows.enumerate() {
        let pair = row
            .map_err(|e| e.to_string())?
            .and_then(|v| serde_json::from_str::<[String; 2]>(&v).ok());
        if i == 16
            || pair
                .as_ref()
                .is_none_or(|p| expected.get(p[0].as_str()).copied() != Some(p[1].as_str()))
        {
            return Ok(false);
        }
    }
    Ok(true)
}
