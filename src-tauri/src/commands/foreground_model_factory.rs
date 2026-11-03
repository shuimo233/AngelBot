//! Foreground model resolution.
//!
//! This module is the seam between a foreground turn and the application's
//! provider configuration.  It owns the order in which non-secret config,
//! keychain credentials, thinking settings, and the provider are resolved.
//! Callers receive a ready provider plus a non-secret identity; credentials
//! never cross this seam.

use crate::keychain::ApiCredentials;
use crate::llm::{
    create_provider_for_app, resolve_thinking_settings, LlmProvider, ModelAuthMode, ProviderConfig,
    ThinkingSettings,
};
use crate::AppState;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;

#[cfg(feature = "desktop-e2e")]
mod desktop_e2e;

/// Inputs that affect one foreground model resolution.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ForegroundModelRequest<'a> {
    pub session_id: &'a str,
    pub requested_thinking_effort: Option<&'a str>,
    pub now: i64,
}

/// Stable, non-secret model identity used by the foreground run and by the
/// internal delegation tool's durable binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForegroundModelIdentity {
    pub provider: String,
    pub model: String,
    pub provider_profile_ref: String,
    pub credential_handle_ref: String,
}

/// The result required by the foreground runner.  API credentials are kept
/// inside the provider implementation and are intentionally not returned as
/// fields or serialized values.
pub(crate) struct ResolvedForegroundModel {
    pub provider: Box<dyn LlmProvider>,
    pub thinking_settings: Option<ThinkingSettings>,
    pub identity: ForegroundModelIdentity,
    /// Non-secret limits captured with this exact provider binding, not re-read
    /// from settings while the turn is being prepared or executed.
    pub context_window_tokens: usize,
    pub max_output_tokens: usize,
}

impl ResolvedForegroundModel {
    /// Optional learning borrows this turn's model identity and resolved effort.
    /// Recheck current configuration before opening credentials; changes defer
    /// learning rather than silently switching providers or reasoning defaults.
    pub(crate) fn completion_review_provider(
        &self,
        current: &ProviderConfig,
        create: impl FnOnce(&ProviderConfig) -> Result<Box<dyn LlmProvider>, String>,
    ) -> Option<(Box<dyn LlmProvider>, Option<ThinkingSettings>)> {
        if current.profile_identity() != self.identity.provider_profile_ref
            || current.credential_identity() != self.identity.credential_handle_ref
            || current.model != self.identity.model
        {
            return None;
        }
        Some((create(current).ok()?, self.thinking_settings.clone()))
    }
}

/// Stable error categories exposed by the model-resolution seam.  The raw
/// provider/keychain error is deliberately not retained: it can contain an
/// endpoint, environment detail, or credential material unsuitable for UI or
/// model context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelResolutionError {
    InvalidConfiguration,
    MissingCredentials,
    CredentialStoreUnavailable,
    InvalidThinkingSettings,
    ProviderUnavailable,
    SessionBindingFailed,
}

impl std::fmt::Display for ModelResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidConfiguration => "LLM provider configuration is invalid",
            Self::MissingCredentials => "LLM provider credentials are not configured",
            Self::CredentialStoreUnavailable => "LLM provider credentials are unavailable",
            Self::InvalidThinkingSettings => "LLM reasoning settings are invalid",
            Self::ProviderUnavailable => "LLM provider could not be initialized",
            Self::SessionBindingFailed => "LLM provider could not be bound to this session",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ModelResolutionError {}

impl ModelResolutionError {
    /// Safe guidance for a model that could not be prepared. A local session
    /// write failure is not a provider problem and keeps ordinary recovery.
    pub(crate) fn provider_unavailable_message(self) -> Option<&'static str> {
        Some(match self {
            Self::InvalidConfiguration => "模型配置无法读取或不完整。请在“模型设置”中检查服务商和模型后发送新消息；不会自动重试已有操作。",
            Self::MissingCredentials => "尚未配置模型凭据。请在“模型设置”中填写 API Key 或连接 ChatGPT 套餐后发送新消息；不会自动重试已有操作。",
            Self::CredentialStoreUnavailable => "无法读取已保存的模型凭据。请在“模型设置”中检查连接后发送新消息；不会自动重试已有操作。",
            Self::InvalidThinkingSettings => "当前模型的思考设置不可用。请在“模型设置”中检查模型与思考设置后发送新消息；不会自动重试已有操作。",
            Self::ProviderUnavailable => "模型暂时无法启动。请在“模型设置”中检查 API 或 ChatGPT 套餐连接后发送新消息；不会自动重试已有操作。",
            Self::SessionBindingFailed => return None,
        })
    }
}

/// Deep module for foreground provider resolution.  It does not create
/// delegations, schedule workers, or expose foreground conversation context.
pub(crate) struct ForegroundModelFactory<'a> {
    app: Option<&'a AppHandle>,
    db: Arc<Mutex<Connection>>,
}

impl<'a> ForegroundModelFactory<'a> {
    pub(crate) fn new(app: Option<&'a AppHandle>, state: &AppState) -> Self {
        Self {
            app,
            db: state.db.clone(),
        }
    }

    pub(crate) fn resolve(
        &self,
        request: ForegroundModelRequest<'_>,
    ) -> Result<ResolvedForegroundModel, ModelResolutionError> {
        #[cfg(feature = "desktop-e2e")]
        if let Some(model) = desktop_e2e::resolve(&self.db, request)? {
            return Ok(model);
        }

        let config = self.resolve_config()?;
        validate_provider_identity(&config)?;

        // Preserve the existing foreground ordering: bind the resolved model
        // before constructing the provider, so a settings change is reflected
        // in the session even when provider initialization fails afterwards.
        bind_session_provider(&self.db, request.session_id, &config, request.now)?;

        let thinking_settings =
            resolve_thinking_settings(&config, request.requested_thinking_effort)
                .map_err(|_| ModelResolutionError::InvalidThinkingSettings)?;

        let provider = create_provider_for_app(&config, self.app)
            .map_err(|_| classify_provider_configuration(&config))?;

        Ok(ResolvedForegroundModel {
            provider,
            thinking_settings,
            context_window_tokens: crate::llm::context_window_tokens(&config),
            max_output_tokens: config.max_tokens as usize,
            identity: ForegroundModelIdentity {
                provider_profile_ref: config.profile_identity(),
                credential_handle_ref: config.credential_identity(),
                provider: config.provider,
                model: config.model,
            },
        })
    }

    fn resolve_config(&self) -> Result<ProviderConfig, ModelResolutionError> {
        resolve_model_config(self.app)
    }
}

pub(crate) fn resolve_model_config(
    app: Option<&AppHandle>,
) -> Result<ProviderConfig, ModelResolutionError> {
    crate::commands::api_config::load_config_from_file_checked()
        .map_err(|_| ModelResolutionError::InvalidConfiguration)?;
    let file_config = crate::commands::api_config::load_api_config_internal();
    let credentials =
        if requires_keychain_api_credentials(&file_config, crate::manual_desktop_acceptance())? {
            read_credentials_safely(app)
        } else {
            Ok(None)
        };
    resolve_config_from_sources(file_config, credentials)
}

/// Decide before reading the credential store. A missing/invalid manual-plan
/// profile must not fall through to an unrelated API-key account.
fn requires_keychain_api_credentials(
    config: &Option<ProviderConfig>,
    plan_only: bool,
) -> Result<bool, ModelResolutionError> {
    if plan_only
        && !config
            .as_ref()
            .is_some_and(|value| value.auth_mode == ModelAuthMode::ChatgptPlan)
    {
        return Err(ModelResolutionError::InvalidConfiguration);
    }
    Ok(!config
        .as_ref()
        .is_some_and(|value| value.auth_mode != ModelAuthMode::ApiKey))
}

/// Resolve the same source precedence used by the previous message command.
/// This pure helper is also the test seam for configuration and keychain
/// fallback without constructing a Tauri AppHandle.
pub(crate) fn resolve_config_from_sources(
    file_config: Option<ProviderConfig>,
    credentials: Result<Option<ApiCredentials>, ()>,
) -> Result<ProviderConfig, ModelResolutionError> {
    if let Some(mut config) = file_config {
        if config.auth_mode == ModelAuthMode::ApiKey && config.api_key.is_empty() {
            if let Ok(Some(creds)) = credentials {
                if !creds.api_key.is_empty()
                    && (config.provider.is_empty()
                        || creds.provider.is_empty()
                        || creds.provider == config.provider)
                {
                    config.api_key = creds.api_key;
                    if config.provider.is_empty() {
                        config.provider = creds.provider;
                    }
                    if config.model.is_empty() {
                        config.model = creds.model;
                    }
                }
            }
        }
        return Ok(config);
    }

    match credentials {
        Ok(Some(creds)) if creds.is_valid() => Ok(ProviderConfig {
            protocol: None,
            auth_mode: crate::llm::ModelAuthMode::ApiKey,
            credential_ref: None,
            provider: creds.provider.clone(),
            model: creds.model.clone(),
            base_url: crate::llm::get_default_base_url(&creds.provider),
            api_key: creds.api_key,
            max_tokens: 4096,
            temperature: 0.7,
        }),
        Ok(Some(_)) => Err(ModelResolutionError::MissingCredentials),
        Ok(None) => Err(ModelResolutionError::MissingCredentials),
        Err(()) => Err(ModelResolutionError::CredentialStoreUnavailable),
    }
}

fn read_credentials_safely(app: Option<&AppHandle>) -> Result<Option<ApiCredentials>, ()> {
    let Some(app) = app else {
        return Ok(None);
    };
    match crate::keychain::read_credentials(app) {
        Ok(credentials) if !credentials.api_key.is_empty() => Ok(Some(credentials)),
        Ok(_) => Ok(None),
        Err(_) => Err(()),
    }
}

fn validate_provider_identity(config: &ProviderConfig) -> Result<(), ModelResolutionError> {
    if config.provider.trim().is_empty() || config.model.trim().is_empty() {
        return Err(ModelResolutionError::InvalidConfiguration);
    }
    Ok(())
}

fn classify_provider_configuration(config: &ProviderConfig) -> ModelResolutionError {
    if config.requires_api_key() && config.api_key.trim().is_empty() {
        ModelResolutionError::MissingCredentials
    } else {
        ModelResolutionError::ProviderUnavailable
    }
}

fn bind_session_provider(
    db: &Arc<Mutex<Connection>>,
    session_id: &str,
    config: &ProviderConfig,
    now: i64,
) -> Result<(), ModelResolutionError> {
    let conn = db
        .lock()
        .map_err(|_| ModelResolutionError::SessionBindingFailed)?;
    bind_session_provider_in_conn(&conn, session_id, config, now)
}

pub(crate) fn bind_session_provider_in_conn(
    conn: &Connection,
    session_id: &str,
    config: &ProviderConfig,
    now: i64,
) -> Result<(), ModelResolutionError> {
    validate_provider_identity(config)?;
    let existing: Option<String> = conn
        .query_row(
            "SELECT metadata FROM sessions WHERE id=?1",
            [session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| ModelResolutionError::SessionBindingFailed)?
        .flatten();
    let mut metadata = existing
        .as_deref()
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| match existing {
            Some(value) => serde_json::json!({"legacyMetadata":value}),
            None => serde_json::json!({}),
        });
    metadata["modelConnectionBinding"] = serde_json::json!({
        "version":1, "profileRef":config.profile_identity(), "credentialRef":config.credential_identity(),
        "modelRef":format!("model:{}",config.model),
    });
    conn.execute(
        "UPDATE sessions SET agent_provider = ?1, agent_model = ?2, updated_at = ?3, metadata=?5 WHERE id = ?4",
        params![config.provider, config.model, now, session_id, metadata.to_string()],
    )
    .map_err(|_| ModelResolutionError::SessionBindingFailed)?;
    Ok(())
}

pub(crate) fn session_model_binding(
    conn: &Connection,
    session_id: &str,
    provider: &str,
    model: &str,
) -> Result<crate::agent::delegated_model_binding::DelegatedModelBindingRequest, String> {
    let metadata: Option<String> = conn
        .query_row(
            "SELECT metadata FROM sessions WHERE id=?1",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|_| "the delegation model binding is unavailable")?;
    let value = metadata
        .as_deref()
        .and_then(|metadata| serde_json::from_str::<serde_json::Value>(metadata).ok());
    let (profile, credential) = if let Some(binding) = value
        .as_ref()
        .and_then(|value| value.get("modelConnectionBinding"))
    {
        if binding["version"] != 1 || binding["modelRef"] != format!("model:{model}") {
            return Err("the delegation model binding is invalid".into());
        }
        (
            binding["profileRef"]
                .as_str()
                .ok_or("missing model profile")?
                .to_string(),
            binding["credentialRef"]
                .as_str()
                .ok_or("missing credential reference")?
                .to_string(),
        )
    } else {
        (format!("provider:{provider}"), "keychain:default".into())
    };
    crate::agent::delegated_model_binding::DelegatedModelBindingRequest::new(
        profile,
        format!("model:{model}"),
        credential,
        1,
    )
    .map_err(|_| "the delegation model binding is invalid".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrate;

    #[test]
    fn confirmation_rebuild_uses_durable_connection_without_runtime_configuration() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute("INSERT INTO sessions(id,title,created_at,updated_at,metadata) VALUES('binding','fixture',1,1,'{\"preserved\":true}')", []).unwrap();
        let config = ProviderConfig {
            provider: "openai".into(),
            model: "fixture".into(),
            protocol: Some(crate::llm::ModelProtocol::OpenaiResponses),
            auth_mode: ModelAuthMode::ChatgptPlan,
            base_url: "https://api.openai.com/v1".into(),
            credential_ref: Some(format!("chatgpt_plan_v1_{}", uuid::Uuid::new_v4())),
            api_key: "must-not-persist".into(),
            ..ProviderConfig::default()
        };
        bind_session_provider_in_conn(&conn, "binding", &config, 2).unwrap();
        let binding = session_model_binding(&conn, "binding", "openai", "fixture").unwrap();
        assert_eq!(binding.provider_profile_ref, config.profile_identity());
        assert_eq!(binding.credential_handle_ref, config.credential_identity());
        let metadata: String = conn
            .query_row(
                "SELECT metadata FROM sessions WHERE id='binding'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!metadata.contains("must-not-persist"));
        assert!(serde_json::from_str::<serde_json::Value>(&metadata).unwrap()["preserved"] == true);
        assert!(session_model_binding(&conn, "binding", "openai", "changed-model").is_err());
    }

    fn config(provider: &str, model: &str, api_key: &str) -> ProviderConfig {
        ProviderConfig {
            provider: provider.to_string(),
            model: model.to_string(),
            api_key: api_key.to_string(),
            ..ProviderConfig::default()
        }
    }

    #[test]
    fn configured_provider_wins_and_keychain_fills_empty_fields() {
        assert!(requires_keychain_api_credentials(&None, false).unwrap());
        assert!(matches!(
            requires_keychain_api_credentials(&None, true),
            Err(ModelResolutionError::InvalidConfiguration)
        ));
        let api = Some(config("openai", "fixture", ""));
        assert!(requires_keychain_api_credentials(&api, false).unwrap());
        assert!(matches!(
            requires_keychain_api_credentials(&api, true),
            Err(ModelResolutionError::InvalidConfiguration)
        ));
        let plan = Some(ProviderConfig {
            auth_mode: ModelAuthMode::ChatgptPlan,
            ..config("openai", "saved-plan", "")
        });
        assert!(!requires_keychain_api_credentials(&plan, true).unwrap());
        assert!(!requires_keychain_api_credentials(&plan, false).unwrap());
        let from_file = config("", "", "");
        let credentials = ApiCredentials {
            provider: "anthropic".into(),
            model: "claude-test".into(),
            api_key: "secret-key".into(),
        };
        let resolved = resolve_config_from_sources(Some(from_file), Ok(Some(credentials))).unwrap();
        assert_eq!(resolved.provider, "anthropic");
        assert_eq!(resolved.model, "claude-test");
        assert_eq!(resolved.api_key, "secret-key");
    }

    #[test]
    fn explicit_config_identity_and_key_are_preserved_over_keychain() {
        let from_file = config("deepseek", "deepseek-v4-pro", "file-key");
        let credentials = ApiCredentials {
            provider: "anthropic".into(),
            model: "claude-test".into(),
            api_key: "keychain-key".into(),
        };
        let resolved = resolve_config_from_sources(Some(from_file), Ok(Some(credentials))).unwrap();
        assert_eq!(resolved.provider, "deepseek");
        assert_eq!(resolved.model, "deepseek-v4-pro");
        assert_eq!(resolved.api_key, "file-key");
    }

    #[test]
    fn credentials_never_cross_provider_identity() {
        let from_file = config("deepseek", "deepseek-v4-pro", "");
        let credentials = ApiCredentials {
            provider: "anthropic".into(),
            model: "claude-test".into(),
            api_key: "anthropic-key".into(),
        };
        let resolved = resolve_config_from_sources(Some(from_file), Ok(Some(credentials))).unwrap();
        assert_eq!(resolved.provider, "deepseek");
        assert!(resolved.api_key.is_empty());
    }

    #[test]
    fn missing_or_unreadable_credentials_fail_closed() {
        assert_eq!(
            resolve_config_from_sources(None, Ok(None)).unwrap_err(),
            ModelResolutionError::MissingCredentials
        );
        assert_eq!(
            resolve_config_from_sources(None, Err(())).unwrap_err(),
            ModelResolutionError::CredentialStoreUnavailable
        );
        let invalid = ApiCredentials {
            provider: "anthropic".into(),
            model: "claude-test".into(),
            api_key: String::new(),
        };
        assert_eq!(
            resolve_config_from_sources(None, Ok(Some(invalid))).unwrap_err(),
            ModelResolutionError::MissingCredentials
        );
    }

    #[test]
    fn invalid_config_and_session_binding_errors_are_typed() {
        assert_eq!(
            validate_provider_identity(&config("", "model", "key")).unwrap_err(),
            ModelResolutionError::InvalidConfiguration
        );
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at) VALUES ('s1', 'test', 1, 1)",
            [],
        )
        .unwrap();
        bind_session_provider_in_conn(&conn, "s1", &config("deepseek", "v4", "key"), 2).unwrap();
        let identity: (String, String) = conn
            .query_row(
                "SELECT agent_provider, agent_model FROM sessions WHERE id='s1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(identity, ("deepseek".into(), "v4".into()));
    }

    #[test]
    fn safe_error_does_not_leak_provider_detail_or_credentials() {
        for error in [
            ModelResolutionError::InvalidConfiguration,
            ModelResolutionError::MissingCredentials,
            ModelResolutionError::CredentialStoreUnavailable,
            ModelResolutionError::InvalidThinkingSettings,
            ModelResolutionError::ProviderUnavailable,
        ] {
            let guidance = error.provider_unavailable_message().unwrap();
            assert!(guidance.contains("模型设置"));
            assert!(guidance.contains("不会自动重试已有操作"));
            assert!(!guidance.contains("sk-super-secret"));
            assert!(!guidance.contains("gpt-test"));
        }
        assert!(ModelResolutionError::SessionBindingFailed
            .provider_unavailable_message()
            .is_none());
        let secret = "sk-super-secret";
        let error = classify_provider_configuration(&config("openai", "gpt-test", ""));
        let text = error.to_string();
        assert!(!text.contains(secret));
        assert!(!text.contains("gpt-test"));
        assert!(!text.contains("OPENAI_API_KEY"));
    }

    #[test]
    fn default_thinking_settings_remain_provider_owned() {
        let deepseek = config("deepseek", "deepseek-v4-pro", "key");
        let resolved = resolve_thinking_settings(&deepseek, None).unwrap();
        assert!(resolved.is_some());
        let custom = config("custom", "internal", "key");
        assert!(resolve_thinking_settings(&custom, None).unwrap().is_none());
    }

    #[test]
    fn completion_review_keeps_bound_thinking_and_rechecks_identity_before_credentials() {
        struct UnusedProvider;
        #[async_trait::async_trait]
        impl LlmProvider for UnusedProvider {
            fn name(&self) -> &str {
                "review-binding-fixture"
            }
            async fn chat(
                &self,
                _: &[crate::llm::Message],
                _: Option<&[crate::llm::ToolSchema]>,
                _: Option<&ThinkingSettings>,
            ) -> Result<crate::llm::LlmResponse, String> {
                panic!("binding tests must not call a model")
            }
        }
        for (current, effort) in [
            (config("deepseek", "deepseek-v4-pro", "fixture"), None),
            (
                config("deepseek", "deepseek-v4-pro", "fixture"),
                Some("high"),
            ),
            (config("custom", "internal", "fixture"), None),
        ] {
            let bound = ResolvedForegroundModel {
                provider: Box::new(UnusedProvider),
                thinking_settings: resolve_thinking_settings(&current, effort).unwrap(),
                identity: ForegroundModelIdentity {
                    provider: current.provider.clone(),
                    model: current.model.clone(),
                    provider_profile_ref: current.profile_identity(),
                    credential_handle_ref: current.credential_identity(),
                },
                context_window_tokens: 8192,
                max_output_tokens: 1024,
            };
            let (provider, thinking) = bound
                .completion_review_provider(&current, |seen| {
                    assert_eq!(seen.model, current.model);
                    Ok(Box::new(UnusedProvider))
                })
                .unwrap();
            assert_eq!(provider.name(), "review-binding-fixture");
            let fields = |value: &Option<ThinkingSettings>| {
                value
                    .as_ref()
                    .map(|t| (t.protocol, t.effort, t.budget_tokens))
            };
            assert_eq!(fields(&thinking), fields(&bound.thinking_settings));
            for field in ["profile", "credential", "model"] {
                let mut changed = current.clone();
                match field {
                    "profile" => changed.base_url = "https://changed.invalid".into(),
                    "credential" => changed.credential_ref = Some("changed-credential".into()),
                    _ => changed.model = "changed-model".into(),
                }
                assert!(bound
                    .completion_review_provider(&changed, |_| {
                        panic!("a changed binding must not open credentials")
                    })
                    .is_none());
            }
            assert!(bound
                .completion_review_provider(&current, |_| Err("fixture unavailable".into()))
                .is_none());
        }
    }
}
