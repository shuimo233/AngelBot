//! Agent lifecycle hooks — Pi-inspired design
//!
//! Inspired by `badlogic/pi-mono/packages/agent/src/agent-loop.ts`.
//!
//! Pi-style hooks:
//! - `transform_context`: LLM 调用前修改 messages
//! - `prepare_next_turn`: turn_end 后更新 context
//! - `should_stop_after_turn`: 决策是否终止循环
//!
//! Unlike Pi, AngelBot retains hard safety bounds (max_iterations,
//! max_tool_calls, circuit breakers) that cannot be overridden by hooks.

use crate::agent::runner::AgentStep;
use crate::agent::task_facts::TaskFacts;
use crate::llm::{LlmProvider, Message as LlmMessage};
use std::sync::Arc;

// ── Core types ──────────────────────────────────────────────────────────────────

/// Context passed to decision hooks
#[derive(Debug, Clone)]
pub struct TurnContext {
    pub iteration: usize,
    pub max_iterations: usize,
    pub messages: Vec<LlmMessage>,
    pub steps: Vec<AgentStep>,
    pub total_tool_calls: usize,
    pub last_response_shape: Option<ResponseKind>,
}

impl TurnContext {
    pub fn new(
        iteration: usize,
        max: usize,
        messages: Vec<LlmMessage>,
        steps: Vec<AgentStep>,
        total_tool_calls: usize,
    ) -> Self {
        Self {
            iteration,
            max_iterations: max,
            messages,
            steps,
            total_tool_calls,
            last_response_shape: None,
        }
    }
    pub fn with_response_shape(mut self, shape: ResponseKind) -> Self {
        self.last_response_shape = Some(shape);
        self
    }
}

/// Whether the loop should terminate after the current turn.
///
/// `PauseForUser` is a soft pause that keeps the run open so the user
/// can resume by sending a new prompt (Issue #096 Day 4). `Stop` is a
/// hard termination that emits `AgentEnd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopDecision {
    Continue,
    PauseForUser { reason: String },
    Stop,
}
impl Default for LoopDecision {
    fn default() -> Self {
        LoopDecision::Continue
    }
}

/// Result of context transformation
#[derive(Debug, Clone)]
pub enum TransformResult {
    PassThrough,
    Transformed(Vec<LlmMessage>),
}

/// Snapshot of turn state for prepare_next_turn.
/// Issue #096 / G3: `model` allows hooks to switch the active model at turn boundaries
/// (e.g. fast model for exploration, powerful model for execution).
#[derive(Debug, Clone)]
pub struct NextTurnSnapshot {
    pub messages: Vec<LlmMessage>,
    pub model: Option<String>,
}
impl Default for NextTurnSnapshot {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            model: None,
        }
    }
}

// ── Observer types (for backward compatibility) ────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseKind {
    Text,
    ToolCalls,
    ProtocolViolation,
}

pub trait HasResponseShape {
    fn response_shape(&self) -> ResponseKind;
}
impl HasResponseShape for crate::llm::LlmResponse {
    fn response_shape(&self) -> ResponseKind {
        match self {
            crate::llm::LlmResponse::Text { .. } => ResponseKind::Text,
            crate::llm::LlmResponse::ToolCalls { .. } => ResponseKind::ToolCalls,
            crate::llm::LlmResponse::ProtocolViolation { .. } => ResponseKind::ProtocolViolation,
            crate::llm::LlmResponse::WithProtocol { response, .. } => response.response_shape(),
        }
    }
}

impl ResponseKind {
    pub fn classify<R: HasResponseShape>(r: &R) -> Self {
        r.response_shape()
    }
}

#[derive(Debug, Clone)]
pub struct LifecycleContext {
    pub iteration: usize,
    pub max_iterations: usize,
    pub messages_snapshot: Vec<LlmMessage>,
    pub steps: Vec<AgentStep>,
    pub total_tool_calls: usize,
    pub last_response_shape: Option<ResponseKind>,
    /// Compact task facts derived from the visible `steps` so a hook can
    /// see "this is a coding task", "no files modified yet", "two failed
    /// steps", etc. without re-deriving them.
    pub task_facts: Option<TaskFacts>,
}

impl LifecycleContext {
    pub fn before_llm(
        i: usize,
        max: usize,
        msgs: Vec<LlmMessage>,
        steps: Vec<AgentStep>,
        tc: usize,
    ) -> Self {
        Self {
            iteration: i,
            max_iterations: max,
            messages_snapshot: msgs,
            steps,
            total_tool_calls: tc,
            last_response_shape: None,
            task_facts: None,
        }
    }
    pub fn after(
        i: usize,
        max: usize,
        msgs: Vec<LlmMessage>,
        steps: Vec<AgentStep>,
        tc: usize,
        shape: ResponseKind,
    ) -> Self {
        Self {
            iteration: i,
            max_iterations: max,
            messages_snapshot: msgs,
            steps,
            total_tool_calls: tc,
            last_response_shape: Some(shape),
            task_facts: None,
        }
    }
    // Alias for backward compatibility
    pub fn before_branch(
        i: usize,
        max: usize,
        msgs: Vec<LlmMessage>,
        steps: Vec<AgentStep>,
        tc: usize,
    ) -> Self {
        Self::before_llm(i, max, msgs, steps, tc)
    }

    /// Attach compact task facts. Constructors above default `None` so
    /// older call sites keep compiling; the runner supplies facts via
    /// this builder when (and only when) it has the steps + goal on hand.
    pub fn with_task_facts(mut self, facts: TaskFacts) -> Self {
        self.task_facts = Some(facts);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalBranchKind {
    MaxIterations,
    /// Reserved for future hard-cap policies. No longer produced by
    /// the default runner, which now relies on soft `LoopGuard`s
    /// (`WallClockGuard`, `NetProgressGuard`,
    /// `SideEffectAutoPauseGuard`). Kept in the enum so existing
    /// observability hooks that match on it keep compiling.
    #[allow(dead_code)]
    MaxToolCalls,
    LlmRepeatedlyFailed,
    CodingCompletionBlocked,
    ProtocolViolation,
    TooManyToolFailures,
    /// Single turn exceeded the configured wall-clock budget (Issue #096).
    WallClockTimeout,
}

// ── Main hook trait ───────────────────────────────────────────────────────────

/// Pi-style lifecycle hook trait.
/// - Decision hooks: transform_context, prepare_next_turn, should_stop_after_turn
/// - Observer hooks: before_llm_call, after_llm_response, after_tool_batch, before_terminal_branch
pub trait AgentLifecycleHook: Send + Sync {
    // Decision hooks (Pi-style)
    fn transform_context(&self, ctx: &TurnContext) -> TransformResult {
        let _ = ctx;
        TransformResult::PassThrough
    }
    fn prepare_next_turn(&self, _ctx: &TurnContext) -> Option<NextTurnSnapshot> {
        None
    }
    fn should_stop_after_turn(&self, ctx: &TurnContext) -> LoopDecision {
        let _ = ctx;
        LoopDecision::Continue
    }

    // Observer hooks (backward compatible)
    fn before_llm_call(&self, _ctx: &LifecycleContext) -> Result<(), String> {
        Ok(())
    }
    fn after_llm_response(&self, _ctx: &LifecycleContext) -> Result<(), String> {
        Ok(())
    }
    fn after_tool_batch(&self, _ctx: &LifecycleContext) -> Result<(), String> {
        Ok(())
    }
    fn before_terminal_branch(
        &self,
        _ctx: &LifecycleContext,
        _kind: TerminalBranchKind,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// Default hook: all no-ops
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultLifecycleHook;
impl AgentLifecycleHook for DefaultLifecycleHook {}

// ── Issue #091: Tool Lifecycle Hooks ─────────────────────────────────────────

/// Context passed to before_tool_call hook
#[derive(Debug, Clone)]
pub struct ToolCallContext {
    pub tool_call: crate::agent::tool::ToolCall,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub session_id: Option<String>,
    pub iteration: usize,
}

/// Result of before_tool_call hook
#[derive(Debug, Clone, Default)]
pub struct BeforeToolCallResult {
    pub block: bool,
    pub reason: Option<String>,
}

/// Context passed to after_tool_call hook
#[derive(Debug, Clone)]
pub struct AfterToolCallContext {
    pub tool_call: crate::agent::tool::ToolCall,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub result: crate::agent::tool::ToolResult,
    pub is_error: bool,
    pub session_id: Option<String>,
    pub iteration: usize,
}

/// Result of after_tool_call hook
#[derive(Debug, Clone, Default)]
pub struct AfterToolCallResult {
    pub content: Option<String>,
    pub is_error: Option<bool>,
    pub terminate: Option<bool>,
}

/// Pi-style tool lifecycle hook trait.
pub trait ToolLifecycleHook: Send + Sync {
    fn before_tool_call(&self, ctx: &ToolCallContext) -> BeforeToolCallResult {
        let _ = ctx;
        BeforeToolCallResult {
            block: false,
            reason: None,
        }
    }
    fn after_tool_call(&self, ctx: &AfterToolCallContext) -> AfterToolCallResult {
        let _ = ctx;
        AfterToolCallResult::default()
    }
}

/// Default tool hook: all no-ops
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultToolLifecycleHook;
impl ToolLifecycleHook for DefaultToolLifecycleHook {}

/// Applies AfterToolCallResult to a ToolResult using field-by-field merge
pub fn apply_after_hook(
    result: crate::agent::tool::ToolResult,
    hook_result: AfterToolCallResult,
) -> crate::agent::tool::ToolResult {
    let success = match hook_result.is_error {
        Some(true) => false,
        Some(false) => true,
        None => result.success,
    };
    let content = hook_result.content.unwrap_or(result.content);
    let terminate_batch = hook_result.terminate.unwrap_or(result.terminate_batch);
    crate::agent::tool::ToolResult {
        name: result.name,
        success,
        content,
        terminate_batch,
        images: if success && !terminate_batch {
            result.images
        } else {
            Vec::new()
        },
    }
}

// ── Issue #092: Dynamic API Key Provider ────────────────────────────────────────

use std::sync::Mutex;

/// Trait for dynamic API key resolution.
///
/// Allows runtime key refresh (e.g., OAuth token refresh) without restarting the agent.
/// If `get_api_key` returns `None`, the runner falls back to the provider's existing key.
pub trait DynamicApiKeyProvider: Send + Sync {
    /// Resolve the API key for a provider at runtime.
    /// Return `None` to use the provider's existing key.
    fn get_api_key(&self, provider: &str) -> Option<String>;
}

/// Default implementation: always returns None (use existing key)
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultApiKeyProvider;
impl DynamicApiKeyProvider for DefaultApiKeyProvider {
    fn get_api_key(&self, _provider: &str) -> Option<String> {
        None
    }
}

/// Wrapper that provides dynamic API key resolution for any LLM provider.
///
/// Wraps an `Arc<dyn LlmProvider>` and an `Arc<dyn DynamicApiKeyProvider>`.
/// Before each `chat()` call, it checks for a fresh key and updates the provider if needed.
#[derive(Clone)]
pub struct DynamicLlmProvider {
    inner: Arc<dyn LlmProvider>,
    api_key_provider: Arc<dyn DynamicApiKeyProvider>,
    /// Holds the current API key for refresh attempts.
    /// Only used if the inner provider supports `refresh_api_key`.
    current_key: Arc<Mutex<Option<String>>>,
}

impl DynamicLlmProvider {
    pub fn new(
        inner: Arc<dyn LlmProvider>,
        api_key_provider: Arc<dyn DynamicApiKeyProvider>,
    ) -> Self {
        Self {
            inner,
            api_key_provider,
            current_key: Arc::new(Mutex::new(None)),
        }
    }

    fn refresh_key_if_needed(&self) {
        let provider_name = self.inner.name();
        if let Some(new_key) = self.api_key_provider.get_api_key(provider_name) {
            let mut current = self.current_key.lock().unwrap();
            if current.as_ref() != Some(&new_key) {
                // Attempt to refresh the inner provider's key. This is a no-op
                // if the provider does not implement `refresh_api_key` (e.g. Ollama).
                let _ = self.inner.refresh_api_key(&new_key);
                *current = Some(new_key);
            }
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for DynamicLlmProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn supports_tool_images(&self) -> bool {
        self.inner.supports_tool_images()
    }

    async fn chat(
        &self,
        messages: &[crate::llm::Message],
        tools: Option<&[crate::llm::ToolSchema]>,
        thinking: Option<&crate::llm::ThinkingSettings>,
    ) -> Result<crate::llm::LlmResponse, String> {
        self.refresh_key_if_needed();
        self.inner.chat(messages, tools, thinking).await
    }

    async fn chat_stream(
        &self,
        messages: &[crate::llm::Message],
        tools: Option<&[crate::llm::ToolSchema]>,
        thinking: Option<&crate::llm::ThinkingSettings>,
        delta_sender: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<crate::llm::LlmResponse, String> {
        self.refresh_key_if_needed();
        self.inner
            .chat_stream(messages, tools, thinking, delta_sender)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::StopReason;

    #[test]
    fn default_decision_is_continue() {
        let h = DefaultLifecycleHook;
        let ctx = TurnContext::new(0, 10, Vec::new(), Vec::new(), 0);
        assert_eq!(h.should_stop_after_turn(&ctx), LoopDecision::Continue);
    }

    #[test]
    fn default_transform_is_passthrough() {
        let h = DefaultLifecycleHook;
        let ctx = TurnContext::new(0, 10, Vec::new(), Vec::new(), 0);
        assert!(matches!(
            h.transform_context(&ctx),
            TransformResult::PassThrough
        ));
    }

    #[test]
    fn response_kind_matches_variants() {
        let text = crate::llm::LlmResponse::Text {
            text: "hi".into(),
            stop_reason: StopReason::EndTurn,
        };
        let tool = crate::llm::LlmResponse::ToolCalls {
            calls: Vec::new(),
            text: None,
            stop_reason: StopReason::ToolUse,
        };
        assert_eq!(ResponseKind::classify(&text), ResponseKind::Text);
        assert_eq!(ResponseKind::classify(&tool), ResponseKind::ToolCalls);
    }

    // Issue #091: ToolLifecycleHook tests
    #[test]
    fn test_tool_lifecycle_default_allows() {
        let hook = DefaultToolLifecycleHook;
        let ctx = ToolCallContext {
            tool_call: crate::agent::tool::ToolCall::new("test".into(), serde_json::json!({})),
            tool_name: "test".into(),
            args: serde_json::json!({}),
            session_id: None,
            iteration: 0,
        };
        let result = hook.before_tool_call(&ctx);
        assert!(!result.block);
        assert!(result.reason.is_none());
    }

    #[test]
    fn test_tool_lifecycle_after_default_passthrough() {
        let hook = DefaultToolLifecycleHook;
        let result = crate::agent::tool::ToolResult::success("test", "hello");
        let ctx = AfterToolCallContext {
            tool_call: crate::agent::tool::ToolCall::new("test".into(), serde_json::json!({})),
            tool_name: "test".into(),
            args: serde_json::json!({}),
            result,
            is_error: false,
            session_id: None,
            iteration: 0,
        };
        let hook_result = hook.after_tool_call(&ctx);
        assert!(hook_result.content.is_none());
        assert!(hook_result.is_error.is_none());
        assert!(hook_result.terminate.is_none());
    }

    #[test]
    fn test_apply_after_hook_merges_content() {
        use super::apply_after_hook;
        let result = crate::agent::tool::ToolResult::success("test", "original");
        let hook_result = AfterToolCallResult {
            content: Some("modified".into()),
            ..Default::default()
        };
        let applied = apply_after_hook(result, hook_result);
        assert_eq!(applied.content, "modified");
        assert!(applied.success);
    }

    #[test]
    fn test_apply_after_hook_can_mark_error() {
        use super::apply_after_hook;
        let result = crate::agent::tool::ToolResult::success("test", "original");
        let hook_result = AfterToolCallResult {
            content: Some("blocked by policy".into()),
            is_error: Some(true),
            ..Default::default()
        };
        let applied = apply_after_hook(result, hook_result);
        assert!(!applied.success);
        assert!(applied.content.contains("blocked by policy"));
    }

    #[test]
    fn test_apply_after_hook_preserves_unmodified_fields() {
        use super::apply_after_hook;
        let result = crate::agent::tool::ToolResult {
            name: "test".into(),
            success: true,
            content: "original".into(),
            terminate_batch: true,
            images: Vec::new(),
        };
        let hook_result = AfterToolCallResult {
            content: Some("modified".into()),
            ..Default::default()
        };
        let applied = apply_after_hook(result, hook_result);
        assert_eq!(applied.name, "test");
        assert_eq!(applied.content, "modified");
        assert!(applied.success);
        assert!(applied.terminate_batch); // preserved
    }

    #[test]
    fn test_before_tool_call_result_block() {
        let result = BeforeToolCallResult {
            block: true,
            reason: Some("dangerous".into()),
        };
        assert!(result.block);
        assert_eq!(result.reason.as_deref(), Some("dangerous"));
    }

    // Issue #092: DynamicApiKeyProvider tests
    #[test]
    fn test_default_api_key_provider_returns_none() {
        let provider = DefaultApiKeyProvider;
        assert!(provider.get_api_key("openai").is_none());
        assert!(provider.get_api_key("deepseek").is_none());
        assert!(provider.get_api_key("any-provider").is_none());
    }

    #[test]
    fn dynamic_provider_forwards_tool_image_capability() {
        struct CapabilityProvider(bool);
        #[async_trait::async_trait]
        impl LlmProvider for CapabilityProvider {
            fn name(&self) -> &str {
                "fixture"
            }
            fn supports_tool_images(&self) -> bool {
                self.0
            }
            async fn chat(
                &self,
                _: &[crate::llm::Message],
                _: Option<&[crate::llm::ToolSchema]>,
                _: Option<&crate::llm::ThinkingSettings>,
            ) -> Result<crate::llm::LlmResponse, String> {
                Err("Capability checks must not make model requests".into())
            }
        }
        for supported in [true, false] {
            let provider = DynamicLlmProvider::new(
                Arc::new(CapabilityProvider(supported)),
                Arc::new(DefaultApiKeyProvider),
            );
            assert_eq!(provider.supports_tool_images(), supported);
        }
    }

    // Test a custom key provider
    #[derive(Debug)]
    struct MockKeyProvider {
        keys: std::collections::HashMap<String, String>,
    }
    impl MockKeyProvider {
        fn new(mappings: Vec<(&str, &str)>) -> Self {
            let mut keys = std::collections::HashMap::new();
            for (k, v) in mappings {
                keys.insert(k.to_string(), v.to_string());
            }
            Self { keys }
        }
    }
    impl DynamicApiKeyProvider for MockKeyProvider {
        fn get_api_key(&self, provider: &str) -> Option<String> {
            self.keys.get(provider).cloned()
        }
    }

    #[test]
    fn test_custom_api_key_provider_resolves_keys() {
        let provider = MockKeyProvider::new(vec![
            ("openai", "sk-test-openai"),
            ("deepseek", "sk-test-deepseek"),
        ]);
        assert_eq!(
            provider.get_api_key("openai"),
            Some("sk-test-openai".to_string())
        );
        assert_eq!(
            provider.get_api_key("deepseek"),
            Some("sk-test-deepseek".to_string())
        );
        assert!(provider.get_api_key("unknown").is_none());
    }

    #[test]
    fn test_lifecycle_context_task_facts_default_none() {
        // Constructors without `with_task_facts` keep the legacy
        // behaviour: hooks see `None` rather than a derived default.
        // This preserves backward compatibility for tests that build
        // contexts by hand.
        let ctx = LifecycleContext::before_llm(0, 100, vec![], vec![], 0);
        assert!(ctx.task_facts.is_none());
        let ctx = LifecycleContext::after(0, 100, vec![], vec![], 0, ResponseKind::Text);
        assert!(ctx.task_facts.is_none());
    }

    #[test]
    fn test_lifecycle_context_with_task_facts_round_trips() {
        let facts = TaskFacts::new("write a Rust function");
        let ctx =
            LifecycleContext::before_llm(0, 100, vec![], vec![], 0).with_task_facts(facts.clone());
        let carried = ctx
            .task_facts
            .expect("with_task_facts must populate the field");
        assert_eq!(carried.goal, "write a Rust function");
    }
}
