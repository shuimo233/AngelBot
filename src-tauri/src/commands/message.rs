//! Message commands — handles chat message send/receive with personality injection
//!
//! The `send_message` command now:
//! 1. Persists the user message
//! 2. Checks token count and auto-compresses if over threshold
//! 3. Fetches user profile and relevant memories from DB
//! 4. Builds a personalized AgentConfig from frontend data + DB data
//! 5. Runs the agent with personality-aware system prompt
//! 6. Persists the AI reply and updates memory frequencies

use crate::agent::compaction::checkpoint_prompt;
use crate::agent::config::{
    AgentConfig, MemoryContext, PersonalityTraits, UserPreferences, UserProfile,
};
use crate::agent::event::{
    AgentEvent, AgentEventEmitter, AgentRunJournal, DurableEventEmitter, TauriEventEmitter,
};
use crate::agent::foreground_lifecycle_control::ForegroundLifecycleControl;
use crate::agent::task_facts::{
    persist_task_facts, PlanStepFact, PlanStepStatus, TaskFacts, TaskTerminalReason,
};
use crate::agent::AgentRunner;
use crate::agent::{derive_run_state, RunStateInput, ToolCall, ToolExecutionContext};
use crate::commands::foreground_history::{
    hydrate_agent_steps, hydrate_llm_history, hydrate_task_facts_from_db, hydrate_task_runs,
    hydrate_task_runs_from_db, parse_inline_tool_calls, persist_task_run, persist_tool_results,
    refresh_persisted_task_run_from_steps, refresh_private_transcript_tool_result,
    summarize_task_run, table_exists,
};
pub use crate::commands::foreground_message_contracts::{
    AgentTaskRun, EditAndResendMessageRequest, Message, PersonalityPayload, PreferencesPayload,
    SendMessageRequest, ToolCallInfo, ToolResultInfo, TraitsPayload, WebSearchSetupPayload,
};
use crate::commands::foreground_run_store::{
    terminalize_pending_confirmation_in_tx, BeginForegroundTurn, ForegroundRunAdmission,
    ForegroundRunAdmissionGuard, ForegroundRunBeginResult, ForegroundRunStore,
};
use crate::commands::foreground_text_attachments;
use crate::commands::foreground_tool_surface::normalize_agent_execution_permission;
pub(crate) use crate::commands::foreground_tool_surface::{
    agent_execution_permission, agent_execution_permission_from_conn,
    AGENT_EXECUTION_PERMISSION_KEY, ESTIMATED_TOOL_COUNT,
};
use crate::llm::{Message as LlmMessage, ToolCall as LlmToolCall, DEEPSEEK_DEFAULT_MODEL};
use crate::AppState;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

/// Generate a short Chinese title (5–15 chars) from the first user message content.
async fn generate_title_from_content(
    content: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let prompt = format!(
        "请根据以下用户提问，用 5 到 15 个汉字生成一个简短的中文会话标题，只需返回标题，不要任何解释或标点符号。\n\n用户提问：{}",
        content.trim()
    );
    let title = crate::commands::call_deepseek(&[], &prompt).await?;
    Ok(title.trim().chars().take(20).collect::<String>())
}

/// Estimate token count for mixed Chinese/English text.
/// Claude: English ~3.3 chars/token, CJK ~1.3 chars/token
/// GPT-4: English ~4 chars/token, CJK ~1.5 chars/token
/// Use a middle-ground: English / 3.5 + CJK / 1.5
fn estimate_tokens(text: &str) -> usize {
    let ascii_count = text.chars().filter(|c| c.is_ascii()).count();
    let non_ascii_count = text.chars().count() - ascii_count;
    (ascii_count as f64 / 3.5) as usize + (non_ascii_count as f64 / 1.5) as usize
}

/// Estimate the token count of the system prompt (built from AgentConfig + tools).
/// Used to deduct from the context window budget.
fn estimate_system_prompt_tokens(config: &crate::agent::AgentConfig, tool_count: usize) -> usize {
    // Base sections always present
    let mut total = 150; // work_dir + identity
    total += 200; // personality traits (verbose Chinese guidelines)
    if config.profile.is_some() {
        total += 150;
    }
    if !config.core_memories.is_empty() {
        total += config.core_memories.len() * 60;
    }
    if config.knowledge_graph_context.is_some() {
        total += 100;
    }
    if !config.preferences.interests.is_empty() || !config.preferences.avoid_topics.is_empty() {
        total += 100;
    }
    if !config.relevant_memories.is_empty() {
        total += config.relevant_memories.len() * 40;
    }
    // Tool section: ~30 tokens per tool
    total += tool_count * 30;
    // Plan-Execute-Reflect section
    total += 300;
    if config.context_summary.is_some() {
        total += 200;
    }
    total
}

/// Read a setting value — checks the in-memory cache first, falls back to DB.
fn get_setting(state: &AppState, key: &str, default: &str) -> String {
    // Try cache first (fast path, no DB lock needed)
    if let Ok(cache) = state.cached_settings.read() {
        if let Some(v) = cache.get(key) {
            return v.clone();
        }
    }
    // Fall back to DB
    if let Ok(conn) = state.db.lock() {
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_else(|_| default.to_string())
    } else {
        default.to_string()
    }
}

/// Parse the assistant message's inline `tool_calls` JSON column into a vector
/// of `ToolCallInfo` records. Returns `None` when the column is absent,
/// malformed, or carries no entries — matching the semantics of the column
/// being NULL.

/// Recreate the provider protocol for persisted tool-use turns. The visible
/// assistant reply stays in the transcript, while its durable tool calls and
/// tool results are appended as a valid assistant-tool / tool-result exchange
/// before the next user prompt.
///
/// After migration 034 the messages table itself stores the protocol inline:
/// - `assistant` rows may carry a `tool_calls` JSON array
/// - `tool` rows carry the `tool_call_id`, `tool_name`, and output content
///
/// We no longer join against `agent_steps` for protocol reconstruction — that
/// join was fragile (it relied on `agent_steps.created_at = messages.created_at`
/// which fails whenever the assistant message is regenerated, e.g. after
/// `handleRegenerateMessage`).
#[derive(Debug, Deserialize)]
pub struct ResolveAgentConfirmationRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "messageId")]
    pub message_id: String,
    #[serde(rename = "callId")]
    pub call_id: String,
    pub decision: String,
    /// Opaque backend-issued binding for a live desktop target. Required for
    /// every external-app write or control invocation.
    #[serde(default, rename = "previewId")]
    pub preview_id: Option<String>,
}

/// User-initiated follow-up for a previously bounded agent slice.
#[derive(Debug, Deserialize)]
pub struct ContinueAgentTaskRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "messageId")]
    pub message_id: String,
}

/// Explicitly dismiss a resumable slice. This only changes durable task state;
/// it never replays or rolls back a tool action.
#[derive(Debug, Deserialize)]
pub struct CancelAgentTaskRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "messageId")]
    pub message_id: String,
}

/// The exact durable state a confirmation resolver is allowed to settle.
///
/// A call id alone is deliberately insufficient: provider ids can be reused,
/// and a stale browser IPC must not settle a different attempt or revive a
/// task that has already been stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingConfirmation {
    step_id: String,
    message_created_at: i64,
    tool_name: String,
    tool_input: String,
    mcp_schema_version: Option<String>,
}

const DESKTOP_ACTION_PREVIEW_TTL: Duration = Duration::from_secs(60);
const DESKTOP_ACTION_INTERRUPTED: &str = r#"{"code":"RESULT_UNKNOWN","message":"Desktop action may have been interrupted. Inspect the target application before retrying."}"#;

/// Ephemeral, runtime-owned preview binding. A restart intentionally invalidates
/// it, so a stale confirmation must inspect the live window again.
struct DesktopActionPreviewLease {
    session_id: String,
    message_id: String,
    call_id: String,
    pending: PendingConfirmation,
    app: crate::commands::desktop::TrustedDesktopApp,
    target: crate::desktop_control::DesktopDraftTargetIdentity,
    expires_at: Instant,
}

#[derive(Default)]
pub struct DesktopActionPreviewStore {
    leases: Mutex<HashMap<String, DesktopActionPreviewLease>>,
}

impl DesktopActionPreviewStore {
    /// Configuration changes invalidate every outstanding live preview,
    /// including deleting and re-adding an otherwise identical entry.
    pub(crate) fn revoke_for_app(&self, app_id: &str) -> Result<(), String> {
        self.leases
            .lock()
            .map_err(|error| error.to_string())?
            .retain(|_, lease| lease.app.id != app_id);
        Ok(())
    }

    fn insert(&self, lease: DesktopActionPreviewLease) -> Result<String, String> {
        let mut leases = self.leases.lock().map_err(|error| error.to_string())?;
        leases.retain(|_, value| value.expires_at > Instant::now());
        if leases.len() >= 64 {
            if let Some(oldest) = leases
                .iter()
                .min_by_key(|(_, value)| value.expires_at)
                .map(|(id, _)| id.clone())
            {
                leases.remove(&oldest);
            }
        }
        let id = Uuid::new_v4().to_string();
        leases.insert(id.clone(), lease);
        Ok(id)
    }

    fn take(
        &self,
        id: &str,
        req: &ResolveAgentConfirmationRequest,
        pending: &PendingConfirmation,
    ) -> Result<DesktopActionPreviewLease, String> {
        let mut leases = self.leases.lock().map_err(|error| error.to_string())?;
        leases.retain(|_, value| value.expires_at > Instant::now());
        let lease = leases
            .remove(id)
            .ok_or_else(|| "Desktop preview expired; inspect the target again".to_string())?;
        if lease.session_id != req.session_id
            || lease.message_id != req.message_id
            || lease.call_id != req.call_id
            || &lease.pending != pending
        {
            return Err("Desktop preview no longer matches the pending action".to_string());
        }
        Ok(lease)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopActionConfirmationPreview {
    preview_id: String,
    operation: &'static str,
    app_display_name: String,
    executable_name: String,
    window_title: Option<String>,
    control_name: String,
    text: Option<String>,
    /// Unix milliseconds, only for the confirmation UI countdown.
    expires_at: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDraftArguments {
    app_id: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredFieldArguments {
    app_id: String,
    field_ref: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredControlArguments {
    app_id: String,
    control_ref: String,
    action: crate::desktop_control::DesktopControlOperation,
}

pub(crate) fn preflight_pending_desktop_action_impl(
    state: &AppState,
    session_id: &str,
    message_id: &str,
    call_id: &str,
) -> Result<DesktopActionConfirmationPreview, String> {
    let pending = {
        let conn = state.db.lock().map_err(|error| error.to_string())?;
        load_pending_confirmation(&conn, session_id, message_id, call_id)?
    };
    let (app_id, text, target_ref, operation, required_capability, control_operation) =
        match pending.tool_name.as_str() {
            "prepare_message_draft" => {
                let args: StoredDraftArguments = serde_json::from_str(&pending.tool_input)
                    .map_err(|error| format!("Stored draft arguments are invalid: {error}"))?;
                (
                    args.app_id,
                    Some(args.text),
                    None,
                    "draft",
                    crate::commands::desktop::CAPABILITY_DRAFT,
                    None,
                )
            }
            "set_trusted_app_text" => {
                let args: StoredFieldArguments = serde_json::from_str(&pending.tool_input)
                    .map_err(|error| format!("Stored field arguments are invalid: {error}"))?;
                if args.field_ref.is_empty() || args.field_ref.len() > 128 {
                    return Err("Stored field reference is invalid".to_string());
                }
                (
                    args.app_id,
                    Some(args.text),
                    Some(args.field_ref),
                    "field",
                    crate::commands::desktop::CAPABILITY_FILL,
                    None,
                )
            }
            "operate_trusted_app_control" => {
                let args: StoredControlArguments = serde_json::from_str(&pending.tool_input)
                    .map_err(|error| format!("Stored control arguments are invalid: {error}"))?;
                if args.control_ref.is_empty() || args.control_ref.len() > 128 {
                    return Err("Stored control reference is invalid".to_string());
                }
                (
                    args.app_id,
                    None,
                    Some(args.control_ref),
                    args.action.as_str(),
                    crate::commands::desktop::CAPABILITY_INTERACT,
                    Some(args.action),
                )
            }
            _ => {
                return Err("This pending action does not use a desktop target preview".to_string())
            }
        };
    if text
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.len() > 20_000 || value.contains('\0'))
    {
        return Err("Stored text is invalid".to_string());
    }
    let app = crate::commands::desktop::get_trusted_desktop_app_from_db(&state.db, &app_id)?
        .ok_or_else(|| "The target application is no longer trusted".to_string())?;
    if !app.enabled
        || !app
            .capabilities
            .iter()
            .any(|value| value == required_capability)
        || !app
            .capabilities
            .iter()
            .any(|value| value == crate::commands::desktop::CAPABILITY_OBSERVE)
        || crate::commands::desktop::trusted_desktop_app_status(&app)
            != crate::commands::desktop::STATUS_AVAILABLE
    {
        return Err("The application needs an enabled, available trusted-app entry with the requested action and observation scope".to_string());
    }
    let (target_app_id, window_title, control_name, identity) = if let Some(target_ref) = target_ref
    {
        let target = if let Some(control_operation) = control_operation {
            state.desktop_adapter.preflight_control_ref(
                &app.id,
                &app.executable_path,
                &target_ref,
                control_operation,
                Duration::from_secs(8),
            )
        } else {
            state.desktop_adapter.preflight_text_ref(
                &app.id,
                &app.executable_path,
                &target_ref,
                Duration::from_secs(8),
            )
        }
        .map_err(|error| error.to_string())?;
        (
            target.app_id,
            target.window_title,
            target.control_label,
            target.identity,
        )
    } else {
        let selector = app
            .draft_selector
            .as_deref()
            .ok_or_else(|| "The trusted draft target is missing".to_string())?;
        let target = state
            .desktop_adapter
            .preflight_draft(
                &app.id,
                &app.executable_path,
                selector,
                Duration::from_secs(8),
            )
            .map_err(|error| error.to_string())?;
        (
            target.app_id,
            target.window_title,
            target.control_name,
            target.identity,
        )
    };
    if target_app_id != app.id {
        return Err("Desktop target does not match the trusted application".to_string());
    }
    // The OS inspection may block. Recheck the durable pending step and app
    // configuration before offering a confirmation for what was just observed.
    // Keep this short critical section through insertion. Configuration
    // deletion uses the same DB -> preview order, so an in-flight old check
    // cannot insert its preview after the revocation has cleared the store.
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    let current = load_pending_confirmation(&conn, session_id, message_id, call_id)?;
    let current_app = crate::commands::desktop::get_trusted_desktop_app_from_conn(&conn, &app.id)?;
    if current != pending || current_app.as_ref() != Some(&app) {
        return Err("Desktop target changed during inspection; retry".to_string());
    }
    let preview_id = state
        .desktop_action_previews
        .insert(DesktopActionPreviewLease {
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
            call_id: call_id.to_string(),
            pending,
            app: app.clone(),
            target: identity,
            expires_at: Instant::now() + DESKTOP_ACTION_PREVIEW_TTL,
        })?;
    drop(conn);
    Ok(DesktopActionConfirmationPreview {
        preview_id,
        operation,
        app_display_name: app.display_name,
        executable_name: std::path::Path::new(&app.executable_path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("application")
            .to_string(),
        window_title,
        control_name,
        text,
        expires_at: (Utc::now()
            + chrono::Duration::from_std(DESKTOP_ACTION_PREVIEW_TTL)
                .map_err(|error| error.to_string())?)
        .timestamp_millis(),
    })
}

#[tauri::command]
pub async fn preflight_pending_desktop_action(
    app: AppHandle,
    session_id: String,
    message_id: String,
    call_id: String,
) -> Result<DesktopActionConfirmationPreview, String> {
    // UIA can take seconds or wait on a poorly behaved provider. Keep the
    // interactive command runtime responsive while the bounded helper runs.
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        preflight_pending_desktop_action_impl(&state, &session_id, &message_id, &call_id)
    })
    .await
    .map_err(|error| format!("Desktop target inspection task failed: {error}"))?
}

fn load_pending_confirmation(
    conn: &Connection,
    session_id: &str,
    message_id: &str,
    call_id: &str,
) -> Result<PendingConfirmation, String> {
    conn.query_row(
        "SELECT steps.id, messages.created_at, steps.tool_name,
                COALESCE(steps.tool_input, ''), steps.mcp_schema_version
         FROM messages
         JOIN agent_steps AS steps
           ON steps.session_id = messages.session_id
          AND steps.created_at = messages.created_at
         WHERE messages.id = ?1
           AND messages.session_id = ?2
           AND messages.role = 'assistant'
           AND COALESCE(steps.call_id, steps.id) = ?3
           AND steps.success != 1
           AND COALESCE(steps.tool_output, '') LIKE '%Confirmation required%'
           AND EXISTS (
                SELECT 1 FROM task_runs
                WHERE task_runs.session_id = ?2
                  AND task_runs.message_id = ?1
                  AND task_runs.status = 'awaiting_confirmation'
                  AND task_runs.confirmation_state = 'pending'
                  AND task_runs.resumable = 1
           )",
        params![message_id, session_id, call_id],
        |row| {
            Ok(PendingConfirmation {
                step_id: row.get(0)?,
                message_created_at: row.get(1)?,
                tool_name: row.get(2)?,
                tool_input: row.get(3)?,
                mcp_schema_version: row.get(4)?,
            })
        },
    )
    .map_err(|_| "confirmation step is no longer pending".to_string())
}

/// Persist a confirmation outcome only while the exact durable pending state
/// remains present.  The caller owns the transaction so the step, task summary
/// and facts cannot be partially revived by a late resolver.
fn settle_pending_confirmation_in_tx(
    tx: &Transaction<'_>,
    session_id: &str,
    message_id: &str,
    call_id: &str,
    pending: &PendingConfirmation,
    success: bool,
    output: &str,
) -> Result<(), String> {
    let changed = tx
        .execute(
            "UPDATE agent_steps
             SET tool_output = ?1, success = ?2
             WHERE id = ?3
               AND session_id = ?4
               AND created_at = ?5
               AND COALESCE(call_id, id) = ?6
               AND success != 1
               AND COALESCE(tool_output, '') LIKE '%Confirmation required%'
               AND EXISTS (
                    SELECT 1 FROM task_runs
                    WHERE task_runs.session_id = ?4
                      AND task_runs.message_id = ?7
                      AND task_runs.status = 'awaiting_confirmation'
                      AND task_runs.confirmation_state = 'pending'
                      AND task_runs.resumable = 1
               )",
            params![
                output,
                if success { 1 } else { 0 },
                &pending.step_id,
                session_id,
                pending.message_created_at,
                call_id,
                message_id,
            ],
        )
        .map_err(|e| e.to_string())?;
    if changed != 1 {
        return Err("confirmation step is no longer pending".to_string());
    }

    tx.execute(
        "UPDATE messages SET content=?1
         WHERE session_id=?2 AND role='tool' AND created_at=?3 AND tool_call_id=?4",
        params![output, session_id, pending.message_created_at, call_id],
    )
    .map_err(|error| error.to_string())?;
    refresh_private_transcript_tool_result(tx, session_id, message_id, call_id, output)?;
    refresh_persisted_task_run_from_steps(tx, session_id, message_id, pending.message_created_at)?;
    refresh_task_facts_after_confirmation(tx, session_id, message_id, call_id, success)
}

/// Commit this fail-closed outcome before crossing the OS side-effect boundary.
/// If the process dies after this transaction, the step is no longer pending
/// and cannot be approved again without a new model/user turn.
fn arm_desktop_action_in_tx(
    tx: &Transaction<'_>,
    session_id: &str,
    message_id: &str,
    call_id: &str,
    pending: &PendingConfirmation,
) -> Result<(), String> {
    settle_pending_confirmation_in_tx(
        tx,
        session_id,
        message_id,
        call_id,
        pending,
        false,
        DESKTOP_ACTION_INTERRUPTED,
    )?;
    let changed = tx
        .execute(
            "UPDATE messages SET content = ?1
         WHERE session_id = ?2 AND role = 'tool' AND created_at = ?3
           AND tool_call_id = ?4",
            params![
                DESKTOP_ACTION_INTERRUPTED,
                session_id,
                pending.message_created_at,
                call_id
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err(
            "Desktop action has no unique durable tool result; nothing was executed".to_string(),
        );
    }
    Ok(())
}

/// Replace only the exact armed outcome. A failed settlement leaves the
/// durable RESULT_UNKNOWN in place instead of re-opening the confirmation.
fn finish_desktop_action_in_tx(
    tx: &Transaction<'_>,
    session_id: &str,
    message_id: &str,
    call_id: &str,
    pending: &PendingConfirmation,
    success: bool,
    output: &str,
) -> Result<(), String> {
    let changed = tx
        .execute(
            "UPDATE agent_steps SET tool_output = ?1, success = ?2
             WHERE id = ?3 AND session_id = ?4 AND created_at = ?5
               AND COALESCE(call_id, id) = ?6 AND success = 0
               AND tool_output = ?7",
            params![
                output,
                if success { 1 } else { 0 },
                &pending.step_id,
                session_id,
                pending.message_created_at,
                call_id,
                DESKTOP_ACTION_INTERRUPTED,
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err(
            "Desktop action outcome can no longer be settled; inspect the target application"
                .to_string(),
        );
    }
    let changed = tx
        .execute(
            "UPDATE messages SET content = ?1
         WHERE session_id = ?2 AND role = 'tool' AND created_at = ?3
           AND tool_call_id = ?4 AND content = ?5",
            params![
                output,
                session_id,
                pending.message_created_at,
                call_id,
                DESKTOP_ACTION_INTERRUPTED,
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err(
            "Desktop action tool result changed; inspect the target application".to_string(),
        );
    }
    refresh_private_transcript_tool_result(tx, session_id, message_id, call_id, output)?;
    refresh_persisted_task_run_from_steps(tx, session_id, message_id, pending.message_created_at)?;
    refresh_task_facts_after_confirmation(tx, session_id, message_id, call_id, success)
}

pub(crate) fn cancel_agent_task_impl(
    state: &AppState,
    session_id: &str,
    message_id: &str,
) -> Result<(), String> {
    // Confirmation resolution is itself a foreground run.  Serialising this
    // durable stop with it prevents a resolver that has already loaded a tool
    // call from executing or settling after this cancellation commits.
    let _run_permit = state.steering.try_start_run(session_id)?;
    let mut conn = state.db.lock().map_err(|e| e.to_string())?;
    let now = Utc::now().timestamp();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let terminalized_confirmation = terminalize_pending_confirmation_in_tx(
        &tx,
        session_id,
        message_id,
        now,
        "Confirmation cancelled: the user stopped this task. The operation was not run.",
    )?;
    let changed = if terminalized_confirmation {
        1
    } else {
        tx.execute(
            "UPDATE task_runs
             SET status = 'stopped', resumable = 0, updated_at = ?1
             WHERE session_id = ?2 AND message_id = ?3 AND resumable = 1",
            params![now, session_id, message_id],
        )
        .map_err(|e| e.to_string())?
    };
    if changed == 0 {
        return Err("No resumable task found to cancel".to_string());
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn build_task_continuation_prompt(
    state: &AppState,
    session_id: &str,
    message_id: &str,
) -> Result<String, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    if !table_exists(&conn, "task_runs")? {
        return Err(
            "This task cannot be continued because task history is unavailable".to_string(),
        );
    }
    let (goal, status, resumable, continuation_context): (String, String, i32, String) = conn
        .query_row(
            "SELECT goal, status, resumable, continuation_context
             FROM task_runs WHERE session_id = ?1 AND message_id = ?2",
            params![session_id, message_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Task run not found".to_string())?;
    let has_unknown_result: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM agent_steps
                WHERE session_id = ?1
                  AND created_at = (
                      SELECT created_at FROM messages WHERE id = ?2 AND session_id = ?1
                  )
                  AND success = 0
                  AND COALESCE(tool_output, '') LIKE '%RESULT_UNKNOWN%'
             )",
            params![session_id, message_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if has_unknown_result {
        return Err(
            "A prior operation has an unknown result. Inspect the target before starting a new task turn."
                .to_string(),
        );
    }
    if resumable == 0 {
        return Err(format!(
            "Task is not resumable in its current state: {status}"
        ));
    }

    let facts = if continuation_context.trim().is_empty() {
        let mut stmt = conn
            .prepare(
                "SELECT tool_name, COALESCE(tool_output, ''), success
                 FROM agent_steps
                 WHERE session_id = ?1
                   AND created_at = (SELECT created_at FROM messages WHERE id = ?2 AND session_id = ?1)
                 ORDER BY seq ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![session_id, message_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)? != 0,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(tool_name, output, success)| {
                let state = if success { "已完成" } else { "未完成" };
                format!(
                    "{state} {tool_name}: {}",
                    output.chars().take(600).collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        continuation_context
    };

    Ok(format!(
        "继续以下开发任务，使用工具前先阅读已完成事实；不要重复已成功或已批准的副作用。\\n\\n原始目标：{goal}\\n\\n已知事实：\\n{facts}\\n\\n请判断下一步：如果目标已经满足，先给出验证证据；如果仍需工作，执行下一个最小步骤，并按既有确认规则请求副作用操作。"
    ))
}

fn confirmation_status_from_output(output: &str, success: bool) -> (bool, Option<String>) {
    if !success && output.contains("Confirmation required") {
        (true, Some("pending".to_string()))
    } else if output.contains("Confirmation rejected") {
        (false, Some("rejected".to_string()))
    } else if output.contains("Confirmation cancelled") {
        (false, Some("cancelled".to_string()))
    } else if success && output.contains("Confirmation approved") {
        (false, Some("approved".to_string()))
    } else {
        (false, None)
    }
}

fn refresh_task_facts_after_confirmation(
    conn: &Connection,
    session_id: &str,
    message_id: &str,
    call_id: &str,
    success: bool,
) -> Result<(), String> {
    if !table_exists(conn, "task_run_facts")? {
        return Ok(());
    }
    let stored: Option<(String, String)> = conn
        .query_row(
            "SELECT task_run_id, facts_json FROM task_run_facts
             WHERE session_id = ?1 AND message_id = ?2",
            params![session_id, message_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((task_run_id, facts_json)) = stored else {
        return Ok(());
    };
    let mut facts: TaskFacts = serde_json::from_str(&facts_json)
        .map_err(|error| format!("Stored task facts are invalid: {error}"))?;

    if facts
        .pending_confirmation
        .as_ref()
        .is_some_and(|pending| pending.call_id == call_id)
    {
        facts.pending_confirmation = None;
    }
    facts.completed_steps.retain(|step| step != call_id);
    facts.failed_steps.retain(|step| step != call_id);
    if success {
        facts.completed_steps.push(call_id.to_string());
    } else {
        facts.failed_steps.push(call_id.to_string());
    }
    if let Some(step) = facts.plan.iter_mut().find(|step| step.id == call_id) {
        step.status = if success {
            PlanStepStatus::Completed
        } else {
            PlanStepStatus::Failed
        };
    }
    facts.terminal_reason = if facts.pending_confirmation.is_some() {
        Some(TaskTerminalReason::AwaitingConfirmation)
    } else if success {
        None
    } else {
        Some(TaskTerminalReason::NeedsAttention)
    };

    persist_task_facts(
        conn,
        &task_run_id,
        session_id,
        message_id,
        &facts,
        Utc::now().timestamp(),
    )
}

pub(crate) fn resolve_agent_confirmation_impl(
    state: &AppState,
    req: ResolveAgentConfirmationRequest,
) -> Result<ToolResultInfo, String> {
    let _run_permit = state.steering.try_start_run(&req.session_id)?;
    let decision = req.decision.to_lowercase();
    if decision != "approved" && decision != "rejected" {
        return Err("decision must be 'approved' or 'rejected'".to_string());
    }

    let pending = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        load_pending_confirmation(&conn, &req.session_id, &req.message_id, &req.call_id)?
    };
    let tool_name = pending.tool_name.clone();
    let tool_input = pending.tool_input.clone();
    let mcp_schema_version = pending.mcp_schema_version.clone();
    let mut desktop_action_armed = false;

    let (new_success, new_output): (bool, String) = if decision == "rejected" {
        if tool_name == "materialize_delegated_change" {
            let arguments: serde_json::Value = serde_json::from_str(&tool_input)
                .map_err(|e| format!("stored materialization arguments are invalid JSON: {e}"))?;
            match crate::commands::foreground_delegation_control::reject_materialize_change_confirmation(
                state.delegation_pump.as_ref(),
                &req.session_id,
                &arguments,
            ) {
                Ok(()) => (
                    false,
                    "Confirmation rejected: user discarded the reviewed delegated change; its temporary resources are being cleaned up.".to_string(),
                ),
                Err(error) => (
                    false,
                    format!(
                        "Confirmation rejected: the reviewed delegated change could not be discarded ({error}). Cleanup remains pending."
                    ),
                ),
            }
        } else {
            (
                false,
                format!(
                    "Confirmation rejected: user rejected side-effect tool '{}'.",
                    tool_name
                ),
            )
        }
    } else {
        let registry =
            crate::commands::foreground_tool_surface::ForegroundToolSurface::new(state, None)
                .build_for_confirmation(&req.session_id, &req.message_id, &req.call_id, &tool_name);
        let unavailable = match registry.get(&tool_name) {
            None => Some(format!(
                "Confirmation could not execute '{}': the capability is no longer available in the current workspace. The operation was not run.",
                tool_name
            )),
            Some(tool) if !tool.requires_confirmation => Some(format!(
                "Confirmation could not execute '{}': this tool does not use the confirmation flow. The operation was not run.",
                tool_name
            )),
            Some(_) => None,
        };
        if let Some(error) = unavailable {
            (false, error)
        } else {
            let arguments: serde_json::Value = serde_json::from_str(&tool_input)
                .map_err(|e| format!("stored tool arguments are invalid JSON: {}", e))?;

            // Tools that need workspace files are absent from personal space. The
            // remaining handlers do not use this argument, so a neutral existing
            // directory lets reminders, goals and desktop actions be approved
            // without silently granting Documents access.
            let work_dir = crate::commands::file::resolve_work_dir(state, Some(&req.session_id))
                .unwrap_or_else(|_| std::env::temp_dir());
            match registry.validate_arguments(&tool_name, &arguments) {
                Ok(()) => {
                    // The run permit serializes normal resolution and explicit
                    // cancellation. Re-read the exact durable pending state at
                    // the side-effect boundary as a second line of defence for
                    // stale IPC or future callers that bypass that permit.
                    let current = {
                        let conn = state.db.lock().map_err(|e| e.to_string())?;
                        load_pending_confirmation(
                            &conn,
                            &req.session_id,
                            &req.message_id,
                            &req.call_id,
                        )?
                    };
                    if current.step_id != pending.step_id {
                        return Err("confirmation step is no longer pending".to_string());
                    }
                    let desktop_target = if matches!(
                        tool_name.as_str(),
                        "prepare_message_draft" | "set_trusted_app_text" | "operate_trusted_app_control"
                    ) {
                        let preview_id = req.preview_id.as_deref().ok_or_else(|| {
                            "Inspect the live desktop target before approving".to_string()
                        })?;
                        let lease = state
                            .desktop_action_previews
                            .take(preview_id, &req, &current)?;
                        let current_app = crate::commands::desktop::get_trusted_desktop_app_from_db(
                            &state.db,
                            &lease.app.id,
                        )?;
                        if current_app.as_ref() != Some(&lease.app) {
                            return Err("Trusted application changed; inspect the target again".to_string());
                        }
                        Some(lease.target)
                    } else {
                        None
                    };
                    if desktop_target.is_some() {
                        let mut conn = state.db.lock().map_err(|error| error.to_string())?;
                        let tx = conn.unchecked_transaction().map_err(|error| error.to_string())?;
                        arm_desktop_action_in_tx(
                            &tx,
                            &req.session_id,
                            &req.message_id,
                            &req.call_id,
                            &pending,
                        )?;
                        tx.commit().map_err(|error| error.to_string())?;
                        desktop_action_armed = true;
                    }
                    state.steering.mark_running(&req.session_id);
                    let tool_call = ToolCall::new(tool_name.clone(), arguments);
                    let _ = AgentRunJournal::new(state.db.clone(), req.message_id.clone()).append(
                        &req.session_id,
                        &AgentEvent::ToolExecutionStart {
                            call_id: req.call_id.clone(),
                            tool_name: tool_name.clone(),
                            arguments: tool_call.arguments.clone(),
                        },
                    );
                    let mut context = ToolExecutionContext::new(
                        &req.call_id,
                        state.steering.cancellation(&req.session_id),
                    )
                    .with_run_id(&req.message_id)
                    .for_confirmation(mcp_schema_version);
                    if let Some(target) = desktop_target {
                        context = context.with_desktop_expected_target(target);
                    }
                    let exec = registry.execute_with_context(
                        &tool_call,
                        &req.call_id,
                        &work_dir,
                        &context,
                    );
                    if exec.result.requires_user_review() {
                        // Preserve the structured code for the confirmation
                        // response and durable history. A free-form wrapper
                        // would hide RESULT_UNKNOWN from the UI and could
                        // cause an automatic continuation after approval.
                        (false, exec.result.content.clone())
                    } else if exec.result.success {
                        (
                            true,
                            format!("Confirmation approved: {}", exec.result.content),
                        )
                    } else {
                        (
                            false,
                            format!(
                                "Confirmation could not complete '{}': {}.",
                                tool_name, exec.result.content
                            ),
                        )
                    }
                }
                Err(error) => (
                    false,
                    format!(
                        "Confirmation could not execute '{}': saved tool arguments are invalid ({error}). The operation was not run.",
                        tool_name
                    ),
                ),
            }
        }
    };

    {
        let mut conn = state.db.lock().map_err(|e| e.to_string())?;
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        if desktop_action_armed {
            finish_desktop_action_in_tx(
                &tx,
                &req.session_id,
                &req.message_id,
                &req.call_id,
                &pending,
                new_success,
                &new_output,
            )?;
        } else {
            settle_pending_confirmation_in_tx(
                &tx,
                &req.session_id,
                &req.message_id,
                &req.call_id,
                &pending,
                new_success,
                &new_output,
            )?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }

    let (confirmation_required, confirmation_status) =
        confirmation_status_from_output(&new_output, new_success);
    // Rejection has no corresponding execution-start event, but it remains a
    // terminal audit fact for this pending call. Approved calls append a start
    // above; both paths append exactly one end event here.
    let _ = AgentRunJournal::new(state.db.clone(), req.message_id.clone()).append(
        &req.session_id,
        &AgentEvent::ToolExecutionEnd {
            call_id: req.call_id.clone(),
            tool_name: tool_name.clone(),
            success: new_success,
            output: new_output.clone(),
            error: (!new_success).then(|| new_output.clone()),
        },
    );
    Ok(ToolResultInfo {
        call_id: req.call_id,
        tool_name,
        success: new_success,
        output: new_output.clone(),
        error: if new_success { None } else { Some(new_output) },
        confirmation_required,
        confirmation_status,
    })
}

// SendMessageRequest is defined in foreground_message_contracts.
// ─── Personality injection ───────────────────────────────────────────
// personality/preferences fields are defined in foreground_message_contracts.
// ─── Thinking budget (extended_thinking) ─────────────────────────────
/// Sensitive setup data is consumed before the user turn is persisted and
/// never included in model history or task context.
// web search setup is defined in foreground_message_contracts.

/// Replaces the future of a user turn and starts a new assistant response from
/// that edited point. The user turn itself remains durable; the response is
/// generated without inserting a duplicate user message.
// EditAndResendMessageRequest is defined in foreground_message_contracts.

/// Personality data from frontend (mirrors types.ts PersonalityTemplate)
// PersonalityPayload is defined in foreground_message_contracts.

/// Trait values from frontend (-5 to +5)
// TraitsPayload is defined in foreground_message_contracts.

/// User preferences from frontend (mirrors types.ts UserPreferences)
// PreferencesPayload is defined in foreground_message_contracts.

/// Call DeepSeek API with tool support (legacy — kept for backward compatibility)
pub async fn call_deepseek_with_tools(
    messages: &[serde_json::Value],
    tool_schemas: &[serde_json::Value],
) -> Result<String, String> {
    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .map_err(|_| "DEEPSEEK_API_KEY not found in environment")?;

    let body = serde_json::json!({
        "model": DEEPSEEK_DEFAULT_MODEL,
        "messages": messages,
        "tools": tool_schemas,
        "tool_choice": "auto",
        "max_tokens": 4096,
        "temperature": 0.7
    });

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.deepseek.com/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("DeepSeek API error {}: {}", status, body));
    }

    let resp_json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    if let Some(tool_call) = resp_json["choices"][0]["message"]["tool_calls"].as_array() {
        if !tool_call.is_empty() {
            let call = &tool_call[0];
            let name = call["function"]["name"].as_str().unwrap_or("unknown");
            let args = call["function"]["arguments"].to_string();
            return Ok(format!(
                r#"{{"tool_call": {{"name": "{}", "arguments": {}}}}}"#,
                name, args
            ));
        }
    }

    resp_json["choices"][0]["message"]["content"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "Unexpected response format from DeepSeek".to_string())
}

/// Legacy function for backward compatibility (no tool support)
pub async fn call_deepseek(
    history: &[(String, String)],
    current_content: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .map_err(|_| "DEEPSEEK_API_KEY not found in environment")?;

    let mut messages: Vec<serde_json::Value> = history
        .iter()
        .map(|(role, content)| {
            serde_json::json!({
                "role": if *role == "assistant" { "assistant" } else { "user" },
                "content": content
            })
        })
        .collect();
    messages.push(serde_json::json!({
        "role": "user",
        "content": current_content
    }));

    let body = serde_json::json!({
        "model": DEEPSEEK_DEFAULT_MODEL,
        "messages": messages,
        "max_tokens": 4096,
        "temperature": 0.7
    });

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.deepseek.com/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("DeepSeek API error {}: {}", status, body).into());
    }

    let resp_json: serde_json::Value = resp.json().await?;
    resp_json["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| "Unexpected response format from DeepSeek".into())
        .map(String::from)
}

/// Auto-compress the session context if the estimated token count exceeds the threshold.
/// Returns the latest context summary text if compression was performed, or None.
fn task_facts_compression_context(
    state: &AppState,
    session_id: &str,
) -> Result<Option<String>, String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    if !table_exists(&conn, "task_run_facts")? {
        return Ok(None);
    }
    let facts_json: Option<String> = conn
        .query_row(
            "SELECT facts_json FROM task_run_facts
             WHERE session_id = ?1 ORDER BY updated_at DESC LIMIT 1",
            params![session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(facts_json) = facts_json else {
        return Ok(None);
    };
    let facts: TaskFacts = serde_json::from_str(&facts_json)
        .map_err(|error| format!("Stored task facts are invalid: {error}"))?;

    let verification = facts
        .verification_evidence
        .iter()
        .map(|evidence| format!("{} (exit {})", evidence.command, evidence.exit_code))
        .collect::<Vec<_>>()
        .join("; ");
    let pending = facts
        .pending_confirmation
        .as_ref()
        .map(|confirmation| confirmation.tool_name.as_str())
        .unwrap_or("none");
    Ok(Some(format!(
        "任务事实（必须保留）：目标={}; 改动={}; 验证={}; 待确认={}; 失败步骤={}; 无进展次数={}",
        facts.goal,
        if facts.modified_files.is_empty() {
            "none".to_string()
        } else {
            facts.modified_files.join(", ")
        },
        if verification.is_empty() {
            "none".to_string()
        } else {
            verification
        },
        pending,
        if facts.failed_steps.is_empty() {
            "none".to_string()
        } else {
            facts.failed_steps.join(", ")
        },
        facts.no_progress_count,
    )))
}

pub(crate) async fn auto_compress_if_needed(
    state: &AppState,
    app: Option<&AppHandle>,
    session_id: &str,
    agent_config: &crate::agent::AgentConfig,
    tool_count: usize,
) -> Result<Option<String>, String> {
    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let has_private_ledger: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE session_id=?1 AND metadata LIKE '%\"privateProtocolTranscript\"%')",
            params![session_id], |row| row.get(0),
        ).map_err(|e| e.to_string())?;
        if has_private_ledger {
            // A text summary cannot replace the private protocol ledger.
            return Ok(None);
        }
    }
    // Use the selected model's advertised context window. The configured
    // completion budget and system prompt occupy part of that total window.
    let provider_config =
        crate::commands::api_config::load_api_config_internal().unwrap_or_default();
    let system_tokens = estimate_system_prompt_tokens(agent_config, tool_count);
    let usable_context = usable_context_tokens(agent_config, tool_count, &provider_config)?;
    // Reserve headroom for a tool-result turn and the final answer. Waiting
    // until the provider's hard context limit is reached makes recovery depend
    // on a failed request; compact while the foreground loop can still proceed.
    let threshold = usable_context.saturating_mul(70) / 100;

    // Get all messages for this session and count tokens
    let messages: Vec<(String, String, String)> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, role, content, metadata FROM messages WHERE session_id = ?1 AND COALESCE(metadata, '') NOT LIKE '%\"compressed\": true%' ORDER BY created_at ASC, id ASC LIMIT 200")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![session_id], |r| {
                let role: String = r.get(1)?;
                let content: String = r.get(2)?;
                let metadata: Option<String> = r.get(3)?;
                let content = foreground_text_attachments::render_stored(
                    &content,
                    &role,
                    metadata.as_deref(),
                )
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        error.into(),
                    )
                })?;
                Ok((r.get::<_, String>(0)?, role, content))
            })
            .map_err(|e| e.to_string())?;
        let result: Vec<_> = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        result
    };

    let msg_tokens: usize = messages
        .iter()
        .map(|(_, role, content)| estimate_tokens(role) + estimate_tokens(content))
        .sum();
    let total_tokens = system_tokens + msg_tokens;

    if total_tokens < threshold {
        return Ok(None);
    }

    // Need compression: take older messages (keep last ~10 for recency),
    // generate a summary using the LLM provider
    let keep_count = 10usize.min(messages.len() / 4);
    let split_point = messages.len().saturating_sub(keep_count);
    let old_messages = &messages[..split_point];

    if old_messages.is_empty() {
        return Ok(None);
    }

    // Build prompt for compression
    let conversation_text: String = old_messages
        .iter()
        .map(|(_, role, content)| format!("{}: {}", role, content))
        .collect::<Vec<_>>()
        .join("\n");

    let facts_context = task_facts_compression_context(state, session_id)?;
    let includes_task_facts = facts_context.is_some();
    // Both foreground turns and delegated runs resume from the same compact,
    // structured checkpoint contract. Keeping one prompt prevents automatic
    // foreground compression from discarding decisions or marking pending work
    // as completed in a free-form summary.
    let compression_prompt = checkpoint_prompt(&conversation_text, facts_context.as_deref());

    // Call the LLM for compression
    let model_config = crate::commands::foreground_model_factory::resolve_model_config(app)
        .map_err(|error| format!("Failed to resolve LLM provider for compression: {error}"))?;
    let provider = crate::llm::create_provider_for_app(&model_config, app)
        .map_err(|e| format!("Failed to create LLM provider for compression: {}", e))?;

    let llm_messages = vec![crate::llm::Message {
        role: "user".to_string(),
        content: compression_prompt,
        tool_calls: None,
        tool_call_id: None,
        tool_images: Vec::new(),
        protocol_state: None,
    }];

    let summary = match provider
        .chat(&llm_messages, None, None)
        .await
        .map(|response| response.into_parts().0)
    {
        Ok(crate::llm::LlmResponse::Text { text, .. }) => text,
        Ok(crate::llm::LlmResponse::ProtocolViolation { reason, .. }) => {
            return Err(format!(
                "Invalid tool-call protocol during compression: {reason}"
            ));
        }
        Ok(crate::llm::LlmResponse::ToolCalls { .. }) => {
            return Err("Unexpected tool call during compression".to_string());
        }
        Ok(crate::llm::LlmResponse::WithProtocol { .. }) => {
            unreachable!("protocol wrappers are flattened by into_parts")
        }
        Err(e) => {
            eprintln!("[AutoCompress] Compression failed: {}", e);
            return Ok(None); // Graceful degradation: skip compression
        }
    };

    // Store the compression result
    let now = Utc::now().timestamp();
    let summary_id = uuid::Uuid::new_v4().to_string();
    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO context_summaries (id, session_id, summary, compressed_at, message_count_before, message_count_after, trigger)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                summary_id,
                session_id,
                summary,
                now,
                old_messages.len() as i32,
                messages.len() as i32,
                "auto_token_threshold",
            ],
        )
        .map_err(|e| e.to_string())?;

        conn.execute(
            "UPDATE sessions SET context_version = context_version + 1, last_compressed_at = ?1 WHERE id = ?2",
            params![now, session_id],
        )
        .map_err(|e| e.to_string())?;

        // Mark exactly the summarized snapshot IDs. Merge the flag instead of
        // replacing metadata: explicit user files must survive history recall,
        // edit/resend and backup even after their context has been compacted.
        for (id, _, _) in old_messages {
            if let Ok(metadata) = conn.query_row(
                "SELECT metadata FROM messages WHERE id = ?1 AND session_id = ?2",
                params![id, session_id],
                |row| row.get::<_, Option<String>>(0),
            ) {
                let merged = foreground_text_attachments::compressed_metadata(metadata.as_deref());
                let _ = conn.execute(
                    "UPDATE messages SET metadata = ?1 WHERE id = ?2 AND session_id = ?3",
                    params![merged, id, session_id],
                );
            }
        }
    }

    eprintln!(
        "[AutoCompress] Session {} compressed: {} total tokens → summary stored ({} old msgs marked compressed)",
        session_id, total_tokens, old_messages.len()
    );
    if let Some(app) = app {
        crate::commands::agent_event::emit_agent_event(
            app,
            session_id,
            AgentEvent::ContextCompressed {
                session_id: session_id.to_string(),
                turn_id: summary_id.clone(),
                before_tokens: total_tokens,
                after_tokens: estimate_tokens(&summary),
                summary_id: Some(summary_id),
                includes_task_facts,
            },
        );
    }

    Ok(Some(summary))
}

pub(crate) fn live_context_compaction_threshold(
    agent_config: &crate::agent::AgentConfig,
    tool_count: usize,
) -> Result<usize, String> {
    let system_tokens = estimate_system_prompt_tokens(agent_config, tool_count);
    let provider_config =
        crate::commands::api_config::load_api_config_internal().unwrap_or_default();
    Ok(usable_context_tokens(agent_config, tool_count, &provider_config)?.saturating_mul(70) / 100)
}

fn usable_context_tokens(
    agent_config: &crate::agent::AgentConfig,
    tool_count: usize,
    provider_config: &crate::llm::ProviderConfig,
) -> Result<usize, String> {
    let system_tokens = estimate_system_prompt_tokens(agent_config, tool_count);
    let context_window = crate::llm::context_window_tokens(provider_config);
    let reserved_tokens = system_tokens.saturating_add(provider_config.max_tokens as usize);
    context_window.checked_sub(reserved_tokens).ok_or_else(|| {
        format!(
            "Configured output budget ({}) and system prompt exceed the {} token context window for {}",
            provider_config.max_tokens, context_window, provider_config.model
        )
    })
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    app: AppHandle,
    req: SendMessageRequest,
) -> Result<Message, String> {
    let id = req
        .client_message_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    send_message_impl(&state, Some(&app), req, true, None, Some(id.clone()), None).await
}

/// Dev HTTP is an adapter, not a second agent runtime.  It deliberately uses
/// the same foreground turn pipeline as Tauri, with durable events written to
/// the development event store instead of a window emitter.
pub(crate) async fn send_message_from_dev_http(
    state: &AppState,
    req: SendMessageRequest,
) -> Result<Message, String> {
    let id = req
        .client_message_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    send_message_impl(state, None, req, true, None, Some(id), None).await
}

/// Deliver one durable automation follow-up through the same Main-Agent turn
/// pipeline as the visible chat. The caller supplies the pre-reserved reply id
/// so a process interruption can be reconciled without executing the prompt a
/// second time. No synthetic user message is persisted; `ForegroundRunStore`
/// keeps the reply attached to the session's current visible leaf.
pub(crate) async fn run_supervised_follow_up(
    state: &AppState,
    app: &AppHandle,
    session_id: String,
    content: String,
    reply_id: String,
) -> Result<Message, String> {
    send_message_impl(
        state,
        Some(app),
        SendMessageRequest {
            session_id,
            role: "user".to_string(),
            content,
            text_attachments: Vec::new(),
            personality: None,
            preferences: None,
            thinking_effort: None,
            client_message_id: None,
            web_search_setup: None,
        },
        false,
        None,
        None,
        Some(reply_id),
    )
    .await
}

/// Result of an internal follow-up whose caller supplied an atomic admission
/// guard. `Skipped` is a normal policy outcome: no provisional reply, task run,
/// or model invocation was created.
#[derive(Debug)]
pub(crate) enum SupervisedFollowUpOutcome {
    Completed(Message),
    Skipped,
}

/// Execute a supervised follow-up only if its caller-owned admission guard
/// approves inside the foreground reservation transaction. The guard is
/// intentionally opaque here, keeping the Main-Agent execution module free of
/// automation, queue, and policy knowledge.
pub(crate) async fn run_supervised_follow_up_with_admission(
    state: &AppState,
    app: &AppHandle,
    session_id: String,
    content: String,
    reply_id: String,
    admission: ForegroundRunAdmissionGuard,
) -> Result<SupervisedFollowUpOutcome, String> {
    run_supervised_follow_up_with_tool_mode(
        state,
        app,
        session_id,
        content,
        reply_id,
        admission,
        crate::commands::foreground_turn_coordinator::ForegroundTurnToolMode::Standard,
    )
    .await
}

/// Deliver a reviewed, read-only delegated result through the same durable
/// Main-Agent turn seam as every other foreground reply.  Its capability
/// surface is intentionally restricted to acknowledging this one delivery;
/// background delivery can never inherit file, desktop, MCP, automation, or
/// delegation-issuance tools.
pub(crate) async fn run_reviewed_delivery_follow_up_with_admission(
    state: &AppState,
    app: &AppHandle,
    session_id: String,
    content: String,
    reply_id: String,
    delivery_id: String,
    admission: ForegroundRunAdmissionGuard,
) -> Result<SupervisedFollowUpOutcome, String> {
    run_supervised_follow_up_with_tool_mode(
        state,
        app,
        session_id,
        content,
        reply_id,
        admission,
        crate::commands::foreground_turn_coordinator::ForegroundTurnToolMode::ReviewedDeliverySummary {
            delivery_id,
        },
    )
    .await
}

async fn run_supervised_follow_up_with_tool_mode(
    state: &AppState,
    app: &AppHandle,
    session_id: String,
    content: String,
    reply_id: String,
    admission: ForegroundRunAdmissionGuard,
    tool_mode: crate::commands::foreground_turn_coordinator::ForegroundTurnToolMode,
) -> Result<SupervisedFollowUpOutcome, String> {
    match send_message_impl_with_admission(
        state,
        Some(app),
        SendMessageRequest {
            session_id,
            role: "user".to_string(),
            content,
            text_attachments: Vec::new(),
            personality: None,
            preferences: None,
            thinking_effort: None,
            client_message_id: None,
            web_search_setup: None,
        },
        false,
        None,
        None,
        Some(reply_id),
        Some(admission),
        tool_mode,
    )
    .await?
    {
        ForegroundMessageDispatch::Completed(message) => {
            Ok(SupervisedFollowUpOutcome::Completed(message))
        }
        ForegroundMessageDispatch::Skipped => Ok(SupervisedFollowUpOutcome::Skipped),
    }
}

enum ForegroundMessageDispatch {
    Completed(Message),
    Skipped,
}

fn always_admit_foreground_run(
    _: &rusqlite::Transaction<'_>,
) -> Result<crate::commands::foreground_run_store::ForegroundRunAdmission, String> {
    Ok(ForegroundRunAdmission::Start)
}

/// Bind a session to the first explicitly resolved provider/model. A later
/// mismatch is a user-visible session boundary, never an implicit failover.
fn bind_session_provider(
    state: &AppState,
    session_id: &str,
    provider: &str,
    model: &str,
    now: i64,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    bind_session_provider_in_conn(&conn, session_id, provider, model, now)
}

fn bind_session_provider_in_conn(
    conn: &rusqlite::Connection,
    session_id: &str,
    provider: &str,
    model: &str,
    now: i64,
) -> Result<(), String> {
    if provider.trim().is_empty() || model.trim().is_empty() {
        return Err(
            "Provider and model must be configured before starting an agent session".to_string(),
        );
    }

    // Always overwrite the binding so that model changes in Settings take effect
    // for the current session without requiring a restart.
    conn.execute(
        "UPDATE sessions SET agent_provider = ?1, agent_model = ?2, updated_at = ?3 WHERE id = ?4",
        params![provider, model, now, session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Execute one bounded agent slice.  Public entry points decide how a slice is
/// initiated; the runner itself never becomes a background task scheduler.
async fn send_message_impl(
    state: &AppState,
    app: Option<&AppHandle>,
    req: SendMessageRequest,
    persist_user_message: bool,
    history_before_created_at: Option<i64>,
    // `user_message_id`: the ID of the user message being replied to.
    // Normal send: the auto-inserted user message's ID (`id` above).
    // Edit-and-resend: the existing user message ID (passed from caller).
    user_message_id: Option<String>,
    // Internal supervised follow-ups reserve this before dispatch so a crash
    // can be reconciled without replaying a potentially side-effecting turn.
    reply_id: Option<String>,
) -> Result<Message, String> {
    match send_message_impl_with_admission(
        state,
        app,
        req,
        persist_user_message,
        history_before_created_at,
        user_message_id,
        reply_id,
        None,
        crate::commands::foreground_turn_coordinator::ForegroundTurnToolMode::Standard,
    )
    .await?
    {
        ForegroundMessageDispatch::Completed(message) => Ok(message),
        ForegroundMessageDispatch::Skipped => {
            Err("unconditional foreground message was unexpectedly skipped".to_string())
        }
    }
}

async fn send_message_impl_with_admission(
    state: &AppState,
    app: Option<&AppHandle>,
    req: SendMessageRequest,
    persist_user_message: bool,
    history_before_created_at: Option<i64>,
    user_message_id: Option<String>,
    reply_id: Option<String>,
    admission: Option<ForegroundRunAdmissionGuard>,
    tool_mode: crate::commands::foreground_turn_coordinator::ForegroundTurnToolMode,
) -> Result<ForegroundMessageDispatch, String> {
    foreground_text_attachments::validate_turn(&req.role, &req.content, &req.text_attachments)?;
    // The chat parser can carry a short-lived Web Search setup envelope so a
    // user never has to paste a credential into the actual conversation.  It
    // must be handled before the turn starts: neither the key nor the setup
    // instruction belongs in durable messages, model context, or telemetry.
    if let Some(setup) = req.web_search_setup.as_ref() {
        let app = app.ok_or_else(|| "联网搜索配置只能在桌面应用中保存。".to_string())?;
        crate::commands::web_search::configure_impl(
            state,
            app,
            crate::commands::web_search::ConfigureWebSearchRequest {
                provider: setup.provider.clone(),
                api_key: setup.api_key.clone(),
                endpoint: setup.endpoint.clone(),
            },
        )?;
    }

    let run_permit = state.steering.try_start_run(&req.session_id)?;
    let session_id = req.session_id.clone();
    let dispatch = send_message_after_run_admission(
        state,
        app,
        req,
        persist_user_message,
        history_before_created_at,
        user_message_id,
        reply_id,
        admission,
        tool_mode,
    )
    .await;
    // The foreground permit is the session's mutual-exclusion boundary. Drop
    // it before waking a deferred follow-up so that retry cannot immediately
    // observe the same session as busy again.
    drop(run_permit);
    if let Some(app) = app {
        state
            .foreground_automation_pump
            .wake_after_session_release(app.clone(), &session_id);
    }
    dispatch
}

/// Execute the durable foreground slice after its session execution slot has
/// been acquired. The outer seam owns the permit lifetime so every success,
/// skip, and error path releases it before considering a deferred follow-up.
async fn send_message_after_run_admission(
    state: &AppState,
    app: Option<&AppHandle>,
    req: SendMessageRequest,
    persist_user_message: bool,
    history_before_created_at: Option<i64>,
    user_message_id: Option<String>,
    reply_id: Option<String>,
    admission: Option<ForegroundRunAdmissionGuard>,
    tool_mode: crate::commands::foreground_turn_coordinator::ForegroundTurnToolMode,
) -> Result<ForegroundMessageDispatch, String> {
    let now = Utc::now().timestamp();
    // Normal send: caller (send_message) supplies `user_message_id`, which becomes
    // both the persisted user message id and the assistant reply's parent_id.
    // Continuations / edit-and-resend also supply it from outside. If absent
    // (rare — e.g. internal continuation flows), generate a fresh id so the
    // assistant reply still points at the freshly inserted user message.
    let id = user_message_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    // Allocate the assistant message ID before the loop so its durable event
    // journal and eventual task_run share one stable run identifier.
    let reply_id = reply_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let session_id = req.session_id.clone();
    // Create Tauri event emitter for real-time frontend updates
    let transport_emitter: Arc<dyn AgentEventEmitter> = match app {
        Some(app) => Arc::new(TauriEventEmitter::new(
            app.clone(),
            state.dev_event_store.clone(),
        )),
        None => state
            .dev_event_store
            .clone()
            .map(|store| store as Arc<dyn AgentEventEmitter>)
            .unwrap_or_else(|| Arc::new(crate::agent::event::NullEventEmitter)),
    };
    let lifecycle_control = Arc::new(ForegroundLifecycleControl::new(
        transport_emitter,
        state.db.clone(),
        reply_id.clone(),
    ));
    let run_store = ForegroundRunStore::new(state.db.clone());
    let admission = admission
        .unwrap_or_else(|| Box::new(always_admit_foreground_run) as ForegroundRunAdmissionGuard);
    let foreground_run = match run_store.begin_with_admission(
        BeginForegroundTurn {
            session_id: session_id.clone(),
            user_message_id: id.clone(),
            reply_id: reply_id.clone(),
            role: req.role.clone(),
            content: req.content.clone(),
            text_attachments: req.text_attachments.clone(),
            persist_user_message,
            now,
        },
        admission,
    )? {
        ForegroundRunBeginResult::Started(run) => run,
        ForegroundRunBeginResult::Skipped => return Ok(ForegroundMessageDispatch::Skipped),
    };
    state.steering.mark_running(&session_id);
    // The durable reservation is intentionally created before context/model
    // preparation. Any later error must promote that one known provisional
    // reply rather than leaving a hidden "running" turn in the active branch.
    let recovery_store = run_store.clone();
    let recovery_run = foreground_run.clone();
    let result = async {

    // 1.5. Auto-generate session title from first user message if session is untitled
    if persist_user_message {
        let req_content = req.content.clone();
        let req_session_id = req.session_id.clone();
        let db_for_title = state.db.clone();
        tokio::spawn(async move {
            let title = match generate_title_from_content(&req_content).await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("[AutoTitle] failed to generate title: {}", e);
                    return;
                }
            };
            let now = Utc::now().timestamp();
            if let Ok(conn) = db_for_title.lock() {
                let _ = conn.execute(
                "UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3 AND (title = '' OR title IS NULL)",
                params![title, now, req_session_id],
            );
            }
        });
    }

    // 1.6. Prepare the complete foreground context through one deep seam.
    // This preserves compaction, native history filtering, workspace expansion,
    // skill guidance, and bounded Attention projection in a single module.
    let prepared_context =
        crate::commands::foreground_context_assembler::ForegroundContextAssembler::prepare(
            crate::commands::foreground_context_assembler::ForegroundContextRequest {
                state: &state,
                app,
                request: &req,
                history_before_created_at: history_before_created_at.unwrap_or(foreground_run.now),
                budget: crate::commands::foreground_context_assembler::ForegroundContextBudget {
                    estimated_tool_count: match &tool_mode {
                        crate::commands::foreground_turn_coordinator::ForegroundTurnToolMode::Standard => ESTIMATED_TOOL_COUNT,
                        crate::commands::foreground_turn_coordinator::ForegroundTurnToolMode::ReviewedDeliverySummary { .. } => 1,
                    },
                },
            },
        )
        .await?;
    let effective_thinking_effort = req.thinking_effort.as_deref();
    // 4. Run the agent with personalized system prompt. Provider/keychain
    // fallback and session binding live behind one reusable factory seam.
    let resolved_model =
        crate::commands::foreground_model_factory::ForegroundModelFactory::new(app, state)
            .resolve(
                crate::commands::foreground_model_factory::ForegroundModelRequest {
                    session_id: &session_id,
                    requested_thinking_effort: effective_thinking_effort,
                    now,
                },
            )
            .map_err(|error| error.to_string())?;
    // Only an interactive user turn receives an issuance identity and
    // app-managed sandbox capability. A reviewed delivery summary gets its
    // own one-tool surface later and must never prepare these credentials or
    // paths, even transiently.
    let (delegation_identity, sandbox_root, delegated_model_binding) = match &tool_mode {
        crate::commands::foreground_turn_coordinator::ForegroundTurnToolMode::Standard => {
            let delegation_identity = {
                let conn = state.db.lock().map_err(|error| error.to_string())?;
                crate::commands::file::resolve_delegation_workspace_root(&conn, &session_id)?
                    .map(|work_dir| {
                        crate::agent::delegate_work::ForegroundRunIdentity::prepare(
                            &conn,
                            &session_id,
                            &reply_id,
                            work_dir,
                        )
                    })
                    .transpose()?
            };
            // Delegated sandboxes are app-managed resources, never children
            // of the user's canonical workspace. The worker receives only the
            // provider's opaque capability; cleanup resolves this root through
            // its manifest.
            let sandbox_root =
                crate::commands::foreground_tool_surface::delegated_sandbox_root()?;
            let model = resolved_model.identity.model.clone();
            let delegated_model_binding =
                crate::agent::delegated_model_binding::DelegatedModelBindingRequest::new(
                    resolved_model.identity.provider_profile_ref.clone(),
                    format!("model:{model}"),
                    resolved_model.identity.credential_handle_ref.clone(),
                    1,
                )
                .map_err(|_| "resolved model identity is invalid".to_string())?;
            (
                delegation_identity,
                Some(sandbox_root),
                Some(delegated_model_binding),
            )
        }
        crate::commands::foreground_turn_coordinator::ForegroundTurnToolMode::ReviewedDeliverySummary { .. } => {
            (None, None, None)
        }
    };
    let trigger_kepa = {
        let conn = state.db.lock().map_err(|error| error.to_string())?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
                params![req.session_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        matches!(
            &tool_mode,
            crate::commands::foreground_turn_coordinator::ForegroundTurnToolMode::Standard
        ) && prepared_context.agent_config.preferences.evolution_enabled
            && count > 0
            && count % 20 == 0
    };
    let before_persist: Option<Box<dyn FnOnce() + Send + 'static>> = trigger_kepa.then(|| {
        let config = crate::commands::foreground_model_factory::resolve_model_config(app).ok()?;
        // Optional evolution must use the same configured identity as this turn.
        // If settings changed during context preparation, defer it to a later turn.
        if config.profile_identity() != resolved_model.identity.provider_profile_ref
            || config.credential_identity() != resolved_model.identity.credential_handle_ref
            || config.model != resolved_model.identity.model
        {
            return None;
        }
        let provider = crate::llm::create_provider_for_app(&config, app).ok()?;
        let db = state.db.clone();
        let session_id = req.session_id.clone();
        Some(Box::new(move || {
            eprintln!("[KEPA] Auto evolution for session {}", session_id);
            tokio::spawn(async move {
                let _ = crate::commands::memory::run_evolution_review_impl(
                    db,
                    &session_id,
                    provider,
                )
                .await;
            });
        }) as Box<dyn FnOnce() + Send + 'static>)
    }).flatten();

    crate::commands::foreground_turn_coordinator::ForegroundTurnCoordinator::new(
        state,
        app,
        lifecycle_control,
        run_store,
    )
    .run(
        crate::commands::foreground_turn_coordinator::ForegroundTurnInput {
            request: req,
            session_id: session_id.clone(),
            message_id: reply_id.clone(),
            reply_id: reply_id.clone(),
            prepared_context,
            resolved_model,
            foreground_run,
            delegation_identity,
            sandbox_root,
            delegated_model_binding,
            tool_mode,
            before_persist,
        },
    )
    .await
    }
    .await;
    match result {
        Ok(message) => Ok(ForegroundMessageDispatch::Completed(message)),
        Err(error) => {
            let _ = recovery_store.recover_interrupted_run(&recovery_run, Utc::now().timestamp());
            Err(error)
        }
    }
}

/// Return the active persisted execution policy for the Settings UI.
#[tauri::command]
pub fn get_agent_execution_permission(state: State<AppState>) -> Result<String, String> {
    agent_execution_permission(&state)
}

/// Persist a user-selected execution policy.  The value is validated here so
/// both Tauri IPC and Dev HTTP share the same fail-closed contract.
#[tauri::command]
pub fn set_agent_execution_permission(
    state: State<AppState>,
    permission: String,
) -> Result<String, String> {
    set_agent_execution_permission_impl_with_expiry(&state, &permission, None, true)
}

pub(crate) fn set_agent_execution_permission_impl(
    state: &AppState,
    permission: &str,
) -> Result<String, String> {
    set_agent_execution_permission_impl_with_expiry(state, permission, None, true)
}

pub(crate) fn set_agent_execution_permission_impl_with_expiry(
    state: &AppState,
    permission: &str,
    expires_at: Option<i64>,
    permanent: bool,
) -> Result<String, String> {
    let permission = normalize_agent_execution_permission(permission)?;
    let effective_expiry = if permanent { None } else { expires_at };
    if let Some(expiry) = effective_expiry {
        if expiry <= Utc::now().timestamp() {
            return Err("execution permission expiry must be in the future".to_string());
        }
    }
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO agent_permission_grants (scope, permission, expires_at, updated_at)
         VALUES ('global', ?1, ?2, ?3)
         ON CONFLICT(scope) DO UPDATE SET permission=excluded.permission,
           expires_at=excluded.expires_at, updated_at=excluded.updated_at",
        params![permission, effective_expiry, Utc::now().timestamp()],
    )
    .map_err(|e| e.to_string())?;
    crate::settings_cache::save_setting(
        &conn,
        &state.cached_settings,
        AGENT_EXECUTION_PERMISSION_KEY,
        permission,
    )?;
    Ok(permission.to_string())
}

#[tauri::command]
pub fn get_agent_execution_permission_expiry(
    state: State<AppState>,
) -> Result<Option<i64>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT expires_at FROM agent_permission_grants WHERE scope = 'global'",
        [],
        |row| row.get::<_, Option<i64>>(0),
    )
    .optional()
    .map_err(|error| error.to_string())
    .map(|value| value.flatten())
}

#[tauri::command]
pub async fn resolve_agent_confirmation(
    app: AppHandle,
    req: ResolveAgentConfirmationRequest,
) -> Result<ToolResultInfo, String> {
    // Approval may call a slow UIA provider or external tool. Reuse the same
    // blocking-task boundary as preflight rather than freezing Tauri's UI.
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        resolve_agent_confirmation_impl(&state, req)
    })
    .await
    .map_err(|_| {
        "Confirmation task was interrupted; inspect the target before continuing".to_string()
    })?
}

/// Resolve file references inserted by the workspace panel into the actual
/// model request. The durable user message keeps the compact marker, while the
/// Agent receives bounded file contents from the session's write sandbox.
pub(crate) fn expand_workspace_file_references(
    state: &crate::AppState,
    session_id: &str,
    content: &str,
) -> String {
    const PREFIX: &str = "[引用文件: ";
    const SUFFIX: &str = "]";
    const MAX_REFERENCE_CHARS: usize = 16_000;
    let Ok(work_dir) = crate::commands::file::resolve_work_dir(state, Some(session_id)) else {
        return content.to_string();
    };
    content.lines().map(|line| {
        let Some(path) = line.trim().strip_prefix(PREFIX).and_then(|value| value.strip_suffix(SUFFIX)) else {
            return line.to_string();
        };
        let path = path.trim();
        let resolved = crate::commands::file::resolve_and_validate(&work_dir, std::path::Path::new(path), true);
        match resolved.and_then(|target| std::fs::read_to_string(target).map_err(|error| crate::commands::file::FileReadError::Io(error))) {
            Ok(file_content) => format!("<referenced_file path=\"{path}\">\n{}\n</referenced_file>", file_content.chars().take(MAX_REFERENCE_CHARS).collect::<String>()),
            Err(error) => format!("<referenced_file path=\"{path}\" unavailable=\"true\">{error}</referenced_file>"),
        }
    }).collect::<Vec<_>>().join("\n")
}

/// Start a fresh, bounded slice from persisted task facts after an explicit
/// user action.  It never resumes a hidden stack or replays side effects.
#[tauri::command]
pub async fn continue_agent_task(
    state: State<'_, AppState>,
    app: AppHandle,
    req: ContinueAgentTaskRequest,
) -> Result<Message, String> {
    let content = build_task_continuation_prompt(&state, &req.session_id, &req.message_id)?;
    send_message_impl(
        &state,
        Some(&app),
        SendMessageRequest {
            session_id: req.session_id,
            role: "user".to_string(),
            content,
            text_attachments: Vec::new(),
            personality: None,
            preferences: None,
            thinking_effort: None,
            client_message_id: None,
            web_search_setup: None,
        },
        false,
        None,
        None,
        None,
    )
    .await
}

#[tauri::command]
pub async fn cancel_agent_task(
    state: State<'_, AppState>,
    req: CancelAgentTaskRequest,
) -> Result<(), String> {
    cancel_agent_task_impl(&state, &req.session_id, &req.message_id)
}

#[tauri::command]
pub fn get_messages(state: State<AppState>, session_id: String) -> Result<Vec<Message>, String> {
    // Tauri and the development adapter share the same durable projection,
    // including hidden-run filtering and explicit text snapshots.
    ForegroundRunStore::new(state.db.clone()).project_session(&session_id)
}

#[tauri::command]
pub fn delete_message(state: State<AppState>, message_id: String) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    // Cascade: if we delete an assistant message, also drop its inline tool
    // messages. The hydrated history depends on this pairing — leaving orphan
    // tool rows would corrupt the LLM protocol on the next replay.
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM messages WHERE id = ?1", params![message_id])
        .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM messages WHERE tool_call_id = ?1",
        params![message_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn update_message_content(
    state: State<AppState>,
    message_id: String,
    content: String,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let rows = conn
        .execute(
            "UPDATE messages SET content = ?1 WHERE id = ?2",
            params![content, message_id],
        )
        .map_err(|e| e.to_string())?;
    if rows == 0 {
        return Err("Message not found".to_string());
    }
    Ok(())
}

/// Prune an edited user message and its descendants before inserting the
/// replacement turn. The caller may immediately reuse `message_id`; this is
/// intentional so optimistic UI rows retain their durable identity across
/// repeated edits.
fn prune_edit_and_resend_branch(
    conn: &Connection,
    session_id: &str,
    message_id: &str,
) -> Result<i64, String> {
    let (stored_session_id, role, created_at, parent_id): (String, String, i64, Option<String>) =
        conn.query_row(
            "SELECT session_id, role, created_at, parent_id FROM messages WHERE id = ?1",
            params![message_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|_| "Message not found".to_string())?;

    if stored_session_id != session_id || role != "user" {
        return Err(
            "Only a user message in the active session can be edited and resent".to_string(),
        );
    }

    let now = Utc::now().timestamp();
    conn.execute(
        "UPDATE sessions SET leaf_message_id = ?1, updated_at = ?2 WHERE id = ?3",
        params![parent_id.as_deref(), now, session_id],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM messages WHERE id = ?1", params![message_id])
        .map_err(|e| e.to_string())?;

    Ok(created_at)
}

/// Recover the exact user-selected data before replacing a turn. Validation
/// runs before the destructive prune, and both reads and prune share one
/// transaction so a malformed snapshot never deletes the original branch.
fn prepare_edit_and_resend_branch(
    conn: &Connection,
    session_id: &str,
    message_id: &str,
    content: &str,
) -> Result<(i64, Vec<foreground_text_attachments::TextAttachment>), String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let metadata: Option<String> = tx
        .query_row(
            "SELECT metadata FROM messages WHERE id = ?1 AND session_id = ?2 AND role = 'user'",
            params![message_id, session_id],
            |row| row.get(0),
        )
        .map_err(|_| {
            "Only a user message in the active session can be edited and resent".to_string()
        })?;
    let attachments = foreground_text_attachments::from_metadata(metadata.as_deref())?;
    foreground_text_attachments::validate_turn("user", content, &attachments)?;
    let before = prune_edit_and_resend_branch(&tx, session_id, message_id)?;
    tx.commit().map_err(|error| error.to_string())?;
    Ok((before, attachments))
}

/// Edit a user message and resend — discards all messages after it permanently.
///
/// When a user edits a message and resends, the entire subtree rooted at that
/// message (the edited message and all its descendants) is hard-deleted, then a
/// fresh user message is inserted and an AI response is generated.
///
/// This is the simplest and safest semantic: "start over from here" means the
/// old branch is gone, not hidden. No is_deleted, no orphan parent_id chains.
#[tauri::command]
pub async fn edit_and_resend_message(
    state: State<'_, AppState>,
    app: AppHandle,
    req: EditAndResendMessageRequest,
) -> Result<Message, String> {
    let content = req.content.trim().to_string();
    // Phase 1: verify message exists, then hard-delete its subtree, update leaf.
    // All DB writes happen here under the mutex — no concurrent edits possible.
    let (history_before_created_at, text_attachments) = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        prepare_edit_and_resend_branch(&conn, &req.session_id, &req.message_id, &content)?
    }; // db lock released here

    // Phase 2: persist the replacement user turn and generate its reply.
    send_message_impl(
        &state,
        Some(&app),
        SendMessageRequest {
            session_id: req.session_id,
            role: "user".to_string(),
            content,
            text_attachments,
            personality: req.personality,
            preferences: req.preferences,
            thinking_effort: req.thinking_effort,
            client_message_id: None,
            web_search_setup: None,
        },
        true,
        Some(history_before_created_at),
        // The frontend has already rendered this user turn optimistically.
        // Reinsert it under the same durable ID after pruning its old branch;
        // otherwise a refresh replaces the visible row with a different ID and
        // can briefly (or permanently after an overlapping reload) drop the
        // edited turn from the active transcript.
        Some(req.message_id),
        None,
    )
    .await
}

// ─── AgentConfig builder ──────────────────────────────────────────────────────

/// Build an AgentConfig from the incoming request and database state.
/// Merges frontend personality/preferences with DB profile and relevant memories.
pub(crate) fn build_agent_config(
    state: &AppState,
    req: &SendMessageRequest,
) -> Result<AgentConfig, String> {
    // `resolve_work_dir` reads settings through the same database mutex. Do it
    // before acquiring the guard below to avoid recursively locking a
    // non-reentrant mutex during every chat turn.
    let work_dir = crate::commands::file::resolve_work_dir(state, Some(&req.session_id)).ok();

    let conn = state.db.lock().map_err(|e| e.to_string())?;

    // ── Fetch user profile ──────────────────────────────────────────────────
    let profile = fetch_profile(&conn);

    // ── Build personality traits ─────────────────────────────────────────────
    let traits = if let Some(ref p) = req.personality {
        if let Some(ref t) = p.traits {
            PersonalityTraits {
                tone: t.tone,
                verbosity: t.verbosity,
                formality: t.formality,
                humor: t.humor,
                dependence: t.dependence,
                intimacy: t.intimacy,
                patience: t.patience,
            }
        } else {
            PersonalityTraits::default()
        }
    } else {
        PersonalityTraits::default()
    };

    // ── Build persona metadata ───────────────────────────────────────────────
    let (persona_name, persona_description, persona_greeting) = if let Some(ref p) = req.personality
    {
        (
            p.name.clone().unwrap_or_default(),
            p.description.clone().unwrap_or_default(),
            p.greeting.clone().unwrap_or_default(),
        )
    } else {
        (String::new(), String::new(), String::new())
    };

    // ── Build user preferences ───────────────────────────────────────────────
    let mut preferences = if let Some(ref p) = req.preferences {
        UserPreferences {
            response_length: p.response_length.clone().unwrap_or_default(),
            response_language: p
                .response_language
                .clone()
                .unwrap_or_else(|| "auto".to_string()),
            use_long_term_memory: p.use_long_term_memory.unwrap_or(true),
            interests: p.interests.clone().unwrap_or_default(),
            avoid_topics: p.avoid_topics.clone().unwrap_or_default(),
            disliked_words: p.disliked_words.clone().unwrap_or_default(),
            pet_peeves: p.pet_peeves.clone().unwrap_or_default(),
            evolution_enabled: p.evolution_enabled.unwrap_or(true),
        }
    } else {
        UserPreferences::default()
    };

    // ── Fetch relevant memories (Phase 5: Memory Recall) ─────────────────────
    let memory_enabled = preferences.use_long_term_memory;
    let relevant_memories = if memory_enabled {
        fetch_relevant_memories(&conn, &req.content)
    } else {
        Vec::new()
    };

    // ── Fetch core memory blocks (Issue #25) ───────────────────────────────
    let core_memories = if memory_enabled {
        fetch_core_memories(&*conn)
    } else {
        Vec::new()
    };

    // ── Fetch latest context summary for this session (Issue #35) ─────────
    let context_summary = fetch_latest_summary(&conn, &req.session_id);
    let workspace_context = workspace_context_for_session(&conn, &req.session_id)?;

    // ── Fetch knowledge graph context (Issue #37 Phase 4) ──────────────────
    let graph_ctx = crate::knowledge_graph::get_graph_context(&conn, &req.content, 2);
    let knowledge_graph_context = if graph_ctx.is_empty() {
        None
    } else {
        Some(graph_ctx)
    };

    Ok(AgentConfig {
        persona_name,
        persona_description,
        persona_greeting,
        traits,
        profile,
        preferences,
        relevant_memories,
        adaptive_constraints: crate::agent::adaptive_constraints::active_for_session(
            &conn,
            &req.session_id,
        ),
        skill_instructions: Vec::new(),
        core_memories,
        knowledge_graph_context,
        context_summary,
        workspace_context,
        work_dir,
        tool_error_recovery: crate::agent::config::ToolErrorRecovery::default(),
        enhanced_tool_descriptions: false,
    })
}

fn workspace_context_for_session(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<String>, String> {
    let workspace = conn.query_row(
        "SELECT p.kind, p.name, p.path FROM sessions s JOIN projects p ON p.id = s.project_id WHERE s.id = ?1",
        params![session_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
    )
    .optional()
    .map_err(|error| error.to_string())?;
    Ok(workspace.map(|(kind, name, path)| {
        if kind == "personal" {
            "## 当前工作区\n{\"kind\":\"personal\",\"name\":\"AngelBot 日常\",\"rule\":\"仅使用日常对话与全局长期记忆；不得假定任何项目文件或项目历史。\"}".to_owned()
        } else {
            format!("## 当前工作区\n{{\"kind\":\"project\",\"name\":{},\"root\":{},\"rule\":\"仅使用本项目的会话、文件和委派结果；不得引用其他项目上下文。\"}}", serde_json::to_string(&name).unwrap_or_default(), serde_json::to_string(&path).unwrap_or_default())
        }
    }))
}

/// Fetch user profile from the database
fn fetch_profile(conn: &std::sync::MutexGuard<rusqlite::Connection>) -> Option<UserProfile> {
    let result = conn.query_row(
        "SELECT name, preferences, habits, background FROM profile WHERE id = 1",
        [],
        |r| {
            Ok(UserProfile {
                name: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                preferences: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                habits: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                background: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            })
        },
    );

    match result {
        Ok(profile) => {
            if profile.name.is_empty() && profile.background.is_empty() {
                None
            } else {
                Some(profile)
            }
        }
        Err(_) => None,
    }
}

/// Fetch the latest context summary for a session (Issue #35).
fn fetch_latest_summary(conn: &rusqlite::Connection, session_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT summary FROM context_summaries WHERE session_id = ?1 ORDER BY compressed_at DESC LIMIT 1",
        params![session_id],
        |r| r.get(0),
    ).ok()
}

/// Fetch core memory blocks for persistent identity/context.
fn fetch_core_memories(conn: &rusqlite::Connection) -> Vec<crate::agent::config::CoreMemoryBlock> {
    let mut stmt = match conn.prepare(
        "SELECT block_type, label, content, importance FROM core_memory_blocks ORDER BY importance DESC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |r| {
        Ok(crate::agent::config::CoreMemoryBlock {
            block_type: r.get(0)?,
            label: r.get(1)?,
            content: r.get(2)?,
            importance: r.get::<_, Option<i32>>(3)?.unwrap_or(5),
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

/// Fetch relevant memories for the given query text.
/// Uses time-based scoring when vectors are disabled (default),
/// or vector similarity when enabled.
fn fetch_relevant_memories(
    conn: &std::sync::MutexGuard<rusqlite::Connection>,
    query_text: &str,
) -> Vec<MemoryContext> {
    // Avoid initializing or querying the vector virtual table until there is
    // actually something to recall. Besides saving work for a new profile,
    // this keeps an empty memory store out of the chat request critical path.
    let has_memories = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM memories LIMIT 1)", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(0)
        != 0;
    if !has_memories {
        return Vec::new();
    }

    // Try semantic search first using TF embedding
    let query_emb = crate::vector::text_to_embedding(query_text, 128);

    // Try vector similarity search
    let query_f32: Vec<f32> = {
        let mut v: Vec<f32> = query_emb.iter().map(|&x| x as f32).collect();
        v.resize(1536, 0.0);
        v
    };

    if let Ok(knn) = crate::vector_store::find_nearest(conn, &query_f32, 5) {
        if !knn.is_empty() {
            let rowids: Vec<i64> = knn.iter().map(|r| r.rowid).collect();
            if let Ok(mapping) = crate::vector_store::rowids_to_memory_ids(conn, &rowids) {
                let id_list: Vec<String> = mapping.iter().map(|(_, id)| id.clone()).collect();
                if !id_list.is_empty() {
                    let placeholders: Vec<String> = id_list
                        .iter()
                        .enumerate()
                        .map(|(i, _)| format!("?{}", i + 1))
                        .collect();
                    let sql = format!(
                        "SELECT category, content, importance FROM memories WHERE id IN ({}) AND forget_stage = 'active'",
                        placeholders.join(",")
                    );
                    if let Ok(mut stmt) = conn.prepare(&sql) {
                        let param_refs: Vec<Box<dyn rusqlite::types::ToSql>> = id_list
                            .iter()
                            .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
                            .collect();
                        let results: Vec<MemoryContext> = stmt
                            .query_map(
                                rusqlite::params_from_iter(param_refs.iter().map(|p| p.as_ref())),
                                |r| {
                                    Ok(MemoryContext {
                                        category: r.get::<_, String>(0)?,
                                        content: r.get::<_, String>(1)?,
                                        importance: r.get::<_, Option<i32>>(2)?.unwrap_or(5),
                                    })
                                },
                            )
                            .unwrap()
                            .filter_map(|r| r.ok())
                            .collect();
                        if !results.is_empty() {
                            return results;
                        }
                    }
                }
            }
        }
    }

    // Fallback to keyword search using query text
    let now = Utc::now().timestamp();
    let mut stmt = match conn.prepare(
        "SELECT id, scope, category, content, importance,
                frequency, last_mentioned
         FROM memories
         WHERE forget_stage = 'active'
         AND (content LIKE '%' || ?1 || '%' OR category LIKE '%' || ?1 || '%')
         ORDER BY is_permanent DESC, importance DESC, frequency DESC
         LIMIT 5",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let rows: Vec<(String, String, String, String, i32, i32, Option<i64>)> =
        match stmt.query_map(params![query_text, query_text], |r| {
            Ok((
                r.get::<_, String>(0)?,                   // id
                r.get::<_, String>(1)?,                   // scope
                r.get::<_, String>(2)?,                   // category
                r.get::<_, String>(3)?,                   // content
                r.get::<_, Option<i32>>(4)?.unwrap_or(0), // importance
                r.get::<_, Option<i32>>(5)?.unwrap_or(0), // frequency
                r.get::<_, Option<i64>>(6)?,              // last_mentioned
            ))
        }) {
            Ok(mapped) => mapped.filter_map(|r| r.ok()).collect(),
            Err(_) => return Vec::new(),
        };

    // Score and sort by recency + importance + frequency
    let mut scored: Vec<(f64, MemoryContext)> = rows
        .into_iter()
        .map(
            |(_id, _scope, category, content, importance, frequency, last_mentioned)| {
                let score =
                    crate::vector::time_based_score(frequency, last_mentioned, importance, now);
                (
                    score,
                    MemoryContext {
                        category,
                        content,
                        importance,
                    },
                )
            },
        )
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(5);

    scored.into_iter().map(|(_, ctx)| ctx).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrate;

    fn private_protocol_edit_fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
            INSERT INTO sessions(id,title,created_at,updated_at) VALUES('s','fixture',1,1);
            INSERT INTO messages(id,session_id,role,content,parent_id,created_at) VALUES
                ('u1','s','user','first',NULL,10), ('a1','s','assistant','first answer','u1',11),
                ('u2','s','user','second','a1',30), ('a2','s','assistant','second answer','u2',31);
            UPDATE sessions SET leaf_message_id='a2' WHERE id='s';",
        )
        .unwrap();
        for (id, marker) in [("a1", "retained-marker"), ("a2", "pruned-marker")] {
            let metadata =
                crate::commands::foreground_history::private_transcript_metadata(&[LlmMessage {
                    role: "assistant".into(),
                    content: "answer".into(),
                    tool_calls: None,
                    tool_call_id: None,
                    tool_images: Vec::new(),
                    protocol_state: Some(crate::llm::ProtocolContinuation {
                        protocol: crate::llm::ModelProtocol::OpenaiResponses,
                        model: "fixture".into(),
                        credential_ref: "fixture-profile".into(),
                        output_items: vec![
                            serde_json::json!({"type":"reasoning","encrypted_content":marker}),
                        ],
                    }),
                }])
                .unwrap();
            conn.execute(
                "UPDATE messages SET metadata=?1 WHERE id=?2",
                params![metadata, id],
            )
            .unwrap();
        }
        conn
    }

    fn private_protocol_confirmation_fixture(work_dir: &std::path::Path) -> AppState {
        let state = setup_confirmation_state(work_dir);
        let conn = state.db.lock().unwrap();
        conn.execute_batch(
            "ALTER TABLE messages ADD COLUMN metadata TEXT;
             ALTER TABLE messages ADD COLUMN tool_calls TEXT;",
        )
        .unwrap();
        let transcript = vec![
            LlmMessage {
                role: "assistant".into(),
                content: "Needs approval".into(),
                tool_calls: Some(vec![crate::llm::ToolCall {
                    id: "call-1".into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({"path":"approved.txt","content":"hello"}),
                }]),
                tool_call_id: None,
                tool_images: Vec::new(),
                protocol_state: Some(crate::llm::ProtocolContinuation {
                    protocol: crate::llm::ModelProtocol::OpenaiResponses,
                    model: "fixture".into(),
                    credential_ref: "fixture-profile".into(),
                    output_items: vec![
                        serde_json::json!({"type":"reasoning","encrypted_content":"approval-marker"}),
                        serde_json::json!({"type":"function_call","id":"fc-1","call_id":"call-1","name":"write_file","arguments":"{\"path\":\"approved.txt\",\"content\":\"hello\"}"}),
                    ],
                }),
            },
            LlmMessage {
                role: "tool".into(),
                content: "Confirmation required before executing side-effect tool 'write_file'."
                    .into(),
                tool_calls: None,
                tool_call_id: Some("call-1".into()),
                tool_images: Vec::new(),
                protocol_state: None,
            },
        ];
        let encoded = crate::commands::foreground_history::private_transcript_metadata(&transcript)
            .unwrap()
            .unwrap();
        let mut metadata: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        metadata["fixtureExtra"] = serde_json::json!("preserved");
        conn.execute(
            "UPDATE messages SET metadata=?1 WHERE id='msg-1'",
            [metadata.to_string()],
        )
        .unwrap();
        drop(conn);
        state
    }

    #[test]
    fn private_protocol_history_never_truncates_a_native_turn_by_ui_row_limit() {
        let conn = private_protocol_edit_fixture();
        for index in 0..120 {
            conn.execute(
                "INSERT INTO messages(id,session_id,role,content,created_at,tool_call_id)
                 VALUES(?1,'s','tool','ui aggregate',11,?1)",
                [format!("z-ui-tool-{index:03}")],
            )
            .unwrap();
        }
        let history = hydrate_llm_history(&conn, "s", i64::MAX).unwrap();
        assert_eq!(history.len(), 4);
        assert_eq!(
            history[1].protocol_state.as_ref().unwrap().output_items[0]["encrypted_content"],
            "retained-marker"
        );
    }

    #[test]
    fn private_protocol_confirmation_settlement_keeps_exact_native_result_and_opaque_items() {
        for (success, output) in [
            (true, "Confirmation approved: written"),
            (false, "Confirmation rejected: the operation was not run."),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let state = private_protocol_confirmation_fixture(dir.path());
            let mut conn = state.db.lock().unwrap();
            let initial = hydrate_llm_history(&conn, "s1", 100).unwrap();
            let pending = load_pending_confirmation(&conn, "s1", "msg-1", "call-1").unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            settle_pending_confirmation_in_tx(
                &tx, "s1", "msg-1", "call-1", &pending, success, output,
            )
            .unwrap();
            tx.commit().unwrap();
            let replay = hydrate_llm_history(&conn, "s1", 100).unwrap();
            assert_eq!(replay[1].content, output);
            assert_eq!(replay[1].tool_call_id.as_deref(), Some("call-1"));
            assert_eq!(
                serde_json::to_value(&replay[0].protocol_state).unwrap(),
                serde_json::to_value(&initial[0].protocol_state).unwrap()
            );
            let metadata: String = conn
                .query_row(
                    "SELECT metadata FROM messages WHERE id='msg-1'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&metadata).unwrap()["fixtureExtra"],
                "preserved"
            );
            let ui_result: String = conn
                .query_row(
                    "SELECT content FROM messages WHERE id='tool-msg-1'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(ui_result, output);
        }
    }

    #[test]
    fn private_protocol_desktop_arm_finish_and_cancel_refresh_native_ledger_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let state = private_protocol_confirmation_fixture(dir.path());
        let pending =
            load_pending_confirmation(&state.db.lock().unwrap(), "s1", "msg-1", "call-1").unwrap();
        {
            let mut conn = state.db.lock().unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            arm_desktop_action_in_tx(&tx, "s1", "msg-1", "call-1", &pending).unwrap();
            tx.commit().unwrap();
            let replay = hydrate_llm_history(&conn, "s1", 100).unwrap();
            assert_eq!(replay[1].content, DESKTOP_ACTION_INTERRUPTED);
        }
        assert!(build_task_continuation_prompt(&state, "s1", "msg-1")
            .unwrap_err()
            .contains("unknown result"));
        {
            let mut conn = state.db.lock().unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            finish_desktop_action_in_tx(
                &tx,
                "s1",
                "msg-1",
                "call-1",
                &pending,
                true,
                "Confirmation approved: verified",
            )
            .unwrap();
            tx.commit().unwrap();
            let replay = hydrate_llm_history(&conn, "s1", 100).unwrap();
            assert_eq!(replay[1].content, "Confirmation approved: verified");
            assert_eq!(
                replay[0].protocol_state.as_ref().unwrap().output_items[0]["encrypted_content"],
                "approval-marker"
            );
        }
        assert!(build_task_continuation_prompt(&state, "s1", "msg-1")
            .unwrap()
            .contains("verified"));
        let cancelled = private_protocol_confirmation_fixture(dir.path());
        cancel_agent_task_impl(&cancelled, "s1", "msg-1").unwrap();
        let conn = cancelled.db.lock().unwrap();
        let replay = hydrate_llm_history(&conn, "s1", 100).unwrap();
        assert!(replay[1].content.contains("Confirmation cancelled"));
        assert_eq!(
            replay[0].protocol_state.as_ref().unwrap().output_items[0]["encrypted_content"],
            "approval-marker"
        );
    }

    #[test]
    fn private_protocol_edit_cutoff_and_prune_preserve_only_the_selected_prefix() {
        let conn = private_protocol_edit_fixture();
        let cutoff = hydrate_llm_history(&conn, "s", 30).unwrap();
        assert_eq!(cutoff.len(), 2);
        assert_eq!(
            cutoff[1].protocol_state.as_ref().unwrap().output_items[0]["encrypted_content"],
            "retained-marker"
        );
        assert_eq!(prune_edit_and_resend_branch(&conn, "s", "u2").unwrap(), 30);
        let replay = hydrate_llm_history(&conn, "s", i64::MAX).unwrap();
        assert_eq!(replay.len(), 2);
        assert_eq!(
            replay[1].protocol_state.as_ref().unwrap().output_items[0]["encrypted_content"],
            "retained-marker"
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE id IN ('u2','a2')",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn private_protocol_auto_compression_keeps_opaque_ledger_without_summary_merge() {
        let conn = private_protocol_edit_fixture();
        let before: String = conn
            .query_row("SELECT metadata FROM messages WHERE id='a1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let state = app_state_from_connection(conn);
        assert!(
            auto_compress_if_needed(&state, None, "s", &AgentConfig::default(), 0)
                .await
                .unwrap()
                .is_none()
        );
        let conn = state.db.lock().unwrap();
        let after: String = conn
            .query_row("SELECT metadata FROM messages WHERE id='a1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM context_summaries", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "protocol-aware compaction is deferred; a text summary must not replace opaque state"
        );
    }

    #[test]
    fn private_protocol_unknown_metadata_version_fails_closed_without_secret_errors() {
        let conn = private_protocol_edit_fixture();
        conn.execute("UPDATE messages SET metadata=?1 WHERE id='a1'", [r#"{"privateProtocolTranscript":{"version":2,"messages":[],"secret":"opaque-marker"}}"#]).unwrap();
        let error = hydrate_llm_history(&conn, "s", 30).unwrap_err();
        assert!(!error.contains("opaque-marker"));
    }

    #[test]
    fn invalid_text_snapshots_fail_at_the_shared_entry_without_transcript_or_run_effects() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let state = app_state_from_connection(conn);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for (role, content, files) in [
            (
                "user",
                "read",
                serde_json::json!([{ "name": "../private.txt", "text": "data" }]),
            ),
            (
                "assistant",
                "read",
                serde_json::json!([{ "name": "notes.txt", "text": "data" }]),
            ),
            ("user", "", serde_json::json!([])),
            (
                "user",
                "read",
                serde_json::json!([{ "name": "notes.txt", "text": "a".repeat(16 * 1024 + 1) }]),
            ),
        ] {
            let req: SendMessageRequest = serde_json::from_value(serde_json::json!({
                "sessionId": "uncreated", "role": role, "content": content, "textAttachments": files,
            })).unwrap();
            assert!(runtime
                .block_on(send_message_from_dev_http(&state, req))
                .is_err());
        }
        let conn = state.db.lock().unwrap();
        for table in ["messages", "task_runs", "usage_stats"] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "invalid attachments must not write {table}");
        }
    }

    #[test]
    fn edit_resend_preserves_the_original_snapshot_and_validates_before_pruning() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;
            CREATE TABLE messages (id TEXT PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, parent_id TEXT REFERENCES messages(id) ON DELETE CASCADE, created_at INTEGER, metadata TEXT);
            CREATE TABLE sessions (id TEXT PRIMARY KEY, leaf_message_id TEXT REFERENCES messages(id), updated_at INTEGER);
            INSERT INTO sessions VALUES ('s', NULL, 1);
            INSERT INTO messages VALUES ('u', 's', 'user', '', NULL, 10, NULL);
            INSERT INTO messages VALUES ('a', 's', 'assistant', 'summary', 'u', 11, NULL);
            UPDATE sessions SET leaf_message_id = 'a' WHERE id = 's';").unwrap();
        let files = vec![foreground_text_attachments::TextAttachment {
            name: "notes.txt".into(),
            text: "original snapshot".into(),
        }];
        let original = foreground_text_attachments::metadata(&files).unwrap();
        conn.execute(
            "UPDATE messages SET metadata = ?1 WHERE id = 'u'",
            params![r#"{"textAttachments":[{"name":"photo.png","text":"invalid"}]}"#],
        )
        .unwrap();
        assert!(prepare_edit_and_resend_branch(&conn, "s", "u", "replacement").is_err());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row(
                "SELECT leaf_message_id FROM sessions WHERE id = 's'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "a"
        );
        conn.execute(
            "UPDATE messages SET metadata = ?1 WHERE id = 'u'",
            params![foreground_text_attachments::compressed_metadata(Some(
                &original
            ))],
        )
        .unwrap();
        assert!(prepare_edit_and_resend_branch(&conn, "other", "u", "replacement").is_err());
        let (before, recovered) = prepare_edit_and_resend_branch(&conn, "s", "u", "").unwrap();
        assert_eq!(before, 10);
        assert_eq!(recovered, files);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn edit_resend_can_reuse_the_same_user_message_id_repeatedly() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                parent_id TEXT REFERENCES messages(id) ON DELETE CASCADE,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                leaf_message_id TEXT REFERENCES messages(id),
                updated_at INTEGER NOT NULL
            );
            INSERT INTO sessions (id, leaf_message_id, updated_at) VALUES ('s1', NULL, 0);
            INSERT INTO messages VALUES ('u1', 's1', 'user', 'first version', NULL, 1);
            INSERT INTO messages VALUES ('a1', 's1', 'assistant', 'first reply', 'u1', 2);
            UPDATE sessions SET leaf_message_id = 'a1' WHERE id = 's1';
            "#,
        )
        .unwrap();

        assert_eq!(prune_edit_and_resend_branch(&conn, "s1", "u1").unwrap(), 1);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );

        // This mirrors send_message_impl: the replacement intentionally keeps
        // the edited user's durable identity, then receives a fresh reply.
        conn.execute(
            "INSERT INTO messages VALUES ('u1', 's1', 'user', 'second version', NULL, 3)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages VALUES ('a2', 's1', 'assistant', 'second reply', 'u1', 4)",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE sessions SET leaf_message_id = 'a2' WHERE id = 's1'",
            [],
        )
        .unwrap();

        assert_eq!(prune_edit_and_resend_branch(&conn, "s1", "u1").unwrap(), 3);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let leaf: Option<String> = conn
            .query_row(
                "SELECT leaf_message_id FROM sessions WHERE id = 's1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(leaf, None);
    }

    #[test]
    fn usable_context_uses_the_selected_model_window() {
        let agent_config = crate::agent::AgentConfig::default();
        let deepseek = crate::llm::ProviderConfig {
            provider: "deepseek".to_string(),
            model: "deepseek-chat".to_string(),
            ..Default::default()
        };
        let custom = crate::llm::ProviderConfig {
            provider: "custom".to_string(),
            model: "internal-model".to_string(),
            ..Default::default()
        };

        let deepseek_budget = usable_context_tokens(&agent_config, 15, &deepseek).unwrap();
        let custom_budget = usable_context_tokens(&agent_config, 15, &custom).unwrap();

        assert!(deepseek_budget > custom_budget);
        assert_eq!(deepseek_budget - custom_budget, 1_000_000 - 8_192);
    }

    #[test]
    fn execution_permission_validation_fails_closed() {
        assert_eq!(normalize_agent_execution_permission("ask").unwrap(), "ask");
        assert_eq!(
            normalize_agent_execution_permission("workspace_auto").unwrap(),
            "workspace_auto"
        );
        assert_eq!(
            normalize_agent_execution_permission("full_access").unwrap(),
            "full_access"
        );
        assert!(normalize_agent_execution_permission("always_yes").is_err());
    }

    #[test]
    fn execution_permission_uses_the_persisted_value_for_new_agent_runs() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, 1)",
            params![AGENT_EXECUTION_PERMISSION_KEY, "full_access"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_permission_grants (scope, permission, expires_at, updated_at)
             VALUES ('global', 'full_access', NULL, 1)",
            [],
        )
        .unwrap();

        assert_eq!(
            agent_execution_permission_from_conn(&conn).unwrap(),
            "full_access"
        );
    }

    #[test]
    fn bind_session_provider_always_overwrites() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at) VALUES ('s1', 'test', 1, 1)",
            [],
        )
        .unwrap();

        bind_session_provider_in_conn(&conn, "s1", "anthropic", "claude-test", 2).unwrap();
        bind_session_provider_in_conn(&conn, "s1", "openai", "gpt-test", 3).unwrap();
        bind_session_provider_in_conn(&conn, "s1", "deepseek", "v3", 4).unwrap();

        let binding: (Option<String>, Option<String>, i64) = conn
            .query_row(
                "SELECT agent_provider, agent_model, updated_at FROM sessions WHERE id = 's1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(binding.0.as_deref(), Some("deepseek"));
        assert_eq!(binding.1.as_deref(), Some("v3"));
        assert_eq!(binding.2, 4, "the last binding overwrites the previous one");

        let empty_error = bind_session_provider_in_conn(&conn, "s1", "", "", 5)
            .expect_err("empty provider identity must fail closed");
        assert!(empty_error.contains("must be configured"));
    }
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    #[test]
    fn hydrate_agent_steps_restores_tool_calls_for_assistant_messages() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE agent_steps (
                id TEXT PRIMARY KEY,
                call_id TEXT,
                session_id TEXT NOT NULL,
                thought TEXT,
                tool_name TEXT NOT NULL,
                tool_input TEXT,
                tool_output TEXT,
                mcp_schema_version TEXT,
                success INTEGER NOT NULL DEFAULT 1,
                latency_ms INTEGER,
                seq INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_steps (id, session_id, tool_name, tool_input, tool_output, success, seq, created_at)
             VALUES ('step-1', 's1', 'recall_memories', '{\"query\":\"Rust\"}', '[preference] User likes Rust', 1, 1, 42)",
            [],
        )
        .unwrap();

        let mut messages = vec![Message {
            id: "m1".to_string(),
            session_id: "s1".to_string(),
            role: "assistant".to_string(),
            text_attachments: Vec::new(),
            content: "Done".to_string(),
            created_at: 42,
            parent_id: None,
            is_deleted: None,
            tool_calls: None,
            tool_results: None,
            task_run: None,
            task_facts: None,
        }];

        hydrate_agent_steps(&conn, &mut messages).unwrap();

        let calls = messages[0].tool_calls.as_ref().unwrap();
        let results = messages[0].tool_results.as_ref().unwrap();
        assert_eq!(calls[0].id, "step-1");
        assert_eq!(calls[0].name, "recall_memories");
        assert_eq!(calls[0].arguments, "{\"query\":\"Rust\"}");
        assert_eq!(results[0].output, "[preference] User likes Rust");
        assert!(results[0].success);
    }

    #[test]
    fn hydrate_agent_steps_restores_results_when_calls_are_already_inline() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE agent_steps (
                id TEXT PRIMARY KEY, call_id TEXT, session_id TEXT NOT NULL,
                thought TEXT, tool_name TEXT NOT NULL, tool_input TEXT,
                tool_output TEXT, mcp_schema_version TEXT, success INTEGER NOT NULL DEFAULT 1,
                latency_ms INTEGER, seq INTEGER NOT NULL, created_at INTEGER NOT NULL
            );
            INSERT INTO agent_steps
                (id, call_id, session_id, tool_name, tool_input, tool_output, success, seq, created_at)
            VALUES
                ('step-1', 'call-1', 's1', 'create_automation', '{}', 'created', 1, 1, 42);
            "#,
        )
        .unwrap();

        let inline_calls = vec![ToolCallInfo {
            id: "call-1".to_string(),
            name: "create_automation".to_string(),
            arguments: "{}".to_string(),
        }];
        let mut messages = vec![Message {
            id: "m1".to_string(),
            session_id: "s1".to_string(),
            role: "assistant".to_string(),
            text_attachments: Vec::new(),
            content: "Automation created".to_string(),
            created_at: 42,
            parent_id: None,
            is_deleted: None,
            tool_calls: Some(inline_calls.clone()),
            tool_results: None,
            task_run: None,
            task_facts: None,
        }];

        hydrate_agent_steps(&conn, &mut messages).unwrap();

        assert_eq!(
            messages[0].tool_calls.as_ref().unwrap()[0].id,
            inline_calls[0].id
        );
        let results = messages[0].tool_results.as_ref().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].call_id, "call-1");
        assert!(results[0].success);
        assert_eq!(results[0].output, "created");
    }

    #[test]
    fn persist_tool_results_keeps_reused_provider_call_ids_from_separate_slices() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE messages (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL,
                content TEXT NOT NULL, tool_call_id TEXT, tool_name TEXT, created_at INTEGER NOT NULL
            );
            CREATE TABLE agent_steps (
                id TEXT PRIMARY KEY, call_id TEXT, session_id TEXT NOT NULL,
                tool_name TEXT NOT NULL, tool_input TEXT, tool_output TEXT,
                mcp_schema_version TEXT, success INTEGER NOT NULL, seq INTEGER NOT NULL, created_at INTEGER NOT NULL
            );
            "#,
        )
        .unwrap();
        let calls = [ToolCallInfo {
            id: "call_00reused".to_string(),
            name: "write_file".to_string(),
            arguments: r#"{"path":"first.txt"}"#.to_string(),
        }];
        let first = [ToolResultInfo {
            call_id: "call_00reused".to_string(),
            tool_name: "write_file".to_string(),
            success: true,
            output: "first result".to_string(),
            error: None,
            confirmation_required: false,
            confirmation_status: None,
        }];
        let mut second = first.clone();
        second[0].output = "second result".to_string();

        persist_tool_results(&conn, "s1", 10, Some(&calls), &first).unwrap();
        persist_tool_results(&conn, "s1", 11, Some(&calls), &second).unwrap();

        let message_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE tool_call_id = 'call_00reused'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let step_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_steps WHERE call_id = 'call_00reused'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let unique_step_ids: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT id) FROM agent_steps WHERE call_id = 'call_00reused'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(message_count, 2);
        assert_eq!(step_count, 2);
        assert_eq!(unique_step_ids, 2);
    }

    #[test]
    fn hydrate_llm_history_restores_native_tool_result_pairs() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // After migration 034 the assistant message carries its tool_calls
        // inline and each tool result is a separate `role='tool'` row. The
        // hydrator must reconstruct the assistant-tool / tool-result exchange
        // directly from these columns, without joining agent_steps.
        conn.execute_batch(
            r#"
            CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                metadata TEXT,
                tool_calls TEXT,
                tool_call_id TEXT,
                tool_name TEXT,
                created_at INTEGER NOT NULL
            );
            INSERT INTO messages VALUES
                ('u1', 's1', 'user', 'read notes', NULL, NULL, NULL, NULL, 1);
            INSERT INTO messages VALUES
                ('call-1', 's1', 'assistant', 'Found the note.',
                 NULL,
                 '[{"id":"call-1","name":"read_file","arguments":{"path":"notes.txt"}}]',
                 NULL, NULL, 2);
            INSERT INTO messages VALUES
                ('t1', 's1', 'tool', 'hello', NULL, NULL, 'call-1', 'read_file', 2);
            "#,
        )
        .unwrap();

        let history = hydrate_llm_history(&conn, "s1", 3).unwrap();

        assert_eq!(history.len(), 3);
        assert_eq!(history[1].role, "assistant");
        let calls = history[1].tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call-1");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(history[2].role, "tool");
        assert_eq!(history[2].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(history[2].content, "hello");
    }

    /// Regression: after `handleRegenerateMessage` sends the same user message
    /// again, the model must see its previous tool calls and results in the
    /// hydrator output. Without this, the model has no idea what it already
    /// tried and ends up re-running the same tools until the soft
    /// LoopGuards pause the run.
    #[test]
    fn hydrate_llm_history_survives_resend_of_same_user_message() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                metadata TEXT,
                tool_calls TEXT,
                tool_call_id TEXT,
                tool_name TEXT,
                created_at INTEGER NOT NULL
            );
            "#,
        )
        .unwrap();

        // Turn 1: user asks, assistant calls read_file, tool returns content.
        conn.execute(
            "INSERT INTO messages VALUES
                ('u1', 's1', 'user', 'read notes', NULL, NULL, NULL, NULL, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages VALUES
                ('c1', 's1', 'assistant', 'Here are the notes.',
                 NULL,
                 '[{\"id\":\"c1\",\"name\":\"read_file\",\"arguments\":{\"path\":\"notes.txt\"}}]',
                 NULL, NULL, 2)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages VALUES
                ('t1', 's1', 'tool', 'hello world', NULL, NULL, 'c1', 'read_file', 2)",
            [],
        )
        .unwrap();

        // Turn 2: the user re-sends the same question (handleRegenerateMessage
        // path). A new user message is appended, then the LLM history is
        // hydrated up to right before it.
        conn.execute(
            "INSERT INTO messages VALUES
                ('u1-dup', 's1', 'user', 'read notes', NULL, NULL, NULL, NULL, 3)",
            [],
        )
        .unwrap();

        let history = hydrate_llm_history(&conn, "s1", 3).unwrap();

        // The hydrated history must contain the original assistant tool call
        // and tool result, so the model can see what it already did.
        let assistant_with_calls = history
            .iter()
            .find(|m| m.role == "assistant" && m.tool_calls.is_some())
            .expect("assistant message with tool_calls must survive resend");
        let calls = assistant_with_calls.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments, serde_json::json!({"path":"notes.txt"}));

        let tool_result = history
            .iter()
            .find(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("c1"))
            .expect("tool result for c1 must be hydrated");
        assert_eq!(tool_result.content, "hello world");
    }

    /// Regression: even when an `assistant` message has no `tool_calls` (a
    /// plain text reply), resend must still emit one user message and skip
    /// the call/result blocks. This guards the case where the previous
    /// assistant turn was a tool-less conversation.
    #[test]
    fn hydrate_llm_history_handles_conversational_turns() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                metadata TEXT,
                tool_calls TEXT,
                tool_call_id TEXT,
                tool_name TEXT,
                created_at INTEGER NOT NULL
            );
            INSERT INTO messages VALUES
                ('u1', 's1', 'user', 'hi', NULL, NULL, NULL, NULL, 1);
            INSERT INTO messages VALUES
                ('a1', 's1', 'assistant', 'hello!', NULL, NULL, NULL, NULL, 2);
            INSERT INTO messages VALUES
                ('u2', 's1', 'user', 'hi again', NULL, NULL, NULL, NULL, 3);
            "#,
        )
        .unwrap();

        let history = hydrate_llm_history(&conn, "s1", 4).unwrap();

        assert_eq!(history.len(), 3);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].content, "hi");
        assert_eq!(history[1].role, "assistant");
        assert!(history[1].tool_calls.is_none());
        assert_eq!(history[2].role, "user");
        assert_eq!(history[2].content, "hi again");
    }

    /// Regression: when a tool result's tool_call_id has no matching assistant
    /// message in the result window (e.g. the assistant was truncated by LIMIT 100
    /// or the tool result was written without its assistant), the hydrator must
    /// silently drop the orphan tool message rather than returning it and causing
    /// "tool result without preceding assistant" errors at the LLM provider.
    #[test]
    fn hydrate_llm_history_drops_orphan_tool_results() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                metadata TEXT,
                tool_calls TEXT,
                tool_call_id TEXT,
                tool_name TEXT,
                created_at INTEGER NOT NULL
            );
            -- user message
            INSERT INTO messages VALUES
                ('u1', 's1', 'user', 'do task', NULL, NULL, NULL, NULL, 1);
            -- assistant message with tool_calls. The assistant id is 'c1', matching the
            -- tool_call id inside the tool_calls JSON.
            INSERT INTO messages VALUES
                ('c1', 's1', 'assistant', 'Doing it.',
                 NULL,
                 '[{"id":"c1","name":"read_file","arguments":{"path":"x"}}]',
                 NULL, NULL, 2);
            -- tool result with tool_call_id='c1' — valid, matching the assistant above
            INSERT INTO messages VALUES
                ('t1', 's1', 'tool', 'file content', NULL, NULL, 'c1', 'read_file', 2);
            -- orphan tool result whose tool_call_id='unknown-id' — no matching assistant
            -- in the window. This must NOT appear in the hydrated history.
            INSERT INTO messages VALUES
                ('t2', 's1', 'tool', 'orphan output', NULL, NULL, 'unknown-id', 'read_file', 3);
            "#,
        )
        .unwrap();

        let history = hydrate_llm_history(&conn, "s1", 4).unwrap();

        // Must have 3 messages: user + assistant with tool_calls + tool result for c1
        assert_eq!(history.len(), 3, "orphan tool result must be dropped");
        assert_eq!(history[0].role, "user");
        assert_eq!(history[1].role, "assistant");
        assert!(history[1].tool_calls.is_some());
        assert_eq!(history[2].role, "tool");
        assert_eq!(history[2].tool_call_id.as_deref(), Some("c1"));

        // Verify no orphan remains by checking all tool results have valid call_ids
        for msg in &history {
            if msg.role == "tool" {
                assert_ne!(
                    msg.tool_call_id.as_deref(),
                    Some("unknown-id"),
                    "orphan tool result 'unknown-id' must not be in history"
                );
            }
        }
    }

    #[test]
    fn hydrate_task_runs_summarizes_agent_step_state() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE agent_steps (
                id TEXT PRIMARY KEY,
                call_id TEXT,
                session_id TEXT NOT NULL,
                thought TEXT,
                tool_name TEXT NOT NULL,
                tool_input TEXT,
                tool_output TEXT,
                mcp_schema_version TEXT,
                success INTEGER NOT NULL DEFAULT 1,
                latency_ms INTEGER,
                seq INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE task_runs (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                goal TEXT NOT NULL,
                status TEXT NOT NULL,
                plan TEXT NOT NULL,
                confirmation_state TEXT NOT NULL DEFAULT 'none',
                resumable INTEGER NOT NULL DEFAULT 0,
                step_count INTEGER NOT NULL DEFAULT 0,
                completed_step_count INTEGER NOT NULL DEFAULT 0,
                continuation_context TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_steps (id, session_id, tool_name, tool_input, tool_output, success, seq, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                "call-write",
                "session-1",
                "write_file",
                r#"{"path":"notes.md"}"#,
                "Confirmation required for side-effect tool 'write_file'.",
                0,
                1,
                101
            ],
        )
        .unwrap();

        let mut messages = vec![
            Message {
                id: "user-1".to_string(),
                session_id: "session-1".to_string(),
                role: "user".to_string(),
                text_attachments: Vec::new(),
                content: "Update my notes".to_string(),
                created_at: 100,
                parent_id: None,
                is_deleted: None,
                tool_calls: None,
                tool_results: None,
                task_run: None,
                task_facts: None,
            },
            Message {
                id: "assistant-1".to_string(),
                session_id: "session-1".to_string(),
                role: "assistant".to_string(),
                text_attachments: Vec::new(),
                content: "I need approval before writing.".to_string(),
                created_at: 101,
                parent_id: None,
                is_deleted: None,
                tool_calls: None,
                tool_results: None,
                task_run: None,
                task_facts: None,
            },
        ];

        hydrate_agent_steps(&conn, &mut messages).unwrap();
        hydrate_task_runs(&mut messages);

        let task_run = messages[1].task_run.as_ref().unwrap();
        assert_eq!(task_run.id, "assistant-1");
        assert_eq!(task_run.goal, "Update my notes");
        assert_eq!(task_run.status, "awaiting_confirmation");
        assert_eq!(task_run.confirmation_state, "pending");
        assert!(task_run.resumable);
        assert_eq!(task_run.plan, vec!["write_file"]);
    }

    #[test]
    fn soft_paused_tool_batch_remains_resumable() {
        let message = Message {
            id: "assistant-paused".to_string(),
            session_id: "session-1".to_string(),
            role: "assistant".to_string(),
            text_attachments: Vec::new(),
            content: "已暂停，等待用户介入（side effect limit reached (10); user confirmation needed before more）。".to_string(),
            created_at: 101,
            parent_id: None,
            is_deleted: None,
            tool_calls: Some(vec![ToolCallInfo {
                id: "call-1".to_string(),
                name: "write_file".to_string(),
                arguments: r#"{"path":"item-01.md","content":"done"}"#.to_string(),
            }]),
            tool_results: Some(vec![ToolResultInfo {
                call_id: "call-1".to_string(),
                tool_name: "write_file".to_string(),
                success: true,
                output: "written".to_string(),
                error: None,
                confirmation_required: false,
                confirmation_status: None,
            }]),
            task_run: None,
            task_facts: None,
        };

        let task_run = summarize_task_run(&message, "Create fifty files".to_string()).unwrap();

        assert_eq!(task_run.status, "needs_attention");
        assert_eq!(task_run.confirmation_state, "none");
        assert!(task_run.resumable);
    }

    #[test]
    fn persist_task_run_stores_durable_summary() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE task_runs (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                goal TEXT NOT NULL,
                status TEXT NOT NULL,
                plan TEXT NOT NULL,
                confirmation_state TEXT NOT NULL DEFAULT 'none',
                resumable INTEGER NOT NULL DEFAULT 0,
                step_count INTEGER NOT NULL DEFAULT 0,
                completed_step_count INTEGER NOT NULL DEFAULT 0,
                continuation_context TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .unwrap();

        let message = Message {
            id: "assistant-1".to_string(),
            session_id: "session-1".to_string(),
            role: "assistant".to_string(),
            text_attachments: Vec::new(),
            content: "I need approval.".to_string(),
            created_at: 101,
            parent_id: None,
            is_deleted: None,
            tool_calls: None,
            tool_results: None,
            task_run: Some(AgentTaskRun {
                id: "assistant-1".to_string(),
                goal: "Update my notes".to_string(),
                status: "awaiting_confirmation".to_string(),
                plan: vec!["write_file".to_string()],
                confirmation_state: "pending".to_string(),
                resumable: true,
                step_count: 1,
                completed_step_count: 0,
            }),
            task_facts: None,
        };

        persist_task_run(&conn, &message).unwrap();

        let stored: (String, String, String, i32, i32, i32) = conn
            .query_row(
                "SELECT goal, status, confirmation_state, resumable, step_count, completed_step_count
                 FROM task_runs WHERE message_id = 'assistant-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!(stored.0, "Update my notes");
        assert_eq!(stored.1, "awaiting_confirmation");
        assert_eq!(stored.2, "pending");
        assert_eq!(stored.3, 1);
        assert_eq!(stored.4, 1);
        assert_eq!(stored.5, 0);

        let plan: String = conn
            .query_row(
                "SELECT plan FROM task_runs WHERE message_id = 'assistant-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(plan, r#"["write_file"]"#);
    }

    #[test]
    fn hydrate_task_runs_prefers_durable_task_run_rows() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE task_runs (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                goal TEXT NOT NULL,
                status TEXT NOT NULL,
                plan TEXT NOT NULL,
                confirmation_state TEXT NOT NULL DEFAULT 'none',
                resumable INTEGER NOT NULL DEFAULT 0,
                step_count INTEGER NOT NULL DEFAULT 0,
                completed_step_count INTEGER NOT NULL DEFAULT 0,
                continuation_context TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            INSERT INTO task_runs (
                id, session_id, message_id, goal, status, plan, confirmation_state,
                resumable, step_count, completed_step_count, created_at, updated_at
            ) VALUES (
                'task-1', 'session-1', 'assistant-1', 'Persisted goal',
                'needs_attention', '["write_file","verify_result"]', 'rejected',
                1, 2, 1, 100, 101
            );
            "#,
        )
        .unwrap();

        let mut messages = vec![Message {
            id: "assistant-1".to_string(),
            session_id: "session-1".to_string(),
            role: "assistant".to_string(),
            text_attachments: Vec::new(),
            content: "Stored task".to_string(),
            created_at: 101,
            parent_id: None,
            is_deleted: None,
            tool_calls: None,
            tool_results: None,
            task_run: None,
            task_facts: None,
        }];

        hydrate_task_runs_from_db(&conn, &mut messages).unwrap();

        let task_run = messages[0].task_run.as_ref().unwrap();
        assert_eq!(task_run.id, "task-1");
        assert_eq!(task_run.goal, "Persisted goal");
        assert_eq!(task_run.status, "needs_attention");
        assert_eq!(task_run.plan, vec!["write_file", "verify_result"]);
        assert_eq!(task_run.confirmation_state, "rejected");
        assert!(task_run.resumable);
        assert_eq!(task_run.step_count, 2);
        assert_eq!(task_run.completed_step_count, 1);
    }

    #[test]
    fn hydrate_task_facts_reads_durable_facts_into_message() {
        // The backend persists TaskFacts to `task_run_facts` on every
        // agent turn. The chat fetch path must read it back so the
        // timeline can render structured task state without a separate
        // IPC call. This is the round-trip half of the durability
        // contract; the persist half is covered in task_facts tests.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE task_run_facts (
                task_run_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                facts_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .unwrap();

        let mut facts = TaskFacts::new("write a Rust function");
        facts.modified_files = vec!["src/lib.rs".to_string()];
        facts.completed_steps = vec!["plan".to_string()];
        facts.terminal_reason = Some(TaskTerminalReason::Completed);
        let facts_json = serde_json::to_string(&facts).unwrap();
        conn.execute(
            "INSERT INTO task_run_facts
                (task_run_id, session_id, message_id, facts_json, created_at, updated_at)
             VALUES ('task-1', 'session-1', 'assistant-1', ?1, 100, 100)",
            params![facts_json],
        )
        .unwrap();

        let mut messages = vec![Message {
            id: "assistant-1".to_string(),
            session_id: "session-1".to_string(),
            role: "assistant".to_string(),
            text_attachments: Vec::new(),
            content: "Done".to_string(),
            created_at: 101,
            parent_id: None,
            is_deleted: None,
            tool_calls: None,
            tool_results: None,
            task_run: None,
            task_facts: None,
        }];
        hydrate_task_facts_from_db(&conn, &mut messages).unwrap();

        let hydrated = messages[0]
            .task_facts
            .as_ref()
            .expect("facts must be populated from task_run_facts");
        assert_eq!(hydrated.goal, "write a Rust function");
        assert_eq!(hydrated.modified_files, vec!["src/lib.rs".to_string()]);
        assert_eq!(hydrated.completed_steps, vec!["plan".to_string()]);
        assert_eq!(
            hydrated.terminal_reason,
            Some(TaskTerminalReason::Completed)
        );
    }

    #[test]
    fn hydrate_task_facts_leaves_user_messages_alone() {
        // `task_run_facts` is keyed by assistant `message_id`. The hydrate
        // loop must skip non-assistant rows so a stray write against a
        // user message id cannot leak into the user-facing view.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE task_run_facts (
                task_run_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                facts_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        let mut messages = vec![Message {
            id: "user-1".to_string(),
            session_id: "session-1".to_string(),
            role: "user".to_string(),
            text_attachments: Vec::new(),
            content: "hi".to_string(),
            created_at: 100,
            parent_id: None,
            is_deleted: None,
            tool_calls: None,
            tool_results: None,
            task_run: None,
            task_facts: None,
        }];
        hydrate_task_facts_from_db(&conn, &mut messages).unwrap();
        assert!(messages[0].task_facts.is_none());
    }

    #[test]
    fn hydrate_task_facts_tolerates_malformed_json() {
        // A historical corruption in `facts_json` must not break the
        // chat fetch path. The hydrate function should propagate the
        // parse error so the caller can log it, but the existing
        // tests use `?` propagation which is the desired contract:
        // failing loud beats silent skip for a durability bug.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE task_run_facts (
                task_run_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                facts_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_run_facts
                (task_run_id, session_id, message_id, facts_json, created_at, updated_at)
             VALUES ('t', 's', 'm', '{ this is not json', 1, 1)",
            [],
        )
        .unwrap();
        let mut messages = vec![Message {
            id: "m".to_string(),
            session_id: "s".to_string(),
            role: "assistant".to_string(),
            text_attachments: Vec::new(),
            content: "x".to_string(),
            created_at: 1,
            parent_id: None,
            is_deleted: None,
            tool_calls: None,
            tool_results: None,
            task_run: None,
            task_facts: None,
        }];
        let result = hydrate_task_facts_from_db(&conn, &mut messages);
        assert!(result.is_err(), "malformed JSON must surface as an error");
    }

    fn setup_confirmation_state(work_dir: &std::path::Path) -> AppState {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                work_dir TEXT
            );
            CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                tool_call_id TEXT,
                tool_name TEXT,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE agent_steps (
                id TEXT PRIMARY KEY,
                call_id TEXT,
                session_id TEXT NOT NULL,
                thought TEXT,
                tool_name TEXT NOT NULL,
                tool_input TEXT,
                tool_output TEXT,
                mcp_schema_version TEXT,
                success INTEGER NOT NULL DEFAULT 1,
                latency_ms INTEGER,
                seq INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE task_runs (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                goal TEXT NOT NULL,
                status TEXT NOT NULL,
                plan TEXT NOT NULL,
                confirmation_state TEXT NOT NULL DEFAULT 'none',
                resumable INTEGER NOT NULL DEFAULT 0,
                step_count INTEGER NOT NULL DEFAULT 0,
                completed_step_count INTEGER NOT NULL DEFAULT 0,
                continuation_context TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TABLE task_run_facts (
                task_run_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                facts_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at, work_dir) VALUES ('s1', 'test', 1, 1, ?1)",
            params![work_dir.to_string_lossy().to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES ('work_directory', ?1, 1)",
            params![work_dir.to_string_lossy().to_string()],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES ('msg-1', 's1', 'assistant', 'Needs approval', 11);
             INSERT INTO messages (id, session_id, role, content, tool_call_id, tool_name, created_at)
             VALUES ('tool-msg-1', 's1', 'tool',
                     'Confirmation required before executing side-effect tool ''write_file''.',
                     'call-1', 'write_file', 11);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_steps (id, session_id, tool_name, tool_input, tool_output, success, seq, created_at)
             VALUES ('call-1', 's1', 'write_file', '{\"path\":\"approved.txt\",\"content\":\"hello\"}', 'Confirmation required before executing side-effect tool ''write_file''.', 0, 1, 11)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_runs (
                id, session_id, message_id, goal, status, plan, confirmation_state,
                resumable, step_count, completed_step_count, created_at, updated_at
            ) VALUES (
                'msg-1', 's1', 'msg-1', 'Write a file', 'awaiting_confirmation',
                '[\"write_file\"]', 'pending', 1, 1, 0, 10, 11
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_run_facts (
                task_run_id, session_id, message_id, facts_json, created_at, updated_at
             ) VALUES (
                'msg-1', 's1', 'msg-1',
                '{\"goal\":\"Write a file\",\"plan\":[{\"id\":\"call-1\",\"description\":\"write_file\",\"depends_on\":[],\"status\":\"pending\"}],\"completed_steps\":[],\"failed_steps\":[],\"modified_files\":[],\"verification_evidence\":[],\"pending_confirmation\":{\"call_id\":\"call-1\",\"tool_name\":\"write_file\",\"arguments\":{\"path\":\"approved.txt\",\"content\":\"hello\"},\"requested_at\":11},\"repair_attempts\":0,\"no_progress_count\":0,\"context_summary\":null,\"provider_attempts\":0,\"provider_binding\":null,\"terminal_reason\":\"awaiting_confirmation\"}',
                10, 11
             )",
            [],
        )
        .unwrap();

        app_state_from_connection(conn)
    }

    fn app_state_from_connection(conn: rusqlite::Connection) -> AppState {
        AppState {
            db: Arc::new(Mutex::new(conn)),
            runtime_health: crate::runtime_health::RuntimeHealthState::default(),
            delegation_pump: None,
            delegation_available: false,
            delegated_model_factory:
                crate::agent::delegated_model_host::DeferredKeychainDelegatedModelHostFactory::new(),
            foreground_automation_pump: Default::default(),
            mcp_manager: Arc::new(crate::mcp_client::McpProcessManager::new()),
            file_watcher: crate::file_watcher::FileWatcherManager::new(),
            cached_settings: crate::settings_cache::new_cache(),
            steering: crate::commands::steering::SteeringState::new(),
            skills: crate::commands::skill::SkillState::new(),
            event_emitter: None,
            dev_event_store: None,
            desktop_adapter: crate::desktop_control::create_mock_adapter(),
            desktop_action_previews: Default::default(),
        }
    }

    fn setup_draft_confirmation_state(
        work_dir: &std::path::Path,
    ) -> (AppState, Arc<crate::desktop_control::MockDesktopAdapter>) {
        let mut state = setup_confirmation_state(work_dir);
        let executable = work_dir.join("draft-target.exe");
        std::fs::write(&executable, b"test executable placeholder").unwrap();
        let executable = std::fs::canonicalize(executable).unwrap();
        let mock = Arc::new(crate::desktop_control::MockDesktopAdapter::default());
        state.desktop_adapter = mock.clone();
        {
            let conn = state.db.lock().unwrap();
            conn.execute_batch(
                "CREATE TABLE desktop_trusted_apps (
                    id TEXT PRIMARY KEY,
                    display_name TEXT NOT NULL,
                    executable_path TEXT NOT NULL,
                    capabilities TEXT NOT NULL,
                    draft_selector TEXT,
                    enabled INTEGER NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO desktop_trusted_apps
                 (id, display_name, executable_path, capabilities, draft_selector, enabled, created_at, updated_at)
                 VALUES ('draft-app', 'Draft App', ?1, '[\"draft\",\"observe\"]', 'Message editor', 1, 1, 1)",
                params![executable.to_string_lossy().to_string()],
            )
            .unwrap();
            conn.execute(
                "UPDATE agent_steps
                 SET tool_name = 'prepare_message_draft',
                     tool_input = '{\"app_id\":\"draft-app\",\"text\":\"exact pending draft\"}',
                     tool_output = 'Confirmation required before executing side-effect tool ''prepare_message_draft''.'
                 WHERE id = 'call-1'",
                [],
            )
            .unwrap();
        }
        (state, mock)
    }

    /// Real UIA stays outside offline `verify.py full`; its Python runner owns
    /// the only trusted process/window and supplies this explicit opt-in.
    #[cfg(windows)]
    #[test]
    #[ignore = "requires scripts/native_uia_smoke.py and its owned interactive WPF fixture"]
    fn native_uia_fixture_confirmation() {
        let executable = std::fs::canonicalize(
            std::env::var_os("ANGELBOT_NATIVE_UIA_SMOKE_EXE")
                .expect("run via scripts/native_uia_smoke.py"),
        )
        .unwrap();
        let snapshot = std::path::PathBuf::from(
            std::env::var_os("ANGELBOT_NATIVE_UIA_SMOKE_SNAPSHOT").unwrap(),
        );
        let process_id: u32 = std::env::var("ANGELBOT_NATIVE_UIA_SMOKE_PID")
            .unwrap()
            .parse()
            .unwrap();
        let title = std::env::var("ANGELBOT_NATIVE_UIA_SMOKE_TITLE").unwrap();
        let text = std::env::var("ANGELBOT_NATIVE_UIA_SMOKE_TEXT").unwrap();
        assert_eq!(
            executable.file_name().unwrap(),
            "AngelBotNativeUiaFixture.exe"
        );
        let fixture_directory = executable.parent().unwrap();
        assert!(fixture_directory
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("angelbot-native-uia-"));
        assert_eq!(
            std::fs::canonicalize(snapshot.parent().unwrap()).unwrap(),
            fixture_directory
        );
        assert_eq!(snapshot.file_name().unwrap(), "fixture-state.json");

        let wait_for_fixture = |read_only: bool, expected_text: &str| {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut last_observed = None;
            loop {
                if let Some(value) = std::fs::read(&snapshot)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                {
                    assert_eq!(value["processId"].as_u64(), Some(u64::from(process_id)));
                    assert_eq!(value["windowTitle"].as_str(), Some(title.as_str()));
                    last_observed = Some(serde_json::json!({
                        "ordinaryReadOnly": value["ordinaryReadOnly"],
                        "ordinaryValue": value["ordinaryValue"],
                    }));
                    if value["ordinaryReadOnly"].as_bool() == Some(read_only)
                        && value["ordinaryValue"].as_str() == Some(expected_text)
                    {
                        assert_eq!(value["readOnlyValue"], "read-only-seed");
                        assert_eq!(value["duplicateOneValue"], "duplicate-one-seed");
                        assert_eq!(value["duplicateTwoValue"], "duplicate-two-seed");
                        assert_eq!(value["passwordUnchanged"], true);
                        break;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "owned fixture readback timed out: expected readOnly={read_only}, text={expected_text:?}; last={last_observed:?}",
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        };
        wait_for_fixture(false, "ordinary-seed");

        let work_dir = tempfile::tempdir_in(fixture_directory).unwrap();
        let make_state = || {
            let (mut state, _) = setup_draft_confirmation_state(work_dir.path());
            state.desktop_adapter = crate::desktop_control::create_runtime_adapter();
            assert_eq!(state.desktop_adapter.name(), "windows-native-uia");
            state.db.lock().unwrap().execute(
                "UPDATE desktop_trusted_apps SET executable_path = ?1,
                 capabilities = '[\"fill\",\"interact\",\"observe\"]', draft_selector = NULL WHERE id = 'draft-app'",
                params![executable.to_string_lossy().to_string()],
            ).unwrap();
            state
        };
        let observe_and_bind = |state: &AppState| {
            let observed = state
                .desktop_adapter
                .observe_trusted_window("draft-app", executable.to_str().unwrap())
                .unwrap();
            assert!(!observed.truncated);
            let serialized = serde_json::to_string(&observed).unwrap();
            for private_value in [
                "ordinary-seed",
                "read-only-seed",
                "native-uia-password-sentinel",
            ] {
                assert!(
                    !serialized.contains(private_value),
                    "observation leaked a field value"
                );
            }
            assert!(!observed
                .controls
                .iter()
                .any(|control| control.automation_id.as_deref() == Some("passwordField")));
            let readonly: Vec<_> = observed
                .controls
                .iter()
                .filter(|control| control.automation_id.as_deref() == Some("readOnlyField"))
                .collect();
            assert_eq!(readonly.len(), 1);
            assert!(readonly[0].field_ref.is_none());
            assert!(!readonly[0]
                .capabilities
                .iter()
                .any(|capability| capability == "setValue"));
            let duplicates: Vec<_> = observed
                .controls
                .iter()
                .filter(|control| control.automation_id.as_deref() == Some("duplicateField"))
                .collect();
            assert_eq!(duplicates.len(), 2);
            assert!(duplicates.iter().all(|control| control.field_ref.is_none()));
            let ordinary: Vec<_> = observed
                .controls
                .iter()
                .filter(|control| control.automation_id.as_deref() == Some("ordinaryField"))
                .collect();
            assert_eq!(ordinary.len(), 1);
            let field_ref = ordinary[0]
                .field_ref
                .as_ref()
                .expect("ordinary field must be fillable");
            assert!(state
                .desktop_adapter
                .preflight_text_ref(
                    "draft-app",
                    executable.to_str().unwrap(),
                    "unissued-field-ref",
                    Duration::from_secs(8),
                )
                .is_err());
            let arguments =
                serde_json::json!({"app_id": "draft-app", "field_ref": field_ref, "text": text});
            state.db.lock().unwrap().execute(
                "UPDATE agent_steps SET tool_name = 'set_trusted_app_text', tool_input = ?1,
                 tool_output = 'Confirmation required before executing side-effect tool ''set_trusted_app_text''.'
                 WHERE id = 'call-1'",
                params![arguments.to_string()],
            ).unwrap();
        };
        let approve = |preview_id| ResolveAgentConfirmationRequest {
            session_id: "s1".into(),
            message_id: "msg-1".into(),
            call_id: "call-1".into(),
            decision: "approved".into(),
            preview_id,
        };

        let state = make_state();
        observe_and_bind(&state);
        let missing = resolve_agent_confirmation_impl(&state, approve(None));
        assert!(missing.unwrap_err().contains("Inspect the live"));
        wait_for_fixture(false, "ordinary-seed");
        let preview =
            preflight_pending_desktop_action_impl(&state, "s1", "msg-1", "call-1").unwrap();
        assert_eq!(preview.window_title.as_deref(), Some(title.as_str()));
        assert_eq!(preview.control_name, "ordinaryField");
        assert_eq!(preview.text.as_deref(), Some(text.as_str()));
        assert_eq!(
            state.desktop_action_previews.leases.lock().unwrap()[&preview.preview_id]
                .target
                .process_id,
            process_id
        );
        wait_for_fixture(false, "ordinary-seed");

        // The approved identity must not bypass a safety change after preview.
        let command_path = snapshot.with_extension("command");
        std::fs::write(&command_path, "read-only").unwrap();
        wait_for_fixture(true, "ordinary-seed");
        let stale =
            resolve_agent_confirmation_impl(&state, approve(Some(preview.preview_id))).unwrap();
        assert!(!stale.success);
        assert!(
            stale.output.contains("TARGET_UNAVAILABLE"),
            "{}",
            stale.output
        );
        wait_for_fixture(true, "ordinary-seed");

        std::fs::write(&command_path, "editable").unwrap();
        wait_for_fixture(false, "ordinary-seed");
        let state = make_state();
        observe_and_bind(&state);
        let preview =
            preflight_pending_desktop_action_impl(&state, "s1", "msg-1", "call-1").unwrap();
        wait_for_fixture(false, "ordinary-seed");
        let result =
            resolve_agent_confirmation_impl(&state, approve(Some(preview.preview_id))).unwrap();
        assert!(result.success, "{}", result.output);
        let approved_output = result
            .output
            .strip_prefix("Confirmation approved: ")
            .expect("successful confirmation must retain its approved response envelope");
        let result: serde_json::Value = serde_json::from_str(approved_output).unwrap();
        assert_eq!(result["adapter"], "windows-native-uia");
        assert_eq!(result["status"], "verified");
        assert_eq!(result["target"], "draft-app");
        wait_for_fixture(false, &text);

        // A second real chain invokes only the fixture-owned button. Receipt
        // dispatch is distinguished from the separately observed effect.
        let invoke_state = make_state();
        let observation = invoke_state
            .desktop_adapter
            .observe_trusted_window("draft-app", executable.to_str().unwrap())
            .unwrap();
        assert!(!observation.truncated);
        let button = observation
            .controls
            .iter()
            .find(|control| control.automation_id.as_deref() == Some("actionButton"))
            .unwrap();
        assert_eq!(button.name.as_deref(), Some("Apply action"));
        let control_ref = button
            .control_ref
            .as_ref()
            .expect("fixture button must be invokable");
        let input =
            serde_json::json!({"app_id":"draft-app","control_ref":control_ref,"action":"invoke"});
        invoke_state.db.lock().unwrap().execute(
            "UPDATE agent_steps SET tool_name = 'operate_trusted_app_control', tool_input = ?1,
             tool_output = 'Confirmation required before executing side-effect tool ''operate_trusted_app_control''.' WHERE id = 'call-1'",
            params![input.to_string()],
        ).unwrap();
        let preview =
            preflight_pending_desktop_action_impl(&invoke_state, "s1", "msg-1", "call-1").unwrap();
        assert_eq!(preview.operation, "invoke");
        assert_eq!(preview.control_name, "Apply action");
        assert!(preview.text.is_none());
        let receipt = resolve_agent_confirmation_impl(
            &invoke_state,
            approve(Some(preview.preview_id.clone())),
        )
        .unwrap();
        assert!(receipt.success, "{}", receipt.output);
        let dispatched: serde_json::Value = serde_json::from_str(
            receipt
                .output
                .strip_prefix("Confirmation approved: ")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(dispatched["status"], "dispatched");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(value) = std::fs::read(&snapshot)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            {
                assert_eq!(value["processId"].as_u64(), Some(u64::from(process_id)));
                if value["invocationCount"].as_u64() == Some(1) {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "fixture Invoke effect not observed"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        let observed_effect = invoke_state
            .desktop_adapter
            .observe_trusted_window("draft-app", executable.to_str().unwrap())
            .unwrap();
        assert!(!observed_effect.truncated);
        assert!(observed_effect
            .controls
            .iter()
            .any(|control| control.name.as_deref() == Some("Action applied")));
        assert!(invoke_state
            .desktop_adapter
            .preflight_control_ref(
                "draft-app",
                executable.to_str().unwrap(),
                control_ref,
                crate::desktop_control::DesktopControlOperation::Invoke,
                Duration::from_secs(8)
            )
            .is_err());
        assert!(
            resolve_agent_confirmation_impl(&invoke_state, approve(Some(preview.preview_id)))
                .is_err()
        );
        // Same production approval path for stateful controls; each action
        // starts with a fresh observation. Repeating an already-desired state
        // must be a verified no-op, not a second fixture event.
        use crate::desktop_control::DesktopControlOperation;
        let read_owned_fixture = || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(value) = std::fs::read(&snapshot)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                {
                    assert_eq!(value["processId"].as_u64(), Some(u64::from(process_id)));
                    assert_eq!(value["windowTitle"].as_str(), Some(title.as_str()));
                    return value;
                }
                assert!(
                    Instant::now() < deadline,
                    "owned fixture snapshot unavailable"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        };
        let mut expected_scroll_count = 0;
        for operation in [
            DesktopControlOperation::Select,
            DesktopControlOperation::Select,
            DesktopControlOperation::Expand,
            DesktopControlOperation::Expand,
            DesktopControlOperation::Collapse,
            DesktopControlOperation::Collapse,
            DesktopControlOperation::ScrollDown,
            DesktopControlOperation::ScrollDown,
            DesktopControlOperation::ScrollUp,
            DesktopControlOperation::ScrollUp,
        ] {
            let control_state = make_state();
            let observation = control_state
                .desktop_adapter
                .observe_trusted_window("draft-app", executable.to_str().unwrap())
                .unwrap();
            assert!(!observation.truncated);
            let (id, label, snapshot_key, desired) = match operation {
                DesktopControlOperation::Select => {
                    ("fixtureOption", "Fixture option", "selected", true)
                }
                DesktopControlOperation::Expand => {
                    ("fixtureMenu", "More actions", "menuExpanded", true)
                }
                DesktopControlOperation::Collapse => {
                    ("fixtureMenu", "More actions", "menuExpanded", false)
                }
                DesktopControlOperation::ScrollDown => (
                    "fixtureScroll",
                    "Fixture scroll area",
                    "scrollAtBottom",
                    true,
                ),
                DesktopControlOperation::ScrollUp => {
                    ("fixtureScroll", "Fixture scroll area", "scrollAtTop", true)
                }
                DesktopControlOperation::Invoke => unreachable!(),
            };
            let before_preview = read_owned_fixture();
            assert!(before_preview["scrollableHeight"].as_f64().unwrap() > 0.0);
            assert!(before_preview["scrollViewportHeight"].as_f64().unwrap() > 0.0);
            assert_eq!(before_preview["scrollCount"], expected_scroll_count);
            assert_eq!(before_preview["scrollHorizontalOffset"].as_f64(), Some(0.0));
            let reference = observation
                .controls
                .iter()
                .find(|control| control.automation_id.as_deref() == Some(id))
                .unwrap()
                .control_ref
                .as_ref()
                .expect("fixture state control must be actionable");
            control_state.db.lock().unwrap().execute(
                "UPDATE agent_steps SET tool_name = 'operate_trusted_app_control', tool_input = ?1,
                 tool_output = 'Confirmation required before executing side-effect tool ''operate_trusted_app_control''.' WHERE id = 'call-1'",
                params![serde_json::json!({"app_id":"draft-app","control_ref":reference,"action":operation}).to_string()],
            ).unwrap();
            let preview =
                preflight_pending_desktop_action_impl(&control_state, "s1", "msg-1", "call-1")
                    .unwrap();
            assert_eq!(preview.operation, operation.as_str());
            assert_eq!(preview.control_name, label);
            assert!(preview.text.is_none());
            let after_preview = read_owned_fixture();
            for key in ["scrollCount", "scrollOffset", "scrollHorizontalOffset"] {
                assert_eq!(
                    after_preview[key], before_preview[key],
                    "observation/preflight must not scroll"
                );
            }
            if matches!(
                operation,
                DesktopControlOperation::ScrollDown | DesktopControlOperation::ScrollUp
            ) {
                assert!(resolve_agent_confirmation_impl(&control_state, approve(None)).is_err());
                assert_eq!(read_owned_fixture()["scrollCount"], expected_scroll_count);
            }
            let receipt =
                resolve_agent_confirmation_impl(&control_state, approve(Some(preview.preview_id)))
                    .unwrap();
            assert!(
                receipt.success,
                "{}: {}",
                operation.as_str(),
                receipt.output
            );
            let output: serde_json::Value = serde_json::from_str(
                receipt
                    .output
                    .strip_prefix("Confirmation approved: ")
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(output["status"], "verified");
            assert_eq!(output["action"], operation.as_str());
            if (operation == DesktopControlOperation::ScrollDown && expected_scroll_count == 0)
                || (operation == DesktopControlOperation::ScrollUp && expected_scroll_count == 1)
            {
                expected_scroll_count += 1;
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(value) = std::fs::read(&snapshot)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                {
                    assert_eq!(value["processId"].as_u64(), Some(u64::from(process_id)));
                    if value[snapshot_key].as_bool() == Some(desired) {
                        assert_eq!(value["scrollCount"], expected_scroll_count);
                        assert_eq!(value["scrollHorizontalOffset"].as_f64(), Some(0.0));
                        assert_eq!(value["selectionCount"], 1);
                        assert_eq!(value["invocationCount"], 1);
                        if operation == DesktopControlOperation::Collapse {
                            assert_eq!(value["expansionCount"], 1);
                            assert_eq!(value["collapseCount"], 1);
                        }
                        break;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "{} effect not observed",
                    operation.as_str()
                );
                std::thread::sleep(Duration::from_millis(25));
            }
            assert!(
                control_state
                    .desktop_adapter
                    .preflight_control_ref(
                        "draft-app",
                        executable.to_str().unwrap(),
                        reference,
                        operation,
                        Duration::from_secs(8)
                    )
                    .is_err(),
                "all refs must be renewed after an operation"
            );
        }
        wait_for_fixture(false, &text);
    }

    #[test]
    fn desktop_draft_approval_requires_attested_live_target_and_exact_pending_text() {
        let dir = tempfile::tempdir().unwrap();
        let (state, mock) = setup_draft_confirmation_state(dir.path());
        let without_preview = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".into(),
                message_id: "msg-1".into(),
                call_id: "call-1".into(),
                decision: "approved".into(),
                preview_id: None,
            },
        );
        assert!(without_preview.unwrap_err().contains("Inspect the live"));
        assert!(mock.actions().is_empty());

        let preview =
            preflight_pending_desktop_action_impl(&state, "s1", "msg-1", "call-1").unwrap();
        assert_eq!(preview.text.as_deref(), Some("exact pending draft"));
        assert_eq!(preview.app_display_name, "Draft App");
        assert_eq!(preview.control_name, "Message editor");
        assert!(mock.actions().is_empty(), "preflight must not write");

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".into(),
                message_id: "msg-1".into(),
                call_id: "call-1".into(),
                decision: "approved".into(),
                preview_id: Some(preview.preview_id),
            },
        )
        .unwrap();
        assert!(result.success);
        let actions = mock.actions();
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            crate::desktop_control::DesktopAction::PrepareDraft {
                app_id,
                text,
                expected_target: Some(_),
                ..
            } if app_id == "draft-app" && text == "exact pending draft"
        ));
    }

    #[test]
    fn desktop_draft_preview_rejects_changed_configuration_or_pending_arguments() {
        for change_config in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let (state, mock) = setup_draft_confirmation_state(dir.path());
            let preview =
                preflight_pending_desktop_action_impl(&state, "s1", "msg-1", "call-1").unwrap();
            {
                let conn = state.db.lock().unwrap();
                if change_config {
                    conn.execute(
                        "UPDATE desktop_trusted_apps SET draft_selector = 'Other editor' WHERE id = 'draft-app'",
                        [],
                    )
                    .unwrap();
                } else {
                    conn.execute(
                        "UPDATE agent_steps SET tool_input = '{\"app_id\":\"draft-app\",\"text\":\"different draft\"}' WHERE id = 'call-1'",
                        [],
                    )
                    .unwrap();
                }
            }
            let result = resolve_agent_confirmation_impl(
                &state,
                ResolveAgentConfirmationRequest {
                    session_id: "s1".into(),
                    message_id: "msg-1".into(),
                    call_id: "call-1".into(),
                    decision: "approved".into(),
                    preview_id: Some(preview.preview_id),
                },
            );
            assert!(result.is_err());
            assert!(mock.actions().is_empty());
            let conn = state.db.lock().unwrap();
            assert!(load_pending_confirmation(&conn, "s1", "msg-1", "call-1").is_ok());
        }
    }

    fn setup_field_confirmation_state(
        work_dir: &std::path::Path,
    ) -> (AppState, Arc<crate::desktop_control::MockDesktopAdapter>) {
        let (state, mock) = setup_draft_confirmation_state(work_dir);
        {
            let conn = state.db.lock().unwrap();
            conn.execute(
                "UPDATE desktop_trusted_apps
                 SET capabilities = '[\"fill\",\"observe\"]', draft_selector = NULL
                 WHERE id = 'draft-app'",
                [],
            )
            .unwrap();
            conn.execute(
                "UPDATE agent_steps
                 SET tool_name = 'set_trusted_app_text',
                     tool_input = '{\"app_id\":\"draft-app\",\"field_ref\":\"mock-field-ref\",\"text\":\"exact field value\"}',
                     tool_output = 'Confirmation required before executing side-effect tool ''set_trusted_app_text''.'
                 WHERE id = 'call-1'",
                [],
            )
            .unwrap();
        }
        let app = crate::commands::desktop::get_trusted_desktop_app_from_db(&state.db, "draft-app")
            .unwrap()
            .unwrap();
        let observed = state
            .desktop_adapter
            .observe_trusted_window(&app.id, &app.executable_path)
            .unwrap();
        assert_eq!(
            observed.controls[0].field_ref.as_deref(),
            Some("mock-field-ref")
        );
        (state, mock)
    }

    fn setup_control_confirmation_state(
        work_dir: &std::path::Path,
    ) -> (AppState, Arc<crate::desktop_control::MockDesktopAdapter>) {
        let (state, mock) = setup_draft_confirmation_state(work_dir);
        {
            let conn = state.db.lock().unwrap();
            conn.execute_batch(
                "UPDATE desktop_trusted_apps SET capabilities = '[\"interact\",\"observe\",\"observeImage\"]', draft_selector = NULL WHERE id = 'draft-app';
                 UPDATE agent_steps SET tool_name = 'operate_trusted_app_control', tool_input = '{\"app_id\":\"draft-app\",\"control_ref\":\"mock-control-ref\",\"action\":\"invoke\"}',
                 tool_output = 'Confirmation required before executing side-effect tool ''operate_trusted_app_control''.' WHERE id = 'call-1';
                 UPDATE messages SET tool_name = 'operate_trusted_app_control', content = 'Confirmation required before executing side-effect tool ''operate_trusted_app_control''.' WHERE id = 'tool-msg-1';"
            ).unwrap();
        }
        let app = crate::commands::desktop::get_trusted_desktop_app_from_db(&state.db, "draft-app")
            .unwrap()
            .unwrap();
        state
            .desktop_adapter
            .observe_trusted_window(&app.id, &app.executable_path)
            .unwrap();
        (state, mock)
    }

    #[test]
    fn control_actions_require_exact_live_preview_and_are_not_task_completion() {
        use crate::desktop_control::DesktopControlOperation;
        for (operation, reference, label) in [
            (
                DesktopControlOperation::Invoke,
                "mock-control-ref",
                "mockButton",
            ),
            (
                DesktopControlOperation::Select,
                "mock-select-ref",
                "mockOption",
            ),
            (
                DesktopControlOperation::Expand,
                "mock-expand-ref",
                "mockMenu",
            ),
            (
                DesktopControlOperation::Collapse,
                "mock-expand-ref",
                "mockMenu",
            ),
            (
                DesktopControlOperation::ScrollDown,
                "mock-scroll-ref",
                "mockScrollPane",
            ),
            (
                DesktopControlOperation::ScrollUp,
                "mock-scroll-ref",
                "mockScrollPane",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (state, mock) = setup_control_confirmation_state(dir.path());
            if matches!(
                operation,
                DesktopControlOperation::ScrollDown | DesktopControlOperation::ScrollUp
            ) {
                let conn = state.db.lock().unwrap();
                conn.execute_batch(
                    "CREATE TABLE agent_permission_grants (scope TEXT PRIMARY KEY, permission TEXT NOT NULL, expires_at INTEGER, updated_at INTEGER NOT NULL);
                     INSERT INTO agent_permission_grants VALUES ('global', 'full_access', NULL, 1);",
                ).unwrap();
                conn.execute(
                    "INSERT INTO settings (key, value, updated_at) VALUES (?1, 'full_access', 1)",
                    params![AGENT_EXECUTION_PERMISSION_KEY],
                )
                .unwrap();
                assert_eq!(
                    agent_execution_permission_from_conn(&conn).unwrap(),
                    "full_access"
                );
            }
            state.db.lock().unwrap().execute(
            "UPDATE agent_steps SET tool_input = ?1 WHERE id = 'call-1'",
            params![serde_json::json!({"app_id":"draft-app","control_ref":reference,"action":operation}).to_string()],
        ).unwrap();
            let request = |preview_id| ResolveAgentConfirmationRequest {
                session_id: "s1".into(),
                message_id: "msg-1".into(),
                call_id: "call-1".into(),
                decision: "approved".into(),
                preview_id,
            };
            assert!(resolve_agent_confirmation_impl(&state, request(None)).is_err());
            assert!(mock.actions().is_empty());
            let preview =
                preflight_pending_desktop_action_impl(&state, "s1", "msg-1", "call-1").unwrap();
            assert_eq!(preview.operation, operation.as_str());
            assert_eq!(preview.control_name, label);
            assert!(preview.text.is_none());
            let displayed = serde_json::to_string(&preview).unwrap();
            assert!(!displayed.contains(reference));
            assert!(!displayed.contains("runtimeId"));
            assert!(mock.actions().is_empty(), "preview cannot invoke");
            let result =
                resolve_agent_confirmation_impl(&state, request(Some(preview.preview_id.clone())))
                    .unwrap();
            assert!(result.success, "{}", result.output);
            let receipt: serde_json::Value = serde_json::from_str(
                result
                    .output
                    .strip_prefix("Confirmation approved: ")
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                receipt["status"],
                if operation == DesktopControlOperation::Invoke {
                    "dispatched"
                } else {
                    "verified"
                }
            );
            assert_eq!(receipt["action"], operation.as_str());
            assert!(matches!(
                mock.actions().as_slice(),
                [crate::desktop_control::DesktopAction::OperateControl {
                    operation: approved_operation,
                    expected_target: Some(_),
                    ..
                }] if *approved_operation == operation
            ));
            assert!(
                resolve_agent_confirmation_impl(&state, request(Some(preview.preview_id))).is_err()
            );
            assert_eq!(mock.actions().len(), 1, "same approval cannot invoke twice");
        }
    }

    #[test]
    fn control_invocation_rejects_forged_expired_changed_or_revoked_targets() {
        for scenario in [
            "forged",
            "unsupported_action",
            "missing_action",
            "unknown_action",
            "expired",
            "changed",
            "changed_action",
            "changed_scroll_direction",
            "revoked",
            "replaced",
            "revoke_restore",
            "delete_readd",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (state, mock) = setup_control_confirmation_state(dir.path());
            if scenario == "changed_scroll_direction" {
                state.db.lock().unwrap().execute(
                    "UPDATE agent_steps SET tool_input = ?1 WHERE id = 'call-1'",
                    params![serde_json::json!({"app_id":"draft-app","control_ref":"mock-scroll-ref","action":"scrolldown"}).to_string()],
                ).unwrap();
            }
            if matches!(
                scenario,
                "forged" | "unsupported_action" | "missing_action" | "unknown_action"
            ) {
                let mut input = serde_json::json!({"app_id":"draft-app","control_ref":"mock-control-ref","action":"invoke"});
                match scenario {
                    "forged" => input["control_ref"] = serde_json::json!("forged"),
                    "unsupported_action" => input["action"] = serde_json::json!("select"),
                    "missing_action" => {
                        input.as_object_mut().unwrap().remove("action");
                    }
                    "unknown_action" => input["action"] = serde_json::json!("toggle"),
                    _ => unreachable!(),
                }
                state
                    .db
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE agent_steps SET tool_input = ?1 WHERE id = 'call-1'",
                        params![input.to_string()],
                    )
                    .unwrap();
                assert!(
                    preflight_pending_desktop_action_impl(&state, "s1", "msg-1", "call-1").is_err()
                );
            } else {
                if scenario == "revoke_restore" {
                    // Force wall-clock writes below the existing revision:
                    // model same-second saves and backwards clock changes.
                    state.db.lock().unwrap().execute("UPDATE desktop_trusted_apps SET updated_at = ?1 WHERE id = 'draft-app'", params![Utc::now().timestamp() + 3600]).unwrap();
                }
                let preview =
                    preflight_pending_desktop_action_impl(&state, "s1", "msg-1", "call-1").unwrap();
                match scenario {
                    "expired" => {
                        state
                            .desktop_action_previews
                            .leases
                            .lock()
                            .unwrap()
                            .get_mut(&preview.preview_id)
                            .unwrap()
                            .expires_at = Instant::now()
                    }
                    "changed" => {
                        state.db.lock().unwrap().execute("UPDATE agent_steps SET tool_input = '{\"app_id\":\"draft-app\",\"control_ref\":\"other-ref\",\"action\":\"invoke\"}' WHERE id = 'call-1'", []).unwrap();
                    }
                    "changed_action" => {
                        state.db.lock().unwrap().execute("UPDATE agent_steps SET tool_input = '{\"app_id\":\"draft-app\",\"control_ref\":\"mock-control-ref\",\"action\":\"select\"}' WHERE id = 'call-1'", []).unwrap();
                    }
                    "changed_scroll_direction" => {
                        state.db.lock().unwrap().execute(
                            "UPDATE agent_steps SET tool_input = ?1 WHERE id = 'call-1'",
                            params![serde_json::json!({"app_id":"draft-app","control_ref":"mock-scroll-ref","action":"scrollup"}).to_string()],
                        ).unwrap();
                    }
                    "revoked" => {
                        state.db.lock().unwrap().execute("UPDATE desktop_trusted_apps SET capabilities = '[\"observe\"]' WHERE id = 'draft-app'", []).unwrap();
                    }
                    "replaced" => {
                        state.db.lock().unwrap().execute("UPDATE desktop_trusted_apps SET updated_at = 999 WHERE id = 'draft-app'", []).unwrap();
                    }
                    "revoke_restore" | "delete_readd" => {
                        let original = crate::commands::desktop::get_trusted_desktop_app_from_db(
                            &state.db,
                            "draft-app",
                        )
                        .unwrap()
                        .unwrap();
                        if scenario == "revoke_restore" {
                            let mut revoked = original.clone();
                            revoked.capabilities.retain(|value| {
                                value != crate::commands::desktop::CAPABILITY_INTERACT
                            });
                            let revoked = crate::commands::desktop::save_desktop_trusted_app_impl(
                                &state, revoked,
                            )
                            .unwrap();
                            let restored = crate::commands::desktop::save_desktop_trusted_app_impl(
                                &state,
                                original.clone(),
                            )
                            .unwrap();
                            assert!(revoked.updated_at > original.updated_at);
                            assert!(restored.updated_at > revoked.updated_at);
                            assert_eq!(restored.capabilities, original.capabilities);
                        } else {
                            crate::commands::desktop::delete_desktop_trusted_app_impl(
                                &state,
                                &original.id,
                            )
                            .unwrap();
                            crate::commands::desktop::save_desktop_trusted_app_impl(
                                &state, original,
                            )
                            .unwrap();
                        }
                    }
                    _ => unreachable!(),
                }
                let resolved = resolve_agent_confirmation_impl(
                    &state,
                    ResolveAgentConfirmationRequest {
                        session_id: "s1".into(),
                        message_id: "msg-1".into(),
                        call_id: "call-1".into(),
                        decision: "approved".into(),
                        preview_id: Some(preview.preview_id),
                    },
                );
                assert!(
                    resolved.is_err() || !resolved.unwrap().success,
                    "{scenario}"
                );
            }
            assert!(mock.actions().is_empty(), "{scenario} must not invoke");
        }
    }

    #[test]
    fn trusted_field_write_uses_observed_ref_and_backend_preview_not_model_target() {
        let dir = tempfile::tempdir().unwrap();
        let (state, mock) = setup_field_confirmation_state(dir.path());
        let missing = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".into(),
                message_id: "msg-1".into(),
                call_id: "call-1".into(),
                decision: "approved".into(),
                preview_id: None,
            },
        );
        assert!(missing.is_err());
        assert!(mock.actions().is_empty());

        let preview =
            preflight_pending_desktop_action_impl(&state, "s1", "msg-1", "call-1").unwrap();
        assert_eq!(preview.operation, "field");
        assert_eq!(preview.control_name, "mockField");
        assert_eq!(preview.text.as_deref(), Some("exact field value"));
        assert!(mock.actions().is_empty());

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".into(),
                message_id: "msg-1".into(),
                call_id: "call-1".into(),
                decision: "approved".into(),
                preview_id: Some(preview.preview_id),
            },
        )
        .unwrap();
        assert!(result.success);
        assert!(matches!(mock.actions().as_slice(), [
            crate::desktop_control::DesktopAction::SetText {
                app_id,
                text,
                expected_target: Some(_),
                ..
            }
        ] if app_id == "draft-app" && text == "exact field value"));
    }

    #[test]
    fn trusted_field_write_rejects_revoked_fill_or_invalid_observation_ref() {
        let dir = tempfile::tempdir().unwrap();
        let (state, mock) = setup_field_confirmation_state(dir.path());
        {
            let conn = state.db.lock().unwrap();
            conn.execute(
                "UPDATE agent_steps SET tool_input = '{\"app_id\":\"draft-app\",\"field_ref\":\"forged-ref\",\"text\":\"exact field value\"}' WHERE id = 'call-1'",
                [],
            )
            .unwrap();
        }
        assert!(preflight_pending_desktop_action_impl(&state, "s1", "msg-1", "call-1").is_err());
        {
            let conn = state.db.lock().unwrap();
            conn.execute(
                "UPDATE agent_steps SET tool_input = '{\"app_id\":\"draft-app\",\"field_ref\":\"mock-field-ref\",\"text\":\"exact field value\"}' WHERE id = 'call-1'",
                [],
            )
            .unwrap();
        }
        let preview =
            preflight_pending_desktop_action_impl(&state, "s1", "msg-1", "call-1").unwrap();
        state.db.lock().unwrap().execute(
            "UPDATE desktop_trusted_apps SET capabilities = '[\"observe\"]' WHERE id = 'draft-app'",
            [],
        ).unwrap();
        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".into(),
                message_id: "msg-1".into(),
                call_id: "call-1".into(),
                decision: "approved".into(),
                preview_id: Some(preview.preview_id),
            },
        );
        let result = result.unwrap();
        assert!(!result.success);
        assert!(result.output.contains("no longer available"));
        assert!(mock.actions().is_empty());
        assert!(
            load_pending_confirmation(&state.db.lock().unwrap(), "s1", "msg-1", "call-1").is_err()
        );
    }

    #[test]
    fn desktop_action_attempt_is_not_reapprovable_after_interruption() {
        for invocation in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let (state, mock) = if invocation {
                setup_control_confirmation_state(dir.path())
            } else {
                setup_field_confirmation_state(dir.path())
            };
            let pending =
                load_pending_confirmation(&state.db.lock().unwrap(), "s1", "msg-1", "call-1")
                    .unwrap();
            {
                let mut conn = state.db.lock().unwrap();
                let tx = conn.unchecked_transaction().unwrap();
                arm_desktop_action_in_tx(&tx, "s1", "msg-1", "call-1", &pending).unwrap();
                tx.commit().unwrap();
            }
            assert!(
                mock.actions().is_empty(),
                "arming precedes any OS side effect"
            );
            let conn = state.db.lock().unwrap();
            assert!(load_pending_confirmation(&conn, "s1", "msg-1", "call-1").is_err());
            assert!(conn
                .query_row(
                    "SELECT tool_output FROM agent_steps WHERE id = 'call-1'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap()
                .contains("RESULT_UNKNOWN"));
            assert!(conn
                .query_row(
                    "SELECT content FROM messages WHERE id = 'tool-msg-1'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap()
                .contains("RESULT_UNKNOWN"));
            drop(conn);
            assert!(
                preflight_pending_desktop_action_impl(&state, "s1", "msg-1", "call-1").is_err()
            );

            {
                let mut conn = state.db.lock().unwrap();
                let tx = conn.unchecked_transaction().unwrap();
                finish_desktop_action_in_tx(
                    &tx,
                    "s1",
                    "msg-1",
                    "call-1",
                    &pending,
                    true,
                    "Confirmation approved: verified",
                )
                .unwrap();
                tx.commit().unwrap();
            }
            let conn = state.db.lock().unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT success FROM agent_steps WHERE id = 'call-1'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                1
            );
            assert!(conn
                .query_row(
                    "SELECT content FROM messages WHERE id = 'tool-msg-1'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap()
                .contains("verified"));
        }
    }

    #[test]
    fn desktop_text_write_does_not_arm_without_durable_tool_result() {
        let dir = tempfile::tempdir().unwrap();
        let (state, mock) = setup_field_confirmation_state(dir.path());
        let pending =
            load_pending_confirmation(&state.db.lock().unwrap(), "s1", "msg-1", "call-1").unwrap();
        let mut conn = state.db.lock().unwrap();
        conn.execute("DELETE FROM messages WHERE id = 'tool-msg-1'", [])
            .unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        assert!(
            arm_desktop_action_in_tx(&tx, "s1", "msg-1", "call-1", &pending)
                .unwrap_err()
                .contains("durable tool result")
        );
        drop(tx);
        assert!(load_pending_confirmation(&conn, "s1", "msg-1", "call-1").is_ok());
        assert!(mock.actions().is_empty());
    }

    #[test]
    fn unknown_desktop_write_blocks_continuation_after_another_approval() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _) = setup_field_confirmation_state(dir.path());
        {
            let conn = state.db.lock().unwrap();
            conn.execute_batch(
                "INSERT INTO messages
                     (id, session_id, role, content, tool_call_id, tool_name, created_at)
                 VALUES ('tool-msg-2', 's1', 'tool',
                     'Confirmation required before executing side-effect tool ''open_windows_setting''.',
                     'call-2', 'open_windows_setting', 11);
                 INSERT INTO agent_steps
                     (id, session_id, tool_name, tool_input, tool_output, success, seq, created_at)
                 VALUES ('call-2', 's1', 'open_windows_setting', '{\"page\":\"sound\"}',
                     'Confirmation required before executing side-effect tool ''open_windows_setting''.',
                     0, 2, 11);",
            )
            .unwrap();
        }
        let first =
            load_pending_confirmation(&state.db.lock().unwrap(), "s1", "msg-1", "call-1").unwrap();
        {
            let mut conn = state.db.lock().unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            arm_desktop_action_in_tx(&tx, "s1", "msg-1", "call-1", &first).unwrap();
            tx.commit().unwrap();
        }
        let second =
            load_pending_confirmation(&state.db.lock().unwrap(), "s1", "msg-1", "call-2").unwrap();
        {
            let mut conn = state.db.lock().unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            settle_pending_confirmation_in_tx(
                &tx,
                "s1",
                "msg-1",
                "call-2",
                &second,
                true,
                "Confirmation approved: dispatched",
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let error = build_task_continuation_prompt(&state, "s1", "msg-1").unwrap_err();
        assert!(error.contains("unknown result"));
    }

    struct LiveMcpEchoServer {
        calls_path: PathBuf,
        schema_path: PathBuf,
    }

    fn start_live_mcp_echo_server(state: &AppState, dir: &tempfile::TempDir) -> LiveMcpEchoServer {
        let calls_path = dir.path().join("mcp-calls.jsonl");
        let schema_path = dir.path().join("mcp-schema-mode.txt");
        let server_path = dir.path().join("mcp-server.cjs");
        let calls_path_js = calls_path.to_string_lossy().replace('\\', "\\\\");
        let schema_path_js = schema_path.to_string_lossy().replace('\\', "\\\\");
        fs::write(&schema_path, "string").unwrap();
        fs::write(
            &server_path,
            format!(
                r#"
const fs = require('fs');
const readline = require('readline');
const callsPath = "{calls_path}";
const schemaPath = "{schema_path}";
const rl = readline.createInterface({{ input: process.stdin }});
function send(id, result) {{
  process.stdout.write(JSON.stringify({{ jsonrpc: '2.0', id, result }}) + '\n');
}}
rl.on('line', (line) => {{
  const req = JSON.parse(line);
  if (req.method === 'initialize') {{
    send(req.id, {{ protocolVersion: '2024-11-05', capabilities: {{ tools: {{}} }}, serverInfo: {{ name: 'angelbot-test-mcp', version: '1.0.0' }} }});
    return;
  }}
  if (req.method === 'notifications/initialized') return;
  if (req.method === 'tools/list') {{
    const schemaMode = fs.readFileSync(schemaPath, 'utf8').trim();
    if (schemaMode === 'removed') {{
      send(req.id, {{ tools: [] }});
      return;
    }}
    const messageType = schemaMode === 'number' ? 'number' : 'string';
    send(req.id, {{ tools: [{{ name: 'echo', description: 'Echo a message', inputSchema: {{ type: 'object', properties: {{ message: {{ type: messageType }} }}, required: ['message'], additionalProperties: false }} }}] }});
    return;
  }}
  if (req.method === 'tools/call') {{
    fs.appendFileSync(callsPath, JSON.stringify(req.params) + '\n');
    send(req.id, {{ content: [{{ type: 'text', text: 'echo:' + req.params.arguments.message }}] }});
    return;
  }}
  send(req.id, {{}});
}});
"#,
                calls_path = calls_path_js,
                schema_path = schema_path_js,
            ),
        )
        .unwrap();

        state
            .mcp_manager
            .start(
                "mcp-test",
                "test mcp",
                "node",
                &server_path.to_string_lossy(),
                "",
            )
            .unwrap();
        assert_eq!(state.mcp_manager.list_tools("mcp-test").unwrap().len(), 1);
        LiveMcpEchoServer {
            calls_path,
            schema_path,
        }
    }

    fn configure_test_mcp_workspace(state: &AppState, enabled: bool) {
        let conn = state.db.lock().unwrap();
        conn.execute_batch(
            r#"
            ALTER TABLE sessions ADD COLUMN project_id TEXT;
            CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE mcp_servers (id TEXT PRIMARY KEY, enabled INTEGER NOT NULL);
            CREATE TABLE mcp_workspace_enablements (
                workspace_id TEXT NOT NULL,
                server_id TEXT NOT NULL,
                enabled_at INTEGER NOT NULL,
                PRIMARY KEY (workspace_id, server_id)
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('workspace-1', 'Test workspace')",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE sessions SET project_id = 'workspace-1' WHERE id = 's1'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mcp_servers (id, enabled) VALUES ('mcp-test', 1)",
            [],
        )
        .unwrap();
        if enabled {
            conn.execute(
                "INSERT INTO mcp_workspace_enablements (workspace_id, server_id, enabled_at)
                 VALUES ('workspace-1', 'mcp-test', 1)",
                [],
            )
            .unwrap();
        }
    }

    fn cached_mcp_echo_schema_version(state: &AppState) -> String {
        state
            .mcp_manager
            .get_cached_tools("mcp-test")
            .into_iter()
            .find(|tool| tool.name == "echo")
            .expect("echo tool should have been discovered")
            .snapshot_for("mcp-test")
            .revision
    }

    fn configure_pending_mcp_echo_confirmation(
        state: &AppState,
        tool_name: &str,
        mcp_schema_version: Option<&str>,
    ) {
        let output =
            format!("Confirmation required before executing side-effect tool '{tool_name}'.");
        let plan = serde_json::json!([tool_name]).to_string();
        let conn = state.db.lock().unwrap();
        conn.execute(
            "UPDATE agent_steps
             SET tool_name = ?1,
                 tool_input = ?2,
                 tool_output = ?3,
                 mcp_schema_version = ?4
             WHERE id = 'call-1'",
            params![
                tool_name,
                r#"{"message":"hello"}"#,
                output,
                mcp_schema_version
            ],
        )
        .unwrap();
        conn.execute(
            "UPDATE task_runs
             SET goal = 'Call MCP echo', plan = ?1
             WHERE message_id = 'msg-1'",
            params![plan],
        )
        .unwrap();
    }

    #[test]
    fn rejecting_confirmation_updates_step_without_executing_tool() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "rejected".to_string(),
                preview_id: None,
            },
        )
        .unwrap();

        assert!(!result.success);
        assert_eq!(result.confirmation_status.as_deref(), Some("rejected"));
        assert!(!dir.path().join("approved.txt").exists());

        let conn = state.db.lock().unwrap();
        let stored: (String, String, i32) = conn
            .query_row(
                "SELECT status, confirmation_state, resumable FROM task_runs WHERE message_id = 'msg-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(stored.0, "needs_attention");
        assert_eq!(stored.1, "rejected");
        assert_eq!(stored.2, 1);
        let facts_json: String = conn
            .query_row(
                "SELECT facts_json FROM task_run_facts WHERE message_id = 'msg-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let facts: TaskFacts = serde_json::from_str(&facts_json).unwrap();
        assert!(facts.pending_confirmation.is_none());
        assert_eq!(facts.failed_steps, vec!["call-1"]);
        assert_eq!(facts.plan[0].status, PlanStepStatus::Failed);
        assert_eq!(
            facts.terminal_reason,
            Some(TaskTerminalReason::NeedsAttention)
        );
    }

    #[test]
    fn cancelling_pending_confirmation_makes_it_non_executable() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());

        cancel_agent_task_impl(&state, "s1", "msg-1").unwrap();

        {
            let conn = state.db.lock().unwrap();
            let stored: (String, String, i32) = conn
                .query_row(
                    "SELECT status, confirmation_state, resumable
                     FROM task_runs WHERE message_id = 'msg-1'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(stored, ("stopped".to_string(), "none".to_string(), 0));

            let step_output: String = conn
                .query_row(
                    "SELECT tool_output FROM agent_steps WHERE id = 'call-1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(step_output.contains("Confirmation cancelled"));
            assert!(!step_output.contains("Confirmation required"));

            let tool_output: String = conn
                .query_row(
                    "SELECT content FROM messages WHERE id = 'tool-msg-1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(tool_output, step_output);

            let facts_json: String = conn
                .query_row(
                    "SELECT facts_json FROM task_run_facts WHERE message_id = 'msg-1'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let facts: TaskFacts = serde_json::from_str(&facts_json).unwrap();
            assert!(facts.pending_confirmation.is_none());
            assert_eq!(facts.plan[0].status, PlanStepStatus::Skipped);
            assert_eq!(facts.terminal_reason, Some(TaskTerminalReason::Stopped));
        }

        let approval = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "approved".to_string(),
                preview_id: None,
            },
        );
        assert!(
            approval.is_err(),
            "cancelled confirmation must not be approved"
        );
        assert!(!dir.path().join("approved.txt").exists());
    }

    #[test]
    fn stale_confirmation_resolver_cannot_execute_or_revive_a_cancelled_task() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());
        // Simulate a browser confirmation request that read its durable inputs
        // just before another user action stopped the task.
        let stale_pending = {
            let conn = state.db.lock().unwrap();
            load_pending_confirmation(&conn, "s1", "msg-1", "call-1").unwrap()
        };

        cancel_agent_task_impl(&state, "s1", "msg-1").unwrap();

        // A late resolver must fail its compare-and-swap rather than changing
        // the cancelled step back to an approved result.
        let stale_settlement = {
            let mut conn = state.db.lock().unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            settle_pending_confirmation_in_tx(
                &tx,
                "s1",
                "msg-1",
                "call-1",
                &stale_pending,
                true,
                "Confirmation approved: stale resolver result",
            )
        };
        assert!(stale_settlement.is_err());

        // The public resolver now rechecks the same exact pending state before
        // it can reach the write_file handler.
        let approval = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "approved".to_string(),
                preview_id: None,
            },
        );
        assert!(approval.is_err());
        assert!(!dir.path().join("approved.txt").exists());

        let conn = state.db.lock().unwrap();
        let state_after: (String, String, i32) = conn
            .query_row(
                "SELECT status, confirmation_state, resumable
                 FROM task_runs WHERE message_id = 'msg-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state_after, ("stopped".into(), "none".into(), 0));
        let output: String = conn
            .query_row(
                "SELECT tool_output FROM agent_steps WHERE id = 'call-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(output.contains("Confirmation cancelled"));
    }

    #[test]
    fn rejecting_a_pending_tool_still_works_after_the_capability_disappears() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());
        state
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE agent_steps SET tool_name='no-longer-available' WHERE id='call-1'",
                [],
            )
            .unwrap();

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "rejected".to_string(),
                preview_id: None,
            },
        )
        .unwrap();

        assert!(!result.success);
        assert_eq!(result.confirmation_status.as_deref(), Some("rejected"));
    }

    #[test]
    fn delegated_change_materialization_uses_the_standard_confirmation_flow() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());
        let registry =
            crate::commands::foreground_tool_surface::ForegroundToolSurface::new(&state, None)
                .build_for_session("s1".to_string());
        assert!(
            registry
                .get("materialize_delegated_change")
                .expect("materialization tool should be registered")
                .requires_confirmation
        );
    }

    #[test]
    fn personal_workspace_can_approve_a_reminder_without_gaining_file_tools() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let when = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        conn.execute(
            "INSERT INTO messages (id,session_id,role,content,created_at)
             VALUES ('personal-reminder-message','personal-main','assistant','Needs approval',11)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_steps
             (id,call_id,session_id,tool_name,tool_input,tool_output,success,seq,created_at)
             VALUES ('personal-reminder-call','personal-reminder-call','personal-main','schedule_reminder',?1,
                     'Confirmation required before executing side-effect tool ''schedule_reminder''.',0,1,11)",
            params![serde_json::json!({
                "title": "喝水",
                "body": "记得喝水",
                "when": when
            })
            .to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_runs
             (id,session_id,message_id,goal,status,plan,confirmation_state,resumable,
              step_count,completed_step_count,continuation_context,created_at,updated_at)
             VALUES ('personal-reminder-message','personal-main','personal-reminder-message',
                     'Create reminder','awaiting_confirmation','[\"schedule_reminder\"]','pending',
                     1,1,0,'',10,11)",
            [],
        )
        .unwrap();
        let state = app_state_from_connection(conn);
        let registry =
            crate::commands::foreground_tool_surface::ForegroundToolSurface::new(&state, None)
                .build_for_session("personal-main".to_string());
        assert!(!registry.contains("read_file"));
        assert!(!registry.contains("organize_workspace_item"));
        assert!(!registry.contains("reveal_workspace_item"));
        assert!(registry.contains("inspect_desktop_capabilities"));
        assert!(registry.contains("schedule_reminder"));

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "personal-main".to_string(),
                message_id: "personal-reminder-message".to_string(),
                call_id: "personal-reminder-call".to_string(),
                decision: "approved".to_string(),
                preview_id: None,
            },
        )
        .unwrap();

        assert!(result.success, "{}", result.output);
        let reminders: i64 = state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM automations WHERE executor_kind='notification' AND workspace_id='personal'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reminders, 1);
    }

    #[test]
    fn approving_confirmation_executes_tool_and_updates_step() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "approved".to_string(),
                preview_id: None,
            },
        )
        .unwrap();

        assert!(result.success);
        assert_eq!(result.confirmation_status.as_deref(), Some("approved"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("approved.txt")).unwrap(),
            "hello"
        );

        let conn = state.db.lock().unwrap();
        let stored: (String, String, i32, i32) = conn
            .query_row(
                "SELECT status, confirmation_state, resumable, completed_step_count
                 FROM task_runs WHERE message_id = 'msg-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(stored.0, "continue_suggested");
        assert_eq!(stored.1, "approved");
        assert_eq!(stored.2, 1);
        assert_eq!(stored.3, 1);
    }

    #[test]
    fn approving_create_directory_confirmation_executes_the_pending_step() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());
        {
            let conn = state.db.lock().unwrap();
            conn.execute(
                "UPDATE agent_steps
                 SET tool_name = 'create_directory',
                     tool_input = '{\"path\":\"created-by-confirmation\"}',
                     tool_output = 'Confirmation required before executing side-effect tool ''create_directory''.'
                 WHERE id = 'call-1'",
                [],
            )
            .unwrap();
        }

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "approved".to_string(),
                preview_id: None,
            },
        )
        .unwrap();

        assert!(result.success);
        assert_eq!(result.tool_name, "create_directory");
        assert_eq!(result.confirmation_status.as_deref(), Some("approved"));
        assert!(dir.path().join("created-by-confirmation").is_dir());
    }

    #[test]
    fn approving_empty_write_file_arguments_does_not_report_approval() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());
        state
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE agent_steps SET tool_input = '{}' WHERE id = 'call-1'",
                [],
            )
            .unwrap();

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "approved".to_string(),
                preview_id: None,
            },
        )
        .unwrap();

        assert!(!result.success);
        assert_ne!(result.confirmation_status.as_deref(), Some("approved"));
        assert!(result.output.contains("saved tool arguments are invalid"));
        assert!(!dir.path().join("approved.txt").exists());
    }

    #[test]
    fn approving_create_directory_confirmation_updates_task_run() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());
        {
            let conn = state.db.lock().unwrap();
            conn.execute(
                "UPDATE agent_steps
                 SET tool_name = 'create_directory',
                     tool_input = '{\"path\":\"approved-directory\"}',
                     tool_output = 'Confirmation required before executing side-effect tool ''create_directory''.'
                 WHERE id = 'call-1'",
                [],
            )
            .unwrap();
            conn.execute(
                "UPDATE task_runs
                 SET goal = 'Create a directory',
                     plan = '[\"create_directory\"]'
                 WHERE message_id = 'msg-1'",
                [],
            )
            .unwrap();
        }

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "approved".to_string(),
                preview_id: None,
            },
        )
        .unwrap();

        assert!(result.success);
        assert_eq!(result.tool_name, "create_directory");
        assert_eq!(result.confirmation_status.as_deref(), Some("approved"));
        assert!(result.output.contains("Confirmation approved"));
        assert!(dir.path().join("approved-directory").is_dir());

        let conn = state.db.lock().unwrap();
        let stored: (String, String, i32, i32) = conn
            .query_row(
                "SELECT status, confirmation_state, resumable, completed_step_count
                 FROM task_runs WHERE message_id = 'msg-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(stored.0, "continue_suggested");
        assert_eq!(stored.1, "approved");
        assert_eq!(stored.2, 1);
        assert_eq!(stored.3, 1);
    }

    #[test]
    fn approving_live_mcp_confirmation_executes_real_mcp_tool_and_updates_task_run() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());
        let server = start_live_mcp_echo_server(&state, &dir);
        configure_test_mcp_workspace(&state, true);
        let tool_name = crate::agent::handlers::mcp_registry_tool_name("mcp-test", "echo");
        let schema_version = cached_mcp_echo_schema_version(&state);
        configure_pending_mcp_echo_confirmation(&state, &tool_name, Some(&schema_version));

        assert!(!server.calls_path.exists());

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "approved".to_string(),
                preview_id: None,
            },
        )
        .unwrap();

        assert!(result.success);
        assert_eq!(result.tool_name, tool_name);
        assert_eq!(result.confirmation_status.as_deref(), Some("approved"));
        assert!(result.output.contains("echo:hello"));

        let calls = fs::read_to_string(&server.calls_path).unwrap();
        let call: serde_json::Value = serde_json::from_str(calls.trim()).unwrap();
        assert_eq!(call["name"], "echo");
        assert_eq!(call["arguments"], serde_json::json!({ "message": "hello" }));

        let conn = state.db.lock().unwrap();
        let stored: (String, String, i32, i32) = conn
            .query_row(
                "SELECT status, confirmation_state, resumable, completed_step_count
                 FROM task_runs WHERE message_id = 'msg-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(stored.0, "continue_suggested");
        assert_eq!(stored.1, "approved");
        assert_eq!(stored.2, 1);
        assert_eq!(stored.3, 1);

        state.mcp_manager.stop("mcp-test").unwrap();
    }

    #[test]
    fn mcp_pre_call_authorization_refusal_never_calls_the_server() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());
        let server = start_live_mcp_echo_server(&state, &dir);
        let expected = state
            .mcp_manager
            .get_cached_tools("mcp-test")
            .into_iter()
            .find(|tool| tool.name == "echo")
            .expect("echo tool should have been discovered")
            .snapshot_for("mcp-test");
        let authorization_checked = Arc::new(AtomicBool::new(false));

        let error = state
            .mcp_manager
            .call_tool_if_current_snapshot(
                "mcp-test",
                "echo",
                &expected,
                &serde_json::json!({ "message": "hello" }),
                {
                    let authorization_checked = authorization_checked.clone();
                    move || {
                        authorization_checked.store(true, Ordering::SeqCst);
                        Err("execution permission was revoked".to_string())
                    }
                },
            )
            .unwrap_err();

        assert!(authorization_checked.load(Ordering::SeqCst));
        assert!(error
            .to_string()
            .contains("execution permission was revoked"));
        assert!(!server.calls_path.exists());
        state.mcp_manager.stop("mcp-test").unwrap();
    }

    #[test]
    fn legacy_mcp_confirmation_without_schema_snapshot_never_calls_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());
        let server = start_live_mcp_echo_server(&state, &dir);
        configure_test_mcp_workspace(&state, true);
        let tool_name = crate::agent::handlers::mcp_registry_tool_name("mcp-test", "echo");
        configure_pending_mcp_echo_confirmation(&state, &tool_name, None);

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "approved".to_string(),
                preview_id: None,
            },
        )
        .unwrap();

        assert!(!result.success);
        assert_ne!(result.confirmation_status.as_deref(), Some("approved"));
        assert!(result.output.contains("历史确认没有可校验的定义版本"));
        assert!(!server.calls_path.exists());
        state.mcp_manager.stop("mcp-test").unwrap();
    }

    #[test]
    fn changed_mcp_schema_after_confirmation_never_calls_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());
        let server = start_live_mcp_echo_server(&state, &dir);
        configure_test_mcp_workspace(&state, true);
        let tool_name = crate::agent::handlers::mcp_registry_tool_name("mcp-test", "echo");
        let schema_version = cached_mcp_echo_schema_version(&state);
        configure_pending_mcp_echo_confirmation(&state, &tool_name, Some(&schema_version));
        fs::write(&server.schema_path, "number").unwrap();

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "approved".to_string(),
                preview_id: None,
            },
        )
        .unwrap();

        assert!(!result.success);
        assert_ne!(result.confirmation_status.as_deref(), Some("approved"));
        assert!(
            result.output.contains("定义已在确认后发生变化"),
            "{}",
            result.output
        );
        assert!(!server.calls_path.exists());
        state.mcp_manager.stop("mcp-test").unwrap();
    }

    #[test]
    fn removed_mcp_tool_after_confirmation_never_calls_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());
        let server = start_live_mcp_echo_server(&state, &dir);
        configure_test_mcp_workspace(&state, true);
        let tool_name = crate::agent::handlers::mcp_registry_tool_name("mcp-test", "echo");
        let schema_version = cached_mcp_echo_schema_version(&state);
        configure_pending_mcp_echo_confirmation(&state, &tool_name, Some(&schema_version));
        fs::write(&server.schema_path, "removed").unwrap();

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "approved".to_string(),
                preview_id: None,
            },
        )
        .unwrap();

        assert!(!result.success);
        assert_ne!(result.confirmation_status.as_deref(), Some("approved"));
        assert!(result.output.contains("未执行"));
        assert!(!server.calls_path.exists());
        state.mcp_manager.stop("mcp-test").unwrap();
    }

    #[test]
    fn rejecting_live_mcp_confirmation_does_not_call_real_mcp_tool() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());
        let server = start_live_mcp_echo_server(&state, &dir);
        configure_test_mcp_workspace(&state, true);
        let tool_name = crate::agent::handlers::mcp_registry_tool_name("mcp-test", "echo");
        configure_pending_mcp_echo_confirmation(&state, &tool_name, None);

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "rejected".to_string(),
                preview_id: None,
            },
        )
        .unwrap();

        assert!(!result.success);
        assert_eq!(result.tool_name, tool_name);
        assert_eq!(result.confirmation_status.as_deref(), Some("rejected"));
        assert!(!server.calls_path.exists());

        let conn = state.db.lock().unwrap();
        let stored: (String, String, i32, i32) = conn
            .query_row(
                "SELECT status, confirmation_state, resumable, completed_step_count
                 FROM task_runs WHERE message_id = 'msg-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(stored.0, "needs_attention");
        assert_eq!(stored.1, "rejected");
        assert_eq!(stored.2, 1);
        assert_eq!(stored.3, 0);

        drop(conn);
        state.mcp_manager.stop("mcp-test").unwrap();
    }

    #[test]
    fn approving_mcp_confirmation_outside_the_enabled_workspace_never_calls_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup_confirmation_state(dir.path());
        let server = start_live_mcp_echo_server(&state, &dir);
        configure_test_mcp_workspace(&state, false);
        let tool_name = crate::agent::handlers::mcp_registry_tool_name("mcp-test", "echo");
        configure_pending_mcp_echo_confirmation(&state, &tool_name, None);

        let result = resolve_agent_confirmation_impl(
            &state,
            ResolveAgentConfirmationRequest {
                session_id: "s1".to_string(),
                message_id: "msg-1".to_string(),
                call_id: "call-1".to_string(),
                decision: "approved".to_string(),
                preview_id: None,
            },
        )
        .unwrap();

        assert!(!result.success);
        assert!(!server.calls_path.exists());
        state.mcp_manager.stop("mcp-test").unwrap();
    }
}
