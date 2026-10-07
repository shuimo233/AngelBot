//! Keychain-backed model host for delegated workers.
//!
//! The Main Agent chooses the model before issuing a delegation and persists
//! only opaque provider/model/keychain references. This adapter resolves those
//! references again when a worker starts, so restart recovery never silently
//! switches to the current foreground model. Secrets remain inside the
//! provider implementation and are never returned by this interface.

use std::sync::{Arc, Mutex};

use tauri::AppHandle;

use crate::{
    commands::foreground_model_factory::resolve_model_config,
    llm::{create_provider_for_app, LlmProvider, ProviderConfig},
};

use super::delegated_model_binding::{
    DelegatedModelBinding, DelegatedModelBindingRequest, DelegatedModelHostFactory,
    ModelHostUnavailable,
};

/// Startup creates the worker graph before Tauri's `setup` callback exposes an
/// `AppHandle`. This slot keeps the graph single and lets the keychain adapter
/// resolve credentials only when a real worker is launched. An empty slot is
/// fail-closed; it never falls back to a different model or a plaintext key.
#[derive(Clone, Default)]
pub struct DeferredKeychainDelegatedModelHostFactory {
    app: Arc<Mutex<Option<AppHandle>>>,
}

impl DeferredKeychainDelegatedModelHostFactory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn install(&self, app: AppHandle) -> Result<(), ModelHostUnavailable> {
        let mut slot = self
            .app
            .lock()
            .map_err(|_| ModelHostUnavailable::Unavailable)?;
        if slot.is_some() {
            return Ok(());
        }
        *slot = Some(app);
        Ok(())
    }
}

impl DelegatedModelHostFactory for DeferredKeychainDelegatedModelHostFactory {
    fn resolve_at_delegation(
        &self,
        request: &DelegatedModelBindingRequest,
    ) -> Result<(), ModelHostUnavailable> {
        request
            .validate()
            .map_err(|_| ModelHostUnavailable::PolicyMismatch)
    }

    fn create_model_host(
        &self,
        binding: &DelegatedModelBinding,
    ) -> Result<Arc<dyn LlmProvider>, ModelHostUnavailable> {
        let app = self
            .app
            .lock()
            .map_err(|_| ModelHostUnavailable::Unavailable)?
            .clone()
            .ok_or(ModelHostUnavailable::Unavailable)?;
        KeychainDelegatedModelHostFactory::new(app).create_model_host(binding)
    }
}

/// Production implementation of the delegated model-binding seam.
#[derive(Clone)]
pub struct KeychainDelegatedModelHostFactory {
    app: AppHandle,
}

impl KeychainDelegatedModelHostFactory {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    fn resolve_config(&self) -> Result<ProviderConfig, ModelHostUnavailable> {
        resolve_model_config(Some(&self.app)).map_err(|_| ModelHostUnavailable::Unavailable)
    }

    fn matches_config(
        config: &ProviderConfig,
        provider_profile_ref: &str,
        model_ref: &str,
        credential_handle_ref: &str,
    ) -> bool {
        provider_profile_ref == config.profile_identity()
            && model_ref == format!("model:{}", config.model)
            && credential_handle_ref == config.credential_identity()
    }

    fn validate_request_against_config(
        &self,
        config: &ProviderConfig,
        request: &DelegatedModelBindingRequest,
    ) -> Result<(), ModelHostUnavailable> {
        if request.policy_version == 0
            || !Self::matches_config(
                config,
                &request.provider_profile_ref,
                &request.model_ref,
                &request.credential_handle_ref,
            )
        {
            return Err(ModelHostUnavailable::PolicyMismatch);
        }
        Ok(())
    }
}

impl DelegatedModelHostFactory for KeychainDelegatedModelHostFactory {
    fn resolve_at_delegation(
        &self,
        request: &DelegatedModelBindingRequest,
    ) -> Result<(), ModelHostUnavailable> {
        let config = self.resolve_config()?;
        self.validate_request_against_config(&config, request)
    }

    fn create_model_host(
        &self,
        binding: &DelegatedModelBinding,
    ) -> Result<Arc<dyn LlmProvider>, ModelHostUnavailable> {
        let config = self.resolve_config()?;
        let request = DelegatedModelBindingRequest {
            provider_profile_ref: binding.provider_profile_ref.clone(),
            model_ref: binding.model_ref.clone(),
            credential_handle_ref: binding.credential_handle_ref.clone(),
            policy_version: binding.policy_version,
        };
        self.validate_request_against_config(&config, &request)?;
        let provider = create_provider_for_app(&config, Some(&self.app))
            .map_err(|_| ModelHostUnavailable::Unavailable)?;
        Ok(Arc::from(provider))
    }
}

/// Testable pure binding check. It lets policy tests avoid constructing a
/// Tauri handle while keeping the production adapter's exact-match rule.
pub fn binding_matches_config(
    config: &ProviderConfig,
    request: &DelegatedModelBindingRequest,
) -> bool {
    KeychainDelegatedModelHostFactory::matches_config(
        config,
        &request.provider_profile_ref,
        &request.model_ref,
        &request.credential_handle_ref,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ProviderConfig {
        ProviderConfig {
            protocol: None,
            auth_mode: crate::llm::ModelAuthMode::ApiKey,
            credential_ref: None,
            provider: "deepseek".into(),
            model: "deepseek-v4-flash".into(),
            base_url: "https://api.deepseek.com".into(),
            api_key: "secret-is-test-only".into(),
            max_tokens: 4096,
            temperature: 0.7,
        }
    }

    fn request(provider: &str, model: &str, credential: &str) -> DelegatedModelBindingRequest {
        DelegatedModelBindingRequest::new(provider, model, credential, 1).unwrap()
    }

    #[test]
    fn exact_non_secret_binding_matches_only_selected_model() {
        assert!(binding_matches_config(
            &config(),
            &request(
                "provider:deepseek",
                "model:deepseek-v4-flash",
                "keychain:default"
            )
        ));
        assert!(!binding_matches_config(
            &config(),
            &request("provider:deepseek", "model:other", "keychain:default")
        ));
    }

    #[test]
    fn arbitrary_credentials_never_match_the_delegated_keychain_handle() {
        assert!(!binding_matches_config(
            &config(),
            &request(
                "provider:deepseek",
                "model:deepseek-v4-flash",
                "keychain:other"
            )
        ));
    }
}
