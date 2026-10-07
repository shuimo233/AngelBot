//! Single foreground-turn orchestration.
//!
//! The command layer owns request admission and context/model preparation. This
//! module owns the bounded runner slice and the terminal durability barrier,
//! keeping the order explicit: execute -> project response -> durable complete
//! -> publish terminal lifecycle event.

use crate::agent::attention::{record_recoverable_runtime_failure, render_visible_projection};
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
/// preparation. The request is owned so completion can consume its goal safely.
pub(crate) struct ForegroundTurnInput {
    pub request: SendMessageRequest,
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
    /// Kept as a command-owned hook so policy/evolution scheduling does not
    /// become a hidden concern of this orchestration module.
    pub before_persist: Option<Box<dyn FnOnce() + Send + 'static>>,
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
            session_id,
            message_id,
            reply_id,
            prepared_context,
            resolved_model,
            foreground_run,
            delegation_identity,
            sandbox_root,
            delegated_model_binding,
            tool_mode,
            before_persist,
        } = input;

        let tool_count = match &tool_mode {
            ForegroundTurnToolMode::Standard => ESTIMATED_TOOL_COUNT,
            // The automatic review summary receives exactly one control tool.
            ForegroundTurnToolMode::ReviewedDeliverySummary { .. } => 1,
        };
        let agent_config = prepared_context.agent_config;
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
                .build_for_session_with_mcp_revisions(session_id.clone()),
            ForegroundTurnToolMode::ReviewedDeliverySummary { delivery_id } => {
                ForegroundToolSurface::new(self.state, self.app)
                    .build_for_reviewed_delivery_summary(session_id.clone(), delivery_id.clone())
            }
        };
        let mcp_tool_revisions = tool_surface.mcp_tool_revisions;
        let mut agent = AgentRunner::with_registry(provider, agent_config, tool_surface.registry)
            .with_emitter(emitter)
            .with_session_id(session_id.clone())
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
                    self.state.delegation_pump.clone(),
                );
            }
        }

        let ai_response = agent
            .run_slice_with_private_transcript(
                &prepared_context.agent_input,
                &prepared_context.history,
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

        let outcome = match project_private_slice_outcome(&request.content, ai_response) {
            Ok(projected) => projected,
            Err(raw_error) => {
                let card = {
                    let conn = self.state.db.lock().map_err(|error| error.to_string())?;
                    record_recoverable_runtime_failure(
                        &conn,
                        &session_id,
                        &format!("message:{message_id}"),
                        &reply_id,
                        &raw_error,
                    )?
                };
                self.run_store.mark_needs_attention(&foreground_run)?;
                return Err(render_visible_projection(&card).to_string());
            }
        };

        if let Some(hook) = before_persist {
            hook();
        }
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
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_response_projection_has_no_tool_protocol_rows() {
        let projected =
            project_agent_response("hello", AgentResponse::Text("done".into())).unwrap();
        assert_eq!(projected.content, "done");
        assert!(projected.tool_calls.is_none());
        assert!(projected.tool_results.is_none());
    }
}
