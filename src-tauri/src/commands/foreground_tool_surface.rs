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
}

/// Conservative pre-build estimate used by context-budget admission.
pub(crate) const ESTIMATED_TOOL_COUNT: usize = 22;

/// One Main-Agent registry and the opaque MCP definition revisions that were
/// visible when it was built. The catalog exists only long enough to bind a
/// pending confirmation to that exact capability definition.
pub(crate) struct ForegroundToolSet {
    pub(crate) registry: ToolRegistry,
    pub(crate) mcp_tool_revisions: crate::agent::handlers::McpToolRevisionCatalog,
}

impl<'a> ForegroundToolSurface<'a> {
    pub(crate) fn new(state: &'a AppState, app: Option<&'a AppHandle>) -> Self {
        Self { state, app }
    }

    /// Install every user-facing handler exactly once into an empty registry.
    /// Delegated workers must use a separate registry and capability policy.
    pub(crate) fn install(&self, registry: &mut ToolRegistry) {
        crate::agent::handlers::register_common_builtin_handlers(registry);
        crate::agent::handlers::register_extension_handlers(registry, self.state.db.clone());
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

    fn install_enabled_mcp_handlers(
        &self,
        registry: &mut ToolRegistry,
        session_id: &str,
    ) -> crate::agent::handlers::McpToolRevisionCatalog {
        let db = self.state.db.clone();
        let session_id = session_id.to_string();
        crate::agent::handlers::register_mcp_handlers_for_servers(
            registry,
            self.state.mcp_manager.clone(),
            move |server_id| {
                db.lock()
                    .map_err(|error| error.to_string())
                    .and_then(|conn| {
                        crate::commands::mcp::enabled_server_ids_for_session(&conn, &session_id)
                    })
                    .map(|server_ids| server_ids.iter().any(|allowed| allowed == server_id))
                    .unwrap_or(false)
            },
        )
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
    pub(crate) fn build_for_session(&self, session_id: String) -> ToolRegistry {
        self.build_for_session_with_mcp_revisions(session_id)
            .registry
    }

    /// Build the isolated capability set used only when the foreground Main
    /// Agent summarizes an already independently reviewed Explorer delivery.
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
        crate::commands::foreground_delegation_control::register_review_only(
            &mut registry,
            session_id,
            self.state.delegation_pump.clone(),
            delivery_id,
        );
        ForegroundToolSet {
            registry,
            mcp_tool_revisions: Default::default(),
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
    ) -> ToolRegistry {
        let mut registry = self.build_for_session(session_id.to_owned());
        if tool_name == "delegate_network_exploration" {
            if let Ok(sandbox_root) = delegated_sandbox_root() {
                let _ = self.install_pending_network_delegation_confirmation_at(
                    &mut registry,
                    session_id,
                    message_id,
                    call_id,
                    sandbox_root,
                );
            }
        }
        registry
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
    ) -> ToolRegistry {
        let mut registry = self.build_for_session(session_id.to_owned());
        if tool_name == "delegate_network_exploration" {
            let _ = self.install_pending_network_delegation_confirmation_at(
                &mut registry,
                session_id,
                message_id,
                call_id,
                sandbox_root,
            );
        }
        registry
    }

    pub(crate) fn build_for_session_with_mcp_revisions(
        &self,
        session_id: String,
    ) -> ForegroundToolSet {
        let mut registry = self.build();
        // Personal space intentionally has no filesystem root. Project and
        // migrated legacy sessions receive file tools only when their durable
        // directory can actually be resolved.
        let workspace_files_available =
            crate::commands::file::resolve_work_dir(self.state, Some(&session_id)).is_ok();
        crate::agent::handlers::register_desktop_handlers(
            &mut registry,
            self.state.db.clone(),
            self.state.desktop_adapter.clone(),
            workspace_files_available,
        );
        if workspace_files_available {
            crate::agent::handlers::register_workspace_file_handlers(&mut registry);
        }
        let mcp_tool_revisions = self.install_enabled_mcp_handlers(&mut registry, &session_id);
        crate::commands::foreground_automation_control::register(
            &mut registry,
            session_id.clone(),
            self.state.db.clone(),
        );
        crate::commands::foreground_delegation_control::register(
            &mut registry,
            session_id.clone(),
            self.state.delegation_pump.clone(),
        );
        crate::commands::foreground_task_understanding_control::register(
            &mut registry,
            session_id,
            self.state.db.clone(),
        );
        ForegroundToolSet {
            registry,
            mcp_tool_revisions,
        }
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
            self.state.delegation_pump.clone(),
        );
        Ok(())
    }
}

/// Delegated sandboxes are app-managed resources, never children of the
/// user's canonical workspace.  Keep this path in the foreground assembly
/// module so initial execution and confirmation continuation cannot drift.
pub(crate) fn delegated_sandbox_root() -> Result<PathBuf, String> {
    Ok(delegated_sandbox_root_from_app_data_dir(
        crate::app_data_dir(),
    ))
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
    let grant = conn
        .query_row(
            "SELECT permission, expires_at FROM agent_permission_grants WHERE scope = 'global'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
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
    fn reviewed_delivery_summary_surface_exposes_only_the_scoped_acknowledgement() {
        let workspace = tempfile::tempdir().unwrap();
        let state = state_with_pending_network_confirmation(workspace.path());
        let surface = ForegroundToolSurface::new(&state, None);

        let tools = surface.build_for_reviewed_delivery_summary(
            "network-session".to_string(),
            "delivery-reviewed".to_string(),
        );

        assert_eq!(tools.registry.len(), 1);
        assert!(tools.registry.contains("review_delegated_delivery"));
        assert!(!tools.registry.contains("materialize_delegated_change"));
        assert!(!tools.registry.contains("read_file"));
        assert!(!tools.registry.contains("delegate_network_exploration"));
        assert!(tools.mcp_tool_revisions.is_empty());
    }

    #[test]
    fn pending_network_confirmation_rebuilds_and_executes_the_exact_scoped_tool() {
        let workspace = tempfile::tempdir().unwrap();
        let state = state_with_pending_network_confirmation(workspace.path());
        let surface = ForegroundToolSurface::new(&state, None);
        let registry = surface.build_for_confirmation_at(
            "network-session",
            "network-reply",
            "network-call",
            "delegate_network_exploration",
            workspace.path().join("delegated-sandboxes"),
        );
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
