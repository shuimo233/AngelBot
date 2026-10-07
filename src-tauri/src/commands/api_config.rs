//! Model provider configuration.
//!
//! Non-secret fields are stored in `app_config.json`; credentials are stored
//! only in the OS keychain. Environment variables override persisted values.

use crate::keychain::ApiCredentials;
use crate::llm::{ModelAuthMode, ModelProtocol, ProviderConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

const CONFIG_FILE_NAME: &str = "app_config.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiProviderConfig {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    pub max_tokens: u32,
    pub temperature: f64,
    #[serde(default)]
    pub has_api_key: bool,
    #[serde(default = "default_credential_source")]
    pub credential_source: String,
    #[serde(default)]
    pub protocol: Option<ModelProtocol>,
    #[serde(default)]
    pub auth_mode: ModelAuthMode,
    #[serde(default)]
    pub credential_ref: Option<String>,
    #[serde(default)]
    pub plan_connected: bool,
    #[serde(default)]
    pub plan_enabled: bool,
    #[serde(default)]
    pub plan_account_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredApiProviderConfig {
    provider: String,
    model: String,
    base_url: String,
    max_tokens: u32,
    temperature: f64,
    #[serde(default)]
    protocol: Option<ModelProtocol>,
    #[serde(default)]
    auth_mode: ModelAuthMode,
    #[serde(default)]
    credential_ref: Option<String>,
}

fn default_credential_source() -> String {
    "none".to_string()
}

impl Default for ApiProviderConfig {
    fn default() -> Self {
        Self {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-20250514".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: String::new(),
            max_tokens: 4096,
            temperature: 0.7,
            has_api_key: false,
            credential_source: default_credential_source(),
            protocol: None,
            auth_mode: ModelAuthMode::ApiKey,
            credential_ref: None,
            plan_connected: false,
            plan_enabled: false,
            plan_account_label: None,
        }
    }
}

impl From<&ApiProviderConfig> for StoredApiProviderConfig {
    fn from(value: &ApiProviderConfig) -> Self {
        Self {
            provider: value.provider.trim().to_string(),
            model: value.model.trim().to_string(),
            base_url: value.base_url.trim().to_string(),
            max_tokens: value.max_tokens,
            temperature: value.temperature,
            protocol: value.protocol,
            auth_mode: value.auth_mode,
            credential_ref: value.credential_ref.clone(),
        }
    }
}

impl From<StoredApiProviderConfig> for ApiProviderConfig {
    fn from(value: StoredApiProviderConfig) -> Self {
        Self {
            provider: value.provider,
            model: value.model,
            base_url: value.base_url,
            max_tokens: value.max_tokens,
            temperature: value.temperature,
            protocol: value.protocol,
            auth_mode: value.auth_mode,
            credential_ref: value.credential_ref,
            ..Self::default()
        }
    }
}

pub fn get_config_dir() -> PathBuf {
    #[cfg(feature = "desktop-e2e")]
    {
        let dir = std::env::temp_dir().join(format!(
            "angelbot-desktop-e2e-config-{}",
            std::process::id()
        ));
        if !dir.exists() {
            let _ = fs::create_dir_all(&dir);
        }
        return dir;
    }

    #[cfg(not(feature = "desktop-e2e"))]
    let dir = dirs::config_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap())
        .join("AngelBot");
    #[cfg(not(feature = "desktop-e2e"))]
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    #[cfg(not(feature = "desktop-e2e"))]
    dir
}

pub fn get_config_path() -> PathBuf {
    get_config_dir().join(CONFIG_FILE_NAME)
}

pub fn load_config_from_file() -> ApiProviderConfig {
    load_config_from_file_checked().unwrap_or_default()
}

fn parse_model_config(content: &str) -> Result<ApiProviderConfig, String> {
    serde_json::from_str::<StoredApiProviderConfig>(&content)
        .map(ApiProviderConfig::from)
        .or_else(|_| {
            serde_json::from_str::<ApiProviderConfig>(&content).map(|mut config| {
                config.api_key.clear();
                config.has_api_key = false;
                config.credential_source = default_credential_source();
                config
            })
        })
        .map_err(|_| {
            "Model configuration is invalid; repair it in Settings before running tasks".into()
        })
}

pub(crate) fn load_config_from_file_checked() -> Result<ApiProviderConfig, String> {
    match fs::read_to_string(get_config_path()) {
        Ok(content) => parse_model_config(&content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ApiProviderConfig::default())
        }
        Err(_) => Err("Model configuration cannot be read; repair it in Settings".into()),
    }
}

fn legacy_file_api_key() -> Option<String> {
    let content = fs::read_to_string(get_config_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    value
        .get("api_key")?
        .as_str()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
}

pub fn save_config_to_file(config: &ApiProviderConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&StoredApiProviderConfig::from(config))
        .map_err(|error| format!("serialize model config: {error}"))?;
    fs::write(get_config_path(), json).map_err(|error| format!("write model config: {error}"))
}

pub(crate) fn api_key_from_environment(provider: &str) -> Option<String> {
    #[cfg(feature = "desktop-e2e")]
    {
        let _ = provider;
        return None;
    }

    #[cfg(not(feature = "desktop-e2e"))]
    let names: &[&str] = match provider {
        "anthropic" => &["ANTHROPIC_API_KEY", "ANGELBOT_API_KEY"],
        "openai" => &["OPENAI_API_KEY", "ANGELBOT_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY", "ANGELBOT_API_KEY"],
        "google" => &["GEMINI_API_KEY", "GOOGLE_API_KEY", "ANGELBOT_API_KEY"],
        "groq" => &["GROQ_API_KEY", "ANGELBOT_API_KEY"],
        "azure" => &["AZURE_OPENAI_KEY", "ANGELBOT_API_KEY"],
        "ollama" => &[],
        _ => &["ANGELBOT_API_KEY"],
    };
    #[cfg(not(feature = "desktop-e2e"))]
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

pub(crate) fn provider_requires_key(provider: &str) -> bool {
    provider != "ollama"
}

/// Load non-secret provider configuration plus a provider-matched environment
/// key. Keychain credentials are resolved by the model factory.
pub fn load_api_config_internal() -> Option<ProviderConfig> {
    #[cfg(feature = "desktop-e2e")]
    {
        let config = ApiProviderConfig::default();
        return Some(ProviderConfig {
            provider: config.provider,
            model: config.model,
            base_url: config.base_url,
            api_key: String::new(),
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            protocol: config.protocol,
            auth_mode: config.auth_mode,
            credential_ref: config.credential_ref,
        });
    }

    #[cfg(not(feature = "desktop-e2e"))]
    let file_config = load_config_from_file();
    #[cfg(not(feature = "desktop-e2e"))]
    let provider = std::env::var("ANGELBOT_API_PROVIDER")
        .or_else(|_| std::env::var("ANGELBOT_PROVIDER"))
        .or_else(|_| std::env::var("LLM_PROVIDER"))
        .unwrap_or_else(|_| file_config.provider.clone());
    #[cfg(not(feature = "desktop-e2e"))]
    let model = std::env::var("ANGELBOT_API_MODEL")
        .or_else(|_| std::env::var("ANGELBOT_MODEL"))
        .or_else(|_| std::env::var("LLM_MODEL"))
        .unwrap_or_else(|_| file_config.model.clone());
    #[cfg(not(feature = "desktop-e2e"))]
    let base_url = std::env::var("ANGELBOT_API_BASE_URL")
        .or_else(|_| std::env::var("ANGELBOT_BASE_URL"))
        .or_else(|_| std::env::var("LLM_BASE_URL"))
        .unwrap_or_else(|_| file_config.base_url.clone());
    #[cfg(not(feature = "desktop-e2e"))]
    let max_tokens = std::env::var("ANGELBOT_MAX_TOKENS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(file_config.max_tokens);
    #[cfg(not(feature = "desktop-e2e"))]
    let temperature = std::env::var("ANGELBOT_TEMPERATURE")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(file_config.temperature);
    #[cfg(not(feature = "desktop-e2e"))]
    let same_provider = provider == file_config.provider;
    #[cfg(not(feature = "desktop-e2e"))]
    let auth_mode = if same_provider {
        file_config.auth_mode
    } else {
        ModelAuthMode::ApiKey
    };
    #[cfg(not(feature = "desktop-e2e"))]
    let api_key = if auth_mode == ModelAuthMode::ApiKey {
        api_key_from_environment(&provider).unwrap_or_default()
    } else {
        String::new()
    };
    #[cfg(not(feature = "desktop-e2e"))]
    let credential_ref = if !api_key.is_empty() {
        Some(format!(
            "keychain:environment:{:x}",
            Sha256::digest(api_key.as_bytes())
        ))
    } else {
        same_provider
            .then_some(file_config.credential_ref)
            .flatten()
    };
    #[cfg(not(feature = "desktop-e2e"))]
    Some(ProviderConfig {
        api_key,
        protocol: same_provider.then_some(file_config.protocol).flatten(),
        auth_mode,
        credential_ref,
        provider,
        model,
        base_url,
        max_tokens,
        temperature,
    })
}

pub fn load_full_config(app: &tauri::AppHandle) -> ApiProviderConfig {
    let runtime = load_api_config_internal().unwrap_or_default();
    if runtime.auth_mode != ModelAuthMode::ApiKey {
        let plan = crate::llm::chatgpt_auth::status(app).ok();
        let matches = plan
            .as_ref()
            .is_some_and(|status| status.credential_ref == runtime.credential_ref);
        return ApiProviderConfig {
            provider: runtime.provider,
            model: runtime.model,
            base_url: runtime.base_url,
            max_tokens: runtime.max_tokens,
            temperature: runtime.temperature,
            protocol: runtime.protocol,
            auth_mode: runtime.auth_mode,
            credential_ref: runtime.credential_ref,
            plan_connected: matches && plan.as_ref().is_some_and(|status| status.connected),
            plan_enabled: matches && plan.as_ref().is_some_and(|status| status.plan_enabled),
            plan_account_label: plan.and_then(|status| status.account_label),
            credential_source: if runtime.auth_mode == ModelAuthMode::ChatgptPlan {
                "chatgpt_plan".into()
            } else {
                "none".into()
            },
            ..ApiProviderConfig::default()
        };
    }
    let mut credentials = crate::keychain::read_credentials(app).unwrap_or_default();

    // Migrate old JSON secrets once, then immediately rewrite the file without them.
    if credentials.api_key.is_empty() {
        if let Some(legacy_key) = legacy_file_api_key() {
            credentials = ApiCredentials {
                provider: runtime.provider.clone(),
                model: runtime.model.clone(),
                api_key: legacy_key,
            };
            if crate::keychain::write_credentials(app, &credentials).is_ok() {
                let _ = save_config_to_file(&ApiProviderConfig {
                    provider: runtime.provider.clone(),
                    model: runtime.model.clone(),
                    base_url: runtime.base_url.clone(),
                    max_tokens: runtime.max_tokens,
                    temperature: runtime.temperature,
                    ..ApiProviderConfig::default()
                });
            }
        }
    }

    let env_has_key = !runtime.api_key.is_empty();
    let keychain_matches = !credentials.api_key.is_empty()
        && (credentials.provider.is_empty() || credentials.provider == runtime.provider);
    ApiProviderConfig {
        provider: runtime.provider,
        model: runtime.model,
        base_url: runtime.base_url,
        api_key: String::new(),
        max_tokens: runtime.max_tokens,
        temperature: runtime.temperature,
        protocol: runtime.protocol,
        auth_mode: runtime.auth_mode,
        credential_ref: runtime.credential_ref,
        has_api_key: env_has_key || keychain_matches,
        credential_source: if env_has_key {
            "environment".into()
        } else if keychain_matches {
            "keychain".into()
        } else {
            "none".into()
        },
        ..ApiProviderConfig::default()
    }
}

pub fn save_full_config(app: &tauri::AppHandle, config: &ApiProviderConfig) -> Result<(), String> {
    let provider = config.provider.trim();
    let model = config.model.trim();
    let base_url = config.base_url.trim();
    if provider.is_empty() || model.is_empty() {
        return Err("Model provider and model are required".into());
    }
    if matches!(provider, "custom" | "azure") && base_url.is_empty() {
        return Err("API address is required for this provider".into());
    }

    let profile = ProviderConfig {
        provider: provider.into(),
        model: model.into(),
        base_url: base_url.into(),
        api_key: config.api_key.clone(),
        protocol: config.protocol,
        auth_mode: config.auth_mode,
        credential_ref: config.credential_ref.clone(),
        max_tokens: config.max_tokens,
        temperature: config.temperature,
    };
    profile.validate_connection()?;
    if config.auth_mode == ModelAuthMode::ChatgptPlan {
        let status = crate::llm::chatgpt_auth::status(app)?;
        if !status.connected
            || !status.plan_enabled
            || status.credential_ref != config.credential_ref
        {
            return Err("Sign in to ChatGPT and authorize plan access before saving".into());
        }
        return save_config_to_file(config);
    }
    if config.auth_mode == ModelAuthMode::None {
        let previous = load_config_from_file();
        let mut saved = config.clone();
        saved.api_key.clear();
        saved.credential_ref = if previous.auth_mode == ModelAuthMode::None
            && previous.provider == config.provider
            && previous.protocol == config.protocol
            && previous.base_url == config.base_url
        {
            previous.credential_ref
        } else {
            None
        }
        .or_else(|| Some(format!("keychain:public:{}", uuid::Uuid::new_v4())));
        return save_config_to_file(&saved);
    }

    let entered_key = config.api_key.trim();
    let existing = crate::keychain::read_credentials(app).unwrap_or_default();
    let existing_matches = !existing.api_key.is_empty()
        && (existing.provider.is_empty() || existing.provider == provider);
    if provider_requires_key(provider)
        && entered_key.is_empty()
        && !existing_matches
        && api_key_from_environment(provider).is_none()
    {
        return Err("API Key is required for the selected provider".into());
    }

    if !entered_key.is_empty() {
        crate::keychain::write_credentials(
            app,
            &ApiCredentials {
                provider: provider.to_string(),
                model: model.to_string(),
                api_key: entered_key.to_string(),
            },
        )?;
    } else if existing_matches && (existing.provider != provider || existing.model != model) {
        crate::keychain::write_credentials(
            app,
            &ApiCredentials {
                provider: provider.to_string(),
                model: model.to_string(),
                api_key: existing.api_key,
            },
        )?;
    }
    let stored = load_config_from_file();
    let mut saved = config.clone();
    saved.credential_ref = if entered_key.is_empty()
        && stored.auth_mode == ModelAuthMode::ApiKey
        && stored.provider == config.provider
        && stored.base_url == config.base_url
        && stored.protocol == config.protocol
    {
        stored
            .credential_ref
            .or_else(|| Some(format!("keychain:profile:{}", uuid::Uuid::new_v4())))
    } else {
        Some(format!("keychain:profile:{}", uuid::Uuid::new_v4()))
    };
    save_config_to_file(&saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_config_never_serializes_api_key() {
        let config = ApiProviderConfig {
            api_key: "secret-value".into(),
            has_api_key: true,
            ..ApiProviderConfig::default()
        };
        let json = serde_json::to_string(&StoredApiProviderConfig::from(&config)).unwrap();
        assert!(!json.contains("secret-value"));
        // `auth_mode: "api_key"` is non-secret configuration, not a key field.
        let stored: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(stored.get("api_key").is_none());
        assert!(stored.get("has_api_key").is_none());
    }

    #[test]
    fn only_ollama_is_keyless() {
        assert!(!provider_requires_key("ollama"));
        assert!(provider_requires_key("anthropic"));
        assert!(provider_requires_key("custom"));
    }

    #[test]
    fn legacy_config_defaults_to_existing_authentication_and_protocol() {
        let config: StoredApiProviderConfig = serde_json::from_value(serde_json::json!({
            "provider":"deepseek", "model":"fixture", "base_url":"https://api.deepseek.com",
            "max_tokens":4096, "temperature":0.7
        }))
        .unwrap();
        assert_eq!(config.auth_mode, ModelAuthMode::ApiKey);
        assert_eq!(config.protocol, None);
        assert_eq!(config.credential_ref, None);
        let mut invalid = serde_json::json!({ "provider":"openai", "model":"fixture", "base_url":"https://api.openai.com/v1", "max_tokens":4096, "temperature":0.7, "auth_mode":"unsupported_plan" });
        assert!(parse_model_config(&invalid.to_string()).is_err());
        invalid["auth_mode"] = serde_json::json!("chatgpt_plan");
        assert_eq!(
            parse_model_config(&invalid.to_string()).unwrap().auth_mode,
            ModelAuthMode::ChatgptPlan
        );
    }

    #[test]
    fn persisted_plan_config_keeps_only_nonsecret_identity_not_readiness() {
        let original = ApiProviderConfig {
            provider: "openai".into(),
            model: "fixture".into(),
            base_url: "https://api.openai.com/v1".into(),
            auth_mode: ModelAuthMode::ChatgptPlan,
            protocol: Some(ModelProtocol::OpenaiResponses),
            credential_ref: Some("opaque-fixture-ref".into()),
            api_key: "must-not-persist".into(),
            plan_enabled: true,
            plan_connected: true,
            ..ApiProviderConfig::default()
        };
        let json = serde_json::to_string(&StoredApiProviderConfig::from(&original)).unwrap();
        assert!(!json.contains("must-not-persist"));
        assert!(!json.contains("plan_enabled"));
        let restored = ApiProviderConfig::from(
            serde_json::from_str::<StoredApiProviderConfig>(&json).unwrap(),
        );
        assert_eq!(restored.credential_ref, original.credential_ref);
        assert_eq!(restored.auth_mode, ModelAuthMode::ChatgptPlan);
        assert!(!restored.plan_enabled);
        assert!(restored.api_key.is_empty());
    }
}
