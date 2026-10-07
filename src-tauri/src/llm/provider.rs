//! Legacy DeepSeek provider — kept for backward compatibility
//!
//! Prefer `create_provider_from_config()` in `mod.rs` for new code.
//! This module delegates to `OpenAiCompatibleProvider` internally.

use super::openai_compat::OpenAiCompatibleProvider;
use super::{
    LlmProvider, LlmResponse, Message, ThinkingSettings, ToolSchema, DEEPSEEK_DEFAULT_MODEL,
};

/// DeepSeek provider (legacy wrapper around OpenAiCompatibleProvider)
pub struct DeepSeekProvider {
    inner: OpenAiCompatibleProvider,
}

impl DeepSeekProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            inner: OpenAiCompatibleProvider::new(
                api_key,
                DEEPSEEK_DEFAULT_MODEL.to_string(),
                "https://api.deepseek.com".to_string(),
                4096,
                0.7,
                "deepseek".to_string(),
            ),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for DeepSeekProvider {
    fn name(&self) -> &str {
        "deepseek"
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, String> {
        self.inner.chat(messages, tools, thinking).await
    }

    fn refresh_api_key(&self, api_key: &str) -> bool {
        self.inner.refresh_api_key(api_key)
    }

    fn set_model(&self, model: &str) -> bool {
        self.inner.set_model(model)
    }
}

/// Create a DeepSeek provider (legacy)
pub fn create_deepseek_provider() -> Result<Box<dyn LlmProvider>, String> {
    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .map_err(|_| "DEEPSEEK_API_KEY not found in environment")?;

    Ok(Box::new(DeepSeekProvider::new(api_key)))
}

/// Create provider from environment variables (legacy — prefer create_provider_from_config)
pub fn create_provider_from_env() -> Result<Box<dyn LlmProvider>, String> {
    // Check for explicit provider selection via env
    let provider_type = std::env::var("LLM_PROVIDER").unwrap_or_else(|_| "deepseek".to_string());

    let config = super::ProviderConfig {
        protocol: None,
        auth_mode: crate::llm::ModelAuthMode::ApiKey,
        credential_ref: None,
        provider: provider_type.clone(),
        model: std::env::var("LLM_MODEL").unwrap_or_else(|_| {
            let p = config_from_env_provider();
            match p.as_str() {
                "openai" => "gpt-4o".to_string(),
                "anthropic" => "claude-sonnet-4-6".to_string(),
                "ollama" => "llama3".to_string(),
                _ => DEEPSEEK_DEFAULT_MODEL.to_string(),
            }
        }),
        base_url: std::env::var("LLM_BASE_URL").unwrap_or_else(|_| {
            let p = config_from_env_provider();
            match p.as_str() {
                "openai" => "https://api.openai.com/v1".to_string(),
                "anthropic" => "https://api.anthropic.com".to_string(),
                "ollama" => "http://localhost:11434/v1".to_string(),
                _ => "https://api.deepseek.com".to_string(),
            }
        }),
        api_key: String::new(), // detected inside create_provider_from_config
        max_tokens: 4096,
        temperature: 0.7,
    };

    super::create_provider_from_config(&config)
}

fn config_from_env_provider() -> String {
    std::env::var("LLM_PROVIDER").unwrap_or_else(|_| {
        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            return "anthropic".to_string();
        }
        if std::env::var("OPENAI_API_KEY").is_ok() {
            return "openai".to_string();
        }
        if std::env::var("DEEPSEEK_API_KEY").is_ok() {
            return "deepseek".to_string();
        }
        "deepseek".to_string()
    })
}
