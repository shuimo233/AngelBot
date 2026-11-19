//! Unified LLM provider abstraction
//!
//! This module provides a common interface for different LLM providers
//! (DeepSeek, OpenAI, Anthropic, Ollama, etc.) with standardized tool calling support.

mod anthropic;
pub mod embedding;
mod openai_compat;
pub(crate) use openai_compat::openai_compatible_request_headers;
mod metering;
mod provider;
pub mod router;
pub(crate) mod tracked;
mod transport;
pub mod usage;

// Issue #103: DeepSeek V4 API rejects legacy `deepseek-chat` / `deepseek-reasoner`
// names with 400 Bad Request. Centralize V4 defaults so callers do not drift.
pub const DEEPSEEK_DEFAULT_MODEL: &str = "deepseek-v4-flash";
pub const DEEPSEEK_PRO_MODEL: &str = "deepseek-v4-pro";

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use anthropic::AnthropicProvider;
pub use embedding::create_embedding_provider;
pub mod auth;
mod catalog;
pub(crate) use catalog::{
    create_foreground_provider_for_app, create_session_foreground_provider_for_app,
    invalidate_foreground_websocket_for_app,
};
pub use catalog::{create_provider_for_app, create_provider_with_credentials};
pub mod chatgpt_auth;
mod openai_responses;
mod tool_image;
pub use tool_image::{ToolImage, MAX_TOOL_IMAGES, MAX_TOOL_IMAGE_TOTAL_BYTES};
pub mod protocol;
pub use openai_compat::OpenAiCompatibleProvider;
#[cfg(test)]
pub(crate) use openai_responses::native_context_shape;
pub(crate) use openai_responses::plan_history_can_replay;
pub use openai_responses::OpenAiResponsesProvider;
pub use protocol::{ModelAuthMode, ModelProtocol, ProtocolContinuation};
#[allow(unused_imports)]
pub use provider::{create_deepseek_provider, create_provider_from_env, DeepSeekProvider};
pub use usage::{CostSummary, UsageStats, UsageTracker};

/// Persisted notice only; image bytes never enter the historical replay.
pub(crate) const TRANSIENT_IMAGE_NOTICE: &str = "[图像瞬时载荷未保留；需要新状态时请重新观察。]";

/// Unified message format for all providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_state: Option<ProtocolContinuation>,
    /// Current tool round only. Never accepted from JSON or persisted in history.
    #[serde(skip)]
    pub tool_images: Vec<ToolImage>,
}

impl Message {
    pub fn text_for_unsupported_tool_images(&self) -> String {
        if self.tool_images.is_empty() {
            self.content.clone()
        } else {
            format!(
                "{}\n[当前模型接口未接入工具图像输入；未发送图像，请勿声称已经查看。]",
                self.content
            )
        }
    }
}

/// Tool call format (provider-agnostic)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// Tool schema format (provider-agnostic, follows OpenAI format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl ToolSchema {
    /// Convert to OpenAI function calling format
    pub fn to_openai_format(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            }
        })
    }
}

#[cfg(test)]
mod stop_reason_tests {
    use super::*;

    #[test]
    fn openai_finish_reason_maps_to_stop_reason() {
        assert_eq!(
            StopReason::from_openai_finish_reason(Some("stop")),
            StopReason::EndTurn
        );
        assert_eq!(
            StopReason::from_openai_finish_reason(Some("tool_calls")),
            StopReason::ToolUse
        );
        assert_eq!(
            StopReason::from_openai_finish_reason(Some("length")),
            StopReason::MaxTokens
        );
        assert_eq!(
            StopReason::from_openai_finish_reason(Some("content_filter")),
            StopReason::ContentFilter
        );
        assert_eq!(
            StopReason::from_openai_finish_reason(None),
            StopReason::EndTurn
        );
        assert_eq!(
            StopReason::from_openai_finish_reason(Some("mystery")),
            StopReason::Unknown
        );
    }

    #[test]
    fn anthropic_stop_reason_maps_to_stop_reason() {
        assert_eq!(
            StopReason::from_anthropic_stop_reason(Some("end_turn")),
            StopReason::EndTurn
        );
        assert_eq!(
            StopReason::from_anthropic_stop_reason(Some("tool_use")),
            StopReason::ToolUse
        );
        assert_eq!(
            StopReason::from_anthropic_stop_reason(Some("max_tokens")),
            StopReason::MaxTokens
        );
        assert_eq!(
            StopReason::from_anthropic_stop_reason(Some("refusal")),
            StopReason::ContentFilter
        );
        assert_eq!(
            StopReason::from_anthropic_stop_reason(None),
            StopReason::EndTurn
        );
    }
}

#[cfg(test)]
mod thinking_tests {
    use super::*;

    fn config(provider: &str, model: &str, max_tokens: u32) -> ProviderConfig {
        ProviderConfig {
            protocol: None,
            auth_mode: crate::llm::ModelAuthMode::ApiKey,
            credential_ref: None,
            provider: provider.to_string(),
            model: model.to_string(),
            base_url: String::new(),
            api_key: String::new(),
            max_tokens,
            temperature: 0.7,
        }
    }

    #[test]
    fn deepseek_uses_only_its_native_reasoning_protocol() {
        let settings =
            resolve_thinking_settings(&config("deepseek", "deepseek-v4-pro", 4096), Some("high"))
                .unwrap()
                .unwrap();
        assert_eq!(settings.protocol, ThinkingProtocol::DeepSeek);
        assert_eq!(settings.effort, ThinkingEffort::High);
    }

    #[test]
    fn manual_claude_budget_must_fit_inside_output_limit() {
        let error = resolve_thinking_settings(
            &config("anthropic", "claude-sonnet-4-20250514", 4096),
            Some("high"),
        )
        .unwrap_err();
        assert!(!error.is_empty());
    }

    #[test]
    fn generic_openai_compatible_provider_gets_no_reasoning_extension() {
        assert!(
            resolve_thinking_settings(&config("openrouter", "some-model", 4096), Some("high"),)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn context_window_comes_from_the_selected_model_not_a_user_setting() {
        for model in ["gpt-6-luna", "gpt-6-sol", "gpt-6.1-sol"] {
            assert_eq!(
                context_window_tokens(&config("openai", model, 4096)),
                1_050_000
            );
        }
        assert_eq!(
            context_window_tokens(&config("deepseek", "deepseek-v4-flash", 4096)),
            1_000_000
        );
        assert_eq!(
            context_window_tokens(&config("groq", "llama-3.3-70b-versatile", 4096)),
            131_072
        );
    }

    #[test]
    fn unknown_models_use_a_safe_fallback() {
        assert_eq!(
            context_window_tokens(&config("custom", "internal-model", 4096)),
            FALLBACK_CONTEXT_WINDOW_TOKENS
        );
        for (provider, model) in [
            ("custom", "gpt-6-luna"),
            ("openai", "gpt-6-luna-unverified"),
            ("openai", "gpt-6.1-sol-unverified"),
            ("custom", "gpt-6.1-sol"),
        ] {
            assert_eq!(
                context_window_tokens(&config(provider, model, 4096)),
                FALLBACK_CONTEXT_WINDOW_TOKENS
            );
        }
    }
}

/// Unified response from any LLM provider.
///
/// `Text` and `ToolCalls` carry `stop_reason` so the runner can distinguish
/// a clean end-of-turn from a token-limit truncation. Issue #078.
#[derive(Debug)]
pub enum LlmResponse {
    /// A completed response with private, replayable wire-protocol output.
    /// Consumers unwrap this before normalizing visible text or tool calls.
    WithProtocol {
        response: Box<LlmResponse>,
        protocol: ProtocolContinuation,
    },
    /// Plain text reply with the underlying stop reason.
    Text {
        text: String,
        stop_reason: StopReason,
    },
    /// Tool-call request with the model's free-text reply (if any) and the
    /// underlying stop reason.
    ToolCalls {
        calls: Vec<ToolCall>,
        text: Option<String>,
        stop_reason: StopReason,
    },
    /// The provider received text that explicitly claims to be a tool call,
    /// but cannot safely normalize it into the canonical tool-call contract.
    ///
    /// This is deliberately distinct from normal assistant text: the agent
    /// loop may request one protocol correction, but must never execute it.
    ProtocolViolation { raw_text: String, reason: String },
}

impl LlmResponse {
    pub fn into_parts(mut self) -> (Self, Option<ProtocolContinuation>) {
        let mut state = None;
        loop {
            match self {
                Self::WithProtocol { response, protocol } => {
                    state.get_or_insert(protocol);
                    self = *response;
                }
                other => return (other, state),
            }
        }
    }
}

/// Token accounting supplied by a provider for its most recent request.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cached_input_tokens: Option<u32>,
    pub cache_write_tokens: Option<u32>,
    pub uncached_input_tokens: Option<u32>,
    pub cache_write_30m_tokens: Option<u32>,
    pub cache_write_12h_tokens: Option<u32>,
    pub reasoning_tokens: Option<u32>,
    pub details_invalid: bool,
}

/// Prepared-body diagnostics for synthetic acceptance only. Item content and
/// private protocol values are hashed, not serialized into public evidence.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRequestFingerprint {
    pub settings: String,
    pub instructions: String,
    pub tools: String,
    pub input: Vec<ProviderInputFingerprint>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInputFingerprint {
    pub kind: String,
    pub role: Option<String>,
    pub sha256: String,
    pub bytes: usize,
}

#[cfg(test)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCacheDiagnostics {
    pub kind: String,
    pub reason: Option<String>,
    pub comparison_reusable_tokens: Option<u32>,
    pub cache_missed_tokens: Option<u32>,
    pub details_invalid: bool,
}

/// Isolated acceptance only. Opaque support identifiers are never serialized
/// into public reports; callers may retain them in task-owned private evidence.
#[cfg(test)]
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderResponseMetadata {
    pub transport: Option<String>,
    pub model_matches_requested: Option<bool>,
    pub service_tier: Option<String>,
    pub organization_matches_previous: Option<bool>,
    pub details_invalid: bool,
    #[serde(skip)]
    pub request_id: Option<String>,
    #[serde(skip)]
    pub organization_fingerprint: Option<String>,
}

/// Provider-reported reason that the assistant message stopped.
///
/// Maps the OpenAI `finish_reason` strings and Anthropic `stop_reason`
/// strings onto a single, runner-friendly enumeration. Issue #078.
///
/// `Unknown` covers unrecognized values; never silently treated as a
/// successful end of turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Natural end of turn (`stop`, `end_turn`, `stop_sequence`).
    EndTurn,
    /// Model invoked a tool (`tool_calls`, `tool_use`).
    ToolUse,
    /// Output hit the token limit (`length`, `max_tokens`). Tool call
    /// arguments may be truncated; the runner must check this before
    /// executing any tool (`Issue #079` will use this).
    MaxTokens,
    /// Provider-side content filter (`content_filter`, `refusal`).
    ContentFilter,
    /// Anything we cannot classify. Treated as recoverable by the runner.
    Unknown,
}

impl StopReason {
    /// Map an OpenAI `finish_reason` value, if any, to `StopReason`.
    pub fn from_openai_finish_reason(value: Option<&str>) -> Self {
        match value.unwrap_or("stop") {
            "stop" | "stop_sequence" | "eos" => Self::EndTurn,
            "tool_calls" | "tool_use" => Self::ToolUse,
            "length" | "max_tokens" => Self::MaxTokens,
            "content_filter" | "refusal" => Self::ContentFilter,
            _ => Self::Unknown,
        }
    }

    /// Map an Anthropic `stop_reason` value, if any, to `StopReason`.
    pub fn from_anthropic_stop_reason(value: Option<&str>) -> Self {
        match value.unwrap_or("end_turn") {
            "end_turn" | "stop_sequence" => Self::EndTurn,
            "tool_use" => Self::ToolUse,
            "max_tokens" => Self::MaxTokens,
            "refusal" => Self::ContentFilter,
            _ => Self::Unknown,
        }
    }
}

/// A user-facing reasoning preference. It is deliberately not a token budget:
/// every provider has its own protocol and token accounting rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingEffort {
    Low,
    Medium,
    High,
}

impl ThinkingEffort {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("medium") {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            value => Err(format!("Unsupported reasoning effort: {value}")),
        }
    }
}

/// The exact provider protocol selected for a model. OpenAI-compatible APIs
/// do not share a reasoning extension, so this must stay explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingProtocol {
    OpenAiResponses,
    AnthropicManual,
    AnthropicAdaptive,
    AnthropicEffortOnly,
    DeepSeek,
}

/// Settings resolved by the backend from the selected provider and model.
#[derive(Debug, Clone)]
pub struct ThinkingSettings {
    pub protocol: ThinkingProtocol,
    pub effort: ThinkingEffort,
    /// Only used by Anthropic's legacy manual extended-thinking protocol.
    pub budget_tokens: Option<u32>,
}

/// Resolve a request into an API supported by the configured provider/model.
/// Unsupported models receive no private reasoning fields.
pub fn resolve_thinking_settings(
    config: &ProviderConfig,
    requested_effort: Option<&str>,
) -> Result<Option<ThinkingSettings>, String> {
    let effort = ThinkingEffort::parse(requested_effort)?;
    let model = config.model.to_ascii_lowercase();

    if config.effective_protocol() == ModelProtocol::OpenaiResponses {
        return Ok(Some(ThinkingSettings {
            protocol: ThinkingProtocol::OpenAiResponses,
            effort,
            budget_tokens: None,
        }));
    }

    match config.provider.as_str() {
        "deepseek" if model.starts_with("deepseek-v4-") || model == "deepseek-reasoner" => {
            Ok(Some(ThinkingSettings {
                protocol: ThinkingProtocol::DeepSeek,
                effort,
                budget_tokens: None,
            }))
        }
        _ if config.effective_protocol() == ModelProtocol::AnthropicMessages
            && model.starts_with("claude-") =>
        {
            let protocol = if model.contains("opus-4-6")
                || model.contains("opus-4-7")
                || model.contains("opus-4-8")
                || model.contains("sonnet-4-6")
            {
                ThinkingProtocol::AnthropicAdaptive
            } else if model.contains("sonnet-5")
                || model.contains("fable-5")
                || model.contains("mythos-5")
            {
                ThinkingProtocol::AnthropicEffortOnly
            } else {
                ThinkingProtocol::AnthropicManual
            };

            let budget_tokens = match protocol {
                ThinkingProtocol::AnthropicManual => match effort {
                    ThinkingEffort::Low => return Ok(None),
                    ThinkingEffort::Medium => Some(1_024),
                    ThinkingEffort::High => Some(4_096),
                },
                _ => None,
            };

            if let Some(budget) = budget_tokens {
                if budget >= config.max_tokens {
                    return Err(format!(
                        "Claude 的推理预算 ({budget}) 必须小于最大输出 Tokens ({})；请提高最大输出 Tokens 或降低推理强度。",
                        config.max_tokens
                    ));
                }
            }

            Ok(Some(ThinkingSettings {
                protocol,
                effort,
                budget_tokens,
            }))
        }
        _ => Ok(None),
    }
}

/// Closed, non-secret recovery categories supplied by a provider adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmFailureKind {
    Unclassified,
    QuotaExceeded,
    TransportUnavailable,
    Authentication,
    PermissionDenied,
    RateLimited,
    ServiceUnavailable,
    TimedOut,
    ResponseInterrupted,
    Protocol {
        stage: LlmProtocolStage,
        http_status: Option<u16>,
    },
}

/// Only fixed states are safe to show; an adapter error string is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProtocolStage {
    PrepareRequest,
    LoadCredentials,
    RequestRejected,
    ProviderReportedFailure,
    ResponseType,
    ResponseValidation,
}

impl LlmProtocolStage {
    fn notice(self) -> &'static str {
        match self {
            Self::PrepareRequest => "模型请求无法按所选接口的格式生成；请求未发送。",
            Self::LoadCredentials => "暂时无法读取、刷新或使用模型登录凭据；请求未发送。",
            Self::RequestRejected => "模型服务拒绝了请求。请检查接口地址、模型和请求设置。",
            Self::ProviderReportedFailure => "模型服务报告处理失败，未返回可用结果。",
            Self::ResponseType => "模型服务返回的响应格式不符合预期。请检查接口地址或代理。",
            Self::ResponseValidation => "模型服务返回的响应格式不符合预期，无法校验完成结果。",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::PrepareRequest => "准备请求",
            Self::LoadCredentials => "读取模型凭据",
            Self::RequestRejected => "请求准入",
            Self::ProviderReportedFailure => "服务端处理",
            Self::ResponseType => "响应类型检查",
            Self::ResponseValidation => "响应协议校验",
        }
    }
}

impl LlmFailureKind {
    pub(crate) fn event_code(self) -> crate::agent::event::ErrorCode {
        use crate::agent::event::ErrorCode;
        match self {
            Self::Unclassified | Self::Protocol { .. } => ErrorCode::LlmError,
            Self::QuotaExceeded => ErrorCode::LlmQuotaExceeded,
            Self::TransportUnavailable => ErrorCode::LlmTransportError,
            Self::Authentication => ErrorCode::LlmAuthentication,
            Self::PermissionDenied => ErrorCode::LlmPermissionDenied,
            Self::RateLimited => ErrorCode::LlmRateLimited,
            Self::ServiceUnavailable => ErrorCode::LlmServiceUnavailable,
            Self::TimedOut => ErrorCode::LlmTimedOut,
            Self::ResponseInterrupted => ErrorCode::LlmResponseInterrupted,
        }
    }

    pub(crate) fn from_event_code(code: &str) -> Option<Self> {
        match code {
            "LlmError" => Some(Self::Unclassified),
            "LlmQuotaExceeded" => Some(Self::QuotaExceeded),
            "LlmTransportError" => Some(Self::TransportUnavailable),
            "LlmAuthentication" => Some(Self::Authentication),
            "LlmPermissionDenied" => Some(Self::PermissionDenied),
            "LlmRateLimited" => Some(Self::RateLimited),
            "LlmServiceUnavailable" => Some(Self::ServiceUnavailable),
            "LlmTimedOut" => Some(Self::TimedOut),
            "LlmResponseInterrupted" => Some(Self::ResponseInterrupted),
            _ => None,
        }
    }

    pub fn visible_notice(&self) -> Option<&'static str> {
        match self {
            Self::Unclassified => None,
            Self::QuotaExceeded => {
                Some("当前模型用量已达限制。请检查服务商额度，恢复后可继续任务。")
            }
            Self::TransportUnavailable => {
                Some("模型服务连接失败或请求超时。请检查网络与系统代理，连接恢复后可继续任务。")
            }
            Self::Authentication => {
                Some("模型身份验证失败。请检查 API Key 或登录状态，恢复后可继续任务。")
            }
            Self::PermissionDenied => {
                Some("模型服务拒绝了请求。请检查账号授权和模型权限，恢复后可继续任务。")
            }
            Self::RateLimited => Some("模型请求过于频繁。稍等片刻后可继续任务。"),
            Self::ServiceUnavailable => Some("模型服务暂时不可用。服务恢复后可继续任务。"),
            Self::TimedOut => Some("等待模型服务响应超时。请检查网络，连接恢复后可继续任务。"),
            Self::ResponseInterrupted => Some("模型输出中途断开。连接恢复后可继续任务。"),
            Self::Protocol { stage, .. } => Some(stage.notice()),
        }
    }

    /// A scoped recovery surface can include the numeric HTTP status without
    /// exposing raw adapter diagnostics, response bodies or supplied text.
    pub(crate) fn visible_detail(&self) -> Option<String> {
        let notice = self.visible_notice()?;
        let Self::Protocol { stage, http_status } = self else {
            return Some(notice.into());
        };
        let status = http_status
            .map(|status| format!("；HTTP {status}"))
            .unwrap_or_default();
        Some(format!("{notice}（阶段：{}{status}）", stage.label()))
    }
}

/// Diagnostics remain private; only the closed kind can select a visible notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmFailure {
    pub kind: LlmFailureKind,
    pub diagnostic: String,
}

impl LlmFailure {
    pub(super) fn at_protocol_stage(
        mut self,
        stage: LlmProtocolStage,
        http_status: Option<u16>,
    ) -> Self {
        match &mut self.kind {
            LlmFailureKind::Unclassified => {
                self.kind = LlmFailureKind::Protocol { stage, http_status }
            }
            LlmFailureKind::Protocol {
                http_status: status,
                ..
            } if status.is_none() => *status = http_status,
            _ => {} // Never overwrite a more specific recovery classification.
        }
        self
    }
}

impl From<String> for LlmFailure {
    fn from(diagnostic: String) -> Self {
        Self {
            kind: LlmFailureKind::Unclassified,
            diagnostic,
        }
    }
}

impl From<&str> for LlmFailure {
    fn from(diagnostic: &str) -> Self {
        diagnostic.to_owned().into()
    }
}

/// Trait for LLM providers
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Get the provider name
    fn name(&self) -> &str;
    /// Synthetic acceptance only: hashes of the prepared body, never raw
    /// reasoning, credentials or a promise of server-side cache affinity.
    #[cfg(test)]
    fn latest_request_fingerprint(&self) -> Option<ProviderRequestFingerprint> {
        None
    }
    /// Explicit synthetic compatibility probe only; never changes a normal
    /// provider binding or retries inference when the route rejects the field.
    #[cfg(test)]
    fn enable_cache_comparison(&self) -> bool {
        false
    }
    /// Synthetic Plan probe only: leave implicit caching in its default mode.
    #[cfg(test)]
    fn enable_implicit_cache_breakpoint(&self) -> bool {
        false
    }
    #[cfg(test)]
    fn latest_cache_diagnostics(&self) -> Option<ProviderCacheDiagnostics> {
        None
    }
    #[cfg(test)]
    fn latest_response_metadata(&self) -> Option<ProviderResponseMetadata> {
        None
    }
    /// Wire support only, not a claim that an arbitrary configured model has vision.
    fn supports_tool_images(&self) -> bool {
        false
    }
    fn latest_usage(&self) -> Option<ProviderUsage> {
        None
    }

    /// Chat completion with optional tool support
    /// `thinking` - optional thinking settings for extended thinking (provider-specific)
    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, String>;

    /// Streaming completion. Providers that do not expose an incremental API
    /// keep the same contract by forwarding the completed visible text once.
    /// The runner can therefore render one consistent live activity surface
    /// without requiring every provider to support SSE.
    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
        _delta_sender: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<LlmResponse, String> {
        self.chat(messages, tools, thinking).await
    }

    /// Preserve compatibility for providers without a machine-code classifier.
    async fn chat_classified(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, LlmFailure> {
        self.chat(messages, tools, thinking)
            .await
            .map_err(Into::into)
    }

    async fn chat_stream_classified(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
        delta_sender: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<LlmResponse, LlmFailure> {
        self.chat_stream(messages, tools, thinking, delta_sender)
            .await
            .map_err(Into::into)
    }

    /// Issue #092 / G2: Refresh the API key at runtime without requiring a
    /// mutable reference. Implementations store `api_key` behind `Arc<Mutex<...>>`
    /// internally so `refresh_api_key` can be called on `&self`.
    /// Returns `true` if the key was refreshed, `false` if not supported.
    fn refresh_api_key(&self, _api_key: &str) -> bool {
        // Default: not supported
        false
    }

    /// Issue #096 / G3: Switch the model at runtime.
    /// Implementations store `model` behind `Arc<Mutex<...>>` so `set_model`
    /// can be called on `&self` without requiring a mutable reference.
    /// Returns `true` if the model was set, `false` if not supported.
    fn set_model(&self, _model: &str) -> bool {
        // Default: not supported
        false
    }
}

/// Provider configuration (can come from settings table or env vars)
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Provider identifier: "deepseek", "openai", "anthropic", "ollama", etc.
    pub provider: String,
    /// Model name (e.g. `DEEPSEEK_DEFAULT_MODEL`, "claude-sonnet-4-6").
    ///
    /// Issue #103: New defaults must point to V4 names. Legacy `deepseek-chat`
    /// is still accepted by `context_window_tokens` for backward compatibility
    /// with old user configs, but is no longer accepted by the DeepSeek API.
    pub model: String,
    /// API base URL
    pub base_url: String,
    /// API key (empty for Ollama)
    pub api_key: String,
    /// Max completion tokens
    pub max_tokens: u32,
    /// Temperature (0.0 - 2.0)
    pub temperature: f64,
    /// None preserves legacy provider-based protocol selection.
    pub protocol: Option<ModelProtocol>,
    pub auth_mode: ModelAuthMode,
    pub credential_ref: Option<String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            provider: "deepseek".to_string(),
            model: DEEPSEEK_DEFAULT_MODEL.to_string(),
            base_url: "https://api.deepseek.com".to_string(),
            api_key: String::new(),
            max_tokens: 4096,
            temperature: 0.7,
            protocol: None,
            auth_mode: ModelAuthMode::ApiKey,
            credential_ref: None,
        }
    }
}

/// Fallback for custom endpoints whose model metadata does not expose a
/// context limit. This is intentionally not a user setting: an arbitrary
/// OpenAI-compatible `/models` response cannot reliably report the limit.
pub const FALLBACK_CONTEXT_WINDOW_TOKENS: usize = 8_192;

/// Return the total context window advertised for a curated provider/model.
///
/// The value represents the model's complete request budget (input plus
/// output). Callers must reserve the configured completion budget before
/// deciding how much conversation history to retain.
pub fn context_window_tokens(config: &ProviderConfig) -> usize {
    let provider = config.provider.trim().to_ascii_lowercase();
    let model = config.model.trim().to_ascii_lowercase();

    match (provider.as_str(), model.as_str()) {
        // https://developers.openai.com/api/docs/models/gpt-6-luna
        // https://developers.openai.com/api/docs/models/gpt-6-sol
        // https://developers.openai.com/api/docs/models/gpt-6.1-sol
        // Public API window, not the smaller default used by Codex/ChatGPT.
        // Unverified aliases and custom providers retain the safe fallback.
        ("openai", "gpt-6-luna" | "gpt-6-sol" | "gpt-6.1-sol") => 1_050_000,
        // DeepSeek V4 Pro/Flash expose a 1M-token context window. The legacy
        // `deepseek-chat` / `deepseek-reasoner` names are kept here only so
        // users still on pre-V4 config keep correct context sizing — the API
        // itself rejects these names (Issue #103). Use V4 names for new work.
        ("deepseek", model)
            if model.starts_with("deepseek-v4-")
                || matches!(model, "deepseek-chat" | "deepseek-reasoner") =>
        {
            1_000_000
        }
        // Gemini 2.5 models have a 1,048,576-token input context.
        ("google", model) if model.starts_with("gemini-2.5-") => 1_048_576,
        // Models currently curated in the Groq selector share its 131,072
        // token context window.
        ("groq", model)
            if matches!(
                model,
                "openai/gpt-oss-120b"
                    | "openai/gpt-oss-20b"
                    | "llama-3.3-70b-versatile"
                    | "llama-3.1-8b-instant"
                    | "groq/compound"
            ) =>
        {
            131_072
        }
        _ => FALLBACK_CONTEXT_WINDOW_TOKENS,
    }
}

/// Get the default base URL for a provider
pub fn get_default_base_url(provider: &str) -> String {
    match provider {
        "deepseek" => "https://api.deepseek.com".to_string(),
        "openai" => "https://api.openai.com/v1".to_string(),
        "anthropic" => "https://api.anthropic.com".to_string(),
        "ollama" => "http://localhost:11434/v1".to_string(),
        "groq" => "https://api.groq.com/openai/v1".to_string(),
        "google" => "https://generativelanguage.googleapis.com/v1beta/openai".to_string(),
        _ => "".to_string(),
    }
}

/// Compatibility entry point for API-key providers. OAuth is resolved by the app factory.
pub fn create_provider_from_config(
    config: &ProviderConfig,
) -> Result<Box<dyn LlmProvider>, String> {
    let mut config = config.clone();
    if config.requires_api_key() && config.api_key.is_empty() {
        config.api_key = crate::commands::api_config::api_key_from_environment(&config.provider)
            .unwrap_or_default();
    }
    create_provider_with_credentials(&config, None)
}
