//! Foreground-only tool surface assembly and execution permission resolution.
//!
//! The user-facing Angel runner owns this surface. Delegated workers receive
//! their own capability-limited registry and must never call this module.

use crate::agent::ToolRegistry;
use crate::AppState;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use tauri::AppHandle;

pub(crate) const AGENT_EXECUTION_PERMISSION_KEY: &str = "agent_execution_permission";

/// Single assembly point for the foreground agent's tool registry.
pub(crate) struct ForegroundToolSurface<'a> {
    state: &'a AppState,
    app: Option<&'a AppHandle>,
    memory_reading_enabled: bool,
}

/// Conservative pre-build estimate used by context-budget admission.
pub(crate) const ESTIMATED_TOOL_COUNT: usize = 22;

/// One Main-Agent registry and the opaque MCP definition revisions that were
/// visible when it was built. The catalog exists only long enough to bind a
/// pending confirmation to that exact capability definition.
pub(crate) struct ForegroundToolSet {
    pub(crate) registry: ToolRegistry,
    pub(crate) mcp_tool_revisions: crate::agent::handlers::McpToolRevisionCatalog,
    /// The same host-resolved root that admitted the file handlers. A caller
    /// must not pair those handlers with a runner's default Documents root.
    pub(crate) file_root: Option<PathBuf>,
}

impl<'a> ForegroundToolSurface<'a> {
    pub(crate) fn new(state: &'a AppState, app: Option<&'a AppHandle>) -> Self {
        Self {
            state,
            app,
            memory_reading_enabled: true,
        }
    }

    pub(crate) fn with_memory_reading(mut self, enabled: bool) -> Self {
        self.memory_reading_enabled = enabled;
        self
    }

    /// Install every user-facing handler exactly once into an empty registry.
    /// Delegated workers must use a separate registry and capability policy.
    pub(crate) fn install(&self, registry: &mut ToolRegistry) {
        crate::agent::handlers::register_common_builtin_handlers(registry);
        crate::agent::handlers::register_extension_handlers(registry, self.state.db.clone());
        if !self.memory_reading_enabled {
            // This is capability admission, not a suggestion to the model.
            // Unknown/stale calls fail registry validation even in full access.
            registry.unregister("recall_memories");
            registry.unregister("traverse_graph");
            registry.unregister("check_constraint");
        }
        crate::agent::handlers::register_skill_import_handlers(
            registry,
            self.state.skills.registry.clone(),
        );
        // Do not advertise a disabled or unconfigured external capability to
        // the model. The handler re-checks this durable state immediately
        // before every request as well, covering revocation after assembly.
        let web_search_enabled = matches!(
            crate::commands::web_search::load_config(self.state),
            Ok(Some(config)) if config.enabled
        );
        if let Some(app) = self.app.filter(|_| web_search_enabled) {
            crate::agent::handlers::register_web_search_handler(
                registry,
                app.clone(),
                self.state.db.clone(),
            );
        }
    }

    /// Build a fresh foreground registry. `ToolRegistry::empty` is intentional:
    /// `ToolRegistry::new` already installs builtins and would double-register
    /// them when the surface is assembled.
    pub(crate) fn build(&self) -> ToolRegistry {
        let mut registry = ToolRegistry::empty();
        self.install(&mut registry);
        registry
    }

    /// Build the complete Main-Agent surface for one durable session.  The
    /// delegated-delivery controls are intentionally session-bound, so they
    /// are installed here rather than left to individual foreground execution
    /// paths (initial turn, confirmation continuation, or retry).
    pub(crate) fn build_for_session(&self, session_id: String) -> Result<ToolRegistry, String> {
        self.build_for_session_with_mcp_revisions(session_id)
            .map(|tools| tools.registry)
    }

    /// Build the isolated capability set used only when the foreground Main
    /// Agent summarizes an independently reviewed delivery. Change candidates
    /// receive a text-only surface; this cannot accept, discard or apply them.
    ///
    /// This deliberately does not call `build`: an automatic follow-up must
    /// not inherit filesystem, desktop, MCP, automation, delegation-issuance,
    /// or skill capabilities from an interactive user turn.  Its one
    /// acknowledgement tool is also bound to the exact durable delivery id.
    pub(crate) fn build_for_reviewed_delivery_summary(
        &self,
        session_id: String,
        delivery_id: String,
    ) -> ForegroundToolSet {
        let mut registry = ToolRegistry::empty();
        let text_only = self.state.db.lock().ok().and_then(|conn|
            crate::agent::delegated_delivery_follow_up::DelegatedDeliveryFollowUpQueue::summary_is_text_only(
                &conn,&session_id,&delivery_id).ok()
        ).unwrap_or(true);
        if !text_only {
            crate::commands::foreground_delegation_control::register_review_only(
                &mut registry,
                session_id,
                self.state
                    .delegation_pump
                    .as_ref()
                    .map(|host| host.service().clone()),
                delivery_id,
            );
        }
        ForegroundToolSet {
            registry,
            mcp_tool_revisions: Default::default(),
            file_root: None,
        }
    }

    /// Rebuild the user-facing surface needed to settle one persisted
    /// confirmation. Most tools are session-bound; the one delegated network
    /// action additionally reconstructs its live foreground authority from
    /// the pending assistant reply.
    pub(crate) fn build_for_confirmation(
        &self,
        session_id: &str,
        message_id: &str,
        call_id: &str,
        tool_name: &str,
    ) -> Result<ToolRegistry, String> {
        let mut registry = self.build_for_session(session_id.to_owned())?;
        if tool_name == "delegate_network_exploration" {
            if let Ok(sandbox_root) = delegated_sandbox_root(self.state) {
                let _ = self.install_pending_network_delegation_confirmation_at(
                    &mut registry,
                    session_id,
                    message_id,
                    call_id,
                    sandbox_root,
                );
            }
        }
        Ok(registry)
    }

    /// Testable form of confirmation assembly. Production code never accepts
    /// a model-selected path; it calls `build_for_confirmation`, which resolves
    /// the app-owned root above.
    pub(crate) fn build_for_confirmation_at(
        &self,
        session_id: &str,
        message_id: &str,
        call_id: &str,
        tool_name: &str,
        sandbox_root: PathBuf,
    ) -> Result<ToolRegistry, String> {
        let mut registry = self.build_for_session(session_id.to_owned())?;
        if tool_name == "delegate_network_exploration" {
            let _ = self.install_pending_network_delegation_confirmation_at(
                &mut registry,
                session_id,
                message_id,
                call_id,
                sandbox_root,
            );
        }
        Ok(registry)
    }

    pub(crate) fn build_for_session_with_mcp_revisions(
        &self,
        session_id: String,
    ) -> Result<ForegroundToolSet, String> {
        let mut registry = self.build();
        // Daily files reuse the same bounded file tools, never project commands
        // or delegated workspace authority.
        let file_root = crate::commands::file::resolve_work_dir(self.state, Some(&session_id)).ok();
        let workspace_files_available = file_root.is_some();
        crate::agent::handlers::register_desktop_handlers(
            &mut registry,
            self.state.db.clone(),
            self.state.desktop_adapter.clone(),
            workspace_files_available,
        );
        if workspace_files_available {
            crate::agent::handlers::register_workspace_file_handlers(&mut registry);
            let project_scope = {
                let conn = self.state.db.lock().map_err(|error| error.to_string())?;
                !conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions s JOIN projects p ON p.id=s.project_id WHERE s.id=?1 AND p.kind='personal')",
                    [&session_id], |row| row.get::<_, bool>(0),
                ).map_err(|error| error.to_string())?
            };
            if !project_scope {
                registry.unregister("run_project_command");
            }
        }
        let mcp_tool_revisions = crate::commands::mcp::register_session_tools(
            self.state,
            self.app,
            &mut registry,
            &session_id,
        )?;
        crate::commands::foreground_automation_control::register(
            &mut registry,
            session_id.clone(),
            self.state.db.clone(),
        );
        crate::commands::foreground_delegation_control::register(
            &mut registry,
            session_id.clone(),
            self.state
                .delegation_pump
                .as_ref()
                .map(|host| host.service().clone()),
        );
        crate::commands::foreground_task_understanding_control::register(
            &mut registry,
            session_id,
            self.state.db.clone(),
        );
        Ok(ForegroundToolSet {
            registry,
            mcp_tool_revisions,
            file_root,
        })
    }

    /// Same confirmation assembly with an explicit app-managed sandbox root.
    /// The narrow parameter keeps the durable reconstruction logic testable
    /// without making tests write into a user's application-data directory.
    pub(crate) fn install_pending_network_delegation_confirmation_at(
        &self,
        registry: &mut ToolRegistry,
        session_id: &str,
        message_id: &str,
        call_id: &str,
        sandbox_root: PathBuf,
    ) -> Result<(), String> {
        let (identity, binding) = {
            let conn = self
                .state
                .db
                .lock()
                .map_err(|_| "the delegation context is no longer available".to_string())?;
            let work_dir =
                crate::commands::file::resolve_delegation_workspace_root(&conn, session_id)?
                    .ok_or_else(|| "the delegation context is no longer available".to_string())?;
            let identity =
                crate::agent::delegate_work::ForegroundRunIdentity::prepare_pending_confirmation(
                    &conn, session_id, message_id, call_id, work_dir,
                )
                .map_err(|_| "the delegation context is no longer available".to_string())?;
            let (provider, model): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT agent_provider, agent_model FROM sessions WHERE id=?1",
                    [session_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|_| "the delegation context is no longer available".to_string())?
                .ok_or_else(|| "the delegation context is no longer available".to_string())?;
            let provider = provider
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "the delegation context is no longer available".to_string())?;
            let model = model
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "the delegation context is no longer available".to_string())?;
            let binding = crate::commands::foreground_model_factory::session_model_binding(
                &conn, session_id, &provider, &model,
            )?;
            (identity, binding)
        };
        crate::agent::delegate_work::register(
            registry,
            self.state.db.clone(),
            identity,
            sandbox_root,
            binding,
            self.state
                .delegation_pump
                .as_ref()
                .map(|host| host.service().clone()),
        );
        Ok(())
    }
}

/// Delegated sandboxes are app-managed resources, never children of the
/// user's canonical workspace.  Keep this path in the foreground assembly
/// module so initial execution and confirmation continuation cannot drift.
pub(crate) fn delegated_sandbox_root(state: &AppState) -> Result<PathBuf, String> {
    let conn = state.db.lock().map_err(|_| "runtime storage unavailable")?;
    let root = match conn.path() {
        Some(path) if !path.is_empty() => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err("runtime storage path must be absolute".into());
            }
            path.parent()
                .ok_or("runtime storage directory unavailable")?
                .to_owned()
        }
        // In-memory instances have no durable root. Preserve the normal
        // application default; file-backed instances must never fall back.
        _ => crate::app_data_dir(),
    };
    Ok(delegated_sandbox_root_from_app_data_dir(root))
}

fn delegated_sandbox_root_from_app_data_dir(app_data_dir: PathBuf) -> PathBuf {
    app_data_dir.join("delegated").join("sandboxes")
}

/// Normalize persisted execution permissions. Unknown values fail closed.
pub(crate) fn normalize_agent_execution_permission(value: &str) -> Result<&'static str, String> {
    match value.trim() {
        "ask" => Ok("ask"),
        "workspace_auto" => Ok("workspace_auto"),
        "full_access" => Ok("full_access"),
        _ => Err(
            "Unknown agent execution permission. Use ask, workspace_auto, or full_access."
                .to_string(),
        ),
    }
}

/// Resolve the effective permission for a foreground run from the persisted
/// grant/settings tables. Missing grants and malformed values resolve to ask.
pub(crate) fn agent_execution_permission(state: &AppState) -> Result<String, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    agent_execution_permission_from_conn(&conn)
}

pub(crate) fn agent_execution_permission_from_conn(conn: &Connection) -> Result<String, String> {
    let has_grants_table = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'agent_permission_grants')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        != 0;
    let grant = if has_grants_table {
        conn.query_row(
            "SELECT permission, expires_at FROM agent_permission_grants WHERE scope = 'global'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?
    } else {
        None
    };
    if let Some((permission, expires_at)) = grant {
        if expires_at
            .map(|value| value <= Utc::now().timestamp())
            .unwrap_or(false)
        {
            return Ok("ask".to_string());
        }
        return Ok(normalize_agent_execution_permission(&permission)
            .unwrap_or("ask")
            .to_string());
    }
    if has_grants_table {
        return Ok("ask".to_string());
    }
    let stored = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![AGENT_EXECUTION_PERMISSION_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| "ask".to_string());
    Ok(normalize_agent_execution_permission(&stored)
        .unwrap_or("ask")
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        delegated_sandbox_root_from_app_data_dir, normalize_agent_execution_permission,
        ForegroundToolSurface,
    };
    use crate::agent::{ToolCall, ToolCancellation, ToolExecutionContext};
    use crate::AppState;
    use rusqlite::{params, Connection};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    struct ForcedMemoryReadProvider {
        tool: &'static str,
        arguments: serde_json::Value,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[test]
    fn personal_results_use_normal_files_without_project_commands_or_delegation_root() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = state_with_pending_network_confirmation(directory.path());
        let conn = Connection::open(directory.path().join("data.db")).unwrap();
        crate::db::migrate(&conn).unwrap();
        state.db = Arc::new(Mutex::new(conn));
        let tools = ForegroundToolSurface::new(&state, None)
            .build_for_session_with_mcp_revisions("personal-main".into())
            .unwrap();
        assert_eq!(
            tools.file_root.as_ref().unwrap(),
            &directory.path().canonicalize().unwrap().join("results")
        );
        let tools = tools.registry;
        for name in [
            "read_file",
            "write_file",
            "list_workspace_items",
            "create_directory",
        ] {
            assert!(tools.contains(name));
        }
        assert!(!tools.contains("run_project_command"));
        let root = crate::commands::file::resolve_work_dir(&state, Some("personal-main")).unwrap();
        let write = tools.execute(
            &ToolCall::new(
                "write_file".into(),
                json!({"path":"result.csv", "content":"item,status\nwater,pending\n"}),
            ),
            "write",
            &root,
        );
        assert!(write.result.success, "{:?}", write.result);
        let read = tools.execute(
            &ToolCall::new("read_file".into(), json!({"path":"result.csv"})),
            "read",
            &root,
        );
        assert!(read.result.success);
        assert!(read.result.content.contains("water,pending"));
        let receipt: serde_json::Value = serde_json::from_str(&read.result.content).unwrap();
        assert_eq!(
            receipt["resolvedPath"],
            crate::commands::file::display_work_dir(
                &root.join("result.csv").canonicalize().unwrap()
            )
        );
        assert_eq!(receipt["content"], "item,status\nwater,pending\n");
        std::fs::write(root.join("download.bin"), [0, 255, 1, 128]).unwrap();
        let (name, bytes) = crate::commands::reply_export::session_file_snapshot(
            &state,
            "personal-main",
            "download.bin",
        )
        .unwrap();
        assert_eq!(name, "download.bin");
        assert_eq!(bytes, [0, 255, 1, 128]);
        assert!(crate::commands::reply_export::session_file_snapshot(
            &state,
            "personal-main",
            "../data.db"
        )
        .is_err());
        assert!(
            !tools
                .execute(
                    &ToolCall::new("read_file".into(), json!({"path":"../data.db"})),
                    "escape",
                    &root
                )
                .result
                .success
        );
        assert!(
            !tools
                .execute(
                    &ToolCall::new(
                        "run_project_command".into(),
                        json!({"program":"python", "args":["--version"]})
                    ),
                    "stale",
                    &root
                )
                .result
                .success
        );
        assert!(crate::commands::file::resolve_delegation_workspace_root(
            &state.db.lock().unwrap(),
            "personal-main"
        )
        .unwrap()
        .is_none());
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmProvider for ForcedMemoryReadProvider {
        fn name(&self) -> &str {
            "disabled-memory-probe"
        }
        async fn chat(
            &self,
            _messages: &[crate::llm::Message],
            tools: Option<&[crate::llm::ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<crate::llm::LlmResponse, String> {
            assert!(!tools.unwrap_or_default().iter().any(|tool| matches!(
                tool.name.as_str(),
                "recall_memories" | "traverse_graph" | "check_constraint"
            )));
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                return Ok(crate::llm::LlmResponse::Text {
                    text: "长期记忆已关闭。".into(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                });
            }
            Ok(crate::llm::LlmResponse::ToolCalls {
                calls: vec![crate::llm::ToolCall {
                    id: "stale-memory-call".into(),
                    name: self.tool.into(),
                    arguments: self.arguments.clone(),
                }],
                text: None,
                stop_reason: crate::llm::StopReason::ToolUse,
            })
        }
    }

    fn state_with_pending_network_confirmation(workspace: &std::path::Path) -> AppState {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO profile (id, updated_at) VALUES (1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO projects
                (id, name, path, created_at, kind, updated_at, active_session_id)
             VALUES ('project-network', 'Network project', ?1, 1, 'project', 1, NULL)",
            params![workspace.to_string_lossy().to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions
                (id, title, created_at, updated_at, work_dir, project_id, agent_provider, agent_model)
             VALUES ('network-session', 'Network', 1, 1, ?1, 'project-network', 'deepseek', 'deepseek-v4')",
            params![workspace.to_string_lossy().to_string()],
        )
        .unwrap();
        conn.execute(
            "UPDATE projects SET active_session_id = 'network-session' WHERE id = 'project-network'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages
                (id, session_id, role, content, is_provisional, created_at)
             VALUES ('network-reply', 'network-session', 'assistant', 'Need approval', 0, 11)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_runs
                (id, session_id, message_id, goal, status, plan, confirmation_state,
                 resumable, step_count, completed_step_count, created_at, updated_at)
             VALUES ('network-reply', 'network-session', 'network-reply',
                     'Inspect public documentation', 'awaiting_confirmation',
                     '[\"network-call\"]', 'pending', 1, 1, 0, 10, 11)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_steps
                (id, call_id, session_id, tool_name, tool_input, tool_output,
                 success, seq, created_at)
             VALUES ('network-call', 'network-call', 'network-session',
                     'delegate_network_exploration', ?1,
                     'Confirmation required before executing side-effect tool ''delegate_network_exploration''.',
                     0, 1, 11)",
            params![json!({
                "goal": "Inspect public documentation",
                "explorer_operations": [{
                    "kind": "search",
                    "provider_host": "docs.example.com",
                    "query": "bounded public documentation"
                }]
            })
            .to_string()],
        )
        .unwrap();
        AppState {
            db: Arc::new(Mutex::new(conn)),
            runtime_health: crate::runtime_health::RuntimeHealthState::default(),
            delegation_pump: None,
            delegation_available: false,
            delegated_model_factory:
                crate::agent::delegated_model_host::DeferredKeychainDelegatedModelHostFactory::new(),
            foreground_automation_pump: Default::default(),
            mcp_manager: Arc::new(crate::mcp_client::McpProcessManager::new()),
            file_watcher: crate::file_watcher::FileWatcherManager::new(),
            cached_settings: crate::settings_cache::new_cache(),
            steering: crate::commands::steering::SteeringState::new(),
            skills: crate::commands::skill::SkillState::new(),
            event_emitter: None,
            dev_event_store: None,
            desktop_adapter: crate::desktop_control::create_mock_adapter(),
            desktop_action_previews: Default::default(),
        }
    }

    #[test]
    fn unknown_execution_permission_is_rejected() {
        assert!(normalize_agent_execution_permission("always_yes").is_err());
        assert_eq!(normalize_agent_execution_permission("ask").unwrap(), "ask");
    }

    #[test]
    fn delegated_sandbox_root_follows_the_database_instance() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = crate::tests::test_app_state();
        state.db = std::sync::Arc::new(std::sync::Mutex::new(
            Connection::open(directory.path().join("data.db")).unwrap(),
        ));
        assert_eq!(
            super::delegated_sandbox_root(&state).unwrap(),
            directory.path().join("delegated/sandboxes"),
            "a temporary database must not issue resources in the owner's application directory"
        );
    }

    #[cfg(feature = "desktop-e2e")]
    #[test]
    fn desktop_e2e_sandbox_root_uses_the_canonical_runtime_override() {
        // This exercises the E2E override through a pure seam instead of
        // mutating the process environment, which would race parallel tests.
        let isolated_root = std::path::PathBuf::from("e2e-isolated-app-data");
        let runtime_root = crate::app_data_dir_from_e2e_override(Some(isolated_root.clone()));

        assert_eq!(
            delegated_sandbox_root_from_app_data_dir(runtime_root),
            isolated_root.join("delegated").join("sandboxes")
        );
    }

    #[test]
    fn cached_mcp_surface_never_launches_and_frozen_handlers_recheck_scope_and_configuration() {
        for revoke in ["scope", "config", "disable"] {
            let workspace = tempfile::tempdir().unwrap();
            let state = state_with_pending_network_confirmation(workspace.path());
            let tool: crate::mcp_client::McpTool = serde_json::from_value(json!({
                "name":"inspect","inputSchema":{"type":"object"}
            }))
            .unwrap();
            {
                let conn = state.db.lock().unwrap();
                conn.execute_batch("INSERT INTO mcp_servers(id,name,command,args,enabled,created_at) VALUES('cached','Cached','never-run-this-command','',1,1),('new','New','never-run-this-command','',1,1);
                    INSERT INTO mcp_workspace_enablements(workspace_id,server_id,enabled_at) VALUES('project-network','cached',1),('project-network','new',1);").unwrap();
                super::super::mcp_catalog::store(
                    &conn,
                    "cached",
                    "never-run-this-command",
                    "",
                    &[tool],
                )
                .unwrap();
            }
            let registry = ForegroundToolSurface::new(&state, None)
                .build_for_session("network-session".into())
                .unwrap();
            let name = crate::agent::handlers::mcp_registry_tool_name("cached", "inspect");
            assert!(registry.contains(&name));
            assert!(!registry
                .get_schemas()
                .iter()
                .any(|schema| schema["function"]["name"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("mcp_new__")));
            assert!(state.mcp_manager.running_servers().is_empty());
            {
                let conn = state.db.lock().unwrap();
                conn.execute_batch(match revoke {
                    "scope" => "DELETE FROM mcp_workspace_enablements WHERE server_id='cached'",
                    "config" => "UPDATE mcp_servers SET args='changed' WHERE id='cached'",
                    _ => "UPDATE mcp_servers SET enabled=0 WHERE id='cached'",
                })
                .unwrap();
            }
            let call = crate::agent::ToolCall::new(name.clone(), json!({}));
            let result = registry.execute(&call, "call", workspace.path()).result;
            assert!(!result.success);
            assert!(result.content.contains("不再向当前工作区启用"));
            assert!(state.mcp_manager.running_servers().is_empty());
            assert!(!ForegroundToolSurface::new(&state, None)
                .build_for_session("network-session".into())
                .unwrap()
                .contains(&name));
        }
    }

    #[tokio::test]
    async fn disabled_memory_reading_removes_handlers_schemas_and_guidance_without_removing_owner_writes(
    ) {
        let workspace = tempfile::tempdir().unwrap();
        let state = state_with_pending_network_confirmation(workspace.path());
        let enabled = ForegroundToolSurface::new(&state, None)
            .build_for_session("network-session".into())
            .unwrap();
        let disabled = ForegroundToolSurface::new(&state, None)
            .with_memory_reading(false)
            .build_for_session_with_mcp_revisions("network-session".into())
            .unwrap()
            .registry;
        for (name, arguments) in [
            (
                "recall_memories",
                json!({"query":"", "include_learned_habits":true}),
            ),
            ("traverse_graph", json!({"entity":"private fact"})),
            ("check_constraint", json!({"action":"read a local file"})),
        ] {
            assert!(enabled.contains(name));
            assert!(!disabled.contains(name));
            assert!(!disabled
                .get_schemas()
                .iter()
                .any(|schema| schema["function"]["name"] == name));
            assert!(!disabled
                .get_prompt_snippets()
                .iter()
                .any(|(tool, _)| tool == name));
            let execution = disabled.execute(
                &ToolCall::new(name.into(), arguments.clone()),
                "stale-memory-call",
                workspace.path(),
            );
            assert!(
                !execution.result.success,
                "a hidden schema alone is not an execution guard"
            );
            for permission in ["ask", "full_access"] {
                let mut config = crate::agent::config::AgentConfig::default();
                config.preferences.use_long_term_memory = false;
                let agent = crate::agent::AgentRunner::with_registry(
                    Box::new(ForcedMemoryReadProvider {
                        tool: name,
                        arguments: arguments.clone(),
                        calls: Default::default(),
                    }),
                    config,
                    disabled.clone(),
                )
                .with_execution_permission(permission.into());
                match agent.run("你好", &[]).await {
                    crate::agent::AgentResponse::TextWithToolCalls { steps, .. } => {
                        let reads: Vec<_> =
                            steps.iter().filter(|step| step.tool_name == name).collect();
                        assert_eq!(reads.len(), 1, "unexpected steps: {steps:#?}");
                        assert!(
                            !reads[0].success,
                            "disabled reading must never execute: {steps:#?}"
                        );
                        assert!(
                            !reads[0].confirmation_required,
                            "an unavailable capability cannot be approved: {steps:#?}"
                        );
                        assert!(
                            reads[0].output.contains("not executed"),
                            "unexpected denial: {steps:#?}"
                        );
                    }
                    other => panic!("Expected denied stale memory call, got {other:?}"),
                }
            }
        }
        for name in ["remember_fact", "update_memory", "forget_memory"] {
            assert!(
                disabled.get(name).unwrap().requires_confirmation,
                "explicit owner writes must retain their existing permission flow"
            );
        }
        assert!(
            disabled.contains("extract_entities"),
            "stateless parsing is not long-term recall"
        );
        assert!(disabled.contains("schedule_reminder"));
        assert!(
            ForegroundToolSurface::new(&state, None)
                .build_for_session("network-session".into())
                .unwrap()
                .contains("recall_memories"),
            "one request's opt-out must not mutate a different registry"
        );
    }

    #[test]
    fn reviewed_delivery_summary_surface_exposes_only_the_scoped_acknowledgement() {
        let workspace = tempfile::tempdir().unwrap();
        let state = state_with_pending_network_confirmation(workspace.path());
        let surface = ForegroundToolSurface::new(&state, None);

        state.db.lock().unwrap().execute_batch(
            "INSERT INTO work_packages(id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,candidate_version,status,created_at,updated_at)
             VALUES ('explore-package','network-session',1,'wk','explore','explorer',1,'scope','approved',100,0,'active',1,1);
             INSERT INTO delegations(id,session_id,message_id,parent_run_id,work_package_id,objective,brief_json,status,state_version,created_at,updated_at)
             VALUES ('explore-delegation','network-session','network-reply','network-reply','explore-package','inspect','{}','awaiting_summary',0,1,1);
             INSERT INTO delegation_attempts(id,delegation_id,attempt_number,status,sandbox_ref,created_at,ended_at)
             VALUES ('explore-attempt','explore-delegation',1,'sealed','sandbox://explore',1,2);
             INSERT INTO delegation_deliveries(id,delegation_id,attempt_id,delivery_version,schema_version,payload_json,acceptance_status,created_at)
             VALUES ('delivery-reviewed','explore-delegation','explore-attempt',1,1,'{}','submitted',2);
             INSERT INTO delegation_review_jobs(id,implementation_attempt_id,delivery_id,delivery_revision,subject_json,status,reviewer_attempt_id,outcome_json,created_at,updated_at,started_at,ended_at)
             VALUES ('review','explore-attempt','delivery-reviewed',1,'{}','passed','reviewer','{}',2,2,2,2);"
        ).unwrap();
        let tools = surface.build_for_reviewed_delivery_summary(
            "network-session".to_string(),
            "delivery-reviewed".to_string(),
        );

        assert_eq!(tools.registry.len(), 1);
        assert!(tools.registry.contains("review_delegated_delivery"));
        assert!(!tools.registry.contains("materialize_delegated_change"));
        assert!(!tools.registry.contains("list_delegated_work"));
        assert!(!tools.registry.contains("stop_delegated_work"));
        assert!(!tools.registry.contains("read_file"));
        assert!(!tools.registry.contains("delegate_network_exploration"));
        assert!(tools.mcp_tool_revisions.is_empty());
        state.db.lock().unwrap().execute_batch(
            "INSERT INTO work_packages(id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,candidate_version,status,created_at,updated_at)
             VALUES ('change-package','network-session',1,'wk','change','implementer',1,'scope','approved',100,0,'active',1,1);
             INSERT INTO delegations(id,session_id,message_id,parent_run_id,work_package_id,objective,brief_json,status,state_version,created_at,updated_at)
             VALUES ('change-delegation','network-session','network-reply','network-reply','change-package','prepare','{}','awaiting_summary',0,1,1);
             INSERT INTO delegation_attempts(id,delegation_id,attempt_number,status,sandbox_ref,created_at,ended_at)
             VALUES ('change-attempt','change-delegation',1,'sealed','sandbox://change',1,2);
             INSERT INTO delegation_deliveries(id,delegation_id,attempt_id,delivery_version,schema_version,payload_json,acceptance_status,created_at)
             VALUES ('change-delivery','change-delegation','change-attempt',1,1,'{}','submitted',2);"
        ).unwrap();
        let candidate = surface.build_for_reviewed_delivery_summary(
            "network-session".into(),
            "change-delivery".into(),
        );
        assert!(
            candidate.registry.is_empty(),
            "automatic Change summaries cannot accept, discard or materialize"
        );
        assert!(candidate.mcp_tool_revisions.is_empty());
        state.db.lock().unwrap().execute_batch(
            "DELETE FROM delegation_review_jobs WHERE id='review';
             INSERT INTO delegation_review_jobs(id,implementation_attempt_id,delivery_id,delivery_revision,subject_json,status,reviewer_attempt_id,outcome_json,created_at,updated_at,started_at,ended_at)
             VALUES ('review','explore-attempt','delivery-reviewed',1,'{}','failed','reviewer','{}',2,2,2,2);"
        ).unwrap();
        let failed = surface.build_for_reviewed_delivery_summary(
            "network-session".into(),
            "delivery-reviewed".into(),
        );
        assert!(
            failed.registry.is_empty(),
            "a failed review cannot be automatically accepted or retried"
        );
        assert!(surface
            .build_for_reviewed_delivery_summary(
                "network-session".into(),
                "missing-delivery".into()
            )
            .registry
            .is_empty());
    }

    #[test]
    fn pending_network_confirmation_rebuilds_and_executes_the_exact_scoped_tool() {
        let workspace = tempfile::tempdir().unwrap();
        let state = state_with_pending_network_confirmation(workspace.path());
        let surface = ForegroundToolSurface::new(&state, None);
        let registry = surface
            .build_for_confirmation_at(
                "network-session",
                "network-reply",
                "network-call",
                "delegate_network_exploration",
                workspace.path().join("delegated-sandboxes"),
            )
            .unwrap();
        let arguments = json!({
            "goal": "Inspect public documentation",
            "explorer_operations": [{
                "kind": "search",
                "provider_host": "docs.example.com",
                "query": "bounded public documentation"
            }]
        });
        let context = ToolExecutionContext::new("network-call", ToolCancellation::new())
            .with_run_id("network-reply")
            .for_confirmation(None);
        let executed = registry.execute_with_context(
            &ToolCall::new("delegate_network_exploration".into(), arguments),
            "network-call",
            workspace.path(),
            &context,
        );
        assert!(executed.result.success, "{}", executed.result.content);
        let conn = state.db.lock().unwrap();
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM delegated_network_scope_approvals",
                [],
                |row| row.get(0),
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM delegations WHERE parent_run_id='network-reply'",
                [],
                |row| row.get(0),
            )
            .unwrap(),
            1
        );
    }
}
