//! Agent runner — handles the tool-calling loop.
//!
//! `AgentRunner::run` advances one user message through bounded LLM and
//! tool iterations, emitting `AgentEvent`s for the UI along the way.
//! Final return value carries the visible text and the auditable
//! `AgentStep`s so the frontend can render tool execution cards.
//!
//! The runtime is event-driven: every LLM call, tool batch, and
//! terminal branch passes through an `AgentLifecycleHook`. Three
//! default `LoopGuard`s (wall-clock, no-progress, side-effect pause)
//! are the only termination contract — Pi-style soft-only termination
//! with no hard iteration cap. Cooperative `ToolCancellation` lets the
//! control plane
//! stop a long turn between iterations, and `PendingMessageQueue`
//! surfaces mid-turn user corrections.

use crate::agent::compaction::{CompactionHook, DefaultCompactionHook};
use crate::agent::config::AgentConfig;
use crate::agent::event::{AgentEvent, ContentBlockData};
use crate::agent::execution_kernel::{ExecutionKernel, KernelModelTurnError};
pub(crate) use crate::agent::foreground_conversation_state::task_facts_from_steps;
use crate::agent::foreground_conversation_state::{
    ForegroundContextDecision, ForegroundConversationState, ForegroundHistory,
};
use crate::agent::foreground_tool_batch_host::ForegroundToolBatchHost;
use crate::agent::guards::{NetProgressGuard, SideEffectAutoPauseGuard, WallClockGuard};
use crate::agent::lifecycle::{
    AfterToolCallContext, AgentLifecycleHook, DefaultApiKeyProvider, DefaultLifecycleHook,
    DefaultToolLifecycleHook, DynamicApiKeyProvider, LifecycleContext, LoopDecision, ResponseKind,
    TerminalBranchKind, ToolCallContext, ToolLifecycleHook, TransformResult, TurnContext,
};
use crate::agent::registry::{requires_immediate_confirmation, ToolRegistry};
use crate::agent::steering::AgentControlState;
use crate::agent::task_facts::{derive_task_phase, DerivedTaskPhase};
use crate::agent::tool::{
    ToolCall as AgentToolCall, ToolCancellation, ToolExecutionContext, ToolExecutionMode,
};
use crate::llm::{
    LlmProvider, LlmResponse, Message as LlmMessage, ThinkingSettings, ToolCall as LlmToolCall,
    ToolSchema,
};
use chrono::Utc;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use uuid::Uuid;

/// Pi-style: no hard iteration limit.
///
/// Termination is governed by the `WallClockGuard` (default 600s) and
/// `NetProgressGuard` (default 5 consecutive non-progress iterations).
/// Both are soft terminators that return a structured `LoopDecision`
/// instead of silently dropping the turn. The side-effect guard
/// (`SideEffectAutoPauseGuard`, default 8 side-effects) keeps the run
/// honest about irreversible actions before the wall-clock backstop
/// fires.
const ITERATION_TIMEOUT_SECS: u64 = 45;
const MAX_LLM_ATTEMPTS_PER_ITERATION: usize = 2;
const MAX_TOOL_RESULT_CONTEXT_CHARS: usize = 6_000;

// Default guard thresholds. The wall-clock guard is the absolute
// backstop; net-progress and side-effect guards are soft pauses that
// return `LoopDecision::PauseForUser` so the UI can show a clear
// prompt before the run continues.
pub(crate) const DEFAULT_WALL_CLOCK_BUDGET: Duration = Duration::from_secs(600);
pub(crate) const DEFAULT_NET_PROGRESS_THRESHOLD: usize = 5;
pub(crate) const DEFAULT_SIDE_EFFECT_THRESHOLD: usize = 8;

/// Default guard set installed by `AgentRunner::new()`.
/// WallClock is the only hard terminator; the other two are soft pauses.
fn default_guard_set() -> Vec<Arc<dyn crate::agent::guards::LoopGuard>> {
    vec![
        Arc::new(WallClockGuard::new(DEFAULT_WALL_CLOCK_BUDGET)),
        Arc::new(NetProgressGuard::new(DEFAULT_NET_PROGRESS_THRESHOLD)),
        Arc::new(SideEffectAutoPauseGuard::new(DEFAULT_SIDE_EFFECT_THRESHOLD)),
    ]
}

#[cfg(test)]
mod lifecycle_decision_tests {
    //! `LoopDecision::PauseForUser` plumbing.
    //!
    //! The hook returns `LoopDecision::PauseForUser(reason)` from
    //! `should_stop_after_turn`; the runner surfaces that as a soft
    //! pause (no AgentEnd), distinct from a hard Stop or natural end.

    use crate::agent::lifecycle::{
        AgentLifecycleHook, LifecycleContext, LoopDecision, ResponseKind, TurnContext,
    };

    struct PauseHook;
    impl AgentLifecycleHook for PauseHook {
        fn should_stop_after_turn(&self, _ctx: &TurnContext) -> LoopDecision {
            LoopDecision::PauseForUser {
                reason: "I want to ask the user".into(),
            }
        }
    }

    struct HardStopHook;
    impl AgentLifecycleHook for HardStopHook {
        fn should_stop_after_turn(&self, _ctx: &TurnContext) -> LoopDecision {
            LoopDecision::Stop
        }
    }

    #[test]
    fn loop_decision_default_is_continue() {
        assert_eq!(LoopDecision::default(), LoopDecision::Continue);
    }

    #[test]
    fn pause_for_user_carries_a_reason() {
        let decision = LoopDecision::PauseForUser {
            reason: "ask me".into(),
        };
        match decision {
            LoopDecision::PauseForUser { reason } => assert_eq!(reason, "ask me"),
            _ => panic!("expected PauseForUser"),
        }
    }

    #[test]
    fn lifecycle_hook_can_return_pause_for_user() {
        let hook = PauseHook;
        let ctx = TurnContext::new(0, 0, Vec::new(), Vec::new(), 0)
            .with_response_shape(ResponseKind::ToolCalls);
        match hook.should_stop_after_turn(&ctx) {
            LoopDecision::PauseForUser { reason } => {
                assert!(!reason.is_empty());
            }
            other => panic!("expected PauseForUser, got {:?}", other),
        }
    }

    #[test]
    fn lifecycle_hook_can_return_hard_stop() {
        let hook = HardStopHook;
        let ctx = TurnContext::new(0, 0, Vec::new(), Vec::new(), 0);
        assert_eq!(hook.should_stop_after_turn(&ctx), LoopDecision::Stop);
    }

    #[test]
    fn lifecycle_context_max_iterations_is_now_meaningful() {
        // The runner must pass a real iteration budget. A zero budget
        // was previously a bug — hooks that read `max_iterations` would
        // see "no iterations allowed" and refuse to act. The new contract
        // is `usize::MAX` for "no budget" and a positive value otherwise.
        let ctx = LifecycleContext::before_llm(2, usize::MAX, Vec::new(), Vec::new(), 0);
        assert_eq!(ctx.max_iterations, usize::MAX);
        assert_eq!(ctx.iteration, 2);
    }
}

/// Hook that returns a non-None `prepare_next_turn` snapshot so the
/// runner records it as an auditable step.
struct PrepareNextTurnHook;

impl AgentLifecycleHook for PrepareNextTurnHook {
    fn prepare_next_turn(
        &self,
        _ctx: &TurnContext,
    ) -> Option<crate::agent::lifecycle::NextTurnSnapshot> {
        Some(crate::agent::lifecycle::NextTurnSnapshot::default())
    }
}

#[cfg(test)]
mod prepare_next_turn_audit_tests {
    //! `prepare_next_turn` must leave an audit trace.
    //!
    //! Pi's `prepareNextTurn` hook transforms the message list before the
    //! next turn; AngelBot already consumes the snapshot, but the runner
    //! does not record the fact that a hook fired. Auditors must be able
    //! to answer "what did the runner see at turn N?" without diffing
    //! messages by hand.

    use super::*;
    use crate::agent::lifecycle::{AgentLifecycleHook, NextTurnSnapshot, TurnContext};
    use crate::agent::{Tool, ToolHandler, ToolResult};
    use crate::llm::{LlmProvider, LlmResponse, ToolSchema};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Two-iteration provider: first a tool call, then a natural Text.
    /// Gives the hook one opportunity to fire `prepare_next_turn`.
    #[derive(Clone)]
    struct ToolThenTextProvider {
        call_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for ToolThenTextProvider {
        fn name(&self) -> &str {
            "tool-then-text-mock"
        }
        async fn chat(
            &self,
            _messages: &[LlmMessage],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            if idx == 0 {
                Ok(LlmResponse::ToolCalls {
                    calls: vec![crate::llm::ToolCall {
                        id: "call-r".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "a.txt"}),
                    }],
                    text: Some("first".into()),
                    stop_reason: crate::llm::StopReason::ToolUse,
                })
            } else {
                Ok(LlmResponse::Text {
                    text: "done".into(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                })
            }
        }
    }

    struct ReadFileHandler;

    impl ToolHandler for ReadFileHandler {
        fn name(&self) -> &str {
            "read_file"
        }
        fn execute(&self, _args: &serde_json::Value, _work_dir: &std::path::Path) -> ToolResult {
            ToolResult::success("read_file", "ok")
        }
    }

    #[tokio::test]
    async fn prepare_next_turn_hook_is_recorded_as_auditable_step() {
        // When the lifecycle hook returns a snapshot, the runner must
        // record a step that auditors can find. Otherwise users have no
        // way to tell that the conversation was rewritten by the hook.
        let mut agent = AgentRunner::new(
            Box::new(ToolThenTextProvider {
                call_count: Arc::new(AtomicUsize::new(0)),
            }),
            AgentConfig::default(),
        );
        agent.tool_registry_mut().register(
            Tool::new("read_file", "Read", serde_json::json!({"type": "object"})),
            Arc::new(ReadFileHandler),
        );
        agent = agent.with_lifecycle_hook(Arc::new(super::PrepareNextTurnHook));

        let result = agent.run("hi", &[]).await;

        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert!(
                    steps.iter().any(|s| s.tool_name == "prepare_next_turn"),
                    "prepare_next_turn hook fire must be visible in audit, got: {:?}",
                    steps.iter().map(|s| &s.tool_name).collect::<Vec<_>>()
                );
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn next_turn_snapshot_default_is_empty_messages() {
        // Defensive: callers must be able to construct a snapshot without
        // touching the private `messages` field. The default empty list
        // is what the hook will return when it has nothing to rewrite.
        let snap = NextTurnSnapshot::default();
        assert_eq!(snap.messages.len(), 0);
    }

    #[test]
    fn prepare_next_turn_default_hook_returns_none() {
        // The default hook (no overrides) must be a no-op so existing
        // integrations keep their behaviour. This is the anchor for
        // backward compatibility with #080.
        let default = crate::agent::lifecycle::DefaultLifecycleHook;
        let ctx = TurnContext::new(0, 0, Vec::new(), Vec::new(), 0);
        assert!(default.prepare_next_turn(&ctx).is_none());
    }
}

#[cfg(test)]
mod uncertain_outcome_tests {
    use super::*;
    use crate::agent::{Tool, ToolHandler, ToolResult};
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct UnknownSideEffect;

    impl ToolHandler for UnknownSideEffect {
        fn name(&self) -> &str {
            "uncertain_side_effect"
        }

        fn execute(&self, _arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
            ToolResult::terminate_batch(
                self.name(),
                r#"{"code":"RESULT_UNKNOWN","message":"inspect first"}"#,
            )
        }
    }

    struct WouldRepeatProvider(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl LlmProvider for WouldRepeatProvider {
        fn name(&self) -> &str {
            "would-repeat"
        }

        async fn chat(
            &self,
            _messages: &[LlmMessage],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(LlmResponse::ToolCalls {
                calls: vec![crate::llm::ToolCall {
                    id: "uncertain-call".into(),
                    name: "uncertain_side_effect".into(),
                    arguments: serde_json::json!({}),
                }],
                text: None,
                stop_reason: crate::llm::StopReason::ToolUse,
            })
        }
    }

    #[tokio::test]
    async fn unknown_side_effect_pauses_before_another_model_turn() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut runner = AgentRunner::new(
            Box::new(WouldRepeatProvider(Arc::clone(&calls))),
            AgentConfig::default(),
        );
        runner.tool_registry_mut().register(
            Tool::new(
                "uncertain_side_effect",
                "Uncertain side effect",
                serde_json::json!({"type": "object"}),
            ),
            Arc::new(UnknownSideEffect),
        );

        let response = runner.run("perform action", &[]).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        match response {
            AgentResponse::TextWithToolCalls { text, steps } => {
                assert!(text.contains("请先检查实际状态"));
                assert!(steps
                    .iter()
                    .any(|step| step.output.contains("RESULT_UNKNOWN")));
            }
            other => panic!("expected user-review pause, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod control_plane_bridge_tests {
    //! `ExternalControlPlane` bridge.
    //!
    //! When the runner is constructed via `with_control_states`, an
    //! external mutation of the control state (e.g., the steering runtime
    //! setting `Interrupted` after a user click) must reach the
    //! cancellation token. Otherwise the in-flight tool handlers keep
    //! running while the UI shows "stopped" — a UX lie that lets a
    //! long-running handler keep writing files for several more seconds.

    use super::*;
    use crate::agent::steering::AgentControlState;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[tokio::test]
    async fn external_control_state_propagates_to_cancellation_token() {
        // Construct a runner with a shared control-state map and verify
        // that flipping the state to `Interrupted` cancels the runner's
        // signal.
        let control_states: Arc<Mutex<HashMap<String, AgentControlState>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let agent = AgentRunner::new(Box::new(super::MinimalTextProvider), AgentConfig::default())
            .with_control_states(control_states.clone())
            .with_session_id("test-session".into());

        // Pre-condition: cancellation must not yet be triggered.
        assert!(!agent.cancellation_token().is_cancelled());

        // External mutation: simulate the steering runtime setting the
        // session state to Interrupted.
        control_states
            .lock()
            .unwrap()
            .insert("test-session".into(), AgentControlState::Interrupted);

        // The bridge must observe the new state on the next check.
        // We invoke `wait_for_control` directly via a probe helper rather
        // than driving a full `run()` — the goal is the propagation,
        // not the LLM path.
        let _ = agent.wait_for_control_for_test("test-session").await;
        assert!(
            agent.cancellation_token().is_cancelled(),
            "Control state Interrupted must propagate to the cancellation token"
        );
    }

    #[tokio::test]
    async fn external_paused_state_does_not_cancel() {
        // Paused and WaitingForInput must NOT cancel — they are
        // observer-only states that the runner should respect by
        // spinning without short-circuiting the run.
        let control_states: Arc<Mutex<HashMap<String, AgentControlState>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let agent = AgentRunner::new(Box::new(super::MinimalTextProvider), AgentConfig::default())
            .with_control_states(control_states.clone())
            .with_session_id("test-session-2".into());

        control_states
            .lock()
            .unwrap()
            .insert("test-session-2".into(), AgentControlState::Paused);

        let _ = agent.wait_for_control_for_test("test-session-2").await;
        assert!(
            !agent.cancellation_token().is_cancelled(),
            "Paused state must not cancel — it is a soft wait"
        );
    }
}

#[cfg(test)]
mod signal_tests {
    //! `signal.aborted` propagation through tool execution.
    //! `ToolCancellation` already exists; the runner just never threaded
    //! it into the registry calls. These tests anchor the new behaviour
    //! so the wiring cannot drift back.

    use super::*;
    use crate::agent::tool::ToolCancellation;
    use crate::agent::{Tool, ToolHandler, ToolResult};
    use crate::llm::{LlmProvider, LlmResponse, Message as LlmMessage, ToolSchema};
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// A handler that records whether it observed a cancellation flag
    /// mid-execution. Used to verify the cancellation reaches inside the
    /// handler, not just the registry entry check.
    struct CancellationObserverHandler {
        cancellation: ToolCancellation,
        calls: Arc<AtomicUsize>,
        observed_cancelled: Arc<AtomicUsize>,
    }

    impl ToolHandler for CancellationObserverHandler {
        fn name(&self) -> &str {
            "slow_tool"
        }
        fn execute(
            &self,
            _arguments: &serde_json::Value,
            _work_dir: &std::path::Path,
        ) -> ToolResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.cancellation.is_cancelled() {
                self.observed_cancelled.fetch_add(1, Ordering::SeqCst);
                return ToolResult::error(self.name(), "cancelled before execute");
            }
            // Simulate work; in real handlers this is where cooperative
            // cancellation must be checked between awaits.
            std::thread::sleep(Duration::from_millis(10));
            if self.cancellation.is_cancelled() {
                self.observed_cancelled.fetch_add(1, Ordering::SeqCst);
            }
            ToolResult::success(self.name(), "done")
        }
    }

    /// Provider that triggers `cancelled` via the runner's control state
    /// before the first iteration, then continues to issue tool calls.
    /// After cancellation, the registry must reject the call before the
    /// handler ever runs.
    #[derive(Clone)]
    struct CancelAfterStartProvider {
        cancel_after_first_response: Arc<ToolCancellation>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for CancelAfterStartProvider {
        fn name(&self) -> &str {
            "cancel-after-start"
        }
        async fn chat(
            &self,
            _messages: &[LlmMessage],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            self.cancel_after_first_response.cancel();
            Ok(LlmResponse::ToolCalls {
                calls: vec![crate::llm::ToolCall {
                    id: "call-after-cancel".into(),
                    name: "slow_tool".into(),
                    arguments: serde_json::json!({}),
                }],
                text: Some("first tool call".into()),
                stop_reason: crate::llm::StopReason::ToolUse,
            })
        }
    }

    #[tokio::test]
    async fn registry_rejects_tool_call_after_cancellation() {
        // The registry entry point must short-circuit cancelled calls.
        let mut registry = ToolRegistry::new();
        let cancellation = ToolCancellation::new();
        cancellation.cancel();
        let tool = Tool::new(
            "slow_tool",
            "Slow tool",
            serde_json::json!({"type": "object"}),
        );
        registry.register(
            tool,
            Arc::new(CancellationObserverHandler {
                cancellation: cancellation.clone(),
                calls: Arc::new(AtomicUsize::new(0)),
                observed_cancelled: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let tc = crate::agent::tool::ToolCall::new("slow_tool".into(), serde_json::json!({}));
        let result = registry.execute_with_context(
            &tc,
            "c1",
            Path::new("."),
            &crate::agent::tool::ToolExecutionContext::new("c1", cancellation.clone()),
        );

        assert!(!result.result.success, "Cancelled call must fail");
        assert!(
            result.terminate_batch,
            "Cancelled call must terminate batch"
        );
        assert!(
            result.result.content.contains("取消"),
            "Error message must mention cancellation, got: {}",
            result.result.content
        );
    }

    #[tokio::test]
    async fn registry_rejects_tool_call_after_deadline_expired() {
        let mut registry = ToolRegistry::new();
        registry.register(
            Tool::new("slow_tool", "Slow", serde_json::json!({"type": "object"})),
            Arc::new(CancellationObserverHandler {
                cancellation: ToolCancellation::new(),
                calls: Arc::new(AtomicUsize::new(0)),
                observed_cancelled: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let tc = crate::agent::tool::ToolCall::new("slow_tool".into(), serde_json::json!({}));
        let ctx = crate::agent::tool::ToolExecutionContext::new("c1", ToolCancellation::new())
            .with_timeout(Duration::from_millis(0));
        std::thread::sleep(Duration::from_millis(2));

        let result = registry.execute_with_context(&tc, "c1", Path::new("."), &ctx);
        assert!(!result.result.success, "Expired call must fail");
        assert!(result.terminate_batch, "Expired call must terminate batch");
    }

    #[tokio::test]
    async fn cancellation_signal_is_unique_per_run() {
        // Two AgentRunner instances must each have their own cancellation,
        // so cancelling one cannot leak into the other.
        let r1 = AgentRunner::new(Box::new(super::MinimalTextProvider), AgentConfig::default());
        let r2 = AgentRunner::new(Box::new(super::MinimalTextProvider), AgentConfig::default());
        let s1 = r1.cancellation_token();
        let s2 = r2.cancellation_token();
        assert!(!s1.is_cancelled());
        assert!(!s2.is_cancelled());
        s1.cancel();
        assert!(s1.is_cancelled());
        assert!(!s2.is_cancelled(), "s2 must not see s1's cancellation");
    }

    /// Provider that records how many times `chat` was actually invoked.
    /// Used to verify the runner short-circuits when cancellation is set
    /// before the LLM is called (Pi's pre-flight pattern).
    #[derive(Clone)]
    struct ChatCountProvider {
        chat_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for ChatCountProvider {
        fn name(&self) -> &str {
            "chat-count-mock"
        }
        async fn chat(
            &self,
            _messages: &[LlmMessage],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            self.chat_calls.fetch_add(1, Ordering::SeqCst);
            Ok(LlmResponse::Text {
                text: "ok".into(),
                stop_reason: crate::llm::StopReason::EndTurn,
            })
        }
    }

    #[tokio::test]
    async fn runner_short_circuits_llm_when_cancelled_pre_flight() {
        // When the runner's cancellation is set before `run()` is invoked,
        // the runner must NOT call the LLM. Pi calls this a "pre-flight
        // cancellation" — the control plane can cancel before the model
        // is touched, saving tokens and reducing latency.
        let provider_calls = Arc::new(AtomicUsize::new(0));
        let provider = ChatCountProvider {
            chat_calls: provider_calls.clone(),
        };
        let agent = AgentRunner::new(Box::new(provider), AgentConfig::default());
        // Cancel BEFORE run() — simulate user pressing stop after
        // submit but before the runner reached the LLM.
        agent.cancellation_token().cancel();

        let result = agent.run("hello", &[]).await;
        assert_eq!(
            provider_calls.load(Ordering::SeqCst),
            0,
            "LLM must not be called when cancellation is pre-set"
        );
        // The result must report the cancellation as a terminal condition.
        match result {
            AgentResponse::Error(msg) => {
                assert!(
                    msg.contains("取消") || msg.contains("取消"),
                    "Error message must explain cancellation, got: {}",
                    msg
                );
            }
            AgentResponse::TextWithToolCalls { steps, .. } => {
                // If we choose a soft-pause response, no real LLM step
                // should have been recorded — but a synthetic terminal
                // step may appear to keep the response auditable.
                assert!(
                    steps.iter().all(|s| s.tool_name == "agent_loop"),
                    "Cancelled pre-flight must not produce real LLM steps, got: {:?}",
                    steps.iter().map(|s| &s.tool_name).collect::<Vec<_>>()
                );
            }
            AgentResponse::Text(text) => {
                panic!(
                    "Pre-flight cancellation must not produce plain Text, got: {}",
                    text
                );
            }
        }
    }

    #[tokio::test]
    async fn runner_can_be_cancelled_mid_run_via_token() {
        // Cancel the token after construction; the next main iteration's
        // pre-flight check must observe it and stop the loop.
        let provider_calls = Arc::new(AtomicUsize::new(0));
        let provider = ChatCountProvider {
            chat_calls: provider_calls.clone(),
        };
        let agent = AgentRunner::new(Box::new(provider), AgentConfig::default());
        let token = agent.cancellation_token();
        token.cancel();

        let _ = agent.run("anything", &[]).await;
        assert_eq!(
            provider_calls.load(Ordering::SeqCst),
            0,
            "Cancelled runner must never reach the LLM"
        );
    }

    /// Provider that captures the messages it sees on every LLM call.
    /// The first call returns a tool request; the second ends the turn.
    /// The test inspects the *first* call to verify the drained pending
    /// message reached the LLM.
    #[derive(Clone)]
    struct InspectMessagesProvider {
        call_count: Arc<AtomicUsize>,
        last_seen_tail: Arc<Mutex<Vec<String>>>,
        user_tail_first_call: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for InspectMessagesProvider {
        fn name(&self) -> &str {
            "inspect-messages"
        }
        async fn chat(
            &self,
            messages: &[LlmMessage],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            // Capture the user-tail each call so the test can inspect.
            let mut tail: Vec<String> = messages
                .iter()
                .rev()
                .take_while(|m| m.role == "user")
                .map(|m| m.content.clone())
                .collect();
            tail.reverse();
            *self.last_seen_tail.lock().unwrap() = tail.clone();
            if idx == 0 {
                *self.user_tail_first_call.lock().unwrap() = tail;
            }

            if idx == 0 {
                Ok(LlmResponse::ToolCalls {
                    calls: vec![crate::llm::ToolCall {
                        id: "call-r".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "a.txt"}),
                    }],
                    text: Some("first".into()),
                    stop_reason: crate::llm::StopReason::ToolUse,
                })
            } else {
                Ok(LlmResponse::Text {
                    text: "done".into(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                })
            }
        }
    }

    #[tokio::test]
    async fn runner_drains_pending_messages_and_audits_them() {
        // A user message pushed between iterations must show up in the
        // next LLM call's `messages` and produce an audit step recording
        // the drain. Without this, mid-turn corrections are silently
        // swallowed.
        use crate::agent::pending_queue::PendingMessageQueue;

        let queue = Arc::new(Mutex::new(PendingMessageQueue::new()));
        let last_seen_tail = Arc::new(Mutex::new(Vec::new()));
        let user_tail_first_call = Arc::new(Mutex::new(Vec::new()));

        let provider = InspectMessagesProvider {
            call_count: Arc::new(AtomicUsize::new(0)),
            last_seen_tail,
            user_tail_first_call: user_tail_first_call.clone(),
        };

        let mut agent = AgentRunner::new(Box::new(provider), AgentConfig::default());
        agent.tool_registry_mut().register(
            Tool::new("read_file", "Read", serde_json::json!({"type": "object"})),
            Arc::new(ReadFileProbeForPending {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let agent = agent.with_pending_messages(queue.clone());

        // Push a mid-turn correction before iteration 1 completes.
        queue.lock().unwrap().push(LlmMessage {
            role: "user".to_string(),
            content: "actually, use Python instead".to_string(),
            tool_calls: None,
            tool_call_id: None,
            tool_images: Vec::new(),
            protocol_state: None,
        });

        let _ = agent.run("write a Rust tool", &[]).await;

        // The FIRST LLM call must have seen the pending message at the
        // tail of the user messages. We check the first call because
        // subsequent calls add assistant/tool pairs that push the
        // pending user message out of the trailing position.
        let first_tail = user_tail_first_call.lock().unwrap();
        assert!(
            first_tail.iter().any(|m| m.contains("Python")),
            "Pending message must reach the LLM, first-call tail was: {:?}",
            first_tail
        );
    }

    struct ReadFileProbeForPending {
        calls: Arc<AtomicUsize>,
    }

    impl ToolHandler for ReadFileProbeForPending {
        fn name(&self) -> &str {
            "read_file"
        }
        fn execute(&self, _args: &serde_json::Value, _work_dir: &std::path::Path) -> ToolResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ToolResult::success("read_file", "ok")
        }
    }

    #[tokio::test]
    async fn empty_queue_does_not_emit_audit_step() {
        use crate::agent::pending_queue::PendingMessageQueue;

        let queue = Arc::new(Mutex::new(PendingMessageQueue::new()));
        let last_seen_tail = Arc::new(Mutex::new(Vec::new()));
        let user_tail_first_call = Arc::new(Mutex::new(Vec::new()));

        let provider = InspectMessagesProvider {
            call_count: Arc::new(AtomicUsize::new(0)),
            last_seen_tail,
            user_tail_first_call,
        };
        let mut agent = AgentRunner::new(Box::new(provider), AgentConfig::default());
        agent.tool_registry_mut().register(
            Tool::new("read_file", "Read", serde_json::json!({"type": "object"})),
            Arc::new(ReadFileProbeForPending {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let agent = agent.with_pending_messages(queue);

        let result = agent.run("hello", &[]).await;
        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert!(
                    !steps.iter().any(|s| s.tool_name == "pending_user_message"),
                    "Empty queue must not produce audit steps, got: {:?}",
                    steps.iter().map(|s| &s.tool_name).collect::<Vec<_>>()
                );
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }
    }
}

/// Minimal provider returning plain text. Used by signal tests that only
/// care about runner construction, not LLM behaviour.
#[derive(Clone)]
pub(super) struct MinimalTextProvider;

#[async_trait::async_trait]
impl LlmProvider for MinimalTextProvider {
    fn name(&self) -> &str {
        "minimal-text"
    }
    async fn chat(
        &self,
        _messages: &[LlmMessage],
        _tools: Option<&[ToolSchema]>,
        _thinking: Option<&crate::llm::ThinkingSettings>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse::Text {
            text: "ok".into(),
            stop_reason: crate::llm::StopReason::EndTurn,
        })
    }
}

#[cfg(test)]
mod batch_termination_tests {
    //! `should_terminate_batch` strict-mode coverage.
    //!
    //! The legacy `Any` mode keeps the original semantics that
    //! `terminate_batch_skips_remaining_tools` anchors. The new `All`
    //! mode is opt-in via `AgentRunner::with_batch_termination_mode`.

    use super::*;

    #[test]
    fn any_mode_stops_on_first_signal() {
        assert!(should_terminate_batch(1, 3, BatchTerminationMode::Any));
    }

    #[test]
    fn any_mode_continues_when_no_signals() {
        assert!(!should_terminate_batch(0, 3, BatchTerminationMode::Any));
    }

    #[test]
    fn all_mode_continues_when_only_some_signal() {
        // Two of three tools signal; partial agreement must not stop.
        assert!(!should_terminate_batch(2, 3, BatchTerminationMode::All));
    }

    #[test]
    fn all_mode_stops_only_when_every_tool_signals() {
        assert!(should_terminate_batch(3, 3, BatchTerminationMode::All));
    }

    #[test]
    fn all_mode_treats_empty_batch_as_no_signal() {
        // An empty batch never terminates — there is nothing to agree on.
        assert!(!should_terminate_batch(0, 0, BatchTerminationMode::All));
    }

    #[test]
    fn default_mode_is_any_for_backward_compatibility() {
        // The #082 `terminate_batch_skips_remaining_tools` test relies on
        // the Any default. Changing the default requires updating that
        // test first — see issue #096 Appendix E.1.
        assert_eq!(BatchTerminationMode::default(), BatchTerminationMode::Any);
    }
}

// Termination mode for tool batches.

/// How `terminate_batch` signals from individual tool calls aggregate into
/// a single runner decision. Mirrors Pi's `shouldStopAfterToolCall`
/// semantics — except Pi has no per-tool confirmation, so it never had to
/// think about this aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchTerminationMode {
    /// Any single tool signalling terminate stops the batch. Legacy default.
    /// Kept as a feature flag because #082's behaviour is anchored by
    /// `terminate_batch_skips_remaining_tools` and downstream code depends
    /// on the early exit.
    Any,
    /// Every tool in the batch must signal terminate. Stricter and more
    /// predictable; preferred for coding workflows where partial batches
    /// must complete for audit symmetry. Pi's `shouldStopAfterToolCall` is
    /// effectively all-or-none for the post-batch decision.
    All,
}

impl Default for BatchTerminationMode {
    fn default() -> Self {
        BatchTerminationMode::Any
    }
}

/// Aggregate per-tool `terminate_batch` signals into one boolean.
///
/// - `Any` mode: any signal stops the batch.
/// - `All` mode: every tool must signal; partial agreement is ignored.
///
/// `tools` is the total number of tools in the batch so `All` mode can
/// distinguish "no signals yet" from "every signal received".
pub fn should_terminate_batch(
    terminate_signals: usize,
    total_tools: usize,
    mode: BatchTerminationMode,
) -> bool {
    match mode {
        BatchTerminationMode::Any => terminate_signals > 0,
        BatchTerminationMode::All => total_tools > 0 && terminate_signals >= total_tools,
    }
}

/// Tracks per-turn state for tool error recovery (Issue #48)
struct TurnState {
    /// Number of failed tool calls in this turn
    failures: usize,
    /// Number of retry attempts for the current tool call
    retry_count: usize,
    /// Tool name currently being retried (None = no active retry)
    retry_tool: Option<String>,
    /// Whether this turn should abort due to too many failures
    should_abort: bool,
    /// Which main-loop iteration this block belongs to. The frontend
    /// uses this to group consecutive tool blocks (sharing the same
    /// iteration) into one "execution slice" so model reasoning and
    /// tool batches stay visually separate.
    iteration_id: u32,
}

impl TurnState {
    fn new() -> Self {
        Self {
            failures: 0,
            retry_count: 0,
            retry_tool: None,
            should_abort: false,
            iteration_id: 0,
        }
    }

    /// Record a tool failure; returns true if the turn should abort.
    fn record_failure(&mut self, max_failures: usize) -> bool {
        self.failures += 1;
        if self.failures >= max_failures {
            self.should_abort = true;
        }
        self.should_abort
    }

    fn start_retry(&mut self, tool_name: &str, max_retries: usize) {
        if self.retry_tool.as_deref() == Some(tool_name) && self.retry_count < max_retries {
            self.retry_count += 1;
        } else {
            self.retry_tool = Some(tool_name.to_string());
            self.retry_count = 1;
        }
    }

    fn can_retry(&self, tool_name: &str, max_retries: usize) -> bool {
        self.retry_tool.as_deref() == Some(tool_name) && self.retry_count < max_retries
    }

    fn reset_retry(&mut self) {
        self.retry_tool = None;
        self.retry_count = 0;
    }
}

/// Represents a message in the agent conversation
#[derive(Debug, Clone)]
pub enum AgentMessage {
    System(String),
    User(String),
    Assistant(String),
}

impl AgentMessage {
    /// Convert to unified LLM message format
    pub fn to_llm_message(&self) -> LlmMessage {
        match self {
            AgentMessage::System(content) => LlmMessage {
                role: "system".to_string(),
                content: content.clone(),
                tool_calls: None,
                tool_call_id: None,
                tool_images: Vec::new(),
                protocol_state: None,
            },
            AgentMessage::User(content) => LlmMessage {
                role: "user".to_string(),
                content: content.clone(),
                tool_calls: None,
                tool_call_id: None,
                tool_images: Vec::new(),
                protocol_state: None,
            },
            AgentMessage::Assistant(content) => LlmMessage {
                role: "assistant".to_string(),
                content: content.clone(),
                tool_calls: None,
                tool_call_id: None,
                tool_images: Vec::new(),
                protocol_state: None,
            },
        }
    }
}

/// A single tool execution step recorded during the agent loop.
/// Serialized to frontend for ToolCard rendering.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AgentStep {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub success: bool,
    pub output: String,
    pub confirmation_required: bool,
    pub confirmation_status: Option<String>,
    /// Issue #102: post-call verification evidence. `None` for tools that
    /// did not register a `PostCallVerifier`. When set, the frontend can
    /// highlight the step in red if `matched` is `false` and the runner's
    /// completion gate will surface unverified artifacts in the next prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<crate::agent::verifier::VerificationEvidence>,
}

fn task_requires_project_verification(user_message: &str) -> bool {
    let goal = user_message.to_ascii_lowercase();
    [
        "code",
        "代码",
        "项目",
        "project",
        "package.json",
        "测试",
        "test",
        "build",
        "编译",
    ]
    .iter()
    .any(|marker| goal.contains(marker))
}

/// A project mutation invalidates earlier verification evidence. A final text
/// response is therefore acceptable only after the facts show fresh evidence.
fn coding_task_has_unverified_changes(user_message: &str, steps: &[AgentStep]) -> bool {
    let facts = task_facts_from_steps(user_message, steps);
    derive_task_phase(&facts, task_requires_project_verification(user_message))
        == crate::agent::task_facts::DerivedTaskPhase::NeedsAttention
}

/// Issue #102: scan the trace for post-call verifier evidence that did not
/// match the disk state. Returns a human-readable summary suitable for
/// splicing into the LLM prompt right before a `complete` call. The
/// completion gate is a soft warning — the agent is allowed to call
/// `complete` anyway, but the user sees the same list in the trace UI.
pub fn collect_unverified_artifacts(steps: &[AgentStep]) -> Vec<String> {
    steps
        .iter()
        .filter_map(|step| {
            let ev = step.verification.as_ref()?;
            if ev.matched || ev.target.is_empty() {
                None
            } else {
                Some(format!(
                    "{}: {} ({})",
                    step.tool_name,
                    ev.target,
                    if ev.note.is_empty() {
                        "disk disagrees"
                    } else {
                        ev.note.as_str()
                    }
                ))
            }
        })
        .collect()
}

fn tool_batch_signature(tool_calls: &[LlmToolCall]) -> String {
    tool_calls
        .iter()
        .map(|call| format!("{}:{}", call.name, call.arguments))
        .collect::<Vec<_>>()
        .join("|")
}

/// Response from the agent
#[derive(Debug)]
pub enum AgentResponse {
    /// Plain text response (no tool calls were made)
    Text(String),
    /// Text response with recorded tool call steps for frontend display
    TextWithToolCalls {
        text: String,
        steps: Vec<AgentStep>,
    },
    Error(String),
}

/// The terminal state of one bounded AgentRunner execution slice.
///
/// This is intentionally not a workflow engine: callers may persist the
/// resulting facts and start a new slice later, but the runner never owns a
/// background task or hidden execution graph.
#[derive(Debug)]
pub enum AgentSliceOutcome {
    Completed(AgentResponse),
    AwaitingConfirmation(AgentResponse),
    NeedsAttention(AgentResponse),
}

/// Backend-only result. Never serialize this transcript into UI responses or
/// events; persistence owns its versioned, private replay ledger.
pub struct NativeAgentSliceResult {
    pub outcome: AgentSliceOutcome,
    pub protocol_transcript: Vec<LlmMessage>,
}

fn record_private_message(
    messages: &mut Vec<LlmMessage>,
    transcript: &mut Vec<LlmMessage>,
    message: LlmMessage,
) {
    let mut saved = message.clone();
    if !saved.tool_images.is_empty() {
        saved.tool_images.clear();
        saved
            .content
            .push_str("\n[图像瞬时载荷未保留；需要新状态时请重新观察。]");
    }
    transcript.push(saved);
    messages.push(message);
}

/// The agent runner that manages the tool-calling loop
#[cfg(test)]
mod private_protocol_tests {
    use super::*;
    use crate::agent::{Tool, ToolHandler, ToolResult};
    use crate::llm::{ModelProtocol, ProtocolContinuation, StopReason};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    fn protocol(items: Vec<serde_json::Value>) -> ProtocolContinuation {
        ProtocolContinuation {
            protocol: ModelProtocol::OpenaiResponses,
            model: "fixture".into(),
            credential_ref: "fixture-profile".into(),
            output_items: items,
        }
    }

    struct NativeProvider {
        calls: AtomicUsize,
        requests: Arc<Mutex<Vec<Vec<LlmMessage>>>>,
        incomplete: bool,
    }
    #[async_trait::async_trait]
    impl LlmProvider for NativeProvider {
        fn name(&self) -> &str {
            "native-fixture"
        }
        async fn chat(
            &self,
            messages: &[LlmMessage],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            self.requests.lock().unwrap().push(messages.to_vec());
            let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
            if first {
                Ok(LlmResponse::WithProtocol {
                    response: Box::new(LlmResponse::ToolCalls {
                        calls: vec![LlmToolCall {
                            id: "call-native".into(),
                            name: "read_file".into(),
                            arguments: serde_json::json!({"path":"a.txt"}),
                        }],
                        text: Some("Inspecting".into()),
                        stop_reason: if self.incomplete {
                            StopReason::MaxTokens
                        } else {
                            StopReason::ToolUse
                        },
                    }),
                    protocol: protocol(vec![
                        serde_json::json!({"type":"reasoning","id":"rs_fixture","encrypted_content":"opaque-marker"}),
                        serde_json::json!({"type":"function_call","id":"fc_fixture","call_id":"call-native","name":"read_file","arguments":"{\"path\":\"a.txt\"}"}),
                    ]),
                })
            } else {
                Ok(LlmResponse::WithProtocol {
                    response: Box::new(LlmResponse::Text {
                        text: "done".into(),
                        stop_reason: StopReason::EndTurn,
                    }),
                    protocol: protocol(vec![
                        serde_json::json!({"type":"message","id":"msg_fixture","phase":"final_answer","content":[{"type":"output_text","text":"done"}]}),
                    ]),
                })
            }
        }
    }
    struct Read(Arc<AtomicUsize>);
    impl ToolHandler for Read {
        fn name(&self) -> &str {
            "read_file"
        }
        fn execute(&self, _args: &serde_json::Value, _path: &std::path::Path) -> ToolResult {
            self.0.fetch_add(1, Ordering::SeqCst);
            ToolResult::success("read_file", "read-result")
        }
    }
    struct TwoCallProvider(NativeProvider);
    #[async_trait::async_trait]
    impl LlmProvider for TwoCallProvider {
        fn name(&self) -> &str {
            "native-two-call-fixture"
        }
        async fn chat(
            &self,
            messages: &[LlmMessage],
            tools: Option<&[ToolSchema]>,
            thinking: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let (mut response, mut state) =
                self.0.chat(messages, tools, thinking).await?.into_parts();
            if let LlmResponse::ToolCalls { calls, .. } = &mut response {
                calls.push(LlmToolCall {
                    id: "call-skipped".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path":"b.txt"}),
                });
                state
                    .as_mut()
                    .unwrap()
                    .output_items
                    .push(serde_json::json!({
                        "type":"function_call","id":"fc_skipped","call_id":"call-skipped",
                        "name":"read_file","arguments":"{\"path\":\"b.txt\"}"
                    }));
            }
            Ok(LlmResponse::WithProtocol {
                response: Box::new(response),
                protocol: state.unwrap(),
            })
        }
    }
    struct TerminatingRead(Arc<AtomicUsize>);
    impl ToolHandler for TerminatingRead {
        fn name(&self) -> &str {
            "read_file"
        }
        fn execute(&self, _args: &serde_json::Value, _path: &std::path::Path) -> ToolResult {
            self.0.fetch_add(1, Ordering::SeqCst);
            ToolResult::terminate_batch("read_file", "Stop this batch without more operations")
        }
    }
    struct Events(Arc<Mutex<Vec<String>>>);
    impl crate::agent::event::AgentEventEmitter for Events {
        fn emit(&self, _session: &str, event: AgentEvent) {
            self.0
                .lock()
                .unwrap()
                .push(serde_json::to_string(&event).unwrap());
        }
    }
    struct RejectTextCompaction;
    #[async_trait::async_trait]
    impl CompactionHook for RejectTextCompaction {
        async fn maybe_compact(
            &self,
            _messages: &mut Vec<LlmMessage>,
        ) -> crate::agent::compaction::CompactionOutcome {
            panic!("text-only compaction must not rewrite private protocol items")
        }
    }
    fn runner(
        incomplete: bool,
        first_index: usize,
    ) -> (
        AgentRunner,
        Arc<Mutex<Vec<Vec<LlmMessage>>>>,
        Arc<AtomicUsize>,
    ) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let executions = Arc::new(AtomicUsize::new(0));
        let mut runner = AgentRunner::new(
            Box::new(NativeProvider {
                calls: AtomicUsize::new(first_index),
                requests: requests.clone(),
                incomplete,
            }),
            AgentConfig::default(),
        );
        runner.tool_registry_mut().register(
            Tool::new("read_file", "Read", serde_json::json!({"type":"object"})),
            Arc::new(Read(executions.clone())),
        );
        (runner, requests, executions)
    }
    #[tokio::test]
    async fn private_transcript_preserves_tool_round_and_final_phase_without_event_payload() {
        let (mut runner, requests, executions) = runner(false, 0);
        let events = Arc::new(Mutex::new(Vec::new()));
        runner.emitter = Some(Arc::new(Events(events.clone())));
        let result = runner
            .run_slice_with_private_transcript("inspect", &[], None)
            .await;
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        assert_eq!(result.protocol_transcript.len(), 3);
        assert_eq!(
            result.protocol_transcript[0]
                .protocol_state
                .as_ref()
                .unwrap()
                .output_items[0]["encrypted_content"],
            "opaque-marker"
        );
        assert_eq!(
            result.protocol_transcript[1].tool_call_id.as_deref(),
            Some("call-native")
        );
        assert_eq!(
            result.protocol_transcript[2]
                .protocol_state
                .as_ref()
                .unwrap()
                .output_items[0]["phase"],
            "final_answer"
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1]
            .iter()
            .any(|message| message.protocol_state.is_some()));
        assert_eq!(
            requests[1].last().unwrap().tool_call_id.as_deref(),
            Some("call-native")
        );
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .all(|event| !event.contains("opaque-marker") && !event.contains("outputItems")));
    }
    #[tokio::test]
    async fn private_protocol_terminated_batch_keeps_skipped_receipt_without_execution() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let executions = Arc::new(AtomicUsize::new(0));
        let mut runner = AgentRunner::new(
            Box::new(TwoCallProvider(NativeProvider {
                calls: AtomicUsize::new(0),
                requests: requests.clone(),
                incomplete: false,
            })),
            AgentConfig::default(),
        );
        runner.tool_registry_mut().register(
            Tool::new("read_file", "Read", serde_json::json!({"type":"object"})),
            Arc::new(TerminatingRead(executions.clone())),
        );
        let result = runner
            .run_slice_with_private_transcript("inspect", &[], None)
            .await;
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        // The ordinary repair gate may add another assistant/user exchange
        // after the failed first call. The completed native batch must still
        // have exactly its two distinct receipts, with no skipped execution.
        let receipts = result
            .protocol_transcript
            .iter()
            .filter(|message| message.role == "tool")
            .collect::<Vec<_>>();
        assert_eq!(receipts.len(), 2);
        assert_eq!(receipts[0].tool_call_id.as_deref(), Some("call-native"));
        assert_eq!(receipts[1].tool_call_id.as_deref(), Some("call-skipped"));
        assert_eq!(
            result.protocol_transcript[0]
                .tool_calls
                .as_ref()
                .unwrap()
                .len(),
            2
        );
        let skipped = result
            .protocol_transcript
            .iter()
            .find(|message| message.tool_call_id.as_deref() == Some("call-skipped"))
            .unwrap();
        assert!(skipped.content.contains("跳过"));
        assert_eq!(
            result.protocol_transcript[0]
                .protocol_state
                .as_ref()
                .unwrap()
                .output_items[2]["call_id"],
            "call-skipped"
        );
        let requests = requests.lock().unwrap();
        assert!(requests[1]
            .iter()
            .any(
                |message| message.tool_call_id.as_deref() == Some("call-skipped")
                    && message.content == skipped.content
            ));
    }
    #[tokio::test]
    async fn incomplete_private_tool_batch_never_executes_or_enters_replay_ledger() {
        let (runner, requests, executions) = runner(true, 0);
        let result = runner
            .run_slice_with_private_transcript("inspect", &[], None)
            .await;
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(result.protocol_transcript.is_empty());
    }
    #[tokio::test]
    async fn private_followup_keeps_history_and_defers_protocol_aware_compaction() {
        let (mut runner, requests, _) = runner(false, 1);
        runner.compaction_hook = Arc::new(RejectTextCompaction);
        let history = vec![LlmMessage {
            role: "assistant".into(),
            content: "prior".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_images: Vec::new(),
            protocol_state: Some(protocol(vec![
                serde_json::json!({"type":"reasoning","encrypted_content":"prior-marker"}),
            ])),
        }];
        let result = runner
            .run_slice_with_private_transcript("follow up", &history, None)
            .await;
        assert_eq!(
            result.protocol_transcript.len(),
            1,
            "new ledger must not copy initial history"
        );
        assert_eq!(
            requests.lock().unwrap()[0][1]
                .protocol_state
                .as_ref()
                .unwrap()
                .output_items[0]["encrypted_content"],
            "prior-marker"
        );
    }
}

#[cfg(test)]
mod tool_image_runtime_tests {
    use super::*;
    use crate::agent::{AfterToolCallResult, Tool, ToolHandler, ToolResult, ToolResultCache};
    use crate::llm::{StopReason, ToolImage};
    use base64::{engine::general_purpose::STANDARD, Engine};
    use std::sync::atomic::AtomicUsize;

    fn image_fixture() -> (String, ToolImage) {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[1, 2, 3, 255, 4, 5, 6, 255])
            .unwrap();
        writer.finish().unwrap();
        let encoded = STANDARD.encode(bytes);
        let image = ToolImage::from_base64("image/png", &encoded).unwrap();
        (encoded, image)
    }

    struct ImageRoundsProvider {
        supports_images: bool,
        requests: Arc<Mutex<Vec<Vec<LlmMessage>>>>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for ImageRoundsProvider {
        fn name(&self) -> &str {
            "image-rounds-fixture"
        }

        fn supports_tool_images(&self) -> bool {
            self.supports_images
        }

        async fn chat(
            &self,
            messages: &[LlmMessage],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let index = {
                let mut requests = self.requests.lock().unwrap();
                let index = requests.len();
                requests.push(messages.to_vec());
                index
            };
            if index < 2 {
                Ok(LlmResponse::ToolCalls {
                    calls: vec![LlmToolCall {
                        id: if index == 0 {
                            "call-image"
                        } else {
                            "call-text"
                        }
                        .into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": if index == 0 { "image" } else { "text" }}),
                    }],
                    text: Some("Inspecting".into()),
                    stop_reason: StopReason::ToolUse,
                })
            } else {
                assert_eq!(index, 2, "fixture must finish after three model rounds");
                Ok(LlmResponse::Text {
                    text: "inspection complete".into(),
                    stop_reason: StopReason::EndTurn,
                })
            }
        }
    }

    struct ImageReadHandler {
        image: ToolImage,
        executions: Arc<AtomicUsize>,
        cancel_after_observation: bool,
    }

    impl ToolHandler for ImageReadHandler {
        fn name(&self) -> &str {
            "read_file"
        }

        fn execute(&self, arguments: &serde_json::Value, _: &std::path::Path) -> ToolResult {
            self.executions.fetch_add(1, Ordering::SeqCst);
            if arguments["path"] == "image" {
                ToolResult::success_with_images(
                    self.name(),
                    "PNG observation",
                    vec![self.image.clone()],
                )
            } else {
                ToolResult::success(self.name(), "text observation")
            }
        }

        fn execute_with_context(
            &self,
            arguments: &serde_json::Value,
            work_dir: &std::path::Path,
            context: &ToolExecutionContext,
        ) -> ToolResult {
            let result = self.execute(arguments, work_dir);
            if self.cancel_after_observation {
                context.cancellation.cancel();
            }
            result
        }
    }

    struct CompactionProbe(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl CompactionHook for CompactionProbe {
        async fn maybe_compact(
            &self,
            messages: &mut Vec<LlmMessage>,
        ) -> crate::agent::compaction::CompactionOutcome {
            assert!(
                messages
                    .iter()
                    .all(|message| message.tool_images.is_empty()),
                "text compaction must never receive transient images"
            );
            self.0.fetch_add(1, Ordering::SeqCst);
            crate::agent::compaction::CompactionOutcome::Skipped
        }
    }

    struct Events(Arc<Mutex<Vec<String>>>);

    impl crate::agent::event::AgentEventEmitter for Events {
        fn emit(&self, _: &str, event: AgentEvent) {
            self.0
                .lock()
                .unwrap()
                .push(serde_json::to_string(&event).unwrap());
        }
    }

    fn assert_no_payload(result: &NativeAgentSliceResult, events: &[String], encoded: &str) {
        assert!(
            result
                .protocol_transcript
                .iter()
                .all(|message| message.tool_images.is_empty()),
            "returned replay ledger must not retain image objects"
        );
        let transcript_json = serde_json::to_string(&result.protocol_transcript).unwrap();
        assert!(!transcript_json.contains(encoded));
        assert!(!transcript_json.contains("tool_images"));
        assert!(!transcript_json.contains("data:image/"));
        assert!(!format!("{:?}", result.outcome).contains(encoded));
        let response = match &result.outcome {
            AgentSliceOutcome::Completed(response)
            | AgentSliceOutcome::AwaitingConfirmation(response)
            | AgentSliceOutcome::NeedsAttention(response) => response,
        };
        if let AgentResponse::TextWithToolCalls { steps, .. } = response {
            let steps_json = serde_json::to_string(steps).unwrap();
            assert!(!steps_json.contains(encoded));
            assert!(!steps_json.contains("data:image/"));
        }
        assert!(
            events
                .iter()
                .any(|event| event.contains("ToolExecutionEnd")),
            "privacy assertion must include real tool result events"
        );
        assert!(events
            .iter()
            .all(|event| !event.contains(encoded) && !event.contains("data:image/")));
    }

    #[tokio::test]
    async fn images_reach_only_the_next_supported_model_round_not_replay_or_ui() {
        for mode in [ToolExecutionMode::Sequential, ToolExecutionMode::Parallel] {
            for supports_images in [true, false] {
                let (encoded, image) = image_fixture();
                let requests = Arc::new(Mutex::new(Vec::new()));
                let executions = Arc::new(AtomicUsize::new(0));
                let compactions = Arc::new(AtomicUsize::new(0));
                let events = Arc::new(Mutex::new(Vec::new()));
                let mut runner = AgentRunner::new(
                    Box::new(ImageRoundsProvider {
                        supports_images,
                        requests: requests.clone(),
                    }),
                    AgentConfig::default(),
                );
                runner.compaction_hook = Arc::new(CompactionProbe(compactions.clone()));
                runner.emitter = Some(Arc::new(Events(events.clone())));
                runner.tool_registry_mut().register(
                    Tool::new(
                        "read_file",
                        "Read fixture",
                        serde_json::json!({"type":"object"}),
                    )
                    .with_execution_mode(mode),
                    Arc::new(ImageReadHandler {
                        image,
                        executions: executions.clone(),
                        cancel_after_observation: false,
                    }),
                );
                let result = runner
                    .run_slice_with_private_transcript("inspect", &[], None)
                    .await;
                let requests = requests.lock().unwrap();
                assert_eq!(requests.len(), 3);
                assert_eq!(executions.load(Ordering::SeqCst), 2);
                assert!(requests[0]
                    .iter()
                    .all(|message| message.tool_images.is_empty()));
                let image_message = requests[1]
                    .iter()
                    .find(|message| message.tool_call_id.as_deref() == Some("call-image"))
                    .unwrap();
                assert_eq!(image_message.role, "tool");
                if supports_images {
                    assert_eq!(image_message.tool_images.len(), 1);
                    assert_eq!(
                        image_message.tool_images[0].data_url(),
                        format!("data:image/png;base64,{encoded}")
                    );
                } else {
                    assert!(image_message.tool_images.is_empty());
                    assert!(image_message
                        .content
                        .contains("未发送图像，请勿声称已经查看"));
                }
                assert!(requests[2]
                    .iter()
                    .all(|message| message.tool_images.is_empty()));
                let released = requests[2]
                    .iter()
                    .find(|message| message.tool_call_id.as_deref() == Some("call-image"))
                    .unwrap();
                if supports_images {
                    assert!(released.content.contains("图像载荷已释放"));
                }
                assert_eq!(
                    compactions.load(Ordering::SeqCst),
                    if supports_images { 2 } else { 3 }
                );
                let saved = result
                    .protocol_transcript
                    .iter()
                    .find(|message| message.tool_call_id.as_deref() == Some("call-image"))
                    .unwrap();
                assert!(saved.content.contains(if supports_images {
                    "图像瞬时载荷未保留"
                } else {
                    "未发送图像"
                }));
                let AgentSliceOutcome::Completed(AgentResponse::TextWithToolCalls { text, steps }) =
                    &result.outcome
                else {
                    panic!(
                        "fixture must complete with tool facts: {:?}",
                        result.outcome
                    );
                };
                assert_eq!(text, "inspection complete");
                assert_eq!(steps.len(), 2);
                assert!(steps.iter().all(|step| step.success));
                // Even serializing a live model request must not leak its transient payload.
                assert!(!serde_json::to_string(&*requests)
                    .unwrap()
                    .contains(&encoded));
                assert_no_payload(&result, &events.lock().unwrap(), &encoded);
            }
        }
    }

    #[tokio::test]
    async fn cancellation_after_observation_never_calls_the_model_or_retains_the_image() {
        for mode in [ToolExecutionMode::Sequential, ToolExecutionMode::Parallel] {
            let (encoded, image) = image_fixture();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let executions = Arc::new(AtomicUsize::new(0));
            let events = Arc::new(Mutex::new(Vec::new()));
            let mut runner = AgentRunner::new(
                Box::new(ImageRoundsProvider {
                    supports_images: true,
                    requests: requests.clone(),
                }),
                AgentConfig::default(),
            );
            runner.emitter = Some(Arc::new(Events(events.clone())));
            runner.tool_registry_mut().register(
                Tool::new(
                    "read_file",
                    "Read fixture",
                    serde_json::json!({"type":"object"}),
                )
                .with_execution_mode(mode),
                Arc::new(ImageReadHandler {
                    image,
                    executions: executions.clone(),
                    cancel_after_observation: true,
                }),
            );
            let result = runner
                .run_slice_with_private_transcript("inspect", &[], None)
                .await;
            assert!(runner.cancellation_token().is_cancelled());
            assert_eq!(executions.load(Ordering::SeqCst), 1);
            assert_eq!(
                requests.lock().unwrap().len(),
                1,
                "cancelled observation must not reach another model call"
            );
            let receipt = result
                .protocol_transcript
                .iter()
                .find(|message| message.tool_call_id.as_deref() == Some("call-image"))
                .unwrap();
            assert!(receipt.content.contains("运行已取消；未将图像交给模型"));
            assert!(receipt.tool_images.is_empty());
            let AgentSliceOutcome::Completed(AgentResponse::Error(message)) = &result.outcome
            else {
                panic!("runner must report cancellation: {:?}", result.outcome);
            };
            assert!(message.contains("取消"));
            assert_no_payload(&result, &events.lock().unwrap(), &encoded);
        }
    }

    #[test]
    fn after_hook_retains_images_only_for_successful_nonterminal_results() {
        let (encoded, image) = image_fixture();
        for original_success in [true, false] {
            for original_terminate in [true, false] {
                for is_error in [None, Some(true), Some(false)] {
                    for terminate in [None, Some(true), Some(false)] {
                        let result = ToolResult {
                            name: "fixture".into(),
                            success: original_success,
                            content: "original".into(),
                            terminate_batch: original_terminate,
                            images: vec![image.clone()],
                        };
                        let applied = crate::agent::apply_after_hook(
                            result,
                            AfterToolCallResult {
                                content: Some("after-hook receipt".into()),
                                is_error,
                                terminate,
                            },
                        );
                        let expected_success =
                            is_error.map(|error| !error).unwrap_or(original_success);
                        let expected_terminate = terminate.unwrap_or(original_terminate);
                        assert_eq!(applied.success, expected_success);
                        assert_eq!(applied.terminate_batch, expected_terminate);
                        assert_eq!(applied.content, "after-hook receipt");
                        assert_eq!(
                            applied.images.len(),
                            usize::from(expected_success && !expected_terminate)
                        );
                        assert!(!serde_json::to_string(&applied).unwrap().contains(&encoded));
                    }
                }
            }
        }
    }

    #[test]
    fn image_observations_are_never_cached_or_evict_text_results() {
        let (_, image) = image_fixture();
        let cache = ToolResultCache::new(1, 0);
        let text_args = serde_json::json!({"path":"text"});
        let image_args = serde_json::json!({"path":"image"});
        cache.put(
            "read_file".into(),
            text_args.clone(),
            ToolResult::success("read_file", "text-only"),
        );
        for success in [true, false] {
            let mut result = ToolResult::success_with_images(
                "read_file",
                "PNG observation",
                vec![image.clone()],
            );
            result.success = success;
            cache.put("read_file".into(), image_args.clone(), result);
            assert!(cache.get("read_file", &image_args).is_none());
            let retained = cache.get("read_file", &text_args).unwrap();
            assert_eq!(retained.content, "text-only");
            assert!(retained.images.is_empty());
        }
        assert_eq!(cache.hit_rate(), (2, 2));
    }
}

/// The agent runner that manages the tool-calling loop
pub struct AgentRunner {
    provider: Arc<dyn LlmProvider>,
    tool_registry: ToolRegistry,
    config: AgentConfig,
    work_dir: std::path::PathBuf,
    session_id: Option<String>,
    emitter: Option<Arc<dyn crate::agent::event::AgentEventEmitter>>,
    thinking_settings: Option<ThinkingSettings>,
    tools_enabled: bool,
    execution_permission: String,
    control_states: Option<Arc<Mutex<HashMap<String, AgentControlState>>>>,
    lifecycle_hook: Arc<dyn AgentLifecycleHook>,
    compaction_hook: Arc<dyn CompactionHook>,
    tool_lifecycle_hook: Arc<dyn ToolLifecycleHook>,
    api_key_provider: Arc<dyn DynamicApiKeyProvider>,
    /// Soft termination guards evaluated at every main iteration boundary.
    /// Guards observe runner state and emit `Continue` / `PauseForUser` /
    /// `Terminate`. They never mutate state directly.
    guards: Vec<Arc<dyn crate::agent::guards::LoopGuard>>,
    /// Cooperative cancellation propagated through tool execution. A new
    /// run always allocates a fresh signal so a cancel from an earlier
    /// run can never leak. External callers may inject a shared signal
    /// via `with_cancellation` to coordinate with parallel control planes
    /// (e.g., `AgentControlState::Interrupted`).
    cancellation: ToolCancellation,
    /// Shared queue of user messages that arrived mid-turn. `None` means
    /// the runner does not consult any queue (the default for tests and
    /// the steering-free code path). When set, the runner drains it on
    /// every iteration before calling the LLM.
    pending_user_messages: Option<Arc<Mutex<crate::agent::pending_queue::PendingMessageQueue>>>,
    /// One sequence shared by text and tool blocks for the whole run.
    content_block_index: Arc<AtomicU32>,
}

impl AgentRunner {
    pub fn latest_usage(&self) -> Option<crate::llm::ProviderUsage> {
        self.provider.latest_usage()
    }
    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }
    fn truncate_tool_result_for_context(result: String) -> String {
        let char_count = result.chars().count();
        if char_count <= MAX_TOOL_RESULT_CONTEXT_CHARS {
            return result;
        }
        let head = result
            .chars()
            .take(MAX_TOOL_RESULT_CONTEXT_CHARS)
            .collect::<String>();
        format!(
            "{head}\n\n[tool output truncated: {char_count} chars total; inspect the relevant file or rerun a narrower command if more detail is needed]"
        )
    }
    /// Preserve a terminal runner condition alongside completed tool facts.
    ///
    /// An LLM timeout or exhausted slice budget is not a tool failure, but it
    /// does mean the requested task was not conclusively finished. Recording a
    /// synthetic, read-only step keeps the UI, persisted task state, and
    /// continuation prompt aligned without pretending a tool was re-run.
    fn terminal_slice_response(steps: Vec<AgentStep>, message: String) -> AgentResponse {
        if steps.is_empty() {
            return AgentResponse::Error(message);
        }

        Self::auditable_terminal_slice_response(steps, message)
    }

    /// Preserve a model-written handoff when the completion gate reaches its
    /// bounded retry limit. The task remains resumable and auditable, but a
    /// verification warning must never replace the assistant's useful summary.
    fn completion_gate_handoff_response(
        mut steps: Vec<AgentStep>,
        summary: String,
    ) -> AgentResponse {
        const DIAGNOSTIC: &str =
            "代码修改后尚无成功的验证证据；已停止自动收尾。请继续任务以选择验证命令、处理失败，或明确跳过验证。";
        steps.push(AgentStep {
            call_id: format!("agent-loop-{}", Uuid::new_v4()),
            tool_name: "agent_loop".to_string(),
            arguments: serde_json::Value::Null,
            success: false,
            output: DIAGNOSTIC.to_string(),
            confirmation_required: false,
            confirmation_status: None,
            verification: None,
        });
        AgentResponse::TextWithToolCalls {
            text: format!("{summary}\n\n验证状态：尚无成功验证证据。可继续任务选择验证命令、处理失败，或明确跳过验证。"),
            steps,
        }
    }

    /// A protocol violation is a terminal condition even before a tool ran.
    /// Persist it as a synthetic step so the task cannot be mistaken for a
    /// successful text-only completion.
    fn protocol_violation_slice_response(steps: Vec<AgentStep>, message: String) -> AgentResponse {
        Self::auditable_terminal_slice_response(steps, message)
    }

    fn auditable_terminal_slice_response(
        mut steps: Vec<AgentStep>,
        message: String,
    ) -> AgentResponse {
        steps.push(AgentStep {
            call_id: format!("agent-loop-{}", Uuid::new_v4()),
            tool_name: "agent_loop".to_string(),
            arguments: serde_json::Value::Null,
            success: false,
            output: message.clone(),
            confirmation_required: false,
            confirmation_status: None,
            verification: None,
        });
        AgentResponse::TextWithToolCalls {
            text: message,
            steps,
        }
    }

    // ── Pi-style terminal helpers ──────────────────────────────────────────────

    /// Simple terminal response for safety bounds
    fn terminal(steps: Vec<AgentStep>, message: String) -> AgentResponse {
        Self::auditable_terminal_slice_response(steps, message)
    }

    /// Terminal with error event emission
    fn terminal_with_error(
        &self,
        steps: Vec<AgentStep>,
        message: String,
        iteration: usize,
        total_tool_calls: usize,
        turn_id: &str,
        session_id: &str,
    ) -> AgentResponse {
        self.emit(AgentEvent::Error {
            code: crate::agent::event::ErrorCode::MaxIterationsReached,
            message: message.clone(),
            recoverable: true,
            turn_id: Some(turn_id.to_string()),
        });
        self.emit_turn_end(turn_id, total_tool_calls);
        self.emit_agent_end(
            session_id,
            iteration,
            total_tool_calls,
            Some(message.clone()),
        );
        Self::terminal(steps, message)
    }

    /// Common closing ceremony for every terminal branch the agent loop
    /// reaches. Centralised so the front-end always sees the same
    /// `TurnEnd` + `AgentEnd` pair regardless of which guard fired, and so
    /// the lifecycle hook gets a single call site per branch.
    fn close_turn_and_end(
        &self,
        turn_id: &str,
        session_id: &str,
        total_tool_calls: usize,
        iteration: usize,
        summary: Option<String>,
    ) {
        self.emit_turn_end(turn_id, total_tool_calls);
        self.emit_agent_end(session_id, iteration, total_tool_calls, summary);
    }

    /// Protocol violation terminal.
    ///
    /// Emits the lifecycle hook and the standard turn/agent end events so the
    /// front-end sees a consistent terminal sequence regardless of which
    /// branch triggered it. The synthetic `agent_loop` step appended by
    /// `auditable_terminal_slice_response` keeps the failure auditable even
    /// when no real tool ran.
    fn protocol_violation(
        &self,
        steps: Vec<AgentStep>,
        reason: String,
        iteration: usize,
        total_tool_calls: usize,
        turn_id: &str,
        session_id: &str,
    ) -> AgentResponse {
        self.notify_terminal_branch(
            TerminalBranchKind::ProtocolViolation,
            iteration,
            &[],
            &steps,
            total_tool_calls,
        );
        self.close_turn_and_end(
            turn_id,
            session_id,
            total_tool_calls,
            iteration,
            Some(format!("工具协议违规: {reason}")),
        );
        Self::protocol_violation_slice_response(steps, format!("工具协议违规: {reason}"))
    }

    /// Emit turn end event
    /// Build the lifecycle context for a terminal-branch hook and call the hook.
    /// Centralised because every terminal branch needed the exact same five
    /// arguments and forgetting any one of them silently produced a hook
    /// call with stale state.
    fn notify_terminal_branch(
        &self,
        kind: TerminalBranchKind,
        iteration: usize,
        messages: &[LlmMessage],
        steps: &[AgentStep],
        total_tool_calls: usize,
    ) {
        let _ = self.lifecycle_hook.before_terminal_branch(
            &LifecycleContext::before_branch(
                iteration,
                0,
                messages.to_vec(),
                steps.to_vec(),
                total_tool_calls,
            ),
            kind,
        );
    }

    /// Emit the start-of-execution event for one tool call. Centralised so
    /// the start payload (call_id, tool_name, arguments) cannot drift across
    /// the validation, confirmation, and execution branches — a missing or
    /// mismatched start is the easiest way to break the front-end's
    /// progress indicator.
    /// Emit ToolExecutionStart (legacy) + ContentBlock::ToolCallStart (new unified stream).
    fn emit_tool_start(
        &self,
        turn_state: &mut TurnState,
        turn_id: &str,
        message_id: &str,
        call_id: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) {
        self.emit(AgentEvent::ToolExecutionStart {
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments: arguments.clone(),
        });
        self.emit_content_block(
            turn_state,
            turn_id,
            message_id,
            ContentBlockData::ToolCallStart {
                call_id: call_id.to_string(),
                tool_name: tool_name.to_string(),
                arguments: arguments.clone(),
            },
        );
    }

    /// Emit a unified ContentBlock event for the inline streaming UI.
    fn emit_content_block(
        &self,
        turn_state: &mut TurnState,
        turn_id: &str,
        message_id: &str,
        block: ContentBlockData,
    ) {
        let index = self.content_block_index.fetch_add(1, Ordering::SeqCst) + 1;
        self.emit(AgentEvent::ContentBlock {
            turn_id: turn_id.to_string(),
            message_id: message_id.to_string(),
            index,
            iteration_id: turn_state.iteration_id,
            block,
        });
    }

    /// Emit ToolExecutionEnd (legacy) + ContentBlock::ToolCallResult (new unified stream).
    fn emit_tool_end(
        &self,
        turn_state: &mut TurnState,
        turn_id: &str,
        message_id: &str,
        call_id: &str,
        tool_name: &str,
        exec_result: &crate::agent::registry::ToolExecResult,
    ) {
        let success = exec_result.result.success;
        let output = &exec_result.result.content;
        let error_value = if success { None } else { Some(output.clone()) };

        self.emit(AgentEvent::ToolExecutionEnd {
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            success,
            output: output.clone(),
            error: error_value.clone(),
        });

        self.emit_content_block(
            turn_state,
            turn_id,
            message_id,
            ContentBlockData::ToolCallResult {
                call_id: call_id.to_string(),
                tool_name: tool_name.to_string(),
                success,
                output: output.clone(),
                error: error_value,
            },
        );
    }

    /// Apply the `after_tool_call` lifecycle hook to a raw registry result and
    /// wrap it back into a `ToolExecResult` for the runner. Centralising this
    /// keeps the parallel and sequential branches byte-identical on hook
    /// semantics — including whether the hook was allowed to mutate the
    /// outcome — so an extension that overrides results cannot drift between
    /// branches. `session_id` may be `None` when the runner was started
    /// without one (e.g. a smoke test); an empty string is the documented
    /// no-op in that case.
    fn apply_post_hook(
        &self,
        tc: &AgentToolCall,
        exec_result: crate::agent::registry::ToolExecResult,
        args: &serde_json::Value,
        session_id: Option<&str>,
        iteration: usize,
    ) -> crate::agent::registry::ToolExecResult {
        let after_ctx = AfterToolCallContext {
            tool_call: tc.clone(),
            tool_name: tc.name.clone(),
            args: args.clone(),
            result: exec_result.result.clone(),
            is_error: !exec_result.result.success,
            session_id: session_id.map(|s| s.to_string()),
            iteration,
        };
        let after_result = self.tool_lifecycle_hook.after_tool_call(&after_ctx);
        let final_result =
            crate::agent::lifecycle::apply_after_hook(exec_result.result.clone(), after_result);
        crate::agent::registry::ToolExecResult {
            result: final_result,
            call_id: exec_result.call_id,
            terminate_batch: exec_result.terminate_batch,
        }
    }

    /// Record one tool execution outcome into the agent step log, emit the
    /// matching `ToolExecutionEnd` event (legacy) + `ContentBlock::ToolCallResult` (new),
    /// and return the LLM-shaped message that should be appended to the running
    /// transcript. Centralising this keeps the parallel and sequential branches
    /// in lock-step on the audit trail (steps + events).
    fn record_tool_outcome(
        &self,
        steps: &mut Vec<AgentStep>,
        turn_state: &mut TurnState,
        turn_id: &str,
        message_id: &str,
        call_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
        exec_result: &crate::agent::registry::ToolExecResult,
        include_hint: bool,
    ) -> LlmMessage {
        // Issue #102: run the tool's post-call verifier (if any) so the
        // step carries disk-truth evidence to the audit log and the
        // completion gate. Verifier is read-only — it never alters the
        // ToolResult the handler returned.
        let evidence = self.tool_registry.get(tool_name).and_then(|tool| {
            tool.post_call_verifier
                .as_ref()
                .map(|v| v.verify(&arguments, &exec_result.result))
        });

        steps.push(AgentStep {
            call_id: exec_result.call_id.clone(),
            tool_name: tool_name.to_string(),
            arguments,
            success: exec_result.result.success,
            output: exec_result.result.content.clone(),
            confirmation_required: false,
            confirmation_status: None,
            verification: evidence,
        });

        self.emit_tool_end(
            turn_state,
            turn_id,
            message_id,
            call_id,
            tool_name,
            exec_result,
        );

        let mut message = LlmMessage {
            role: "tool".to_string(),
            content: Self::truncate_tool_result_for_context(
                exec_result.result.format_for_llm(include_hint),
            ),
            tool_calls: None,
            tool_call_id: Some(exec_result.call_id.clone()),
            tool_images: Vec::new(),
            protocol_state: None,
        };
        if !exec_result.result.images.is_empty() {
            if self.cancellation.is_cancelled() {
                message
                    .content
                    .push_str("\n[运行已取消；未将图像交给模型。]");
            } else if exec_result.result.success && self.provider.supports_tool_images() {
                message.tool_images = exec_result.result.images.clone();
            } else {
                message
                    .content
                    .push_str("\n[当前模型接口未接入工具图像输入；未发送图像，请勿声称已经查看。]");
            }
        }
        message
    }

    fn emit_turn_end(&self, turn_id: &str, total_tool_calls: usize) {
        self.emit(AgentEvent::TurnEnd {
            turn_id: turn_id.to_string(),
            success: true,
            tool_calls_count: total_tool_calls,
        });
    }

    /// Emit agent end event
    fn emit_agent_end(
        &self,
        session_id: &str,
        total_turns: usize,
        total_tool_calls: usize,
        summary: Option<String>,
    ) {
        self.emit(AgentEvent::AgentEnd {
            session_id: session_id.to_string(),
            summary,
            total_turns,
            total_tool_calls,
        });
    }

    /// Emit message end event
    fn emit_msg_end(&self, turn_id: &str, message_id: &str, text: &str) {
        self.emit(AgentEvent::MessageEnd {
            turn_id: turn_id.to_string(),
            message_id: message_id.to_string(),
            full_text: text.to_string(),
        });
    }

    /// Run one bounded tool-use slice and report whether user action is needed.
    pub async fn run_slice(
        &self,
        user_message: &str,
        history: &[(String, String)],
    ) -> AgentSliceOutcome {
        let response = self.run(user_message, history).await;
        self.slice_outcome(user_message, response)
    }

    /// Run one bounded slice with provider-native history.  This keeps
    /// assistant tool calls paired with their tool results when a persisted
    /// session is resumed.
    pub async fn run_slice_with_native_history(
        &self,
        user_message: &str,
        history: &[LlmMessage],
    ) -> AgentSliceOutcome {
        let initial_messages =
            self.build_initial_messages_with_native_history(user_message, history);
        let response = self
            .run_with_initial_messages(user_message, initial_messages)
            .await;
        self.slice_outcome(user_message, response)
    }

    /// Same as native-history execution, with a bounded structured attention
    /// projection supplied by the foreground host. The runner never loads it
    /// from storage, keeping persistence and access policy outside this loop.
    pub async fn run_slice_with_native_history_and_attention(
        &self,
        user_message: &str,
        history: &[LlmMessage],
        attention_context: Option<&str>,
    ) -> AgentSliceOutcome {
        self.run_slice_with_private_transcript(user_message, history, attention_context)
            .await
            .outcome
    }

    pub async fn run_slice_with_private_transcript(
        &self,
        user_message: &str,
        history: &[LlmMessage],
        attention_context: Option<&str>,
    ) -> NativeAgentSliceResult {
        let mut initial_messages =
            self.build_initial_messages_with_native_history(user_message, history);
        if let Some(context) = attention_context {
            initial_messages.insert(
                1,
                AgentMessage::System(context.to_string()).to_llm_message(),
            );
        }
        let mut protocol_transcript = Vec::new();
        let response = self
            .run_with_initial_messages_collect(
                user_message,
                initial_messages,
                &mut protocol_transcript,
            )
            .await;
        NativeAgentSliceResult {
            outcome: self.slice_outcome(user_message, response),
            protocol_transcript,
        }
    }

    fn slice_outcome(&self, user_message: &str, response: AgentResponse) -> AgentSliceOutcome {
        match &response {
            AgentResponse::TextWithToolCalls { steps, .. } => match derive_task_phase(
                &task_facts_from_steps(user_message, steps),
                task_requires_project_verification(user_message),
            ) {
                DerivedTaskPhase::AwaitingConfirmation => {
                    AgentSliceOutcome::AwaitingConfirmation(response)
                }
                DerivedTaskPhase::NeedsAttention | DerivedTaskPhase::Stopped => {
                    AgentSliceOutcome::NeedsAttention(response)
                }
                DerivedTaskPhase::Running | DerivedTaskPhase::Completed => {
                    AgentSliceOutcome::Completed(response)
                }
            },
            _ => AgentSliceOutcome::Completed(response),
        }
    }

    /// Create a new agent runner with the given provider and configuration.
    /// Defaults work directory to user's Documents.
    pub fn new(provider: Box<dyn LlmProvider>, config: AgentConfig) -> Self {
        let provider: Arc<dyn LlmProvider> = provider.into();
        let default_work_dir = config
            .work_dir
            .clone()
            .or_else(|| dirs::document_dir())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        Self {
            provider: provider.clone(),
            tool_registry: ToolRegistry::new(),
            config,
            work_dir: default_work_dir,
            session_id: None,
            emitter: None,
            thinking_settings: None,
            tools_enabled: true,
            execution_permission: "ask".to_string(),
            control_states: None,
            lifecycle_hook: Arc::new(DefaultLifecycleHook),
            compaction_hook: Arc::new(DefaultCompactionHook::new(provider.clone())),
            tool_lifecycle_hook: Arc::new(DefaultToolLifecycleHook),
            api_key_provider: Arc::new(DefaultApiKeyProvider),
            guards: default_guard_set(),
            cancellation: ToolCancellation::new(),
            pending_user_messages: None,
            content_block_index: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Create a new agent runner with a custom tool registry
    pub fn with_registry(
        provider: Box<dyn LlmProvider>,
        config: AgentConfig,
        tool_registry: ToolRegistry,
    ) -> Self {
        let provider: Arc<dyn LlmProvider> = provider.into();
        let default_work_dir = config
            .work_dir
            .clone()
            .or_else(|| dirs::document_dir())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        Self {
            provider: provider.clone(),
            tool_registry,
            config,
            work_dir: default_work_dir,
            session_id: None,
            emitter: None,
            thinking_settings: None,
            tools_enabled: true,
            execution_permission: "ask".to_string(),
            control_states: None,
            lifecycle_hook: Arc::new(DefaultLifecycleHook),
            compaction_hook: Arc::new(DefaultCompactionHook::new(provider.clone())),
            tool_lifecycle_hook: Arc::new(DefaultToolLifecycleHook),
            api_key_provider: Arc::new(DefaultApiKeyProvider),
            guards: default_guard_set(),
            cancellation: ToolCancellation::new(),
            pending_user_messages: None,
            content_block_index: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Replace the lifecycle hook with a custom observer. The runner
    /// keeps behaving the same; the hook simply observes.
    pub fn with_lifecycle_hook(mut self, hook: Arc<dyn AgentLifecycleHook>) -> Self {
        self.lifecycle_hook = hook;
        self
    }

    /// Replace the legacy fixed-size live compaction policy with a budget
    /// derived from the selected provider/model. The retained tail is also a
    /// token budget so long tool turns are not split merely by message count.
    pub fn with_context_compaction_budget(
        mut self,
        threshold: usize,
        keep_recent_tokens: usize,
    ) -> Self {
        self.compaction_hook = Arc::new(DefaultCompactionHook::with_token_budget(
            self.provider.clone(),
            threshold,
            keep_recent_tokens,
        ));
        self
    }

    /// Append a soft-termination guard. Each guard is evaluated at
    /// every main iteration boundary. Multiple guards compose; the first
    /// non-`Continue` verdict wins. `AgentRunner::new()` already
    /// registers three default guards — use `add_guard` to layer on top,
    /// `with_default_guards_off()` to clear, or `with_guards` to replace.
    pub fn add_guard(mut self, guard: Arc<dyn crate::agent::guards::LoopGuard>) -> Self {
        self.guards.push(guard);
        self
    }

    /// Return the currently registered guards. A freshly constructed
    /// runner carries three default guards (WallClock / NetProgress /
    /// SideEffectAutoPause). Callers that prefer to configure these
    /// themselves can clear the list with `with_default_guards_off()`
    /// and add their own via `add_guard(...)`.
    pub fn guards(&self) -> &[Arc<dyn crate::agent::guards::LoopGuard>] {
        &self.guards
    }

    /// Disable the default guard registration. Use this when the
    /// embedding application wants to bring its own guard set (e.g.,
    /// the user has configured `MaxSideEffects = 12` in settings, or
    /// wants a different wall-clock budget). Without this, every
    /// `AgentRunner` ships with sensible defaults (WallClock = 10min,
    /// NetProgress = 5, SideEffectAutoPause = 8) — see `AgentRunner::new()`
    /// for the exact thresholds.
    pub fn with_default_guards_off(mut self) -> Self {
        self.guards.clear();
        self
    }

    /// Replace the entire guard list. Useful for tests and for callers
    /// that want full control over the guard order.
    pub fn with_guards(mut self, guards: Vec<Arc<dyn crate::agent::guards::LoopGuard>>) -> Self {
        self.guards = guards;
        self
    }

    /// Inject a shared cancellation signal. The signal is consulted by
    /// `wait_for_control` (which cancels it on Interrupted/Stopped) and
    /// by `registry.execute_with_context` (which short-circuits tool
    /// handlers once cancelled). Default runners get a fresh signal per
    /// `run()` invocation — only inject one when an external control
    /// plane (e.g., the steering runtime) needs to cancel in lockstep.
    pub fn with_cancellation(mut self, cancellation: ToolCancellation) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// Return a clone of the runner's cancellation signal. External
    /// callers (e.g., `commands/message.rs`) can hold this clone and
    /// call `cancel()` from a steering thread; the next tool handler
    /// invocation or `wait_for_control` tick will observe it.
    pub fn cancellation_token(&self) -> ToolCancellation {
        self.cancellation.clone()
    }

    /// Inject a shared pending-message queue.
    ///
    /// The Tauri command layer pushes user messages into this queue
    /// while the agent turn is in progress; the runner drains it on
    /// each iteration so the model sees mid-turn corrections. Without
    /// this, a user typing "actually, use Python" during a long turn
    /// has to wait for the turn to finish before the correction is
    /// acknowledged.
    pub fn with_pending_messages(
        mut self,
        queue: Arc<Mutex<crate::agent::pending_queue::PendingMessageQueue>>,
    ) -> Self {
        self.pending_user_messages = Some(queue);
        self
    }

    /// Replace the tool lifecycle hook with a custom hook. Allows
    /// blocking or overriding tool execution.
    pub fn with_tool_lifecycle_hook(mut self, hook: Arc<dyn ToolLifecycleHook>) -> Self {
        self.tool_lifecycle_hook = hook;
        self
    }

    /// Replace the API key provider with a custom resolver. Allows
    /// dynamic key refresh (e.g., OAuth token refresh).
    pub fn with_api_key_provider(mut self, provider: Arc<dyn DynamicApiKeyProvider>) -> Self {
        self.api_key_provider = provider;
        self
    }

    /// Set provider-native reasoning settings resolved by the command layer.
    pub fn with_thinking_settings(mut self, settings: Option<ThinkingSettings>) -> Self {
        self.thinking_settings = settings;
        self
    }

    /// Disable tool schemas for conversational turns that do not ask the
    /// assistant to perform an action. This prevents a provider from entering
    /// a tool loop for a greeting or other ordinary chat message.
    pub fn with_tools_enabled(mut self, enabled: bool) -> Self {
        self.tools_enabled = enabled;
        self
    }

    pub fn with_execution_permission(mut self, permission: String) -> Self {
        self.execution_permission = permission;
        self
    }

    /// Set the Tauri event emitter for frontend event emission
    pub fn with_emitter(
        mut self,
        emitter: Arc<dyn crate::agent::event::AgentEventEmitter>,
    ) -> Self {
        self.emitter = Some(emitter);
        self
    }

    /// Bind a runner to the session used by frontend events and run controls.
    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Observe a shared control state at safe execution boundaries.
    pub fn with_control_states(
        mut self,
        control_states: Arc<Mutex<HashMap<String, AgentControlState>>>,
    ) -> Self {
        self.control_states = Some(control_states);
        self
    }

    async fn wait_for_control(&self, session_id: &str) -> Result<(), String> {
        loop {
            let state = self
                .control_states
                .as_ref()
                .and_then(|states| {
                    states
                        .lock()
                        .ok()
                        .and_then(|states| states.get(session_id).copied())
                })
                .unwrap_or(AgentControlState::Running);
            match state {
                AgentControlState::Running => return Ok(()),
                AgentControlState::Paused | AgentControlState::WaitingForInput => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                AgentControlState::Interrupted | AgentControlState::Stopped => {
                    // Propagate cancellation through the tool execution
                    // path so handlers in flight can exit cooperatively
                    // on their next check. The main loop also sees the
                    // error on its next boundary check.
                    self.cancellation.cancel();
                    return Err("运行已由用户停止".to_string());
                }
            }
        }
    }

    /// Test-only helper: drive `wait_for_control` with a fixed session id
    /// without spinning the inner 100ms loop indefinitely. Returns Ok(())
    /// once the runner is allowed to proceed or Err when the user stops.
    /// Used by `control_plane_bridge_tests` to verify that an external
    /// `AgentControlState::Interrupted` reaches the cancellation token.
    #[cfg(test)]
    pub(crate) async fn wait_for_control_for_test(&self, session_id: &str) -> Result<(), String> {
        // Cap the inner spin to 200ms so a stuck test cannot deadlock.
        for _ in 0..2 {
            let state = self
                .control_states
                .as_ref()
                .and_then(|states| {
                    states
                        .lock()
                        .ok()
                        .and_then(|states| states.get(session_id).copied())
                })
                .unwrap_or(AgentControlState::Running);
            match state {
                AgentControlState::Running => return Ok(()),
                AgentControlState::Paused | AgentControlState::WaitingForInput => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                AgentControlState::Interrupted | AgentControlState::Stopped => {
                    self.cancellation.cancel();
                    return Err("运行已由用户停止".to_string());
                }
            }
        }
        Err("test helper timed out".to_string())
    }

    /// Set the work directory for tool operations.
    pub fn set_work_dir(&mut self, path: std::path::PathBuf) {
        self.work_dir = path;
    }

    /// Emit an event via the configured Tauri emitter (if any). The
    /// in-memory `EventChannel` path was removed in #101 — the
    /// frontend only consumes the durable `TauriEventEmitter` and the
    /// optional `DurableEventEmitter` writes to `agent_run_events`.
    fn emit(&self, event: AgentEvent) {
        if let Some(emitter) = &self.emitter {
            let session_id = self.session_id.as_deref().unwrap_or("");
            emitter.emit(session_id, event);
        }
    }

    /// Get a mutable reference to the tool registry for handler registration.
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    pub fn tool_registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tool_registry
    }

    fn requires_confirmation(&self, tool_name: &str, arguments: &serde_json::Value) -> bool {
        // Confirmation is an execution-context decision, never a model
        // argument. The approval command replays a saved step directly through
        // the registry, so a provider-supplied `confirmed` flag must not bypass
        // this gate.
        if requires_immediate_confirmation(tool_name) {
            return true;
        }
        match self.execution_permission.as_str() {
            // Full access applies to every tool already admitted to this
            // session's surface. MCP admission is still independently limited
            // to explicitly enabled servers in the current workspace.
            "full_access" => return false,
            // Workspace auto applies to write_file and MCP tools whose server
            // has been explicitly enabled for this workspace. Other file
            // operations retain their confirmation marker.
            // This keeps confirmation semantics aligned with the user-selected
            // execution mode rather than treating MCP as a separate, hidden
            // fourth permission level.
            "workspace_auto" if tool_name == "write_file" || tool_name.starts_with("mcp_") => {
                return false
            }
            _ => {}
        }
        if tool_name == "write_file" {
            return true;
        }
        if tool_name == "observe_trusted_app_window"
            && arguments.get("mode").and_then(serde_json::Value::as_str) == Some("image")
        {
            // Scope admission does not replace permission to disclose window
            // pixels to the current model. Controls remain read-only metadata;
            // image mode uses the existing normal operation approval flow.
            return true;
        }
        self.tool_registry
            .get(tool_name)
            .map(|tool| tool.requires_confirmation)
            .unwrap_or(false)
    }

    /// Run the agent with the given user message and conversation history.
    /// Returns the final text response along with any tool call steps that were executed.
    pub async fn run(&self, user_message: &str, history: &[(String, String)]) -> AgentResponse {
        let initial_messages = self.build_initial_messages(user_message, history);
        self.run_with_initial_messages(user_message, initial_messages)
            .await
    }

    async fn run_with_initial_messages(
        &self,
        user_message: &str,
        initial_messages: Vec<LlmMessage>,
    ) -> AgentResponse {
        self.run_with_initial_messages_collect(user_message, initial_messages, &mut Vec::new())
            .await
    }

    async fn run_with_initial_messages_collect(
        &self,
        user_message: &str,
        initial_messages: Vec<LlmMessage>,
        transcript: &mut Vec<LlmMessage>,
    ) -> AgentResponse {
        let session_id = self
            .session_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let turn_id = Uuid::new_v4().to_string();
        let message_id = Uuid::new_v4().to_string();

        // Emit AgentStart and TurnStart events
        self.emit(AgentEvent::AgentStart {
            session_id: session_id.clone(),
            timestamp: Utc::now().timestamp(),
        });
        self.emit(AgentEvent::TurnStart {
            turn_id: turn_id.clone(),
            session_id: session_id.clone(),
            message: user_message.to_string(),
        });

        let mut messages = initial_messages;
        let tool_schemas = if self.tools_enabled {
            self.get_tool_schemas()
        } else {
            Vec::new()
        };

        let mut steps: Vec<AgentStep> = Vec::new();
        let mut total_tool_calls: usize = 0;
        // A few non-conformant OpenAI-compatible gateways occasionally emit a
        // textual `<function_result>` instead of a real tool call after a
        // tool-result turn. Give them one corrective retry, without exposing
        // the protocol fragment as the assistant's final reply.
        let mut malformed_protocol_repairs: usize = 0;
        // One model correction is enough to prevent an early summary after a
        // coding change. A second attempt without verification becomes a
        // visible, resumable attention state instead of a hidden loop.
        let mut completion_repairs: usize = 0;
        // Keep the immediately previous batch so duplicate side effects are
        // never executed again. A duplicate is *not* a terminal condition:
        // the model receives its existing results and is allowed to revise
        // its plan. The no-progress guard owns any eventual soft pause.
        let mut last_tool_batch_signature: Option<String> = None;
        // Per-iteration guard accounting. Reset each `AgentRunner::run` so
        // counters never leak across turns.
        let mut side_effect_count: usize = 0;
        let mut no_progress_streak: usize = 0;
        let mut last_response_was_progress = true;
        let turn_wall_clock_start = std::time::Instant::now();
        let recovery_config = &self.config.tool_error_recovery;
        let include_hint = recovery_config.include_recovery_hints;

        for iteration in 0.. {
            // ── Safety bounds ──
            //
            // Pi-style: termination is driven by `LoopGuard`s
            // (`WallClockGuard`, `NetProgressGuard`,
            // `SideEffectAutoPauseGuard`) instead of a fixed iteration
            // cap. Long-running coding / exploration tasks routinely
            // exceed a few dozen tool calls; capping them silently
            // truncates progress. The wall-clock guard remains the
            // hard terminator — every guard is evaluated below before
            // any LLM call.

            // Soft-termination guards. `PauseForUser` keeps the run open
            // (no `AgentEnd`); `Terminate` hard-stops with the guard's
            // branch kind. Both preserve every recorded step for audit.
            let guard_ctx = crate::agent::guards::GuardContext {
                iteration,
                turn_wall_clock_elapsed: turn_wall_clock_start.elapsed(),
                side_effect_count,
                no_progress_streak,
                last_response_was_progress,
            };
            for guard in &self.guards {
                match guard.evaluate(&guard_ctx) {
                    crate::agent::guards::GuardVerdict::Continue => continue,
                    crate::agent::guards::GuardVerdict::PauseForUser { reason } => {
                        self.emit_turn_end(&turn_id, total_tool_calls);
                        return AgentResponse::TextWithToolCalls {
                            text: format!("已暂停，等待用户介入（{}）。", reason),
                            steps,
                        };
                    }
                    crate::agent::guards::GuardVerdict::Terminate { reason, branch } => {
                        self.notify_terminal_branch(
                            branch,
                            iteration,
                            &messages,
                            &steps,
                            total_tool_calls,
                        );
                        self.emit_turn_end(&turn_id, total_tool_calls);
                        self.emit_agent_end(
                            &session_id,
                            iteration + 1,
                            total_tool_calls,
                            Some(reason.clone()),
                        );
                        return Self::terminal(steps, reason);
                    }
                }
            }
            if let Err(msg) = self.wait_for_control(&session_id).await {
                self.notify_terminal_branch(
                    TerminalBranchKind::LlmRepeatedlyFailed,
                    iteration,
                    &messages,
                    &steps,
                    total_tool_calls,
                );
                self.emit_turn_end(&turn_id, total_tool_calls);
                self.emit_agent_end(&session_id, iteration, total_tool_calls, Some(msg.clone()));
                return AgentResponse::Error(msg);
            }

            // Pre-flight cancellation. The control plane may have cancelled
            // the run between iterations; honour that before any LLM tokens.
            if self.cancellation.is_cancelled() {
                let reason = "运行在 LLM 调用前已被取消。".to_string();
                self.notify_terminal_branch(
                    TerminalBranchKind::LlmRepeatedlyFailed,
                    iteration,
                    &messages,
                    &steps,
                    total_tool_calls,
                );
                self.emit_turn_end(&turn_id, total_tool_calls);
                self.emit_agent_end(
                    &session_id,
                    iteration,
                    total_tool_calls,
                    Some(reason.clone()),
                );
                return AgentResponse::Error(reason);
            }

            // Drain mid-turn user messages. Sits after the cancellation
            // check so a cancelled run never silently consumes queued
            // corrections.
            if let Some(queue) = &self.pending_user_messages {
                let drained = queue.lock().unwrap().drain();
                for pending in drained {
                    steps.push(AgentStep {
                        call_id: format!("pending-{}", iteration),
                        tool_name: "pending_user_message".to_string(),
                        arguments: serde_json::json!({
                            "content": pending.content,
                        }),
                        success: true,
                        output: "已并入下一轮 LLM 调用".to_string(),
                        confirmation_required: false,
                        confirmation_status: None,
                        verification: None,
                    });
                    record_private_message(&mut messages, transcript, pending);
                }
            }

            // ── Pi: transformContext hook ──
            // `usize::MAX` means "no budget" — the previous `0` was a silent
            // bug for any hook that read `max_iterations`.
            let ctx = TurnContext::new(
                iteration,
                usize::MAX,
                messages.clone(),
                steps.clone(),
                total_tool_calls,
            );
            if let TransformResult::Transformed(msgs) = self.lifecycle_hook.transform_context(&ctx)
            {
                messages = msgs;
            }

            // ForegroundConversationState owns context preparation and reports
            // its decisions. The runner remains responsible for presenting
            // those decisions in the existing auditable step stream.
            let prepared_context = if messages
                .iter()
                .any(|message| message.protocol_state.is_some() || !message.tool_images.is_empty())
            {
                // Text-only compaction cannot rewrite an opaque Responses
                // transcript. Keep its item order and tool-call grouping intact.
                crate::agent::foreground_conversation_state::PreparedForegroundContext {
                    messages: std::mem::take(&mut messages),
                    task_facts: ForegroundConversationState::select_task_facts(
                        user_message,
                        &steps,
                    ),
                    decisions: Vec::new(),
                }
            } else {
                ForegroundConversationState::prepare_next_turn(
                    user_message,
                    std::mem::take(&mut messages),
                    &steps,
                    self.compaction_hook.as_ref(),
                )
                .await
            };
            messages = prepared_context.messages;
            for decision in prepared_context.decisions {
                match decision {
                    ForegroundContextDecision::Compacted {
                        before_tokens,
                        after_tokens,
                    } => steps.push(AgentStep {
                    call_id: format!("context-compaction-{iteration}"),
                    tool_name: "context_compaction".to_string(),
                    arguments: serde_json::json!({
                        "before_tokens": before_tokens,
                        "after_tokens": after_tokens,
                    }),
                    success: true,
                    output: format!(
                        "Context compacted before LLM call ({before_tokens} -> {after_tokens} estimated tokens)."
                    ),
                    confirmation_required: false,
                    confirmation_status: None,
                    verification: None,
                }),
                    ForegroundContextDecision::CompactionFailed {
                        before_tokens,
                        reason,
                    } => steps.push(AgentStep {
                    call_id: format!("context-compaction-{iteration}"),
                    tool_name: "context_compaction".to_string(),
                    arguments: serde_json::json!({ "before_tokens": before_tokens }),
                    success: false,
                    output: format!("Context compaction failed: {reason}"),
                    confirmation_required: false,
                    confirmation_status: None,
                    verification: None,
                }),
                }
            }

            // Observational hook. Branching stays with the runner.
            // Context decisions are projected into the foreground audit trace
            // before selecting facts, preserving the previous ordering (a
            // failed compaction remains visible to lifecycle observers).
            let task_facts = ForegroundConversationState::select_task_facts(user_message, &steps);
            let _ = self.lifecycle_hook.before_llm_call(
                &LifecycleContext::before_llm(
                    iteration,
                    usize::MAX,
                    messages.clone(),
                    steps.clone(),
                    total_tool_calls,
                )
                .with_task_facts(task_facts),
            );
            let mut turn_state = TurnState::new();
            // Mark every block emitted in this iteration with a stable
            // group key so the frontend can fold consecutive tool
            // blocks into one "execution slice". `iteration` is bounded
            // by the wall-clock guard, not a hard cap, so values can
            // grow indefinitely.
            turn_state.iteration_id = iteration as u32;

            // Emit MessageStart for each LLM call
            self.emit(AgentEvent::MessageStart {
                turn_id: turn_id.clone(),
                message_id: message_id.clone(),
            });

            // Text and tool blocks share one runner-wide sequence. The delta
            // receiver is asynchronous, so per-turn counters let narration
            // jump behind a tool in the rendered timeline.
            // Use an explicit `String` sender type so the channel type can be inferred.
            let (delta_sender, mut delta_receiver): (
                tokio::sync::mpsc::UnboundedSender<String>,
                _,
            ) = tokio::sync::mpsc::unbounded_channel();
            let live_emitter = self.emitter.clone();
            let live_session_id = session_id.clone();
            let live_turn_id = turn_id.clone();
            let live_message_id = message_id.clone();
            let live_block_index = self.content_block_index.clone();
            let live_iteration_id: u32 = iteration as u32;
            let delta_task = tokio::spawn(async move {
                while let Some(delta) = delta_receiver.recv().await {
                    if let Some(emitter) = &live_emitter {
                        // Emit the legacy MessageDelta for backward compatibility
                        emitter.emit(
                            &live_session_id,
                            AgentEvent::MessageDelta {
                                turn_id: live_turn_id.clone(),
                                message_id: live_message_id.clone(),
                                delta: delta.clone(),
                            },
                        );
                        // Emit the new unified ContentBlock::Text with a sequential index
                        let index = live_block_index.fetch_add(1, Ordering::SeqCst) + 1;
                        emitter.emit(
                            &live_session_id,
                            AgentEvent::ContentBlock {
                                turn_id: live_turn_id.clone(),
                                message_id: live_message_id.clone(),
                                index,
                                iteration_id: live_iteration_id,
                                block: ContentBlockData::Text { content: delta },
                            },
                        );
                    }
                }
            });
            let execution_kernel = ExecutionKernel::new(
                self.cancellation.clone(),
                Duration::from_secs(ITERATION_TIMEOUT_SECS),
                MAX_LLM_ATTEMPTS_PER_ITERATION,
            );
            let chat_result = match execution_kernel
                .run_model_turn(
                    self.provider.as_ref(),
                    &messages,
                    (!tool_schemas.is_empty()).then_some(tool_schemas.as_slice()),
                    self.thinking_settings.as_ref(),
                    Some(delta_sender.clone()),
                )
                .await
            {
                Ok(response) => Ok(response),
                Err(KernelModelTurnError::Provider(error)) => Err(error),
                Err(KernelModelTurnError::TimedOut { attempts, timeout }) => {
                    self.notify_terminal_branch(
                        TerminalBranchKind::LlmRepeatedlyFailed,
                        iteration,
                        &messages,
                        &steps,
                        total_tool_calls,
                    );
                    self.emit(AgentEvent::Error {
                        code: crate::agent::event::ErrorCode::Timeout,
                        message: format!(
                            "LLM 调用连续 {} 次超时（每次 {} 秒）",
                            attempts,
                            timeout.as_secs()
                        ),
                        recoverable: true,
                        turn_id: Some(turn_id.clone()),
                    });
                    self.close_turn_and_end(
                        &turn_id,
                        &session_id,
                        total_tool_calls,
                        iteration,
                        Some(format!(
                            "模型连续 {} 次未在 {} 秒内响应",
                            attempts,
                            timeout.as_secs()
                        )),
                    );
                    return Self::protocol_violation_slice_response(
                        steps,
                        format!(
                            "模型连续 {} 次未在 {} 秒内响应。此前已完成的工具结果已保留；请继续任务以从已知事实恢复。",
                            attempts,
                            timeout.as_secs()
                        ),
                    );
                }
            };
            // The provider has returned, so no more deltas can be produced.
            // Close and drain the receiver before emitting ToolCallStart. This
            // preserves the model's visible "text → tool" chronology even
            // when the receiver was briefly scheduled behind the main loop.
            drop(delta_sender);
            let _ = delta_task.await;

            match chat_result {
                Ok(response) => {
                    // A successful model round consumes its transient observations.
                    // A failed request keeps them available only for this run's retry.
                    for message in &mut messages {
                        if !message.tool_images.is_empty() {
                            message.tool_images.clear();
                            message
                                .content
                                .push_str("\n[图像载荷已释放；需要新状态时请重新观察。]");
                        }
                    }
                    // Observational hook. Branching stays with the runner.
                    let task_facts = task_facts_from_steps(user_message, &steps);
                    let _ = self.lifecycle_hook.after_llm_response(
                        &LifecycleContext::after(
                            iteration,
                            0,
                            messages.clone(),
                            steps.clone(),
                            total_tool_calls,
                            ResponseKind::classify(&response),
                        )
                        .with_task_facts(task_facts),
                    );
                    let (response, protocol_state) = response.into_parts();
                    if protocol_state
                        .as_ref()
                        .is_some_and(|state| state.validate().is_err())
                    {
                        return self.protocol_violation(
                            steps,
                            "Invalid private model continuation".into(),
                            iteration,
                            total_tool_calls,
                            &turn_id,
                            &session_id,
                        );
                    }
                    match response {
                        LlmResponse::Text {
                            text,
                            stop_reason: _,
                        } => {
                            record_private_message(
                                &mut messages,
                                transcript,
                                LlmMessage {
                                    role: "assistant".into(),
                                    content: text.clone(),
                                    tool_calls: None,
                                    tool_call_id: None,
                                    tool_images: Vec::new(),
                                    protocol_state,
                                },
                            );
                            if self.tools_enabled
                                && malformed_protocol_repairs == 0
                                && is_malformed_tool_protocol_response(&text)
                            {
                                malformed_protocol_repairs += 1;
                                record_private_message(&mut messages, transcript, LlmMessage {
                                    role: "user".to_string(),
                                    content: "System correction: the previous response used a textual function-result tag, which is not a valid final answer or tool call. Continue the original task. Use the API tool-call protocol for any required action; do not output function_result/function_calls tags as chat text.".to_string(),
                                    tool_calls: None,
                                    tool_call_id: None,
                                    tool_images: Vec::new(),
                                    protocol_state: None,
                                });
                                continue;
                            }
                            if self.tools_enabled
                                && coding_task_has_unverified_changes(user_message, &steps)
                            {
                                if completion_repairs == 0 {
                                    completion_repairs += 1;
                                    // Issue #102: surface post-call verifier
                                    // mismatches so the agent knows the disk
                                    // disagrees with the tool's claim. This
                                    // is advisory — the completion gate
                                    // remains a soft repair, not a hard
                                    // block, in line with §1 user-sovereignty.
                                    let unverified = collect_unverified_artifacts(&steps);
                                    let verification_hint = if unverified.is_empty() {
                                        String::new()
                                    } else {
                                        format!(
                                            "\n\nDisk verification mismatches so far:\n- {}\n\
                                             If a claim is wrong, the tool's success report did not\n\
                                             match the file system. Re-run the file operation or\n\
                                             re-read the artifact with `read_file` to confirm.",
                                            unverified.join("\n- ")
                                        )
                                    };
                                    record_private_message(&mut messages, transcript, LlmMessage {
                                        role: "user".to_string(),
                                        content: format!(
                                            "System completion gate: this coding task changed project files but has no successful verification command after the latest change. Do not summarize yet. Run the smallest appropriate project verification command. If it fails, make at most one targeted repair and verify again; otherwise explain the concrete blocker.{verification_hint}"
                                        ),
                                        tool_calls: None,
                                        tool_call_id: None,
                                        tool_images: Vec::new(),
                                        protocol_state: None,
                                    });
                                    continue;
                                }
                                self.notify_terminal_branch(
                                    TerminalBranchKind::CodingCompletionBlocked,
                                    iteration,
                                    &messages,
                                    &steps,
                                    total_tool_calls,
                                );
                                self.close_turn_and_end(
                                    &turn_id,
                                    &session_id,
                                    total_tool_calls,
                                    iteration,
                                    Some("代码修改后尚无成功的验证证据".to_string()),
                                );
                                let response = Self::completion_gate_handoff_response(steps, text);
                                let visible_text = match &response {
                                    AgentResponse::TextWithToolCalls { text, .. }
                                    | AgentResponse::Text(text) => text.clone(),
                                    AgentResponse::Error(_) => unreachable!(
                                        "completion-gate handoff always has auditable steps"
                                    ),
                                };
                                // Emit the same persisted handoff text that we return, so a
                                // streaming view cannot briefly show a different terminal reply.
                                self.emit(AgentEvent::MessageEnd {
                                    turn_id: turn_id.clone(),
                                    message_id: message_id.clone(),
                                    full_text: visible_text,
                                });
                                return response;
                            }
                            self.emit(AgentEvent::MessageEnd {
                                turn_id: turn_id.clone(),
                                message_id: message_id.clone(),
                                full_text: text.clone(),
                            });
                            self.emit(AgentEvent::TurnEnd {
                                turn_id: turn_id.clone(),
                                success: true,
                                tool_calls_count: total_tool_calls,
                            });
                            self.emit(AgentEvent::AgentEnd {
                                session_id: session_id.clone(),
                                summary: None,
                                total_turns: iteration + 1,
                                total_tool_calls,
                            });
                            if steps.is_empty() {
                                return AgentResponse::Text(text);
                            } else {
                                return AgentResponse::TextWithToolCalls { text, steps };
                            }
                        }
                        LlmResponse::ProtocolViolation { raw_text, reason } => {
                            // Protocol violation: stop immediately, no repair loop.
                            return self.protocol_violation(
                                steps,
                                reason,
                                iteration,
                                total_tool_calls,
                                &turn_id,
                                &session_id,
                            );
                        }
                        LlmResponse::ToolCalls {
                            calls: tool_calls,
                            text,
                            stop_reason,
                        } => {
                            // When the provider reports `MaxTokens`, the
                            // tool-call arguments may be incomplete.
                            // Executing them would produce corrupted
                            // side effects that cannot be audited.
                            if stop_reason == crate::llm::StopReason::MaxTokens
                                || (protocol_state.is_some()
                                    && stop_reason != crate::llm::StopReason::ToolUse)
                            {
                                for tc in &tool_calls {
                                    steps.push(AgentStep {
                                        call_id: tc.id.clone(),
                                        tool_name: tc.name.clone(),
                                        arguments: tc.arguments.clone(),
                                        success: false,
                                        output: "模型输出被 token 限制截断；未执行该工具调用。请减少上下文后重试。".to_string(),
                                        confirmation_required: false,
                                        confirmation_status: None,
                                        verification: None,
                                    });
                                }
                                self.emit_turn_end(&turn_id, total_tool_calls);
                                self.emit_agent_end(
                                    &session_id,
                                    iteration + 1,
                                    total_tool_calls,
                                    Some("工具调用前的模型输出已被 token 限制截断。".to_string()),
                                );
                                return Self::terminal_slice_response(
                                    steps,
                                    "工具调用前的模型输出已被 token 限制截断。未执行任何可能不完整的工具调用。请减少上下文长度后重试，或将任务拆分为多个独立步骤。".to_string(),
                                );
                            }
                            // Repeating the same batch without consuming any new evidence is
                            // a common provider failure mode. Never execute duplicate effects,
                            // but do not hard-stop a normal task either: give the model its
                            // prior results and let NetProgressGuard decide when a resumable
                            // pause is warranted.
                            let batch_signature = tool_batch_signature(&tool_calls);
                            if last_tool_batch_signature.as_deref() == Some(&batch_signature) {
                                last_response_was_progress = false;
                                no_progress_streak += 1;
                                if protocol_state.is_some() {
                                    record_private_message(
                                        &mut messages,
                                        transcript,
                                        LlmMessage {
                                            role: "assistant".into(),
                                            content: text.unwrap_or_default(),
                                            tool_calls: Some(tool_calls.clone()),
                                            tool_call_id: None,
                                            tool_images: Vec::new(),
                                            protocol_state,
                                        },
                                    );
                                    for call in &tool_calls {
                                        let output = steps.iter().rev().find(|step| {
                                            step.tool_name == call.name && step.arguments == call.arguments
                                        }).map(|step| step.output.clone()).unwrap_or_else(|| {
                                            "Duplicate batch was not executed; prior result unavailable.".into()
                                        });
                                        record_private_message(
                                            &mut messages,
                                            transcript,
                                            LlmMessage {
                                                role: "tool".into(),
                                                content: output,
                                                tool_calls: None,
                                                tool_call_id: Some(call.id.clone()),
                                                tool_images: Vec::new(),
                                                protocol_state: None,
                                            },
                                        );
                                    }
                                } else {
                                    record_private_message(
                                        &mut messages,
                                        transcript,
                                        LlmMessage {
                                            role: "assistant".to_string(),
                                            content: text.unwrap_or_default(),
                                            tool_calls: None,
                                            tool_call_id: None,
                                            tool_images: Vec::new(),
                                            protocol_state: None,
                                        },
                                    );
                                }
                                record_private_message(&mut messages, transcript, LlmMessage {
                                    role: "user".to_string(),
                                    content: "System loop guard: the previous tool batch is identical to the immediately preceding batch and produced no new evidence. Do not repeat it. Inspect the available results and choose a different minimal next step, request confirmation, or explain the blocker.".to_string(),
                                    tool_calls: None,
                                    tool_call_id: None,
                                    tool_images: Vec::new(),
                                    protocol_state: None,
                                });
                                continue;
                            }
                            last_tool_batch_signature = Some(batch_signature);
                            // This batch will produce fresh tool results below, including
                            // useful error results that let the model change course.
                            last_response_was_progress = true;
                            no_progress_streak = 0;
                            let private_batch = protocol_state.is_some();
                            let batch_message_start = messages.len();
                            let batch_step_start = steps.len();
                            // Add assistant message with tool calls
                            record_private_message(
                                &mut messages,
                                transcript,
                                LlmMessage {
                                    role: "assistant".to_string(),
                                    content: text.clone().unwrap_or_default(),
                                    tool_calls: Some(
                                        tool_calls
                                            .iter()
                                            .map(|tc| LlmToolCall {
                                                id: tc.id.clone(),
                                                name: tc.name.clone(),
                                                arguments: tc.arguments.clone(),
                                            })
                                            .collect(),
                                    ),
                                    tool_call_id: None,
                                    tool_images: Vec::new(),
                                    protocol_state,
                                },
                            );

                            // The foreground host keeps confirmation, audit,
                            // registry dispatch, and ordered settlement. The
                            // shared kernel only observes the bounded batch
                            // lifecycle; its guard balances terminal/recovery
                            // exits as well.
                            let controlled_tool_batch =
                                execution_kernel.begin_controlled_tool_batch();

                            // Prepare model-order call metadata through the foreground
                            // boundary. The existing loop below remains the owner of
                            // UI events/audit settlement while this extraction is
                            // migrated incrementally.
                            let foreground_batch_host =
                                ForegroundToolBatchHost::new(&self.tool_registry);
                            let foreground_batch_plan = foreground_batch_host
                                .prepare(&tool_calls, |tool_name, args| {
                                    self.requires_confirmation(tool_name, args)
                                });
                            let agent_tool_calls: Vec<AgentToolCall> = foreground_batch_plan
                                .dispositions()
                                .iter()
                                .map(|disposition| disposition.call().clone())
                                .collect();

                            // ── Issue #47: Group by execution mode ─────────────────
                            let mut executable_tool_calls: Vec<AgentToolCall> = Vec::new();
                            let mut confirmation_pending = false;
                            for tc in agent_tool_calls {
                                let call_id = tc.id.clone().unwrap_or_default();
                                let tool_name = tc.name.clone();
                                let args = tc.arguments.clone();
                                if let Err(validation_error) =
                                    self.tool_registry.validate_arguments(&tool_name, &args)
                                {
                                    let output = format!(
                                        "Tool call for '{}' was not executed: {}. Provide every required argument and retry the tool call.",
                                        tool_name, validation_error
                                    );
                                    self.emit_tool_start(
                                        &mut turn_state,
                                        &turn_id,
                                        &message_id,
                                        &call_id,
                                        &tool_name,
                                        &args,
                                    );
                                    steps.push(AgentStep {
                                        call_id: call_id.clone(),
                                        tool_name: tool_name.clone(),
                                        arguments: args,
                                        success: false,
                                        output: output.clone(),
                                        confirmation_required: false,
                                        confirmation_status: None,
                                        verification: None,
                                    });
                                    self.emit(AgentEvent::ToolExecutionEnd {
                                        call_id: call_id.clone(),
                                        tool_name: tool_name.clone(),
                                        success: false,
                                        output: output.clone(),
                                        error: Some(output.clone()),
                                    });
                                    self.emit_content_block(
                                        &mut turn_state,
                                        &turn_id,
                                        &message_id,
                                        ContentBlockData::ToolCallFailed {
                                            call_id: call_id.clone(),
                                            tool_name: tool_name.clone(),
                                            error: output.clone(),
                                        },
                                    );
                                    record_private_message(
                                        &mut messages,
                                        transcript,
                                        LlmMessage {
                                            role: "tool".to_string(),
                                            content: format!("InvalidToolArguments: {}", output),
                                            tool_calls: None,
                                            tool_call_id: Some(call_id),
                                            tool_images: Vec::new(),
                                            protocol_state: None,
                                        },
                                    );
                                    total_tool_calls += 1;
                                    continue;
                                }
                                if self.requires_confirmation(&tool_name, &args) {
                                    let output = format!(
                                        "Confirmation required before executing side-effect tool '{}'.",
                                        tool_name
                                    );
                                    self.emit_tool_start(
                                        &mut turn_state,
                                        &turn_id,
                                        &message_id,
                                        &call_id,
                                        &tool_name,
                                        &args,
                                    );
                                    steps.push(AgentStep {
                                        call_id: call_id.clone(),
                                        tool_name: tool_name.clone(),
                                        arguments: args.clone(),
                                        success: false,
                                        output: output.clone(),
                                        confirmation_required: true,
                                        confirmation_status: Some("pending".to_string()),
                                        verification: None,
                                    });
                                    self.emit(AgentEvent::ToolExecutionEnd {
                                        call_id: call_id.clone(),
                                        tool_name: tool_name.clone(),
                                        success: false,
                                        output: output.clone(),
                                        error: Some(output.clone()),
                                    });
                                    self.emit_content_block(
                                        &mut turn_state,
                                        &turn_id,
                                        &message_id,
                                        ContentBlockData::ToolCallNeedsApproval {
                                            call_id: call_id.clone(),
                                            tool_name: tool_name.clone(),
                                            arguments: args,
                                            reason: output.clone(),
                                        },
                                    );
                                    record_private_message(&mut messages, transcript, LlmMessage {
                                        role: "tool".to_string(),
                                        content: format!(
                                            "ConfirmationRequired: {} Ask the user to approve or reject this action before proceeding.",
                                            output
                                        ),
                                        tool_calls: None,
                                        tool_call_id: Some(call_id),
                                        tool_images: Vec::new(),
                                        protocol_state: None,
                                    });
                                    total_tool_calls += 1;
                                    confirmation_pending = true;
                                } else {
                                    executable_tool_calls.push(tc);
                                }
                            }

                            // When the batch contains only confirmation-gated calls, all of
                            // them already have a protocol tool result. End the turn rather
                            // than asking the model to continue before user approval.
                            if confirmation_pending && executable_tool_calls.is_empty() {
                                self.emit(AgentEvent::TurnEnd {
                                    turn_id: turn_id.clone(),
                                    success: true,
                                    tool_calls_count: total_tool_calls,
                                });
                                return AgentResponse::TextWithToolCalls {
                                    text: text.unwrap_or_else(|| {
                                        "该操作需要你的确认后才能继续。".to_string()
                                    }),
                                    steps,
                                };
                            }

                            // Track batch execution state for `terminate_batch`.
                            let mut should_terminate_batch = false;
                            let mut requires_user_review = false;
                            let mut executed_call_ids: HashSet<String> = HashSet::new();
                            let mut skipped_call_ids: HashSet<String> = HashSet::new();

                            let (parallel, sequential) =
                                ForegroundToolBatchHost::split_execution_modes(
                                    &executable_tool_calls,
                                );

                            // ── Execute parallel calls concurrently ─────────────
                            if !parallel.is_empty() {
                                let registry = self.tool_registry.clone();
                                let work_dir = self.work_dir.clone();
                                let tool_lifecycle_hook = self.tool_lifecycle_hook.clone();
                                let session_id = self.session_id.clone();
                                let parallel_iteration = iteration;

                                // Run `before_tool_call` hooks before dispatching parallel tasks.
                                let mut blocked_calls: Vec<(String, String)> = Vec::new(); // (call_id, reason)
                                let parallel_to_execute: Vec<_> = parallel
                                    .into_iter()
                                    .filter_map(|tc| {
                                        let call_id = tc.id.clone().unwrap_or_default();
                                        let tool_name = tc.name.clone();

                                        let hook_ctx = ToolCallContext {
                                            tool_call: tc.clone(),
                                            tool_name: tool_name.clone(),
                                            args: tc.arguments.clone(),
                                            session_id: session_id.clone(),
                                            iteration: parallel_iteration,
                                        };
                                        let before_result =
                                            tool_lifecycle_hook.before_tool_call(&hook_ctx);

                                        if before_result.block {
                                            blocked_calls.push((
                                                call_id.clone(),
                                                before_result
                                                    .reason
                                                    .unwrap_or_else(|| "policy restriction".into()),
                                            ));
                                            self.emit_tool_start(
                                                &mut turn_state,
                                                &turn_id,
                                                &message_id,
                                                &call_id,
                                                &tool_name,
                                                &tc.arguments,
                                            );
                                            steps.push(AgentStep {
                                                call_id: call_id.clone(),
                                                tool_name: tool_name.clone(),
                                                arguments: tc.arguments.clone(),
                                                success: false,
                                                output: format!(
                                                    "Blocked by tool hook: {}",
                                                    blocked_calls
                                                        .last()
                                                        .map(|(_, r)| r.as_str())
                                                        .unwrap_or("policy restriction")
                                                ),
                                                confirmation_required: false,
                                                confirmation_status: None,
                                                verification: None,
                                            });
                                            self.emit(AgentEvent::ToolExecutionEnd {
                                                call_id: call_id.clone(),
                                                tool_name: tool_name.clone(),
                                                success: false,
                                                output: format!(
                                                    "Blocked: {}",
                                                    blocked_calls
                                                        .last()
                                                        .map(|(_, r)| r.as_str())
                                                        .unwrap_or("policy restriction")
                                                ),
                                                error: Some(
                                                    blocked_calls
                                                        .last()
                                                        .map(|(_, r)| r.clone())
                                                        .unwrap_or_default(),
                                                ),
                                            });
                                            self.emit_content_block(
                                                &mut turn_state,
                                                &turn_id,
                                                &message_id,
                                                ContentBlockData::ToolCallFailed {
                                                    call_id: call_id.clone(),
                                                    tool_name: tool_name.clone(),
                                                    error: blocked_calls
                                                        .last()
                                                        .map(|(_, r)| r.clone())
                                                        .unwrap_or_default(),
                                                },
                                            );
                                            None
                                        } else {
                                            Some(tc)
                                        }
                                    })
                                    .collect();

                                // Record blocked calls as skipped
                                for (call_id, _) in &blocked_calls {
                                    skipped_call_ids.insert(call_id.clone());
                                    executed_call_ids.insert(call_id.clone());
                                }

                                let emitter = self.emitter.clone();

                                let mut handles: Vec<_> = parallel_to_execute
                                    .into_iter()
                                    .map(|tc| {
                                        let registry = registry.clone();
                                        let work_dir = work_dir.clone();
                                        let call_id_opt = tc.id.clone();
                                        let tool_name = tc.name.clone();
                                        let args = tc.arguments.clone();
                                        let cancellation = self.cancellation.clone();

                                        self.emit_tool_start(
                                            &mut turn_state,
                                            &turn_id,
                                            &message_id,
                                            &call_id_opt.clone().unwrap_or_default(),
                                            &tool_name,
                                            &args,
                                        );

                                        let emitter = self.emitter.clone();
                                        let session_id = session_id.clone();
                                        let call_id_for_progress = call_id_opt.clone().unwrap_or_default();

                                        let progress_callback: crate::agent::tool::ProgressCallback =
                                            Box::new(move |
                                                tool_name: &str,
                                                progress: &str,
                                                percent: Option<f32>| {
                                                if let Some(emit) = &emitter {
                                                    if let Some(sid) = &session_id {
                                                        emit.emit(
                                                            sid,
                                                            AgentEvent::ToolExecutionProgress {
                                                                call_id: call_id_for_progress.clone(),
                                                                tool_name: tool_name.to_string(),
                                                                progress: progress.to_string(),
                                                                percent,
                                                            },
                                                        );
                                                    }
                                                }
                                            });

                                        let cb = std::sync::Arc::new(Some(progress_callback));

                                        // Tool handlers are synchronous and may wait on OS
                                        // processes. Keep them off Tokio's executor threads.
                                        tokio::task::spawn_blocking(move || {
                                            let call_id_for_result = call_id_opt.clone();
                                            let call_id_str = call_id_opt.as_deref().unwrap_or("");
                                            // Propagate runner cancellation
                                            // through the registry so a
                                            // concurrent interrupt short-circuits
                                            // before the handler runs.
                                            let ctx = ToolExecutionContext::new(
                                                call_id_str,
                                                cancellation.clone(),
                                            )
                                            .with_progress_callback(cb.clone());
                                            let exec_result = registry.execute_with_context(
                                                &tc,
                                                call_id_str,
                                                &work_dir,
                                                &ctx,
                                            );
                                            ctx.settle();
                                            (call_id_for_result, tool_name, args, exec_result, tc)
                                        })
                                    })
                                    .collect();

                                for handle in handles.drain(..) {
                                    let (call_id_for_result, tool_name, args, exec_result, tc) =
                                        handle.await.unwrap_or_else(|_| {
                                            // Panicked task — fabricate error
                                            (
                                                None,
                                                String::new(),
                                                serde_json::Value::Null,
                                                crate::agent::registry::ToolExecResult {
                                                    result: crate::agent::tool::ToolResult::error(
                                                        "unknown",
                                                        "并行任务执行时发生异常",
                                                    ),
                                                    call_id: String::new(),
                                                    terminate_batch: false,
                                                },
                                                AgentToolCall::new(
                                                    String::new(),
                                                    serde_json::Value::Null,
                                                ),
                                            )
                                        });

                                    // `after_tool_call` hook — apply result override via the shared
                                    // helper so parallel and sequential branches stay identical.
                                    let exec_result = self.apply_post_hook(
                                        &tc,
                                        exec_result,
                                        &args,
                                        session_id.as_deref(),
                                        parallel_iteration,
                                    );

                                    requires_user_review |=
                                        exec_result.result.requires_user_review();

                                    let tool_msg = self.record_tool_outcome(
                                        &mut steps,
                                        &mut turn_state,
                                        &turn_id,
                                        &message_id,
                                        &exec_result.call_id,
                                        &tool_name,
                                        args,
                                        &exec_result,
                                        include_hint,
                                    );

                                    if exec_result.terminate_batch {
                                        should_terminate_batch = true;
                                    }
                                    executed_call_ids.insert(exec_result.call_id.clone());
                                    skipped_call_ids
                                        .insert(call_id_for_result.clone().unwrap_or_default());

                                    record_private_message(&mut messages, transcript, tool_msg);
                                    total_tool_calls += 1;
                                    // Count successful side-effect calls for SideEffectAutoPauseGuard.
                                    // The registry's predicate is the single source of truth
                                    // so the guard cannot drift from the confirmation list.
                                    if exec_result.result.success
                                        && crate::agent::guards::is_side_effect(&tool_name)
                                    {
                                        side_effect_count += 1;
                                    }
                                }
                            }

                            // If a parallel tool set terminate_batch, skip the rest.
                            if should_terminate_batch {
                                for tc in sequential.iter() {
                                    let call_id = tc.id.clone().unwrap_or_default();
                                    if executed_call_ids.contains(&call_id) {
                                        continue;
                                    }
                                    executed_call_ids.insert(call_id.clone());
                                    skipped_call_ids.insert(call_id.clone());
                                    steps.push(AgentStep {
                                        call_id,
                                        tool_name: tc.name.clone(),
                                        arguments: tc.arguments.clone(),
                                        success: false,
                                        output:
                                            "因批内前置工具返回 terminate_batch，本工具调用已跳过。"
                                                .to_string(),
                                        confirmation_required: false,
                                        confirmation_status: None,
                                        verification: None,
                                    });
                                }
                            }

                            // ── Execute sequential calls in order ──────────────
                            for tc in &sequential {
                                let call_id = tc.id.clone().unwrap_or_default();
                                let tool_name = tc.name.clone();
                                let args = tc.arguments.clone();

                                // Skip if a prior tool terminated the batch.
                                if should_terminate_batch {
                                    // Guard against the double-write that would
                                    // create duplicate agent_steps rows.
                                    if skipped_call_ids.contains(&call_id) {
                                        continue;
                                    }
                                    skipped_call_ids.insert(call_id.clone());
                                    executed_call_ids.insert(call_id.clone());
                                    steps.push(AgentStep {
                                        call_id: call_id.clone(),
                                        tool_name: tc.name.clone(),
                                        arguments: args,
                                        success: false,
                                        output:
                                            "因批内前置工具返回 terminate_batch，本工具调用已跳过。"
                                                .to_string(),
                                        confirmation_required: false,
                                        confirmation_status: None,
                                        verification: None,
                                    });
                                    continue;
                                }

                                self.emit_tool_start(
                                    &mut turn_state,
                                    &turn_id,
                                    &message_id,
                                    &call_id,
                                    &tool_name,
                                    &args,
                                );

                                // `before_tool_call` hook.
                                let hook_ctx = ToolCallContext {
                                    tool_call: tc.clone(),
                                    tool_name: tool_name.clone(),
                                    args: args.clone(),
                                    session_id: self.session_id.clone(),
                                    iteration,
                                };
                                let before_result =
                                    self.tool_lifecycle_hook.before_tool_call(&hook_ctx);

                                let exec_result = if before_result.block {
                                    // Blocked by hook — return error result
                                    skipped_call_ids.insert(call_id.clone());
                                    crate::agent::registry::ToolExecResult {
                                        result: crate::agent::tool::ToolResult {
                                            name: tool_name.clone(),
                                            success: false,
                                            content: format!(
                                                "Blocked by tool hook: {}",
                                                before_result
                                                    .reason
                                                    .as_deref()
                                                    .unwrap_or("policy restriction")
                                            ),
                                            terminate_batch: false,
                                            images: Vec::new(),
                                        },
                                        call_id: call_id.clone(),
                                        terminate_batch: false,
                                    }
                                } else {
                                    // Propagate runner cancellation so an
                                    // Interrupted/Stopped control state
                                    // short-circuits before the handler runs.
                                    let call_id_str = call_id.clone();
                                    let progress_session_id = session_id.clone();
                                    let emitter_opt = self.emitter.clone();
                                    let cb: crate::agent::tool::ProgressCallback = Box::new(
                                        move |tool_name: &str, progress: &str, percent: Option<f32>| {
                                            if let Some(emit) = &emitter_opt {
                                                emit.emit(
                                                    &progress_session_id,
                                                    AgentEvent::ToolExecutionProgress {
                                                        call_id: call_id_str.clone(),
                                                        tool_name: tool_name.to_string(),
                                                        progress: progress.to_string(),
                                                        percent,
                                                    },
                                                );
                                            }
                                        },
                                    );
                                    let ctx = ToolExecutionContext::new(
                                        &call_id,
                                        self.cancellation.clone(),
                                    )
                                    .with_progress_callback(std::sync::Arc::new(Some(cb)));
                                    let registry = self.tool_registry.clone();
                                    let tc_for_execution = tc.clone();
                                    let work_dir = self.work_dir.clone();
                                    let call_id_for_execution = call_id.clone();
                                    let ctx_for_execution = ctx.clone();
                                    let result = tokio::task::spawn_blocking(move || {
                                        registry.execute_with_context(
                                            &tc_for_execution,
                                            &call_id_for_execution,
                                            &work_dir,
                                            &ctx_for_execution,
                                        )
                                    })
                                    .await
                                    .unwrap_or_else(|_| crate::agent::registry::ToolExecResult {
                                        result: crate::agent::tool::ToolResult::error(
                                            &tool_name,
                                            "工具执行时发生异常",
                                        ),
                                        call_id: call_id.clone(),
                                        terminate_batch: false,
                                    });
                                    ctx.settle();
                                    result
                                };

                                // `after_tool_call` hook — apply result override via shared helper.
                                let exec_result = self.apply_post_hook(
                                    tc,
                                    exec_result,
                                    &args,
                                    self.session_id.as_deref(),
                                    iteration,
                                );

                                requires_user_review |= exec_result.result.requires_user_review();

                                let tool_msg = self.record_tool_outcome(
                                    &mut steps,
                                    &mut turn_state,
                                    &turn_id,
                                    &message_id,
                                    &call_id,
                                    &tool_name,
                                    args,
                                    &exec_result,
                                    include_hint,
                                );

                                // Record batch-termination signal.
                                if exec_result.terminate_batch {
                                    should_terminate_batch = true;
                                }
                                executed_call_ids.insert(call_id.clone());

                                record_private_message(&mut messages, transcript, tool_msg);
                                total_tool_calls += 1;
                                // Count successful side-effect calls.
                                if exec_result.result.success
                                    && crate::agent::guards::is_side_effect(&tool_name)
                                {
                                    side_effect_count += 1;
                                }

                                // Record failure and decide next action
                                if !exec_result.result.success {
                                    turn_state
                                        .record_failure(recovery_config.max_failures_per_turn);
                                    if turn_state.should_abort && !confirmation_pending {
                                        self.emit(AgentEvent::Error {
                                            code: crate::agent::event::ErrorCode::MaxIterationsReached,
                                            message: format!(
                                                "工具失败次数过多（{}次），终止此轮",
                                                recovery_config.max_failures_per_turn
                                            ),
                                            recoverable: false,
                                            turn_id: Some(turn_id.clone()),
                                        });
                                        // Append error summary to last assistant message and break
                                        record_private_message(&mut messages, transcript, LlmMessage {
                                            role: "user".to_string(),
                                            content: format!(
                                                "[系统] 工具调用失败次数已达上限（{}次），请向用户说明当前状况并停止尝试。",
                                                recovery_config.max_failures_per_turn
                                            ),
                                            tool_calls: None,
                                            tool_call_id: None,
                                            tool_images: Vec::new(),
                                            protocol_state: None,
                                        });
                                        // Observational hook on a terminal-feeling branch.
                                        // The runner keeps the existing `break`; the hook is observational.
                                        self.notify_terminal_branch(
                                            TerminalBranchKind::TooManyToolFailures,
                                            iteration,
                                            &messages,
                                            &steps,
                                            total_tool_calls,
                                        );
                                        break;
                                    }
                                    // Allow LLM to retry or choose another tool — do not break here
                                }
                            }

                            if private_batch {
                                // A completed native output is indivisible. A
                                // blocked/skipped call still needs an honest
                                // receipt; never drop its opaque call item or
                                // run a skipped operation merely to fill a gap.
                                for call in &tool_calls {
                                    if messages[batch_message_start..].iter().any(|message| {
                                        message.role == "tool"
                                            && message.tool_call_id.as_deref() == Some(&call.id)
                                    }) {
                                        continue;
                                    }
                                    let output = if let Some(step) =
                                        steps[batch_step_start..].iter().find(|step| {
                                            step.call_id == call.id && step.tool_name == call.name
                                        }) {
                                        step.output.clone()
                                    } else {
                                        let output = "Skipped: the tool batch stopped before this operation ran.".to_string();
                                        steps.push(AgentStep {
                                            call_id: call.id.clone(),
                                            tool_name: call.name.clone(),
                                            arguments: call.arguments.clone(),
                                            success: false,
                                            output: output.clone(),
                                            confirmation_required: false,
                                            confirmation_status: None,
                                            verification: None,
                                        });
                                        output
                                    };
                                    record_private_message(
                                        &mut messages,
                                        transcript,
                                        LlmMessage {
                                            role: "tool".into(),
                                            content: output,
                                            tool_calls: None,
                                            tool_call_id: Some(call.id.clone()),
                                            tool_images: Vec::new(),
                                            protocol_state: None,
                                        },
                                    );
                                }
                            }
                            // Observational hook for tool-batch completion.
                            let task_facts = task_facts_from_steps(user_message, &steps);
                            let _ = self.lifecycle_hook.after_tool_batch(
                                &LifecycleContext::after(
                                    iteration,
                                    0,
                                    messages.clone(),
                                    steps.clone(),
                                    total_tool_calls,
                                    ResponseKind::ToolCalls,
                                )
                                .with_task_facts(task_facts),
                            );
                            controlled_tool_batch.complete();
                            // An interrupted side effect may already have changed its
                            // target. Do not ask the model for another turn that could
                            // repeat it; the user must inspect the real state first.
                            if requires_user_review {
                                self.emit_turn_end(&turn_id, total_tool_calls);
                                return AgentResponse::TextWithToolCalls {
                                    text: "操作结果尚未确认，目标内容可能已经改变。请先检查实际状态，再决定是否重试。".to_string(),
                                    steps,
                                };
                            }
                            // Pi: prepareNextTurn + shouldStopAfterTurn hooks
                            let turn_ctx = TurnContext::new(
                                iteration,
                                usize::MAX,
                                messages.clone(),
                                steps.clone(),
                                total_tool_calls,
                            )
                            .with_response_shape(ResponseKind::ToolCalls);
                            if let Some(snap) = self.lifecycle_hook.prepare_next_turn(&turn_ctx) {
                                // Record the snapshot length so auditors can see
                                // the hook fired without diffing the entire message
                                // list by hand.
                                let model_changed =
                                    snap.model.as_ref().is_some_and(|m| !m.is_empty())
                                        && self.provider.set_model(snap.model.as_ref().unwrap());
                                steps.push(AgentStep {
                                    call_id: format!("prepare_next_turn-{}", iteration),
                                    tool_name: "prepare_next_turn".to_string(),
                                    arguments: serde_json::json!({
                                        "iteration": iteration,
                                        "rewritten_message_count": snap.messages.len(),
                                        "model": snap.model,
                                    }),
                                    success: true,
                                    output: if model_changed {
                                        format!(
                                            "lifecycle hook 准备了下一轮上下文（{} 条消息），model 已切换至 {}。",
                                            snap.messages.len(),
                                            snap.model.as_ref().unwrap()
                                        )
                                    } else {
                                        format!(
                                            "lifecycle hook 准备了下一轮上下文（{} 条消息）。",
                                            snap.messages.len()
                                        )
                                    },
                                    confirmation_required: false,
                                    confirmation_status: None,
                                    verification: None,
                                });
                                messages = snap.messages;
                            }
                            match self.lifecycle_hook.should_stop_after_turn(&turn_ctx) {
                                LoopDecision::Stop => {
                                    self.close_turn_and_end(
                                        &turn_id,
                                        &session_id,
                                        total_tool_calls,
                                        iteration + 1,
                                        Some("由 hook 决定终止".to_string()),
                                    );
                                    return Self::terminal_slice_response(
                                        steps,
                                        "由 hook 决定终止".into(),
                                    );
                                }
                                LoopDecision::PauseForUser { reason } => {
                                    self.emit_turn_end(&turn_id, total_tool_calls);
                                    return AgentResponse::TextWithToolCalls {
                                        text: format!("已暂停，等待用户介入（{}）。", reason),
                                        steps,
                                    };
                                }
                                LoopDecision::Continue => {}
                            }
                            // A tool call is a protocol turn: every call in the assistant
                            // message must be answered before another request is sent. The
                            // read-only calls above have now produced their tool results; stop
                            // before another model request while a side effect awaits approval.
                            if confirmation_pending {
                                self.emit(AgentEvent::TurnEnd {
                                    turn_id: turn_id.clone(),
                                    success: true,
                                    tool_calls_count: total_tool_calls,
                                });
                                return AgentResponse::TextWithToolCalls {
                                    text: text.unwrap_or_else(|| {
                                        "该操作需要你的确认后才能继续。".to_string()
                                    }),
                                    steps,
                                };
                            }
                        }
                        LlmResponse::WithProtocol { .. } => {
                            unreachable!("protocol wrappers are flattened by into_parts")
                        }
                    }
                }
                Err(e) => {
                    self.notify_terminal_branch(
                        TerminalBranchKind::LlmRepeatedlyFailed,
                        iteration,
                        &messages,
                        &steps,
                        total_tool_calls,
                    );
                    self.emit(AgentEvent::Error {
                        code: crate::agent::event::ErrorCode::LlmError,
                        message: e.clone(),
                        recoverable: true,
                        turn_id: Some(turn_id.clone()),
                    });
                    self.close_turn_and_end(
                        &turn_id,
                        &session_id,
                        total_tool_calls,
                        iteration,
                        Some(format!("模型请求未完成：{}", e)),
                    );
                    return Self::terminal_slice_response(
                        steps,
                        format!(
                            "模型请求未完成：{}。此前已完成的工具结果已保留；请继续任务以从已知事实恢复。",
                            e
                        ),
                    );
                }
            }
        }
        unreachable!("Agent loop exited without return");
    }

    /// Get a reference to the agent configuration
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Build the initial messages including personalized system prompt and history
    fn build_initial_messages(
        &self,
        user_message: &str,
        history: &[(String, String)],
    ) -> Vec<LlmMessage> {
        let tool_schemas = self.get_tool_schemas();
        let snippets = self.tool_registry().get_prompt_snippets();
        ForegroundConversationState::initial_context(
            &self.config,
            &tool_schemas,
            &snippets,
            user_message,
            ForegroundHistory::Flat(history),
        )
    }

    /// Build the first provider request without flattening tool-call protocol
    /// fields from persisted session history.
    fn build_initial_messages_with_native_history(
        &self,
        user_message: &str,
        history: &[LlmMessage],
    ) -> Vec<LlmMessage> {
        let tool_schemas = self.get_tool_schemas();
        let snippets = self.tool_registry().get_prompt_snippets();
        ForegroundConversationState::initial_context(
            &self.config,
            &tool_schemas,
            &snippets,
            user_message,
            ForegroundHistory::Native(history),
        )
    }

    /// Get tool schemas in unified format.
    /// Uses enhanced descriptions when `config.enhanced_tool_descriptions` is true (Issue #52).
    fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        self.tool_registry
            .get_schemas_with_enhanced(self.config.enhanced_tool_descriptions)
            .into_iter()
            .map(|v| {
                let obj = v.get("function").unwrap_or(&v);
                ToolSchema {
                    name: obj["name"].as_str().unwrap_or("").to_string(),
                    description: obj["description"].as_str().unwrap_or("").to_string(),
                    parameters: obj["parameters"].clone(),
                }
            })
            .collect()
    }

    /// Execute a tool by name, within the current work directory sandbox
    fn execute_tool(
        &self,
        name: &str,
        arguments: &serde_json::Value,
        call_id: &str,
    ) -> crate::agent::registry::ToolExecResult {
        let tool_call = AgentToolCall {
            name: name.to_string(),
            arguments: arguments.clone(),
            id: Some(call_id.to_string()),
            execution_mode: ToolExecutionMode::default(),
        };
        self.tool_registry
            .execute(&tool_call, call_id, &self.work_dir)
    }
}

/// Detect only protocol-looking output that a model should have returned via
/// the API tool-call field. This never executes the text; it merely enables a
/// single corrective model retry inside the existing iteration limit.
fn is_malformed_tool_protocol_response(text: &str) -> bool {
    let normalized = text.trim().to_ascii_lowercase();
    normalized.contains("<function_result") && normalized.contains("action:")
}

#[cfg(test)]
mod repeated_tool_batch_tests {
    //! Regression coverage for providers that re-propose an already completed
    //! tool batch instead of consuming the tool result in their next turn.

    use super::*;
    use crate::agent::{Tool, ToolHandler, ToolResult};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct RepeatingBatchProvider {
        calls: Arc<AtomicUsize>,
        saw_guard_feedback: Arc<AtomicUsize>,
        tool_call_limit: usize,
    }

    #[async_trait::async_trait]
    impl LlmProvider for RepeatingBatchProvider {
        fn name(&self) -> &str {
            "repeating-batch-mock"
        }

        async fn chat(
            &self,
            messages: &[LlmMessage],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
            if messages.iter().any(|message| {
                message.role == "user" && message.content.contains("System loop guard")
            }) {
                self.saw_guard_feedback.fetch_add(1, Ordering::SeqCst);
            }

            if call_index < self.tool_call_limit {
                return Ok(LlmResponse::ToolCalls {
                    calls: vec![LlmToolCall {
                        id: format!("duplicate-{call_index}"),
                        name: "read_file".to_string(),
                        arguments: serde_json::json!({"path": "status.txt"}),
                    }],
                    text: Some("inspect status".to_string()),
                    stop_reason: crate::llm::StopReason::ToolUse,
                });
            }

            Ok(LlmResponse::Text {
                text: "recovered after guard feedback".to_string(),
                stop_reason: crate::llm::StopReason::EndTurn,
            })
        }
    }

    struct CountingReadHandler {
        executions: Arc<AtomicUsize>,
    }

    impl ToolHandler for CountingReadHandler {
        fn name(&self) -> &str {
            "read_file"
        }

        fn execute(&self, _args: &serde_json::Value, _work_dir: &std::path::Path) -> ToolResult {
            self.executions.fetch_add(1, Ordering::SeqCst);
            ToolResult::success("read_file", "status is available")
        }
    }

    #[tokio::test]
    async fn repeated_tool_batch_is_deduped_without_terminating_the_agent() {
        let provider_calls = Arc::new(AtomicUsize::new(0));
        let guard_feedback = Arc::new(AtomicUsize::new(0));
        let executions = Arc::new(AtomicUsize::new(0));
        let provider = RepeatingBatchProvider {
            calls: provider_calls.clone(),
            saw_guard_feedback: guard_feedback.clone(),
            tool_call_limit: 3,
        };

        let mut runner = AgentRunner::new(Box::new(provider), AgentConfig::default());
        runner.tool_registry_mut().register(
            Tool::new(
                "read_file",
                "Read a file",
                serde_json::json!({"type": "object"}),
            ),
            Arc::new(CountingReadHandler {
                executions: executions.clone(),
            }),
        );

        let response = runner.run("inspect the workspace", &[]).await;

        match response {
            AgentResponse::TextWithToolCalls { text, steps } => {
                assert_eq!(text, "recovered after guard feedback");
                assert!(
                    !steps
                        .iter()
                        .any(|step| step.tool_name == "agent_loop_no_progress"),
                    "a duplicate batch must not become a terminal failure"
                );
            }
            other => panic!("expected a recovered response, got {other:?}"),
        }
        assert_eq!(
            executions.load(Ordering::SeqCst),
            1,
            "the duplicate read batch must not be executed again"
        );
        assert_eq!(provider_calls.load(Ordering::SeqCst), 4);
        assert!(
            guard_feedback.load(Ordering::SeqCst) >= 2,
            "the model must receive corrective feedback before continuing"
        );
    }

    #[tokio::test]
    async fn persistent_duplicate_batches_end_in_a_resumable_progress_pause() {
        let provider_calls = Arc::new(AtomicUsize::new(0));
        let executions = Arc::new(AtomicUsize::new(0));
        let provider = RepeatingBatchProvider {
            calls: provider_calls.clone(),
            saw_guard_feedback: Arc::new(AtomicUsize::new(0)),
            tool_call_limit: usize::MAX,
        };
        let mut runner = AgentRunner::new(Box::new(provider), AgentConfig::default());
        runner.tool_registry_mut().register(
            Tool::new(
                "read_file",
                "Read a file",
                serde_json::json!({"type": "object"}),
            ),
            Arc::new(CountingReadHandler {
                executions: executions.clone(),
            }),
        );

        // This provider keeps returning the same tool call. The runner must
        // suppress all duplicate executions and then use the configured
        // NetProgressGuard pause, rather than returning the old terminal error.
        let response = runner.run("inspect the workspace", &[]).await;

        match response {
            AgentResponse::TextWithToolCalls { text, .. } => assert!(
                text.starts_with("已暂停，等待用户介入（no progress"),
                "expected a resumable no-progress pause, got: {text}"
            ),
            other => panic!("expected a resumable pause, got {other:?}"),
        }
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        assert_eq!(
            provider_calls.load(Ordering::SeqCst),
            DEFAULT_NET_PROGRESS_THRESHOLD + 1,
            "the initial execution plus exactly the configured duplicate corrections"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn window_image_observation_uses_normal_permission_without_confirmed_flags() {
        for permission in ["ask", "workspace_auto", "full_access"] {
            let runner = AgentRunner::new(
                Box::new(MutationThenUnverifiedSummaries {
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
                AgentConfig::default(),
            )
            .with_execution_permission(permission.into());
            for arguments in [
                serde_json::json!({"app_id":"observer"}),
                serde_json::json!({"app_id":"observer","mode":"controls"}),
            ] {
                assert!(!runner.requires_confirmation("observe_trusted_app_window", &arguments));
            }
            for confirmed in [false, true] {
                assert_eq!(
                    runner.requires_confirmation(
                        "observe_trusted_app_window",
                        &serde_json::json!({"app_id":"observer","mode":"image","confirmed":confirmed}),
                    ),
                    permission != "full_access",
                    "permission={permission}, confirmed={confirmed}",
                );
            }
            for protected in [
                "prepare_message_draft",
                "set_trusted_app_text",
                "operate_trusted_app_control",
            ] {
                assert!(
                    runner.requires_confirmation(protected, &serde_json::json!({"confirmed":true}))
                );
            }
        }
    }

    struct MutationThenUnverifiedSummaries {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for MutationThenUnverifiedSummaries {
        fn name(&self) -> &str {
            "mutation-then-unverified-summaries"
        }

        async fn chat(
            &self,
            _messages: &[LlmMessage],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            match self.calls.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(LlmResponse::ToolCalls {
                    calls: vec![LlmToolCall {
                        id: "write-1".to_string(),
                        name: "write_file".to_string(),
                        arguments: serde_json::json!({"path":"note.txt","content":"done"}),
                    }],
                    text: None,
                    stop_reason: crate::llm::StopReason::ToolUse,
                }),
                1 => Ok(LlmResponse::Text {
                    text: "第一次收尾".to_string(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                }),
                _ => Ok(LlmResponse::Text {
                    text: "已完成修改；验证仍待后续执行。".to_string(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                }),
            }
        }
    }

    struct SuccessfulWriteHandler;

    impl crate::agent::ToolHandler for SuccessfulWriteHandler {
        fn name(&self) -> &str {
            "write_file"
        }

        fn execute(&self, _: &serde_json::Value, _: &std::path::Path) -> crate::agent::ToolResult {
            crate::agent::ToolResult::success("write_file", "written")
        }
    }

    #[tokio::test]
    async fn completion_gate_preserves_the_model_summary_when_verification_is_still_needed() {
        let mut runner = AgentRunner::new(
            Box::new(MutationThenUnverifiedSummaries {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            AgentConfig::default(),
        )
        .with_execution_permission("full_access".to_string());
        runner.tool_registry_mut().register(
            crate::agent::Tool::new(
                "write_file",
                "Write a file",
                serde_json::json!({"type":"object"}),
            ),
            Arc::new(SuccessfulWriteHandler),
        );

        let response = runner.run("修改这个项目，然后运行测试", &[]).await;

        match response {
            AgentResponse::TextWithToolCalls { text, steps } => {
                assert!(text.starts_with("已完成修改；验证仍待后续执行。"));
                assert!(text.contains("验证状态：尚无成功验证证据"));
                assert!(steps.iter().any(|step| {
                    step.tool_name == "agent_loop" && step.output.contains("尚无成功的验证证据")
                }));
            }
            other => panic!("expected resumable response, got {other:?}"),
        }
    }

    #[test]
    fn content_blocks_use_one_monotonic_sequence_across_async_producers() {
        let runner = AgentRunner::new(Box::new(MinimalTextProvider), AgentConfig::default());
        let text_index = runner.content_block_index.fetch_add(1, Ordering::SeqCst) + 1;
        let tool_index = runner.content_block_index.fetch_add(1, Ordering::SeqCst) + 1;
        let next_turn_text_index = runner.content_block_index.fetch_add(1, Ordering::SeqCst) + 1;

        assert_eq!([text_index, tool_index, next_turn_text_index], [1, 2, 3]);
    }

    #[test]
    fn test_turn_state_record_failure() {
        let mut state = TurnState::new();
        assert!(!state.record_failure(3));
        assert_eq!(state.failures, 1);
        assert!(!state.record_failure(3));
        assert_eq!(state.failures, 2);
        assert!(state.record_failure(3)); // 3rd failure → should_abort
        assert!(state.should_abort);
    }

    #[test]
    fn test_turn_state_retry_tracking() {
        let mut state = TurnState::new();
        state.start_retry("read_file", 2);
        assert!(state.can_retry("read_file", 2));
        assert_eq!(state.retry_count, 1);
        state.start_retry("read_file", 2);
        assert_eq!(state.retry_count, 2);
        assert!(!state.can_retry("read_file", 2)); // exhausted
        state.reset_retry();
        assert!(state.retry_tool.is_none());
    }

    #[test]
    fn detects_only_malformed_function_result_protocol_text() {
        assert!(is_malformed_tool_protocol_response(
            "<function_result>action: read_file(\"brief.txt\")</function_result>",
        ));
        assert!(!is_malformed_tool_protocol_response(
            "I read the brief and will continue."
        ));
        assert!(!is_malformed_tool_protocol_response(
            "<function_result>done</function_result>"
        ));
    }

    #[test]
    fn terminal_protocol_failures_are_auditable_even_before_any_tool_runs() {
        let response = AgentRunner::protocol_violation_slice_response(
            Vec::new(),
            "tool protocol could not be normalized".to_string(),
        );

        match response {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert_eq!(steps.len(), 1);
                assert_eq!(steps[0].tool_name, "agent_loop");
                assert!(!steps[0].success);
                assert_eq!(
                    derive_task_phase(&task_facts_from_steps("write a file", &steps), false,),
                    DerivedTaskPhase::NeedsAttention
                );
            }
            _ => panic!("terminal protocol failure must be persisted as an agent step"),
        }
    }

    #[test]
    fn coding_completion_requires_verification_after_latest_mutation() {
        let write = AgentStep {
            call_id: "write-1".to_string(),
            tool_name: "write_file".to_string(),
            arguments: serde_json::json!({}),
            success: true,
            output: "written".to_string(),
            confirmation_required: false,
            confirmation_status: None,
            verification: None,
        };
        let verify = AgentStep {
            call_id: "verify-1".to_string(),
            tool_name: "run_project_command".to_string(),
            arguments: serde_json::json!({"command": "npm test"}),
            success: true,
            output: "passed".to_string(),
            confirmation_required: false,
            confirmation_status: None,
            verification: None,
        };

        assert!(coding_task_has_unverified_changes(
            "修改这个项目并运行测试",
            &[write.clone()]
        ));
        assert!(!coding_task_has_unverified_changes(
            "修改这个项目并运行测试",
            &[write.clone(), verify]
        ));
        assert!(!coding_task_has_unverified_changes(
            "写一条普通笔记",
            &[write]
        ));
    }

    #[test]
    fn tool_batch_signature_changes_with_tool_or_arguments() {
        let read_a = LlmToolCall {
            id: "call-1".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "a.txt"}),
        };
        let same_read_new_id = LlmToolCall {
            id: "call-2".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "a.txt"}),
        };
        let read_b = LlmToolCall {
            id: "call-3".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "b.txt"}),
        };

        assert_eq!(
            tool_batch_signature(&[read_a.clone()]),
            tool_batch_signature(&[same_read_new_id])
        );
        assert_ne!(
            tool_batch_signature(&[read_a]),
            tool_batch_signature(&[read_b])
        );
    }

    #[test]
    fn test_turn_state_different_tool_resets_count() {
        let mut state = TurnState::new();
        state.start_retry("read_file", 2);
        assert_eq!(state.retry_count, 1);
        state.start_retry("write_file", 2); // different tool → reset count
        assert_eq!(state.retry_count, 1);
    }
}
