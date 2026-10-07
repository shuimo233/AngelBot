//! Web Search configuration. Provider metadata is stored locally; secrets stay
//! in the operating-system keychain and are never written to conversation data.

use crate::{keychain, AppState};
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};

const CONFIG_KEY: &str = "web_search_config";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchConfig {
    pub provider: String,
    pub endpoint: String,
    #[serde(rename = "apiKeyConfigured")]
    pub api_key_configured: bool,
    /// Whether the Main Agent may currently expose this network capability.
    /// Missing on legacy rows deliberately defaults to `true`: a user who
    /// explicitly configured search before this field existed should not lose
    /// that capability during an upgrade.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct ConfigureWebSearchRequest {
    pub provider: String,
    #[serde(default, rename = "apiKey")]
    pub api_key: String,
    #[serde(default)]
    pub endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetWebSearchEnabledRequest {
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

pub(crate) fn normalized_provider(provider: &str) -> Result<&'static str, String> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "tavily" => Ok("tavily"),
        "brave" | "brave_search" => Ok("brave"),
        "exa" => Ok("exa"),
        "searxng" | "searx" => Ok("searxng"),
        _ => Err("不支持该联网搜索服务；请从受支持的服务中选择。".to_string()),
    }
}

pub(crate) fn default_endpoint(provider: &str) -> &'static str {
    match provider {
        "tavily" => "https://api.tavily.com/search",
        "brave" => "https://api.search.brave.com/res/v1/web/search",
        "exa" => "https://api.exa.ai/search",
        "searxng" => "",
        _ => "",
    }
}

pub(crate) fn provider_requires_api_key(provider: &str) -> bool {
    provider != "searxng"
}

/// Return the only endpoint a provider configuration is allowed to use.
///
/// Hosted providers receive an API credential on every request, so accepting
/// an arbitrary endpoint here would turn a local configuration edit into a
/// credential exfiltration path. SearXNG is the deliberately user-hosted
/// exception: it uses no credential and its explicitly configured http(s)
/// endpoint is its identity.
pub(crate) fn validated_endpoint(provider: &str, endpoint: &str) -> Result<String, String> {
    let endpoint = endpoint.trim();
    if provider != "searxng" {
        let trusted = default_endpoint(provider);
        if endpoint.is_empty() || endpoint == trusted {
            return Ok(trusted.to_string());
        }
        return Err(format!(
            "{provider} 仅允许使用内置的受信任服务地址；为保护凭据，不能填写自定义地址。"
        ));
    }

    if endpoint.is_empty() {
        return Err("使用 SearXNG 时需要填写服务地址。".to_string());
    }
    let url = reqwest::Url::parse(endpoint)
        .map_err(|_| "联网搜索地址必须是有效的 http(s) 地址。".to_string())?;
    if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
        return Err("联网搜索地址必须是有效的 http(s) 地址。".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("联网搜索地址中不能包含账号或凭据。".to_string());
    }
    Ok(url.to_string())
}

/// Revalidate metadata read from SQLite before it can drive a network request.
/// The database is durable user state, not an authority to bypass the same
/// provider/endpoint constraints enforced when a configuration is saved.
pub(crate) fn normalize_config(mut config: WebSearchConfig) -> Result<WebSearchConfig, String> {
    let provider = normalized_provider(&config.provider)?;
    config.provider = provider.to_string();
    config.endpoint = validated_endpoint(provider, &config.endpoint)?;
    Ok(config)
}

pub fn load_config(state: &AppState) -> Result<Option<WebSearchConfig>, String> {
    load_config_from_db(&state.db)
}

pub fn load_config_from_db(
    db: &Arc<Mutex<rusqlite::Connection>>,
) -> Result<Option<WebSearchConfig>, String> {
    let conn = db.lock().map_err(|error| error.to_string())?;
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![CONFIG_KEY],
            |row| row.get(0),
        )
        .ok();
    value
        .map(|value| {
            serde_json::from_str::<WebSearchConfig>(&value)
                .map_err(|error| error.to_string())
                .and_then(normalize_config)
        })
        .transpose()
}

pub fn configure_impl(
    state: &AppState,
    app: &AppHandle,
    request: ConfigureWebSearchRequest,
) -> Result<WebSearchConfig, String> {
    let provider = normalized_provider(&request.provider)?;
    if provider_requires_api_key(provider) && request.api_key.trim().is_empty() {
        return Err("该联网搜索服务需要 API Key。".to_string());
    }
    let endpoint = validated_endpoint(
        provider,
        request
            .endpoint
            .as_deref()
            .unwrap_or_else(|| default_endpoint(provider)),
    )?;
    if provider_requires_api_key(provider) {
        keychain::write_web_search_key(app, provider, &request.api_key)?;
    }
    let config = normalize_config(WebSearchConfig {
        provider: provider.to_string(),
        endpoint,
        api_key_configured: provider_requires_api_key(provider),
        enabled: true,
    })?;
    let serialized = serde_json::to_string(&config).map_err(|error| error.to_string())?;
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)\
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![CONFIG_KEY, serialized, Utc::now().timestamp()],
    )
    .map_err(|error| error.to_string())?;
    Ok(config)
}

pub fn set_enabled_impl(state: &AppState, enabled: bool) -> Result<WebSearchConfig, String> {
    let mut config = load_config(state)?
        .ok_or_else(|| "联网搜索尚未配置；请先保存服务配置后再启用。".to_string())?;
    config.enabled = enabled;
    let serialized = serde_json::to_string(&config).map_err(|error| error.to_string())?;
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)\
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![CONFIG_KEY, serialized, Utc::now().timestamp()],
    )
    .map_err(|error| error.to_string())?;
    Ok(config)
}

fn clear_config_from_db(db: &Arc<Mutex<rusqlite::Connection>>) -> Result<(), String> {
    let conn = db.lock().map_err(|error| error.to_string())?;
    conn.execute("DELETE FROM settings WHERE key = ?1", params![CONFIG_KEY])
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Revoke the active Web Search capability completely. The credential is
/// removed before its metadata so a database failure leaves the capability
/// fail-closed rather than silently retaining a usable key.
pub fn clear_config_impl(state: &AppState, app: &AppHandle) -> Result<(), String> {
    if let Some(config) = load_config(state)? {
        if provider_requires_api_key(&config.provider) {
            keychain::delete_web_search_key(app, &config.provider)?;
        }
    }
    clear_config_from_db(&state.db)
}

#[tauri::command]
pub fn configure_web_search(
    state: State<'_, AppState>,
    app: AppHandle,
    request: ConfigureWebSearchRequest,
) -> Result<WebSearchConfig, String> {
    configure_impl(&state, &app, request)
}

#[tauri::command]
pub fn get_web_search_config(
    state: State<'_, AppState>,
) -> Result<Option<WebSearchConfig>, String> {
    load_config(&state)
}

#[tauri::command]
pub fn set_web_search_enabled(
    state: State<'_, AppState>,
    request: SetWebSearchEnabledRequest,
) -> Result<WebSearchConfig, String> {
    set_enabled_impl(&state, request.enabled)
}

#[tauri::command]
pub fn clear_web_search_config(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    clear_config_impl(&state, &app)
}

#[cfg(test)]
mod tests {
    use super::{
        clear_config_from_db, load_config_from_db, normalize_config, validated_endpoint,
        WebSearchConfig,
    };
    use rusqlite::{params, Connection};
    use std::sync::{Arc, Mutex};

    #[test]
    fn legacy_config_defaults_to_enabled_and_uses_the_trusted_provider_endpoint() {
        let config: WebSearchConfig = serde_json::from_value(serde_json::json!({
            "provider": "brave_search",
            "endpoint": "https://api.search.brave.com/res/v1/web/search",
            "apiKeyConfigured": true
        }))
        .unwrap();

        let config = normalize_config(config).unwrap();
        assert!(config.enabled);
        assert_eq!(config.provider, "brave");
        assert_eq!(
            config.endpoint,
            "https://api.search.brave.com/res/v1/web/search"
        );
    }

    #[test]
    fn hosted_provider_refuses_an_endpoint_that_would_receive_its_credential() {
        let error = validated_endpoint("tavily", "https://example.invalid/search").unwrap_err();
        assert!(error.contains("内置的受信任服务地址"));
    }

    #[test]
    fn persisted_config_is_revalidated_before_use() {
        let db = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        let conn = db.lock().unwrap();
        conn.execute(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)",
            params![
                "web_search_config",
                serde_json::json!({
                    "provider": "exa",
                    "endpoint": "https://untrusted.example/search",
                    "apiKeyConfigured": true,
                    "enabled": true
                })
                .to_string()
            ],
        )
        .unwrap();
        drop(conn);

        let error = load_config_from_db(&db).unwrap_err();
        assert!(error.contains("内置的受信任服务地址"));
    }

    #[test]
    fn clearing_config_removes_only_the_web_search_metadata_row() {
        let db = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        let conn = db.lock().unwrap();
        conn.execute(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2), (?3, ?4)",
            params!["web_search_config", "{}", "unrelated_setting", "preserved"],
        )
        .unwrap();
        drop(conn);

        clear_config_from_db(&db).unwrap();

        let conn = db.lock().unwrap();
        let web_search: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'web_search_config'",
                [],
                |row| row.get(0),
            )
            .ok();
        let unrelated: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'unrelated_setting'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(web_search.is_none());
        assert_eq!(unrelated, "preserved");
    }
}
