//! Foreground-only conversation context preparation.
//!
//! This module owns the data that is safe and useful to hand to the next
//! foreground model turn: the personalized system prompt, the selected
//! conversation history, live-context compaction, and compact task facts.
//! It deliberately does **not** own tool execution, confirmation, lifecycle
//! control, persistence, auditing, or UI event emission.

use crate::agent::compaction::{CompactionHook, CompactionOutcome};
use crate::agent::config::AgentConfig;
use crate::agent::prompt::{PromptBuilder, TurnClock};
use crate::agent::runner::{AgentMessage, AgentStep};
use crate::agent::task_facts::{PendingConfirmationFact, TaskFacts, TaskTerminalReason};
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
        Self::initial_context_with_turn_clock(
            config,
            tool_schemas,
            prompt_snippets,
            user_message,
            history,
            None,
        )
    }

    pub(crate) fn initial_context_with_turn_clock(
        config: &AgentConfig,
        tool_schemas: &[ToolSchema],
        prompt_snippets: &[(String, String)],
        user_message: &str,
        history: ForegroundHistory<'_>,
        turn_clock: Option<TurnClock>,
    ) -> Vec<LlmMessage> {
        let system_prompt = PromptBuilder::new(config, tool_schemas, prompt_snippets)
            .build_with_inline_clock(turn_clock.is_none());
        let history_len = match history {
            ForegroundHistory::Flat(history) => history.len(),
            ForegroundHistory::Native(history) => history.len(),
        };
        let mut messages = Vec::with_capacity(history_len + 2 + usize::from(turn_clock.is_some()));
        messages.push(AgentMessage::System(system_prompt).to_llm_message());

        match history {
            ForegroundHistory::Flat(history) => {
                messages.extend(history.iter().map(|(role, content)| match role.as_str() {
                    "assistant" => AgentMessage::Assistant(content.clone()).to_llm_message(),
                    _ => AgentMessage::User(content.clone()).to_llm_message(),
                }));
            }
            ForegroundHistory::Native(history) => messages.extend(
                history
                    .iter()
                    .filter(|message| {
                        turn_clock.is_some() || TurnClock::from_message(message).is_none()
                    })
                    .cloned(),
            ),
        }

        if let Some(clock) = turn_clock {
            messages.push(clock.to_message());
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
    let mut failed_operations: Vec<&AgentStep> = Vec::new();
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
            if step.confirmation_required
                || step.confirmation_status.is_some()
                || !crate::agent::compaction::retained_original_history_receipt(
                    &step.tool_name,
                    &step.call_id,
                    &step.arguments,
                    &step.output,
                )
            {
                failed_operations.push(step);
            }
            continue;
        }

        // Only a later success of the same operation repairs a failure. An
        // earlier success, or one for a different target, is not repair evidence.
        // A host-rejected null argument may be corrected before any operation
        // was dispatched; keep that failed receipt, but do not block delivery
        // after the corresponding real success. Handler errors cannot opt in.
        failed_operations.retain(|failure| {
            !crate::agent::registry::operation_repair_matches(
                &failure.tool_name,
                &failure.call_id,
                &failure.arguments,
                &failure.output,
                &step.tool_name,
                &step.arguments,
            )
        });

        facts.record_successful_project_step(&step.tool_name, &step.arguments, &step.output);
    }
    facts.failed_steps = failed_operations
        .iter()
        .map(|failure| failure.call_id.clone())
        .collect();
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

    #[test]
    fn turn_clock_keeps_the_prior_input_and_appends_the_current_local_time() {
        let first_clock = TurnClock::parse("2026-11-11T23:59:00+08:00").unwrap();
        let next_clock = TurnClock::parse("2026-11-12T00:01:00+08:00").unwrap();
        let config = AgentConfig::default();
        let first = ForegroundConversationState::initial_context_with_turn_clock(
            &config,
            &[],
            &[],
            "first turn",
            ForegroundHistory::Native(&[]),
            Some(first_clock),
        );
        let continued = ForegroundConversationState::initial_context_with_turn_clock(
            &config,
            &[],
            &[],
            "next turn",
            ForegroundHistory::Native(&first[1..]),
            Some(next_clock),
        );

        assert!(!first[0].content.contains("## 当前时间"));
        assert!(first[0].content.contains("以最新标记解释当前请求"));
        assert_eq!(first[0].content, continued[0].content);
        assert_eq!(first[1].content, continued[1].content);
        assert!(!first[1].content.contains("解释当前请求"));
        assert_eq!(TurnClock::from_message(&continued[1]), Some(first_clock));
        assert_eq!(continued[2].content, "first turn");
        assert_eq!(TurnClock::from_message(&continued[3]), Some(next_clock));
        assert_eq!(continued[4].content, "next turn");

        let inline = ForegroundConversationState::initial_context(
            &config,
            &[],
            &[],
            "without turn clock",
            ForegroundHistory::Native(&first[1..]),
        );
        assert!(inline[0].content.contains("## 当前时间"));
        assert_eq!(inline[1].content, "first turn");
    }
}
