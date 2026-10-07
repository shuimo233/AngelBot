//! MCP server commands — includes CRUD, connection testing, and process management
use crate::llm::DEEPSEEK_DEFAULT_MODEL;
use crate::mcp_client::{McpProcessManager, McpServerStatus, McpTool};
use crate::mcp_credentials::{self, KeyringMcpCredentialStore, McpEnvVar};
use crate::AppState;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::net::TcpStream;
use std::time::Duration;
use tauri::State;

#[derive(Debug, Serialize, Deserialize)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: String,
    pub env: String,
    #[serde(rename = "envKeys", default)]
    pub env_keys: Vec<String>,
    #[serde(rename = "envUnavailable", default)]
    pub env_unavailable: bool,
    pub enabled: bool,
    #[serde(rename = "enabledWorkspaceIds", default)]
    pub enabled_workspace_ids: Vec<String>,
}

/// The single Workspace-MCP access interface. Installed server configuration
/// and per-workspace enablement deliberately remain separate: the former is a
/// potential capability, while this module resolves the capability visible to
/// one foreground session.
pub(crate) fn enabled_server_ids_for_session(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<String>, String> {
    let workspace_id = conn
        .query_row(
            "SELECT project_id FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .flatten();
    let Some(workspace_id) = workspace_id else {
        return Ok(Vec::new());
    };
    let mut statement = conn
        .prepare(
            "SELECT enablement.server_id
             FROM mcp_workspace_enablements AS enablement
             JOIN mcp_servers AS server ON server.id = enablement.server_id
             WHERE enablement.workspace_id = ?1 AND server.enabled = 1
             ORDER BY enablement.server_id",
        )
        .map_err(|error| error.to_string())?;
    let server_ids = statement
        .query_map([workspace_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(server_ids)
}

fn enabled_workspace_ids_for_server(
    conn: &Connection,
    server_id: &str,
) -> Result<Vec<String>, String> {
    let mut statement = conn
        .prepare(
            "SELECT workspace_id FROM mcp_workspace_enablements
             WHERE server_id = ?1 ORDER BY workspace_id",
        )
        .map_err(|error| error.to_string())?;
    let workspace_ids = statement
        .query_map([server_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(workspace_ids)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EnvProviderInfo {
    pub provider: String,
    pub model: String,
    #[serde(rename = "base_url")]
    pub base_url: String,
    #[serde(rename = "has_api_key")]
    pub has_api_key: bool,
    #[serde(rename = "is_local")]
    pub is_local: bool,
    #[serde(rename = "cost_tier")]
    pub cost_tier: String,
}

#[tauri::command]
pub fn get_mcp_servers(
    app: tauri::AppHandle,
    state: State<AppState>,
) -> Result<Vec<McpServer>, String> {
    state.mcp_manager.with_policy_gate(|| {
        let mut conn = state.db.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, command, args, enabled FROM mcp_servers ORDER BY created_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                let id: String = r.get(0)?;
                Ok(McpServer {
                    id,
                    name: r.get(1)?,
                    command: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    args: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    // Never send credential values or legacy plaintext to JS.
                    env: String::new(),
                    env_keys: Vec::new(),
                    env_unavailable: false,
                    enabled: r.get::<_, i32>(4)? != 0,
                    enabled_workspace_ids: Vec::new(),
                })
            })
            .map_err(|e| e.to_string())?;
        let mut servers = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        drop(stmt);
        let store = KeyringMcpCredentialStore::new(&app);
        for server in &mut servers {
            server.enabled_workspace_ids = enabled_workspace_ids_for_server(&conn, &server.id)?;
            match mcp_credentials::read_effective_env(&mut conn, &store, &server.id) {
                Ok(vars) => server.env_keys = vars.into_iter().map(|var| var.key).collect(),
                Err(_) => server.env_unavailable = true,
            }
        }
        Ok(servers)
    })
}

#[tauri::command]
pub fn save_mcp_server(
    app: tauri::AppHandle,
    state: State<AppState>,
    server: McpServer,
) -> Result<(), String> {
    if !server.env.is_empty() {
        return Err("MCP 环境变量必须通过安全凭据设置保存".to_string());
    }
    state.mcp_manager.with_server_policy_gate(&server.id, || {
        let mut conn = state.db.lock().map_err(|e| e.to_string())?;
        let existing: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE id = ?1)",
                [&server.id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if existing {
            // A failed vault migration cannot be erased by an unrelated edit.
            let store = KeyringMcpCredentialStore::new(&app);
            mcp_credentials::read_effective_env(&mut conn, &store, &server.id)?;
        }
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        let now = chrono::Utc::now().timestamp();
        let previous = tx
            .query_row(
                "SELECT command, COALESCE(args, '') FROM mcp_servers WHERE id = ?1",
                [&server.id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let configuration_changed = previous
            .as_ref()
            .map(|(command, args)| command != &server.command || args != &server.args)
            .unwrap_or(false);
        if configuration_changed || !server.enabled {
            // A failed stop must leave both configuration and grants unchanged.
            stop_server_if_running(&state.mcp_manager, &server.id)?;
        }

        tx.execute(
            r#"INSERT INTO mcp_servers (id, name, command, args, env, enabled, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
               ON CONFLICT(id) DO UPDATE SET
               name = excluded.name, command = excluded.command,
               args = excluded.args, env = excluded.env,
               enabled = excluded.enabled"#,
            rusqlite::params![
                &server.id,
                &server.name,
                &server.command,
                &server.args,
                "",
                server.enabled as i32,
                now
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM mcp_workspace_enablements WHERE server_id = ?1",
            [&server.id],
        )
        .map_err(|error| error.to_string())?;
        if !configuration_changed {
            let mut seen_workspace_ids = HashSet::new();
            for workspace_id in server
                .enabled_workspace_ids
                .iter()
                .filter(|id| !id.trim().is_empty())
            {
                if !seen_workspace_ids.insert(workspace_id.as_str()) {
                    continue;
                }
                let inserted = tx
                    .execute(
                        "INSERT OR IGNORE INTO mcp_workspace_enablements (workspace_id, server_id, enabled_at)
                         SELECT id, ?1, ?2 FROM projects WHERE id = ?3",
                        rusqlite::params![&server.id, now, workspace_id],
                    )
                    .map_err(|error| error.to_string())?;
                if inserted == 0 {
                    return Err("MCP 工作区不存在".to_string());
                }
            }
        }
        tx.commit().map_err(|error| error.to_string())
    })
}

fn stop_server_if_running(manager: &McpProcessManager, server_id: &str) -> Result<(), String> {
    if manager
        .managed_server_ids()?
        .iter()
        .any(|managed_id| managed_id == server_id)
    {
        manager.stop(server_id)?;
    }
    Ok(())
}

#[tauri::command]
pub fn delete_mcp_server(
    app: tauri::AppHandle,
    state: State<AppState>,
    id: String,
) -> Result<(), String> {
    state.mcp_manager.with_server_policy_gate(&id, || {
        // A stopped/missing child is already revoked; a failed stop is not.
        stop_server_if_running(&state.mcp_manager, &id)?;
        let mut conn = state.db.lock().map_err(|e| e.to_string())?;
        let existing: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE id = ?1)",
                [&id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !existing {
            return Ok(());
        }
        let store = KeyringMcpCredentialStore::new(&app);
        mcp_credentials::delete_env(&mut conn, &store, &id)?;
        conn.execute("DELETE FROM mcp_servers WHERE id = ?1", [&id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// A write-only MCP environment value. Reads return variable names only.
#[tauri::command]
pub fn set_mcp_env_var(
    app: tauri::AppHandle,
    state: State<AppState>,
    server_id: String,
    key: String,
    value: String,
) -> Result<(), String> {
    if value.is_empty() {
        return Err("MCP 凭据不能为空".to_string());
    }
    let replacement = McpEnvVar { key, value };
    mcp_credentials::validate_env_vars(std::slice::from_ref(&replacement))?;
    state.mcp_manager.with_server_policy_gate(&server_id, || {
        let store = KeyringMcpCredentialStore::new(&app);
        let mut conn = state.db.lock().map_err(|error| error.to_string())?;
        let mut vars = mcp_credentials::read_effective_env(&mut conn, &store, &server_id)?;
        if let Some(existing) = vars
            .iter_mut()
            .find(|var| var.key.eq_ignore_ascii_case(&replacement.key))
        {
            if existing == &replacement {
                return Ok(());
            }
            *existing = replacement;
        } else {
            vars.push(replacement);
        }
        mcp_credentials::validate_env_vars(&vars)?;
        stop_server_if_running(&state.mcp_manager, &server_id)?;
        mcp_credentials::replace_env(&mut conn, &store, &server_id, &vars)
    })
}

#[tauri::command]
pub fn remove_mcp_env_var(
    app: tauri::AppHandle,
    state: State<AppState>,
    server_id: String,
    key: String,
) -> Result<(), String> {
    mcp_credentials::validate_env_vars(&[McpEnvVar {
        key: key.clone(),
        value: String::new(),
    }])?;
    state.mcp_manager.with_server_policy_gate(&server_id, || {
        let store = KeyringMcpCredentialStore::new(&app);
        let mut conn = state.db.lock().map_err(|error| error.to_string())?;
        let mut vars = mcp_credentials::read_effective_env(&mut conn, &store, &server_id)?;
        let before = vars.len();
        vars.retain(|var| !var.key.eq_ignore_ascii_case(&key));
        if before == vars.len() {
            return Ok(());
        }
        stop_server_if_running(&state.mcp_manager, &server_id)?;
        mcp_credentials::replace_env(&mut conn, &store, &server_id, &vars)
    })
}

/// Recovery action for a missing/corrupt keyring record. This does not need
/// to read the old blob and leaves the server without any configured secrets.
#[tauri::command]
pub fn clear_mcp_env(
    app: tauri::AppHandle,
    state: State<AppState>,
    server_id: String,
) -> Result<(), String> {
    state.mcp_manager.with_server_policy_gate(&server_id, || {
        stop_server_if_running(&state.mcp_manager, &server_id)?;
        let store = KeyringMcpCredentialStore::new(&app);
        let mut conn = state.db.lock().map_err(|error| error.to_string())?;
        mcp_credentials::delete_env(&mut conn, &store, &server_id)
    })
}

#[tauri::command]
pub fn check_ollama() -> Result<bool, String> {
    TcpStream::connect_timeout(
        &"127.0.0.1:11434"
            .parse()
            .map_err(|e: std::net::AddrParseError| e.to_string())?,
        Duration::from_secs(2),
    )
    .map(|_| true)
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn test_api_connection(
    app: tauri::AppHandle,
    url: String,
    api_key: Option<String>,
    provider: Option<String>,
) -> Result<bool, String> {
    let provider_id = api_provider_from_url(&url, provider.as_deref()).to_string();
    let resolved_key = api_key
        .filter(|key| !key.trim().is_empty())
        .or_else(|| crate::commands::api_config::api_key_from_environment(&provider_id))
        .or_else(|| {
            crate::keychain::read_credentials(&app)
                .ok()
                .and_then(|credentials| {
                    (!credentials.api_key.is_empty()
                        && (credentials.provider.is_empty() || credentials.provider == provider_id))
                        .then_some(credentials.api_key)
                })
        });
    test_api_connection_core(url, resolved_key, Some(provider_id))
}

pub(crate) fn test_api_connection_core(
    url: String,
    api_key: Option<String>,
    provider: Option<String>,
) -> Result<bool, String> {
    let provider = api_provider_from_url(&url, provider.as_deref());
    let endpoint = api_test_endpoint(&url, provider)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;

    let mut request = client.get(endpoint);
    if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
        request = match provider {
            "anthropic" => request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01"),
            "ollama" => request,
            _ => {
                for (name, value) in crate::llm::openai_compatible_request_headers(provider, &key) {
                    request = request.header(name, value);
                }
                request
            }
        };
    }

    let response = request
        .send()
        .map_err(|e| format!("Connection request failed: {e}"))?;
    Ok(response.status().is_success())
}

fn api_provider_from_url<'a>(url: &str, provider: Option<&'a str>) -> &'a str {
    provider.unwrap_or_else(|| {
        if url.contains("anthropic.com") {
            "anthropic"
        } else if url.contains("googleapis.com") {
            "google"
        } else if url.contains("azure.com") {
            "azure"
        } else if url.contains("ollama") || url.contains("localhost:11434") {
            "ollama"
        } else {
            "openai-compatible"
        }
    })
}

fn api_test_endpoint(base_url: &str, provider: &str) -> Result<String, String> {
    let base = base_url.trim_end_matches('/');
    if base.is_empty() {
        return Err("API base URL is required".to_string());
    }

    let suffix = match provider {
        "anthropic" => "/v1/models",
        "azure" if base.ends_with("/openai/v1") => "/models",
        "azure" => "/openai/models?api-version=2024-10-21",
        _ => "/models",
    };
    Ok(format!("{base}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::{api_test_endpoint, enabled_server_ids_for_session};
    use crate::db::migrate;
    use rusqlite::Connection;

    #[test]
    fn builds_provider_specific_test_endpoints() {
        assert_eq!(
            api_test_endpoint("https://api.deepseek.com", "deepseek").unwrap(),
            "https://api.deepseek.com/models"
        );
        assert_eq!(
            api_test_endpoint("https://api.openai.com/v1/", "openai").unwrap(),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            api_test_endpoint("https://api.anthropic.com", "anthropic").unwrap(),
            "https://api.anthropic.com/v1/models"
        );
        assert_eq!(
            api_test_endpoint(
                "https://generativelanguage.googleapis.com/v1beta/openai",
                "google"
            )
            .unwrap(),
            "https://generativelanguage.googleapis.com/v1beta/openai/models"
        );
        assert_eq!(
            api_test_endpoint("https://resource.openai.azure.com/openai/v1", "azure").unwrap(),
            "https://resource.openai.azure.com/openai/v1/models"
        );
        assert_eq!(
            api_test_endpoint("https://resource.openai.azure.com", "azure").unwrap(),
            "https://resource.openai.azure.com/openai/models?api-version=2024-10-21"
        );
    }

    #[test]
    fn session_resolves_only_its_workspace_enabled_servers() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO projects (id,name,path,created_at,kind,updated_at) VALUES ('project-a','A','C:/a',1,'project',1),('project-b','B','C:/b',1,'project',1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id,title,created_at,updated_at,context_version,project_id) VALUES ('session-a','',1,1,0,'project-a'),('session-b','',1,1,0,'project-b')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mcp_servers (id,name,command,args,env,enabled,created_at) VALUES ('server-a','A','cmd','','',1,1),('server-b','B','cmd','','',1,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mcp_workspace_enablements (workspace_id,server_id,enabled_at) VALUES ('project-a','server-a',1),('project-b','server-b',1)",
            [],
        )
        .unwrap();

        assert_eq!(
            enabled_server_ids_for_session(&conn, "session-a").unwrap(),
            vec!["server-a"]
        );
        assert_eq!(
            enabled_server_ids_for_session(&conn, "session-b").unwrap(),
            vec!["server-b"]
        );

        conn.execute(
            "UPDATE mcp_servers SET enabled = 0 WHERE id = 'server-a'",
            [],
        )
        .unwrap();
        assert!(enabled_server_ids_for_session(&conn, "session-a")
            .unwrap()
            .is_empty());
        assert!(enabled_server_ids_for_session(&conn, "missing")
            .unwrap()
            .is_empty());
    }
}

#[tauri::command]
pub fn get_env_config() -> Result<Option<EnvProviderInfo>, String> {
    let providers = vec![
        (
            "OPENAI_API_KEY",
            "openai",
            "https://api.openai.com/v1",
            "gpt-4o",
            false,
            "$$$",
        ),
        (
            "ANTHROPIC_API_KEY",
            "anthropic",
            "https://api.anthropic.com",
            "claude-sonnet-4-20250514",
            false,
            "$$$",
        ),
        (
            "GEMINI_API_KEY",
            "google",
            "https://generativelanguage.googleapis.com/v1beta",
            "gemini-2.0-flash-exp",
            false,
            "$$$",
        ),
        (
            "DEEPSEEK_API_KEY",
            "deepseek",
            "https://api.deepseek.com",
            DEEPSEEK_DEFAULT_MODEL,
            false,
            "$$$",
        ),
        (
            "GROQ_API_KEY",
            "groq",
            "https://api.groq.com/openai/v1",
            "llama-3.3-70b-versatile",
            false,
            "$$$",
        ),
        ("AZURE_OPENAI_KEY", "azure", "", "gpt-4o", false, "$$"),
        (
            "MISTRAL_API_KEY",
            "mistral",
            "https://api.mistral.ai/v1",
            "mistral-large-latest",
            false,
            "$$",
        ),
        (
            "TOGETHER_API_KEY",
            "together",
            "https://api.together.xyz/v1",
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            false,
            "$$",
        ),
        (
            "XAI_API_KEY",
            "xai",
            "https://api.x.ai/v1",
            "grok-2",
            false,
            "$$$",
        ),
        (
            "OPENROUTER_API_KEY",
            "openrouter",
            "https://openrouter.ai/api/v1",
            "openai/gpt-4o",
            false,
            "$$$",
        ),
    ];

    for (env_key, provider, base_url, default_model, is_local, cost_tier) in providers {
        if let Ok(api_key) = env::var(env_key) {
            if !api_key.is_empty() {
                return Ok(Some(EnvProviderInfo {
                    provider: provider.to_string(),
                    model: default_model.to_string(),
                    base_url: base_url.to_string(),
                    has_api_key: true,
                    is_local,
                    cost_tier: cost_tier.to_string(),
                }));
            }
        }
    }

    if let Ok(ollama_url) = env::var("OLLAMA_BASE_URL") {
        if !ollama_url.is_empty() {
            return Ok(Some(EnvProviderInfo {
                provider: "ollama".to_string(),
                model: "llama3".to_string(),
                base_url: ollama_url,
                has_api_key: false,
                is_local: true,
                cost_tier: "$".to_string(),
            }));
        }
    }

    Ok(None)
}

// ─── MCP Process Management ──────────────────────────────────────────────────

/// Start an MCP server process by its config ID.
/// Looks up the server configuration from the database, then spawns the process.
#[tauri::command]
pub fn start_mcp_server(
    app: tauri::AppHandle,
    state: State<AppState>,
    server_id: String,
) -> Result<McpServerStatus, String> {
    let pending = state.mcp_manager.with_server_policy_gate(&server_id, || {
        // Keep lookup and launch together so a concurrent edit cannot start
        // an obsolete or disabled server after its revocation commits.
        let config = {
            let mut conn = state.db.lock().map_err(|e| e.to_string())?;
            let mut stmt = conn
                .prepare("SELECT name, command, args, enabled FROM mcp_servers WHERE id = ?1")
                .map_err(|e| e.to_string())?;
            stmt.query_row([&server_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    r.get::<_, i32>(3)? != 0,
                ))
            })
            .map_err(|_| "MCP 服务配置不存在".to_string())?
        };

        let (name, command, args, enabled) = config;
        if !enabled {
            return Err("MCP 服务已停用；请先在设置中启用它。".to_string());
        }
        if command.is_empty() {
            return Err(
                "Server command is empty — please configure it in MCP settings".to_string(),
            );
        }

        let vars = {
            let mut conn = state.db.lock().map_err(|error| error.to_string())?;
            let store = KeyringMcpCredentialStore::new(&app);
            mcp_credentials::read_effective_env(&mut conn, &store, &server_id)?
        };
        let env_str = mcp_credentials::launch_env(&vars);
        state
            .mcp_manager
            .begin_start(&server_id, &name, &command, &args, &env_str)
    })?;
    // The child is registered before releasing the gate. Stop and config
    // revocation can now terminate a silent first handshake promptly.
    state.mcp_manager.finish_start(pending)?;
    state.mcp_manager.get_status(&server_id)
}

/// Stop a running MCP server process.
#[tauri::command]
pub fn stop_mcp_server(
    state: State<AppState>,
    server_id: String,
) -> Result<McpServerStatus, String> {
    state
        .mcp_manager
        .with_server_lifecycle_gate(&server_id, || {
            state.mcp_manager.stop(&server_id)?;
            state.mcp_manager.get_status(&server_id)
        })
}

/// Get the current status of an MCP server process.
#[tauri::command]
pub fn get_mcp_server_status(
    state: State<AppState>,
    server_id: String,
) -> Result<McpServerStatus, String> {
    state.mcp_manager.get_status(&server_id)
}

/// List tools from a running MCP server.
#[tauri::command]
pub fn list_mcp_tools(state: State<AppState>, server_id: String) -> Result<Vec<McpTool>, String> {
    state.mcp_manager.list_tools(&server_id)
}

/// Force a fresh tools/list request for an explicitly user-started service.
/// Cached discovery is sufficient for display, but a readiness check must show
/// what this process advertises now rather than an earlier snapshot.
#[tauri::command]
pub fn refresh_mcp_tools(
    state: State<AppState>,
    server_id: String,
) -> Result<Vec<McpTool>, String> {
    state.mcp_manager.refresh_tools(&server_id)
}
