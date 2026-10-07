//! Secure credential storage via OS Keychain (Windows Credential Manager / macOS Keychain / Linux Secret Service)
//!
//! Provides a typed wrapper around tauri-plugin-keyring-store for storing API keys securely.
//!
//! Uses the KeyringExt trait to access the plugin state from AppHandle.

use crate::llm::DEEPSEEK_DEFAULT_MODEL;
use serde::{Deserialize, Serialize};
use tauri_plugin_keyring_store::KeyringExt;

/// Keychain account names
pub const KEYCHAIN_ACCOUNT_API_KEY: &str = "api_key";
pub const KEYCHAIN_ACCOUNT_PROVIDER: &str = "api_provider";
pub const KEYCHAIN_ACCOUNT_MODEL: &str = "api_model";
const WEB_SEARCH_ACCOUNT_PREFIX: &str = "web_search_";

fn desktop_e2e_mode() -> bool {
    #[cfg(feature = "desktop-e2e")]
    {
        return true;
    }

    #[cfg(not(feature = "desktop-e2e"))]
    false
}

fn channel_account(connection_id: &str) -> String {
    format!("channel_{}_credential", connection_id)
}

fn web_search_account(provider: &str) -> String {
    format!("{WEB_SEARCH_ACCOUNT_PREFIX}{provider}_api_key")
}

/// Store a Web Search provider key separately from the model-provider key.
/// The key is deliberately never mirrored to SQLite.
pub fn write_web_search_key(
    app: &tauri::AppHandle,
    provider: &str,
    api_key: &str,
) -> Result<(), String> {
    if api_key.trim().is_empty() {
        return Err("该联网搜索服务需要 API Key。".to_string());
    }
    if desktop_e2e_mode() {
        let _ = (app, provider);
        return Ok(());
    }
    app.keyring()
        .store
        .set_password(&web_search_account(provider), api_key)
        .map_err(|error| format!("Failed to store Web Search credential: {error}"))
}

pub fn read_web_search_key(app: &tauri::AppHandle, provider: &str) -> Result<String, String> {
    if desktop_e2e_mode() {
        let _ = (app, provider);
        return Ok(String::new());
    }
    app.keyring()
        .store
        .get_password(&web_search_account(provider))
        .map_err(|error| format!("Failed to read Web Search credential: {error}"))
        .map(|key| key.unwrap_or_default())
}

/// Remove only the credential belonging to the currently configured Web
/// Search provider. Looking up the account first makes an already-removed key
/// a successful, idempotent cleanup rather than an obstacle to revocation.
pub fn delete_web_search_key(app: &tauri::AppHandle, provider: &str) -> Result<(), String> {
    if desktop_e2e_mode() {
        let _ = (app, provider);
        return Ok(());
    }
    let account = web_search_account(provider);
    let store = &app.keyring().store;
    match store
        .get_password(&account)
        .map_err(|error| format!("无法读取联网搜索凭据：{error}"))?
    {
        Some(_) => store
            .delete(&account)
            .map_err(|error| format!("无法删除联网搜索凭据：{error}")),
        None => Ok(()),
    }
}

/// Store a channel secret in the OS keychain. The secret is never mirrored to SQLite.
pub fn write_channel_credential(
    app: &tauri::AppHandle,
    connection_id: &str,
    secret: &str,
) -> Result<(), String> {
    if secret.trim().is_empty() {
        return Err("Channel credential cannot be empty".to_string());
    }
    if desktop_e2e_mode() {
        let _ = (app, connection_id);
        return Ok(());
    }
    app.keyring()
        .store
        .set_password(&channel_account(connection_id), secret)
        .map_err(|e| format!("Failed to store channel credential: {}", e))
}

pub fn delete_channel_credential(
    app: &tauri::AppHandle,
    connection_id: &str,
) -> Result<(), String> {
    if desktop_e2e_mode() {
        let _ = (app, connection_id);
        return Ok(());
    }
    let _ = app.keyring().store.delete(&channel_account(connection_id));
    Ok(())
}

/// API credentials stored in Keychain
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiCredentials {
    pub provider: String,
    pub model: String,
    pub api_key: String,
}

impl ApiCredentials {
    pub fn is_empty(&self) -> bool {
        self.api_key.is_empty()
    }

    pub fn is_valid(&self) -> bool {
        !self.provider.is_empty() && !self.api_key.is_empty()
    }
}

/// Read the api_key only from Keychain using app handle.
pub fn read_api_key(app: &tauri::AppHandle) -> Result<String, String> {
    if desktop_e2e_mode() {
        return Ok(String::new());
    }

    let store = &app.keyring().store;
    match store.get_password(KEYCHAIN_ACCOUNT_API_KEY) {
        Ok(Some(key)) => Ok(key),
        Ok(None) => Ok(String::new()),
        Err(e) => {
            eprintln!("[Keychain] Failed to read api_key: {}", e);
            Err(format!("Keychain read error: {}", e))
        }
    }
}

/// Write the api_key only to Keychain using app handle.
pub fn write_api_key(app: &tauri::AppHandle, api_key: &str) -> Result<(), String> {
    if desktop_e2e_mode() {
        let _ = (app, api_key);
        return Ok(());
    }
    let store = &app.keyring().store;
    if api_key.is_empty() {
        return Ok(());
    }
    store
        .set_password(KEYCHAIN_ACCOUNT_API_KEY, api_key)
        .map_err(|e| format!("Failed to store api_key: {}", e))?;
    eprintln!("[Keychain] Stored api_key");
    Ok(())
}

/// Read credentials from Keychain using app handle
pub fn read_credentials(app: &tauri::AppHandle) -> Result<ApiCredentials, String> {
    if desktop_e2e_mode() {
        return Ok(ApiCredentials {
            provider: String::new(),
            model: String::new(),
            api_key: String::new(),
        });
    }

    let store = &app.keyring().store;

    let api_key = match store.get_password(KEYCHAIN_ACCOUNT_API_KEY) {
        Ok(Some(key)) => key,
        Ok(None) => String::new(),
        Err(e) => {
            eprintln!("[Keychain] Failed to read api_key: {}", e);
            return Err(format!("Keychain read error: {}", e));
        }
    };

    let provider = match store.get_password(KEYCHAIN_ACCOUNT_PROVIDER) {
        Ok(Some(val)) => val,
        Ok(None) => String::new(),
        Err(_) => String::new(),
    };

    let model = match store.get_password(KEYCHAIN_ACCOUNT_MODEL) {
        Ok(Some(val)) => val,
        Ok(None) => String::new(),
        Err(_) => String::new(),
    };

    Ok(ApiCredentials {
        provider,
        model,
        api_key,
    })
}

/// Write credentials to Keychain using app handle
pub fn write_credentials(app: &tauri::AppHandle, creds: &ApiCredentials) -> Result<(), String> {
    if desktop_e2e_mode() {
        let _ = (app, creds);
        return Ok(());
    }
    let store = &app.keyring().store;

    // Store API key (required)
    if !creds.api_key.is_empty() {
        store
            .set_password(KEYCHAIN_ACCOUNT_API_KEY, &creds.api_key)
            .map_err(|e| format!("Failed to store api_key: {}", e))?;
    }

    // Store provider
    store
        .set_password(KEYCHAIN_ACCOUNT_PROVIDER, &creds.provider)
        .map_err(|e| format!("Failed to store provider: {}", e))?;

    // Store model
    store
        .set_password(KEYCHAIN_ACCOUNT_MODEL, &creds.model)
        .map_err(|e| format!("Failed to store model: {}", e))?;

    eprintln!(
        "[Keychain] Stored credentials: provider={}, model={}",
        creds.provider, creds.model
    );
    Ok(())
}

/// Delete credentials from Keychain
pub fn delete_credentials(app: &tauri::AppHandle) -> Result<(), String> {
    if desktop_e2e_mode() {
        let _ = app;
        return Ok(());
    }
    let store = &app.keyring().store;

    let _ = store.delete(KEYCHAIN_ACCOUNT_API_KEY);
    let _ = store.delete(KEYCHAIN_ACCOUNT_PROVIDER);
    let _ = store.delete(KEYCHAIN_ACCOUNT_MODEL);

    eprintln!("[Keychain] Deleted all credentials");
    Ok(())
}

/// Check if Keychain has credentials stored
pub fn has_credentials(app: &tauri::AppHandle) -> Result<bool, String> {
    if desktop_e2e_mode() {
        return Ok(false);
    }

    let store = &app.keyring().store;

    match store.exists_nonempty(KEYCHAIN_ACCOUNT_API_KEY) {
        Ok(exists) => Ok(exists),
        Err(e) => {
            eprintln!("[Keychain] Failed to check existence: {}", e);
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_credentials_is_empty() {
        let empty = ApiCredentials::default();
        assert!(empty.is_empty());
        assert!(!empty.is_valid());

        let valid = ApiCredentials {
            provider: "deepseek".to_string(),
            model: DEEPSEEK_DEFAULT_MODEL.to_string(),
            api_key: "sk-123".to_string(),
        };
        assert!(!valid.is_empty());
        assert!(valid.is_valid());
    }
}
