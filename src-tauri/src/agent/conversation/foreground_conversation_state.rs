//! Foreground-only conversation context preparation.
//!
//! This module owns the data that is safe and useful to hand to the next
//! foreground model turn: the personalized system prompt, the selected
//! conversation history, live-context compaction, and compact task facts.
//! It deliberately does **not** own tool execution, confirmation, lifecycle
//! control, persistence, auditing, or UI event emission.

use crate::agent::compaction::{CompactionHook, CompactionOutcome};
use crate::agent::config::AgentConfig;
use crate::agent::runner::{AgentMessage, AgentStep};
use crate::agent::task_facts::{
    PendingConfirmationFact, TaskFacts, TaskTerminalReason, VerificationEvidence,
};
use crate::llm::{Message as LlmMessage, ToolSchema};
use chrono::Utc;

/// The history representation selected by the foreground host.
///
/// Flat history is used for a new in-memory conversation. Native history
/// preserves provider tool-call fields when a persisted conversation resumes.
/// The distinction is explicit so recovery never accidentally flattens the
/// tool protocol.
pub enum ForegroundHistory<'a> {
    Flat(&'a [(String, String)]),
    Native(&'a [LlmMessage]),
}

/// A context decision made while preparing a foreground model turn.
///
/// Callers decide whether and how to persist or display these decisions. This
/// module merely reports them, keeping audit and UI concerns outside its seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForegroundContextDecision {
    Compacted {
        before_tokens: usize,
        after_tokens: usize,
    },
    CompactionFailed {
        before_tokens: usize,
        reason: String,
    },
}

/// The context ready for the next foreground model call.
#[derive(Debug, Clone)]
pub struct PreparedForegroundContext {
    pub messages: Vec<LlmMessage>,
    pub task_facts: TaskFacts,
    pub decisions: Vec<ForegroundContextDecision>,
}

/// A deep module for foreground conversation state.
///
/// Its interface accepts all variable dependencies explicitly and returns a
/// prepared context plus structured decisions. It has no session, emitter, or
/// persistence globals, so a host can keep those policies independent.
pub struct ForegroundConversationState;

impl ForegroundConversationState {
    /// Select compact, observable task facts from the current foreground
    /// execution trace. Hosts may call this after they project context
    /// decisions into their own audit representation.
    pub fn select_task_facts(user_message: &str, steps: &[AgentStep]) -> TaskFacts {
        task_facts_from_steps(user_message, steps)
    }

    /// Prepare the first model request for a foreground turn.
    pub fn initial_context(
        config: &AgentConfig,
        tool_schemas: &[ToolSchema],
        prompt_snippets: &[(String, String)],
        user_message: &str,
        history: ForegroundHistory<'_>,
    ) -> Vec<LlmMessage> {
        let system_prompt =
            crate::agent::prompt::build_agent_system_prompt(config, tool_schemas, prompt_snippets);
        let history_len = match history {
            ForegroundHistory::Flat(history) => history.len(),
            ForegroundHistory::Native(history) => history.len(),
        };
        let mut messages = Vec::with_capacity(history_len + 2);
        messages.push(AgentMessage::System(system_prompt).to_llm_message());

        match history {
            ForegroundHistory::Flat(history) => {
                messages.extend(history.iter().map(|(role, content)| match role.as_str() {
                    "assistant" => AgentMessage::Assistant(content.clone()).to_llm_message(),
                    _ => AgentMessage::User(content.clone()).to_llm_message(),
                }));
            }
            ForegroundHistory::Native(history) => messages.extend(history.iter().cloned()),
        }

        messages.push(AgentMessage::User(user_message.to_string()).to_llm_message());
        messages
    }

    /// Transform the live model context immediately before a foreground model
    /// call, preserving the compaction outcome as a structured decision.
    pub async fn prepare_next_turn(
        user_message: &str,
        mut messages: Vec<LlmMessage>,
        steps: &[AgentStep],
        compaction_hook: &dyn CompactionHook,
    ) -> PreparedForegroundContext {
        let mut decisions = Vec::new();
        match compaction_hook.maybe_compact(&mut messages).await {
            CompactionOutcome::Compressed {
                before_tokens,
                after_tokens,
            } => decisions.push(ForegroundContextDecision::Compacted {
                before_tokens,
                after_tokens,
            }),
            CompactionOutcome::Failed {
                before_tokens,
                reason,
            } => decisions.push(ForegroundContextDecision::CompactionFailed {
                before_tokens,
                reason,
            }),
            CompactionOutcome::Skipped => {}
        }

        PreparedForegroundContext {
            messages,
            task_facts: Self::select_task_facts(user_message, steps),
            decisions,
        }
    }
}

/// Convert visible runner steps into compact facts. This keeps the runner's
/// completion rules tied to observable results while persisted continuation
/// supplies these facts to a resumed slice.
pub(crate) fn task_facts_from_steps(user_message: &str, steps: &[AgentStep]) -> TaskFacts {
    let mut facts = TaskFacts::new(user_message);
    let mut failed_step_tools = Vec::new();
    let mut successful_tools = std::collections::HashSet::new();
    for step in steps {
        if step.tool_name == "agent_loop_no_progress" {
            facts.no_progress_count = step
                .arguments
                .get("count")
                .and_then(|value| value.as_u64())
                .unwrap_or(1) as u32;
            facts.terminal_reason = Some(TaskTerminalReason::NeedsAttention);
            continue;
        }
        if step.confirmation_status.as_deref() == Some("pending") {
            facts.pending_confirmation = Some(PendingConfirmationFact {
                call_id: step.call_id.clone(),
                tool_name: step.tool_name.clone(),
                arguments: step.arguments.clone(),
                requested_at: Utc::now().timestamp(),
            });
            continue;
        }

        if !step.success {
            facts.failed_steps.push(step.call_id.clone());
            failed_step_tools.push((step.call_id.clone(), step.tool_name.clone()));
            continue;
        }

        successful_tools.insert(step.tool_name.as_str());

        match step.tool_name.as_str() {
            "write_file" | "create_directory" => {
                let path = step
                    .arguments
                    .get("path")
                    .and_then(|value| value.as_str())
                    .unwrap_or(&step.tool_name);
                facts.record_project_mutation(path.to_string());
            }
            "run_project_command" => facts.verification_evidence.push(VerificationEvidence {
                command: step
                    .arguments
                    .get("command")
                    .and_then(|value| value.as_str())
                    .unwrap_or("run_project_command")
                    .to_string(),
                exit_code: 0,
                summary: step.output.clone(),
                verified_at: Utc::now().timestamp(),
            }),
            _ => {}
        }
    }
    // A later successful invocation of the same tool is an observable repair
    // attempt. Keep compact call IDs for audit/UI, but do not make an already
    // recovered failure block the task from its next state.
    facts.failed_steps.retain(|call_id| {
        !failed_step_tools.iter().any(|(failed_call_id, tool_name)| {
            failed_call_id == call_id && successful_tools.contains(tool_name.as_str())
        })
    });
    facts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::CompactionOutcome;

    struct CompressingHook;

    #[async_trait::async_trait]
    impl CompactionHook for CompressingHook {
        async fn maybe_compact(&self, messages: &mut Vec<LlmMessage>) -> CompactionOutcome {
            messages[1].content = "checkpoint".to_string();
            CompactionOutcome::Compressed {
                before_tokens: 20,
                after_tokens: 8,
            }
        }
    }

    #[tokio::test]
    async fn next_turn_returns_compaction_as_a_structured_decision() {
        let messages = vec![
            AgentMessage::System("system".to_string()).to_llm_message(),
            AgentMessage::User("long history".to_string()).to_llm_message(),
        ];
        let prepared = ForegroundConversationState::prepare_next_turn(
            "check the project",
            messages,
            &[],
            &CompressingHook,
        )
        .await;

        assert_eq!(prepared.messages[1].content, "checkpoint");
        assert_eq!(
            prepared.decisions,
            vec![ForegroundContextDecision::Compacted {
                before_tokens: 20,
                after_tokens: 8,
            }]
        );
        assert_eq!(prepared.task_facts.goal, "check the project");
    }

    #[test]
    fn native_history_preserves_provider_tool_protocol_fields() {
        let native = vec![LlmMessage {
            tool_images: Vec::new(),
            protocol_state: None,
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![crate::llm::ToolCall {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "src/lib.rs"}),
            }]),
            tool_call_id: None,
        }];
        let messages = ForegroundConversationState::initial_context(
            &AgentConfig::default(),
            &[],
            &[],
            "continue",
            ForegroundHistory::Native(&native),
        );

        let preserved = messages[1].tool_calls.as_ref().expect("tool call retained");
        assert_eq!(preserved.len(), 1);
        assert_eq!(preserved[0].id, "call-1");
        assert_eq!(preserved[0].name, "read_file");
        assert_eq!(
            preserved[0].arguments,
            serde_json::json!({"path": "src/lib.rs"})
        );
    }
}
