//! One resolution seam: vendor identity is not a wire protocol or a credential.

use super::{
    auth::{ModelCredentialSource, StaticCredentialSource},
    *,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

impl ProviderConfig {
    pub fn effective_protocol(&self) -> ModelProtocol {
        self.protocol.unwrap_or(if self.provider == "anthropic" {
            ModelProtocol::AnthropicMessages
        } else {
            ModelProtocol::OpenaiChatCompletions
        })
    }

    pub fn requires_api_key(&self) -> bool {
        self.auth_mode == ModelAuthMode::ApiKey && self.provider != "ollama"
    }

    pub fn credential_identity(&self) -> String {
        self.credential_ref
            .clone()
            .unwrap_or_else(|| "keychain:default".into())
    }

    pub fn profile_identity(&self) -> String {
        let default_url = get_default_base_url(&self.provider);
        if self.protocol.is_none()
            && self.auth_mode == ModelAuthMode::ApiKey
            && self.provider != "custom"
            && self.base_url.trim_end_matches('/') == default_url.trim_end_matches('/')
        {
            return format!("provider:{}", self.provider);
        }
        let signature = format!(
            "{:?}|{:?}|{}",
            self.effective_protocol(),
            self.auth_mode,
            self.base_url.trim_end_matches('/')
        );
        format!(
            "provider:{}:v2:{:x}",
            self.provider,
            Sha256::digest(signature.as_bytes())
        )
    }

    pub fn validate_connection(&self) -> Result<(), String> {
        if self.provider.trim().is_empty() || self.model.trim().is_empty() {
            return Err("Model provider and model are required".into());
        }
        if self.auth_mode == ModelAuthMode::ChatgptPlan {
            if self.provider != "openai"
                || self.effective_protocol() != ModelProtocol::OpenaiResponses
                || self.base_url.trim_end_matches('/') != "https://api.openai.com/v1"
                || self
                    .credential_ref
                    .as_deref()
                    .unwrap_or_default()
                    .is_empty()
            {
                return Err(
                    "ChatGPT plan requires an authorized OpenAI Responses connection".into(),
                );
            }
        } else {
            let base = if self.base_url.is_empty() {
                get_default_base_url(&self.provider)
            } else {
                self.base_url.clone()
            };
            let url = reqwest::Url::parse(&base).map_err(|_| "Invalid model API address")?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(
                    "Model API address must be HTTP(S), without credentials or fragment".into(),
                );
            }
        }
        Ok(())
    }
}

pub fn create_provider_with_credentials(
    config: &ProviderConfig,
    source: Option<Arc<dyn ModelCredentialSource>>,
) -> Result<Box<dyn LlmProvider>, String> {
    let mut config = config.clone();
    if config.base_url.is_empty() {
        config.base_url = get_default_base_url(&config.provider);
    }
    // Compatibility with pre-profile OpenAI configs initialized from DeepSeek defaults.
    if config.provider == "openai"
        && config.protocol.is_none()
        && config.base_url == "https://api.deepseek.com"
    {
        config.base_url = get_default_base_url("openai");
    }
    config.validate_connection()?;
    if config.auth_mode == ModelAuthMode::ChatgptPlan && source.is_none() {
        return Err("ChatGPT plan needs sign in; API keys are never a fallback".into());
    }
    if config.auth_mode == ModelAuthMode::None {
        config.api_key.clear();
    }
    if config.requires_api_key() && config.api_key.trim().is_empty() {
        return Err("API Key is required for the selected provider".into());
    }
    match config.effective_protocol() {
        ModelProtocol::OpenaiResponses => {
            let credentials = source.unwrap_or_else(|| {
                StaticCredentialSource::new(config.api_key.clone(), config.credential_identity())
            });
            Ok(Box::new(OpenAiResponsesProvider::new(
                &config,
                credentials,
            )?))
        }
        ModelProtocol::AnthropicMessages => Ok(Box::new(
            AnthropicProvider::new(
                config.api_key.clone(),
                config.model.clone(),
                config.max_tokens,
                config.temperature,
            )
            .with_endpoint(config.base_url, config.provider),
        )),
        ModelProtocol::OpenaiChatCompletions => Ok(Box::new(OpenAiCompatibleProvider::new(
            config.api_key,
            config.model,
            config.base_url,
            config.max_tokens,
            config.temperature,
            config.provider,
        ))),
    }
}

pub fn create_provider_for_app(
    config: &ProviderConfig,
    app: Option<&tauri::AppHandle>,
) -> Result<Box<dyn LlmProvider>, String> {
    let source = if config.auth_mode == ModelAuthMode::ChatgptPlan {
        let app = app.ok_or("ChatGPT plan credential store is unavailable")?;
        Some(super::chatgpt_auth::credential_source(
            app,
            config.credential_ref.as_deref().unwrap_or_default(),
        )?)
    } else {
        None
    };
    create_provider_with_credentials(config, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_protocol_is_independent_of_vendor_and_auth() {
        let config = ProviderConfig {
            provider: "custom".into(),
            model: "local".into(),
            base_url: "http://localhost:8000/v1".into(),
            protocol: Some(ModelProtocol::AnthropicMessages),
            auth_mode: ModelAuthMode::None,
            ..ProviderConfig::default()
        };
        assert!(!config.requires_api_key());
        assert_eq!(
            create_provider_with_credentials(&config, None)
                .unwrap()
                .name(),
            "custom"
        );
    }

    #[test]
    fn subscription_never_falls_back_to_key_or_custom_endpoint() {
        let mut config = ProviderConfig {
            provider: "openai".into(),
            model: "fixture".into(),
            base_url: "https://api.openai.com/v1".into(),
            protocol: Some(ModelProtocol::OpenaiResponses),
            auth_mode: ModelAuthMode::ChatgptPlan,
            credential_ref: Some("fixture".into()),
            api_key: "unused-fixture-key".into(),
            ..ProviderConfig::default()
        };
        assert!(create_provider_with_credentials(&config, None).is_err());
        config.base_url = "https://example.com/v1".into();
        assert!(config.validate_connection().is_err());
    }

    #[test]
    fn endpoint_and_protocol_changes_invalidate_delegated_profile() {
        let original = ProviderConfig::default();
        assert_eq!(original.profile_identity(), "provider:deepseek");
        let mut changed = original.clone();
        changed.base_url = "https://example.com/v1".into();
        assert_ne!(original.profile_identity(), changed.profile_identity());
        changed = original.clone();
        changed.protocol = Some(ModelProtocol::OpenaiResponses);
        assert_ne!(original.profile_identity(), changed.profile_identity());
    }
}
