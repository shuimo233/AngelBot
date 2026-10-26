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
    #[serde(rename = "catalogReady", default)]
    pub catalog_ready: bool,
    #[serde(rename = "catalogError", default)]
    pub catalog_error: Option<String>,
}

#[derive(Serialize)]
pub struct BrowserMcpDraft {
    id: String,
    name: String,
    command: String,
    args: String,
}

#[tauri::command]
pub async fn preview_browser_cleanup(
    app: tauri::AppHandle,
) -> Result<crate::browser_storage::cleanup::CleanupPreview, String> {
    use tauri::Manager;
    tauri::async_runtime::spawn_blocking(move || {
        crate::browser_storage::cleanup::preview(&app.state::<AppState>())
    })
    .await
    .map_err(|_| "无法检查浏览器临时文件。".to_owned())?
}

#[tauri::command]
pub async fn clear_browser_temp_files(
    app: tauri::AppHandle,
    request: crate::browser_storage::cleanup::CleanupRequest,
) -> Result<crate::browser_storage::cleanup::CleanupResult, String> {
    use tauri::Manager;
    tauri::async_runtime::spawn_blocking(move || {
        crate::browser_storage::cleanup::execute(&app.state::<AppState>(), request)
    })
    .await
    .map_err(|_| "清理过程已中断，请重新查看，勿重复旧请求。".to_owned())?
}

#[tauri::command]
pub fn prepare_browser_mcp(
    state: State<AppState>,
    session_id: String,
) -> Result<BrowserMcpDraft, String> {
    prepare_browser_mcp_impl(&state, &session_id)
}

pub(crate) fn prepare_browser_mcp_impl(
    state: &AppState,
    session_id: &str,
) -> Result<BrowserMcpDraft, String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    let personal: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sessions s JOIN projects p ON p.id=s.project_id WHERE s.id=?1 AND p.kind='personal')", [session_id], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if !personal {
        return Err("浏览器预设需要日常主会话。".into());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let output = crate::browser_storage::prepare_output(&conn, &id)?;
    let args = serde_json::to_string(&[
        "-y",
        "@playwright/mcp@0.0.83",
        "--browser",
        "msedge",
        "--isolated",
        "--no-webmcp",
        "--image-responses",
        "omit",
        "--output-dir",
        &super::file::display_work_dir(&output),
    ])
    .map_err(|error| error.to_string())?;
    Ok(BrowserMcpDraft {
        id,
        name: "Playwright 浏览器".into(),
        command: "npx.cmd".into(),
        args,
    })
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

/// Assemble a fixed Main catalog without launching processes. Scope, launch
/// configuration and live tool schemas are rechecked at actual execution.
pub(crate) fn register_session_tools(
    state: &AppState,
    app: Option<&tauri::AppHandle>,
    registry: &mut crate::agent::ToolRegistry,
    session_id: &str,
) -> Result<crate::agent::handlers::McpToolRevisionCatalog, String> {
    use std::sync::Arc;
    use tauri::Manager;
    let (definitions, fingerprints, browser_paths) = state.mcp_manager.with_policy_gate(|| {
        let conn = state.db.lock().map_err(|_| "MCP 配置暂不可用")?;
        let mut definitions = Vec::new();
        let mut fingerprints = std::collections::HashMap::new();
        let mut browser_paths = Vec::new();
        for id in enabled_server_ids_for_session(&conn, session_id)? {
            let (command, args): (String, String) = conn
                .query_row(
                    "SELECT command,COALESCE(args,'') FROM mcp_servers WHERE id=?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|_| "MCP 配置暂不可用")?;
            let live = if state.mcp_manager.get_status(&id)?.status == "running" {
                state.mcp_manager.cached_discovery(&id)?
            } else {
                None
            };
            let tools = if let Some(tools) = live {
                super::mcp_catalog::store(&conn, &id, &command, &args, &tools)?;
                Some(tools)
            } else {
                super::mcp_catalog::load(&conn, &id)?
            };
            if let Some(tools) = tools {
                if let Some(output) = crate::browser_storage::configured_output(&conn, &id, &args)?
                {
                    let root = super::file::personal_files_root(&conn)?;
                    browser_paths.push(
                        output
                            .strip_prefix(root)
                            .map_err(|error| error.to_string())?
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
                fingerprints.insert(
                    id.clone(),
                    super::mcp_catalog::configuration_fingerprint(&command, &args),
                );
                definitions.push((id, tools));
            }
        }
        Ok((definitions, fingerprints, browser_paths))
    })?;
    let db = state.db.clone();
    let session_id = session_id.to_owned();
    let is_enabled: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |id| {
        db.lock()
            .map_err(|_| "MCP 配置暂不可用".to_string())
            .and_then(|conn| {
                let allowed = enabled_server_ids_for_session(&conn, &session_id)?;
                let fingerprint: Option<String> = conn
                    .query_row(
                        "SELECT config_fingerprint FROM mcp_tool_catalog WHERE server_id=?1",
                        [id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|_| "MCP 目录暂不可用".to_string())?;
                Ok(allowed.iter().any(|allowed| allowed == id)
                    && fingerprint
                        .as_ref()
                        .is_some_and(|value| fingerprints.get(id) == Some(value)))
            })
            .unwrap_or(false)
    });
    let authority = is_enabled.clone();
    let app = app.cloned();
    let manager = state.mcp_manager.clone();
    let connect: crate::agent::handlers::McpEnsureConnected = Arc::new(move |id, context| {
        let check = || {
            if !authority(id) || context.is_some_and(|ctx| ctx.is_cancelled() || ctx.is_expired()) {
                return Err("MCP 操作已取消、过期或权限／配置已变化，未执行工具".into());
            }
            Ok(())
        };
        check()?;
        if let Some(app) = &app {
            ensure_mcp_server_connected(app, &app.state::<AppState>(), id, check)
        } else if manager.get_status(id)?.status == "running" {
            // The state-only test/dev adapter cannot access native credentials.
            Ok(())
        } else {
            Err("此执行环境不能启动 MCP 服务；请在桌面应用中继续".into())
        }
    });
    let revisions = crate::agent::handlers::register_mcp_catalog_handlers(
        registry,
        state.mcp_manager.clone(),
        definitions,
        is_enabled,
        Some(connect),
    );
    if !browser_paths.is_empty() {
        let note = format!("宿主登记的浏览器临时目录：{}。其中快照、日志与下载不随备份，不能当成持久交付。日常 Main 需要交付下载时，用 organize_workspace_item 的 copy 将精确源文件复制到临时区外的普通结果路径，不覆盖已有目标，再读回并提供 angelbot-file 链接。其它项目没有跨日常目录复制权限，应切回日常处理，不用命令绕过。", browser_paths.join("、"));
        registry.append_prompt_snippet("organize_workspace_item", &note);
    }
    Ok(revisions)
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
                    catalog_ready: false,
                    catalog_error: None,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut servers = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        drop(stmt);
        let store = KeyringMcpCredentialStore::new(&app);
        for server in &mut servers {
            match super::mcp_catalog::load(&conn, &server.id) {
                Ok(catalog) => server.catalog_ready = catalog.is_some(),
                Err(error) => server.catalog_error = Some(error),
            }
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
        conn.execute(
            "DELETE FROM mcp_tool_catalog WHERE server_id=?1",
            [&server_id],
        )
        .map_err(|_| "MCP 工具目录无法失效")?;
        mcp_credentials::replace_env(&mut conn, &store, &server_id, &vars)?;
        Ok(())
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
        conn.execute(
            "DELETE FROM mcp_tool_catalog WHERE server_id=?1",
            [&server_id],
        )
        .map_err(|_| "MCP 工具目录无法失效")?;
        mcp_credentials::replace_env(&mut conn, &store, &server_id, &vars)?;
        Ok(())
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
        conn.execute(
            "DELETE FROM mcp_tool_catalog WHERE server_id=?1",
            [&server_id],
        )
        .map_err(|_| "MCP 工具目录无法失效")?;
        mcp_credentials::delete_env(&mut conn, &store, &server_id)?;
        Ok(())
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
    fn browser_root_metadata_is_limited_to_the_existing_host_owned_preset() {
        let directory = tempfile::tempdir().unwrap();
        let conn = Connection::open(directory.path().join("data.db")).unwrap();
        migrate(&conn).unwrap();
        let root = crate::commands::file::personal_files_root(&conn).unwrap();
        let make_args = |package: &str, output: &std::path::Path| {
            serde_json::to_string(&["-y", package, "--output-dir", &output.to_string_lossy()])
                .unwrap()
        };
        let args = make_args("@playwright/mcp@0.0.83", &root);
        assert_eq!(
            super::personal_browser_root(&conn, "npx.cmd", &args).unwrap(),
            Some(root.clone())
        );
        assert!(super::personal_browser_root(&conn, "node", &args)
            .unwrap()
            .is_none());
        assert!(
            super::personal_browser_root(&conn, "npx", &make_args("other-package", &root))
                .unwrap()
                .is_none()
        );
        assert!(super::personal_browser_root(
            &conn,
            "npx",
            &make_args("@playwright/mcp@0.0.83", directory.path())
        )
        .unwrap()
        .is_none());
        assert!(super::personal_browser_root(
            &conn,
            "npx",
            r#"["-y","@playwright/mcp@0.0.83","--output-dir"]"#
        )
        .unwrap()
        .is_none());
        let duplicate = serde_json::to_string(&[
            "-y",
            "@playwright/mcp@0.0.83",
            "--output-dir",
            &root.to_string_lossy(),
            "--output-dir",
            &root.to_string_lossy(),
        ])
        .unwrap();
        assert!(super::personal_browser_root(&conn, "npx", &duplicate)
            .unwrap()
            .is_none());
        let spoofed = serde_json::to_string(&[
            "-y",
            "other-package",
            "--config",
            "@playwright/mcp@0.0.83",
            "--output-dir",
            &root.to_string_lossy(),
        ])
        .unwrap();
        assert!(super::personal_browser_root(&conn, "npx", &spoofed)
            .unwrap()
            .is_none());
        assert!(
            super::personal_browser_root(&Connection::open_in_memory().unwrap(), "npx", &args)
                .is_err()
        );
    }

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

/// The existing preset already allows the browser to access Daily results via
/// outputDir. Advertise that same host-owned root, not a server-selected path.
/// Other services and user-chosen output locations retain their old handshake.
fn personal_browser_root(
    conn: &Connection,
    command: &str,
    args: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let args = crate::mcp_client::parse_command_args(args)?;
    let outputs = args
        .iter()
        .enumerate()
        .filter(|(_, arg)| *arg == "--output-dir")
        .collect::<Vec<_>>();
    if outputs.len() != 1 {
        return Ok(None);
    }
    let Some(output) = args.get(outputs[0].0 + 1) else {
        return Ok(None);
    };
    if !["npx", "npx.cmd"].contains(&command.to_ascii_lowercase().as_str())
        || args.first().map(String::as_str) != Some("-y")
        || args.get(1).map(String::as_str) != Some("@playwright/mcp@0.0.83")
    {
        return Ok(None);
    }
    let root = crate::commands::file::personal_files_root(conn)?;
    Ok(std::fs::canonicalize(output)
        .ok()
        .filter(|output| *output == root))
}

/// Start an MCP server process by its config ID.
/// Looks up the server configuration from the database, then spawns the process.
#[tauri::command]
pub fn start_mcp_server(
    app: tauri::AppHandle,
    state: State<AppState>,
    server_id: String,
) -> Result<McpServerStatus, String> {
    ensure_mcp_server_connected(&app, &state, &server_id, || Ok(()))?;
    state.mcp_manager.get_status(&server_id)
}

/// Shared by explicit Settings connection and an already admitted Main tool.
/// Discovery is durable, but never substitutes for live schema verification.
pub(crate) fn ensure_mcp_server_connected(
    app: &tauri::AppHandle,
    state: &AppState,
    server_id: &str,
    check: impl Fn() -> Result<(), String>,
) -> Result<(), String> {
    state
        .mcp_manager
        .with_server_start_gate(server_id, &check, |check| {
            let pending = state.mcp_manager.with_server_policy_gate(server_id, || {
                check()?;
                // Keep lookup and launch together so a concurrent edit cannot start
                // an obsolete or disabled server after its revocation commits.
                let config =
                    {
                        let conn = state.db.lock().map_err(|e| e.to_string())?;
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
                if state.mcp_manager.get_status(server_id)?.status == "running" {
                    return Ok(None);
                }

                let vars = {
                    let mut conn = state.db.lock().map_err(|error| error.to_string())?;
                    let store = KeyringMcpCredentialStore::new(app);
                    mcp_credentials::read_effective_env(&mut conn, &store, server_id)?
                };
                let env_str = mcp_credentials::launch_env(&vars);
                let root = {
                    let conn = state.db.lock().map_err(|error| error.to_string())?;
                    if crate::browser_storage::configured_output(&conn, server_id, &args)?.is_some()
                    {
                        crate::browser_storage::prepare_output(&conn, server_id)?;
                        Some(super::file::personal_files_root(&conn)?)
                    } else {
                        personal_browser_root(&conn, &command, &args)?
                    }
                };
                state
                    .mcp_manager
                    .begin_start(server_id, &name, &command, &args, &env_str)
                    .map(|pending| Some((pending, root)))
            })?;
            // The child is registered before releasing the gate. Stop and config
            // revocation can now terminate a silent first handshake promptly.
            let discover =
                pending.is_some() || state.mcp_manager.cached_discovery(server_id)?.is_none();
            if let Some((pending, root)) = pending {
                state
                    .mcp_manager
                    .finish_start_with_root(pending, root.as_deref())?;
            }
            if discover {
                // initialize alone does not discover tools. Keep this wait
                // outside lifecycle/policy-write locks so Stop stays usable.
                state.mcp_manager.refresh_tools(server_id)?;
            }
            state.mcp_manager.with_server_policy_gate(server_id, || {
                check()?;
                if state.mcp_manager.get_status(server_id)?.status != "running" {
                    return Err("MCP 连接已停止，未执行工具".into());
                }
                let tools = state
                    .mcp_manager
                    .cached_discovery(server_id)?
                    .ok_or("MCP 工具目录尚未成功读取，请重新连接")?;
                persist_tool_catalog(state, server_id, &tools)
            })
        })
}

fn persist_tool_catalog(
    state: &AppState,
    server_id: &str,
    tools: &[McpTool],
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|_| "MCP 配置暂不可用")?;
    let (command, args): (String, String) = conn
        .query_row(
            "SELECT command,COALESCE(args,'') FROM mcp_servers WHERE id=?1 AND enabled=1",
            [server_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "MCP 配置已变化或停用")?;
    super::mcp_catalog::store(&conn, server_id, &command, &args, tools)
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
    state.mcp_manager.refresh_tools(&server_id)?;
    state.mcp_manager.with_server_policy_gate(&server_id, || {
        if state.mcp_manager.get_status(&server_id)?.status != "running" {
            return Err("MCP 连接已停止，请重新连接".into());
        }
        let tools = state
            .mcp_manager
            .cached_discovery(&server_id)?
            .ok_or("MCP 工具目录尚未成功读取，请重新连接")?;
        persist_tool_catalog(&state, &server_id, &tools)?;
        Ok(tools)
    })
}
