//! Single foreground-turn orchestration.
//!
//! The command layer owns request admission and context/model preparation. This
//! module owns the bounded runner slice and the terminal durability barrier,
//! keeping the order explicit: execute -> project response -> durable complete
//! -> publish terminal lifecycle event.

use crate::agent::attention::record_recoverable_runtime_failure;
use crate::agent::delegate_work::{register, ForegroundRunIdentity};
use crate::agent::delegated_model_binding::DelegatedModelBindingRequest;
use crate::agent::foreground_lifecycle_control::ForegroundLifecycleControl;
#[cfg(test)]
use crate::agent::AgentResponse;
use crate::agent::AgentRunner;
use crate::commands::foreground_context_assembler::PreparedForegroundContext;
use crate::commands::foreground_message_contracts::{Message, SendMessageRequest};
use crate::commands::foreground_model_factory::ResolvedForegroundModel;
#[cfg(test)]
use crate::commands::foreground_response_projector::project_agent_response;
use crate::commands::foreground_response_projector::{
    project_private_slice_outcome, PersistedTurnOutcome,
};
use crate::commands::foreground_run_store::{ForegroundRun, ForegroundRunStore};
use crate::commands::foreground_tool_surface::{
    agent_execution_permission, ForegroundToolSurface, ESTIMATED_TOOL_COUNT,
};
use crate::AppState;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::AppHandle;

/// One confirmed tool's pixels, owned only by the immediately composed turn.
/// This is deliberately not serializable and is never placed in AppState.
pub(crate) struct ConfirmedToolOutput {
    pub message_id: String,
    pub call_id: String,
    pub arguments: serde_json::Value,
    pub result: crate::agent::ToolResult,
    pub desktop_scope: Option<crate::commands::desktop::TrustedDesktopApp>,
    pub mcp_revision: Option<String>,
    pub execution_permission: String,
    pub context: crate::agent::ToolExecutionContext,
}

impl ConfirmedToolOutput {
    pub(super) fn attach_to_history(
        &mut self,
        state: &AppState,
        session_id: &str,
        supports_tool_images: bool,
        mcp_revisions: &crate::agent::handlers::McpToolRevisionCatalog,
        history: &mut [crate::llm::Message],
    ) -> Result<(), String> {
        if self.result.images.is_empty() || !self.result.success {
            return Err("图像已消费或没有成功的图像回执。".into());
        }
        if self.context.is_cancelled() || self.context.is_expired() {
            return Err("运行已取消或已超过截止时间；图像未发送。".into());
        }
        if !supports_tool_images {
            return Err("当前模型接口未接入工具图像输入；未发送图像，请勿声称已经查看。".into());
        }
        if agent_execution_permission(state)? != self.execution_permission {
            return Err("操作权限已改变；图像未发送。".into());
        }
        let stored: (String, String, Option<String>) = {
            let conn = state.db.lock().map_err(|error| error.to_string())?;
            conn.query_row(
                "SELECT steps.tool_name,steps.tool_input,steps.mcp_schema_version
                 FROM messages JOIN agent_steps AS steps
                   ON steps.session_id=messages.session_id AND steps.created_at=messages.created_at
                 WHERE messages.id=?1 AND messages.session_id=?2 AND messages.role='assistant'
                   AND COALESCE(steps.call_id,steps.id)=?3 AND steps.success=1",
                rusqlite::params![self.message_id, session_id, self.call_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|_| "已确认的图像来源已不在当前会话中。")?
        };
        if stored.0 != self.result.name
            || serde_json::from_str::<serde_json::Value>(&stored.1)
                .ok()
                .as_ref()
                != Some(&self.arguments)
            || stored.2 != self.mcp_revision
        {
            return Err("已确认的图像来源已改变；图像未发送。".into());
        }
        if self.result.name == "observe_trusted_app_window" {
            let expected = self
                .desktop_scope
                .as_ref()
                .ok_or("图像来源权限无法校验。")?;
            if !crate::agent::handlers::image_observation_scope_is_current(&state.db, expected) {
                return Err("应用的窗口图像权限已改变；图像未发送。".into());
            }
        } else if self.result.name.starts_with("mcp_") {
            if self
                .mcp_revision
                .as_ref()
                .is_none_or(|expected| mcp_revisions.get(&self.result.name) != Some(expected))
            {
                return Err("MCP 服务权限或工具定义已改变；图像未发送。".into());
            }
        } else {
            return Err("此工具的图像来源无法校验；图像未发送。".into());
        }
        let mut calls = Vec::new();
        let mut rows = Vec::new();
        for (index, message) in history.iter().enumerate() {
            for call in message.tool_calls.iter().flatten() {
                if call.id == self.call_id {
                    if call.name != self.result.name || call.arguments != self.arguments {
                        return Err("已确认的工具调用与会话历史不一致。".into());
                    }
                    calls.push(index);
                }
            }
            if message.role == "tool" && message.tool_call_id.as_deref() == Some(&self.call_id) {
                rows.push(index);
            }
        }
        let ([call], [row]) = (calls.as_slice(), rows.as_slice()) else {
            return Err("会话历史中缺少唯一的已确认工具调用；图像未发送。".into());
        };
        if call >= row
            || history
                .iter()
                .any(|message| !message.tool_images.is_empty())
        {
            return Err("会话图像绑定不明确；图像未发送。".into());
        }
        // Consume the only payload owner. AgentRunner releases its transient
        // copy after the first successful model round, including native replay.
        history[*row].tool_images = std::mem::take(&mut self.result.images);
        Ok(())
    }
}

/// Selects the capability surface for one foreground turn.  Normal user
/// turns retain the complete session-bound surface.  A reviewed delegated
/// delivery is instead summarized through a deliberately single-purpose
/// registry: it may acknowledge only the delivery that caused the follow-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForegroundTurnToolMode {
    Standard,
    ReviewedDeliverySummary { delivery_id: String },
}

/// Input captured by the command after durable run admission and context/model
/// preparation. Model input and durable task identity remain separate.
pub(crate) struct ForegroundTurnInput {
    pub request: SendMessageRequest,
    /// Stable user task, separate from model-only continuation instructions.
    pub goal: String,
    pub session_id: String,
    pub message_id: String,
    pub reply_id: String,
    pub prepared_context: PreparedForegroundContext,
    pub resolved_model: ResolvedForegroundModel,
    pub foreground_run: ForegroundRun,
    pub delegation_identity: Option<ForegroundRunIdentity>,
    /// These are built only for an interactive Standard turn. A constrained
    /// reviewed-delivery summary must not even prepare an issuance capability.
    pub sandbox_root: Option<PathBuf>,
    pub delegated_model_binding: Option<DelegatedModelBindingRequest>,
    pub tool_mode: ForegroundTurnToolMode,
    /// Optional command-owned work, dispatched only after a completed reply
    /// and its terminal lifecycle record are durable. Never for pending or
    /// failed execution, and never before persistence.
    pub after_completion: Option<Box<dyn FnOnce() + Send + 'static>>,
    pub confirmed_output: Option<ConfirmedToolOutput>,
    /// Every admitted turn owns one control-plane signal, including setup and
    /// confirmation continuation. A runner must never create an unlinked one.
    pub cancellation: crate::agent::ToolCancellation,
}

pub(crate) struct ForegroundTurnCoordinator<'a> {
    state: &'a AppState,
    app: Option<&'a AppHandle>,
    lifecycle_control: Arc<ForegroundLifecycleControl>,
    run_store: ForegroundRunStore,
}

impl<'a> ForegroundTurnCoordinator<'a> {
    pub(crate) fn new(
        state: &'a AppState,
        app: Option<&'a AppHandle>,
        lifecycle_control: Arc<ForegroundLifecycleControl>,
        run_store: ForegroundRunStore,
    ) -> Self {
        Self {
            state,
            app,
            lifecycle_control,
            run_store,
        }
    }

    pub(crate) async fn run(self, input: ForegroundTurnInput) -> Result<Message, String> {
        let ForegroundTurnInput {
            request,
            goal,
            session_id,
            message_id,
            reply_id,
            mut prepared_context,
            resolved_model,
            foreground_run,
            delegation_identity,
            sandbox_root,
            delegated_model_binding,
            tool_mode,
            after_completion,
            mut confirmed_output,
            cancellation,
        } = input;

        let tool_count = match &tool_mode {
            ForegroundTurnToolMode::Standard => ESTIMATED_TOOL_COUNT,
            // The automatic review summary receives exactly one control tool.
            ForegroundTurnToolMode::ReviewedDeliverySummary { .. } => 1,
        };
        let mut agent_config = prepared_context.agent_config;
        let execution_permission = agent_execution_permission(self.state)?;
        let live_compaction_threshold =
            crate::commands::message::live_context_compaction_threshold(&agent_config, tool_count)
                .unwrap_or(crate::agent::compaction::DEFAULT_CONTEXT_COMPRESSION_THRESHOLD);
        let live_keep_recent_tokens = (live_compaction_threshold / 2).max(2_048);

        let crate::commands::foreground_model_factory::ResolvedForegroundModel {
            provider,
            thinking_settings,
            identity: _,
        } = resolved_model;
        let emitter = self.lifecycle_control.clone();
        let tool_surface = match &tool_mode {
            ForegroundTurnToolMode::Standard => ForegroundToolSurface::new(self.state, self.app)
                .with_memory_reading(agent_config.preferences.use_long_term_memory)
                .build_for_session_with_mcp_revisions(session_id.clone())?,
            ForegroundTurnToolMode::ReviewedDeliverySummary { delivery_id } => {
                ForegroundToolSurface::new(self.state, self.app)
                    .build_for_reviewed_delivery_summary(session_id.clone(), delivery_id.clone())
            }
        };
        let mcp_tool_revisions = tool_surface.mcp_tool_revisions;
        if let Some(root) = tool_surface.file_root {
            agent_config.work_dir = Some(root);
        }
        let supports_tool_images = provider.supports_tool_images();
        let mut agent = AgentRunner::with_registry(provider, agent_config, tool_surface.registry)
            .with_emitter(emitter.clone())
            .with_tool_lifecycle_hook(emitter)
            .with_session_id(session_id.clone())
            .with_cancellation(cancellation)
            .with_control_states(self.state.steering.control_states())
            .with_thinking_settings(thinking_settings)
            .with_execution_permission(execution_permission)
            .with_context_compaction_budget(live_compaction_threshold, live_keep_recent_tokens);
        if matches!(&tool_mode, ForegroundTurnToolMode::Standard) {
            if let (Some(delegation_identity), Some(sandbox_root), Some(delegated_model_binding)) =
                (delegation_identity, sandbox_root, delegated_model_binding)
            {
                register(
                    agent.tool_registry_mut(),
                    self.state.db.clone(),
                    delegation_identity,
                    sandbox_root,
                    delegated_model_binding,
                    self.state
                        .delegation_pump
                        .as_ref()
                        .map(|host| host.service().clone()),
                );
            }
        }

        if let Some(output) = confirmed_output.as_mut() {
            output.attach_to_history(
                self.state,
                &session_id,
                supports_tool_images,
                &mcp_tool_revisions,
                &mut prepared_context.history,
            )?;
        }
        drop(confirmed_output);
        let ai_response = agent
            .run_slice_with_owned_history(
                &prepared_context.agent_input,
                std::mem::take(&mut prepared_context.history),
                prepared_context.attention_context.as_deref(),
            )
            .await;
        if let Some(usage) = agent.latest_usage() {
            let tracker = crate::llm::UsageTracker::new(self.state.db.clone());
            let _ = tracker.record_usage(
                Some(&request.session_id),
                agent.provider_name(),
                "active-model",
                usage.input_tokens,
                usage.output_tokens,
                None,
            );
        }

        let outcome = match project_private_slice_outcome(&goal, ai_response) {
            Ok(projected) => projected,
            Err(raw_error) => {
                {
                    let conn = self.state.db.lock().map_err(|error| error.to_string())?;
                    record_recoverable_runtime_failure(
                        &conn,
                        &session_id,
                        &format!("message:{message_id}"),
                        &reply_id,
                        &raw_error,
                    )?
                };
                // Return the same safe current-run projection that was just
                // committed, not raw diagnostics or a second generic reason.
                return Err(self.run_store.mark_needs_attention(&foreground_run)?);
            }
        };

        let reply = self.run_store.complete_with_mcp_revisions(
            &foreground_run,
            outcome,
            &mcp_tool_revisions,
        )?;
        self.lifecycle_control.commit_terminal().map_err(|error| {
            format!(
                "The reply was saved, but its terminal lifecycle record could not be persisted. Completion was not exposed; retry or resume safely. ({error})"
            )
        })?;
        if reply
            .task_run
            .as_ref()
            .map_or(true, |run| run.status == "completed")
        {
            if let Some(hook) = after_completion {
                hook();
            }
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::foreground_context_assembler::ForegroundContextSources;
    use crate::commands::foreground_model_factory::ForegroundModelIdentity;
    use crate::commands::foreground_run_store::{BeginForegroundTurn, ForegroundUserTurn};
    use crate::llm::{LlmFailure, LlmProvider, LlmResponse, StopReason, ToolSchema};
    use rusqlite::Connection;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    const PRIVATE_PROVIDER_DIAGNOSTIC: &str = "transport closed: Bearer private-coordinator-fault https://private.invalid/?request=private";

    struct SurfaceCaptureProvider {
        browser_name: String,
        browser_parameters: serde_json::Value,
        browser_enabled: bool,
        calls: Arc<AtomicUsize>,
        fail_after_write: Option<bool>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for SurfaceCaptureProvider {
        fn name(&self) -> &str {
            "coordinator-surface-capture"
        }

        async fn chat(
            &self,
            _: &[crate::llm::Message],
            _: Option<&[ToolSchema]>,
            _: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            panic!("the foreground runner must use the classified streaming boundary")
        }

        async fn chat_stream_classified(
            &self,
            messages: &[crate::llm::Message],
            tools: Option<&[ToolSchema]>,
            _: Option<&crate::llm::ThinkingSettings>,
            _: Option<tokio::sync::mpsc::UnboundedSender<String>>,
        ) -> Result<LlmResponse, LlmFailure> {
            let attempt = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_after_write.is_some()
                && (attempt > 0 || self.fail_after_write == Some(false))
            {
                return Err(LlmFailure {
                    kind: crate::llm::LlmFailureKind::Unclassified,
                    diagnostic: PRIVATE_PROVIDER_DIAGNOSTIC.into(),
                });
            }
            assert_eq!(attempt, 0);
            assert!(messages
                .iter()
                .any(|message| message.role == "user" && message.content.contains("request.json")));
            let tools = tools.expect("Standard must provide the session tool surface");
            for name in ["read_file", "write_file"] {
                let schema = tools.iter().find(|tool| tool.name == name).unwrap();
                assert_eq!(schema.parameters["properties"]["path"]["type"], "string");
            }
            assert!(!tools.iter().any(|tool| tool.name == "run_project_command"));
            let browser = tools
                .iter()
                .filter(|tool| tool.name == self.browser_name)
                .collect::<Vec<_>>();
            assert_eq!(browser.len(), usize::from(self.browser_enabled));
            if let Some(schema) = browser.first() {
                assert_eq!(schema.parameters, self.browser_parameters);
            }
            if self.fail_after_write == Some(true) {
                return Ok(LlmResponse::ToolCalls {
                    calls: vec![crate::llm::ToolCall {
                        id: "known-write".into(),
                        name: "write_file".into(),
                        arguments: json!({"path":"receipt.json","content":"one known effect"}),
                    }],
                    text: None,
                    stop_reason: StopReason::ToolUse,
                });
            }
            // A successful plain response still claims no business operation.
            Ok(LlmResponse::Text {
                text: "No operation was performed.".into(),
                stop_reason: StopReason::EndTurn,
            })
        }
    }

    #[tokio::test]
    async fn standard_daily_turn_passes_cached_mcp_and_file_schemas_to_provider() {
        for (browser_enabled, fail_after_write) in [
            (true, None),
            (true, Some(false)),
            (true, Some(true)),
            (false, None),
            (false, Some(false)),
            (false, Some(true)),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut state = crate::tests::test_app_state();
            let conn = Connection::open(directory.path().join("data.db")).unwrap();
            crate::db::migrate(&conn).unwrap();
            conn.execute_batch(
                "INSERT INTO mcp_servers(id,name,command,args,enabled,created_at)
                VALUES('browser','Browser','never-run-this-command','',1,1);",
            )
            .unwrap();
            if browser_enabled {
                conn.execute_batch(
                    "INSERT INTO mcp_workspace_enablements(workspace_id,server_id,enabled_at)
                    VALUES('personal','browser',1);",
                )
                .unwrap();
            }
            let browser_parameters = json!({
                "type":"object", "properties":{"url":{"type":"string"}},
                "required":["url"], "additionalProperties":false
            });
            let tool = serde_json::from_value(json!({
                "name":"browser_navigate", "description":"Navigate an owned page",
                "inputSchema":browser_parameters
            }))
            .unwrap();
            crate::commands::mcp_catalog::store(
                &conn,
                "browser",
                "never-run-this-command",
                "",
                &[tool],
            )
            .unwrap();
            state.db = Arc::new(Mutex::new(conn));
            crate::commands::message::set_agent_execution_permission_impl(&state, "full_access")
                .unwrap();
            let request: SendMessageRequest = serde_json::from_value(json!({
                "sessionId":"personal-main", "role":"user",
                "content":"Read request.json, inspect the browser and deliver receipt.json."
            }))
            .unwrap();
            let store = ForegroundRunStore::new(state.db.clone());
            let run = store
                .begin(BeginForegroundTurn {
                    session_id: request.session_id.clone(),
                    user_message_id: "user".into(),
                    reply_id: "reply".into(),
                    role: request.role.clone(),
                    content: request.content.clone(),
                    text_attachments: vec![],
                    user_turn: ForegroundUserTurn::Append,
                    now: 100,
                })
                .unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let input = ForegroundTurnInput {
                goal: request.content.clone(),
                session_id: request.session_id.clone(),
                message_id: "user".into(),
                reply_id: "reply".into(),
                prepared_context: PreparedForegroundContext {
                    agent_config: Default::default(),
                    history: vec![],
                    agent_input: request.content.clone(),
                    attention_context: None,
                    sources: ForegroundContextSources {
                        session_id: request.session_id.clone(),
                        history_before_created_at: 100,
                        used_fresh_summary: false,
                        attention_card_ids: vec![],
                        pending_delivery_ids: vec![],
                        task_understanding_record_ids: vec![],
                    },
                },
                resolved_model: ResolvedForegroundModel {
                    provider: Box::new(SurfaceCaptureProvider {
                        browser_name: crate::agent::handlers::mcp_registry_tool_name(
                            "browser",
                            "browser_navigate",
                        ),
                        browser_parameters,
                        browser_enabled,
                        calls: calls.clone(),
                        fail_after_write,
                    }),
                    thinking_settings: None,
                    identity: ForegroundModelIdentity {
                        provider: "synthetic".into(),
                        model: "capture".into(),
                        provider_profile_ref: "synthetic-profile".into(),
                        credential_handle_ref: "none".into(),
                    },
                },
                foreground_run: run,
                request,
                delegation_identity: None,
                sandbox_root: None,
                delegated_model_binding: None,
                tool_mode: ForegroundTurnToolMode::Standard,
                after_completion: None,
                confirmed_output: None,
                cancellation: crate::agent::ToolCancellation::new(),
            };
            let control = Arc::new(ForegroundLifecycleControl::new(
                Arc::new(crate::agent::event::NullEventEmitter),
                state.db.clone(),
                "reply".into(),
            ));
            let result = ForegroundTurnCoordinator::new(&state, None, control, store)
                .run(input)
                .await;
            let stored_reply = ForegroundRunStore::new(state.db.clone())
                .project_session("personal-main")
                .unwrap()
                .into_iter()
                .find(|reply| reply.id == "reply")
                .unwrap();
            let reply = if fail_after_write.is_none() {
                result.as_ref().unwrap()
            } else {
                &stored_reply
            };
            assert_eq!(
                calls.load(Ordering::SeqCst),
                if fail_after_write == Some(true) { 2 } else { 1 }
            );
            let visible = format!("{result:?} {reply:?}");
            for secret in [
                "private-coordinator-fault",
                "private.invalid",
                "request=private",
            ] {
                assert!(
                    !visible.contains(secret),
                    "private provider error leaked into a public result"
                );
            }
            let conn = state.db.lock().unwrap();
            let diagnostics = conn
                .prepare("SELECT payload FROM attention_diagnostic_evidence")
                .unwrap()
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(
                diagnostics,
                if fail_after_write.is_some() {
                    vec![PRIVATE_PROVIDER_DIAGNOSTIC.to_string()]
                } else {
                    vec![]
                },
                "every failed inference needs one protected diagnostic, including partial outcomes"
            );
            let public_rows = [
                "SELECT content FROM messages",
                "SELECT tool_output FROM agent_steps",
                "SELECT payload FROM agent_run_events",
            ];
            for query in public_rows {
                let values = conn
                    .prepare(query)
                    .unwrap()
                    .query_map([], |row| row.get::<_, String>(0))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                assert!(values
                    .iter()
                    .all(|value| !value.contains("private-coordinator-fault")));
            }
            drop(conn);
            assert!(state.mcp_manager.running_servers().is_empty());
            if fail_after_write.is_some() {
                assert_eq!(
                    directory.path().join("results/receipt.json").exists(),
                    fail_after_write == Some(true)
                );
                if fail_after_write == Some(true) {
                    assert_eq!(
                        std::fs::read_to_string(directory.path().join("results/receipt.json"))
                            .unwrap(),
                        "one known effect"
                    );
                    assert_eq!(
                        reply
                            .tool_results
                            .as_ref()
                            .unwrap()
                            .iter()
                            .filter(|r| r.tool_name == "write_file" && r.success)
                            .count(),
                        1
                    );
                }
                continue;
            }
            result.as_ref().unwrap();
            assert_eq!(reply.content, "No operation was performed.");
            assert!(reply.tool_results.is_none());
            assert!(
                reply.task_run.is_none(),
                "a plain reply is not a business task receipt"
            );
            assert!(state.mcp_manager.running_servers().is_empty());
            assert!(!directory.path().join("results/receipt.json").exists());
            assert_eq!(
                state
                    .db
                    .lock()
                    .unwrap()
                    .query_row("SELECT COUNT(*) FROM agent_steps", [], |row| row
                        .get::<_, i64>(0),)
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn plain_response_projection_has_no_tool_protocol_rows() {
        let projected =
            project_agent_response("hello", AgentResponse::Text("done".into())).unwrap();
        assert_eq!(projected.content, "done");
        assert!(projected.tool_calls.is_none());
        assert!(projected.tool_results.is_none());
    }
}
