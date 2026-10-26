//! AngelBot - Tauri application entry point
//!
//! Architecture:
//!   - `AppState` holds shared resources (DB connection)
//!   - `commands/*` contains domain-specific Tauri command handlers
//!   - `db.rs` handles SQLite init and migrations
//!   - `vector.rs` handles vector similarity and embedding
//!   - Dev HTTP server (debug only) mirrors all commands for frontend proxy
//!   - `agent/*` handles tool registry and function calling
//!   - `llm/*` provides unified LLM provider abstraction

mod agent;
mod automation_clock;
mod backup;
mod bounded_png;
mod browser_storage;
mod child_process_env;
mod child_process_tree;
mod commands;
mod db;
mod desktop_control;
mod document_text;
mod file_watcher;
mod fts;
mod keychain;
mod knowledge_graph;
mod llm;
mod maa_routine;
mod mcp_client;
mod mcp_credentials;
mod profile_storage;
mod reranker;
mod runtime_health;
mod runtime_instance;
mod settings_cache;
mod skill;
mod vector;
mod vector_store;
mod window_capture;

use rusqlite::Connection;
use std::sync::Mutex;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};

use crate::agent::delegated_model_host::DeferredKeychainDelegatedModelHostFactory;
use crate::agent::delegation_pump::DelegationPumpHostHandle;
use crate::agent::event::{DevEventStore, EventEmitterState};
use crate::runtime_health::{RuntimeHealthState, RuntimeIssue};

/// Persisted setting used as the second (lower precedence) Explorer override
/// source. The environment and setting are resolved together at startup.
const DELEGATED_EXPLORER_SETTING: &str = crate::agent::explorer_runtime_config::ENABLE_SETTING;

pub struct AppState {
    pub db: std::sync::Arc<Mutex<Connection>>,
    /// Structured, user-actionable diagnostics. The desktop remains available
    /// in a degraded mode instead of crashing or failing silently.
    pub runtime_health: RuntimeHealthState,
    /// Type-erased delegated orchestration loop. `None` is used by isolated
    /// command tests; production startup installs the one delegated host loop
    /// (or its durable fail-closed fallback).
    pub delegation_pump: Option<DelegationPumpHostHandle>,
    pub delegation_available: bool,
    /// Installed during Tauri setup; delegated workers resolve the selected
    /// model through this opaque keychain adapter and never receive AppHandle.
    pub delegated_model_factory: DeferredKeychainDelegatedModelHostFactory,
    /// Coalesced application-owned wake path for durable agent automations.
    /// It delivers work through the existing foreground Main-Agent seam.
    pub(crate) foreground_automation_pump:
        commands::foreground_automation_pump::ForegroundAutomationPump,
    pub mcp_manager: std::sync::Arc<mcp_client::McpProcessManager>,
    pub file_watcher: file_watcher::FileWatcherManager,
    pub cached_settings: settings_cache::SettingsCache,
    pub steering: commands::steering::SteeringState,
    pub skills: commands::skill::SkillState,
    pub event_emitter: Option<EventEmitterState>,
    /// Pollable agent events used only by the debug HTTP command gateway.
    pub dev_event_store: Option<std::sync::Arc<DevEventStore>>,
    pub desktop_adapter: std::sync::Arc<dyn desktop_control::DesktopAdapter>,
    pub(crate) desktop_action_previews: commands::message::DesktopActionPreviewStore,
}

#[cfg(feature = "desktop-e2e")]
const DESKTOP_E2E_DATA_DIR_ENV: &str = "ANGELBOT_DESKTOP_E2E_DATA_DIR";

pub(crate) fn manual_desktop_acceptance() -> bool {
    cfg!(feature = "desktop-e2e")
        && std::env::var("ANGELBOT_DESKTOP_E2E_MANUAL").as_deref() == Ok("1")
}

fn model_recovery_acceptance() -> bool {
    cfg!(feature = "desktop-e2e")
        && matches!(
            std::env::var("ANGELBOT_DESKTOP_E2E_MODEL_RECOVERY").as_deref(),
            Ok("first" | "reopen")
        )
}

fn default_app_data_dir() -> std::path::PathBuf {
    dirs::data_local_dir()
        .expect("Windows local profile directory")
        .join(profile_storage::DIRECTORY)
}

/// Pure E2E root selection seam. Keeping the override out of individual
/// consumers prevents foreground-issued sandboxes from drifting away from the
/// runtime's isolated database and cleanup roots.
#[cfg(feature = "desktop-e2e")]
pub(crate) fn app_data_dir_from_e2e_override(
    e2e_root: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    e2e_root.unwrap_or_else(default_app_data_dir)
}

/// Returns the single durable root shared by the database and delegated-work
/// isolation. Desktop E2E may replace it with a temporary directory so tests
/// never read or write the user's local application state.
pub(crate) fn app_data_dir() -> std::path::PathBuf {
    #[cfg(test)]
    if let Some(root) = std::env::var_os("ANGELBOT_PRIVATE_VERIFY_CONFIG_DIR") {
        return std::path::PathBuf::from(root).join("runtime");
    }
    #[cfg(feature = "desktop-e2e")]
    {
        let isolated = std::env::var_os(DESKTOP_E2E_DATA_DIR_ENV).map(std::path::PathBuf::from);
        assert!(
            !manual_desktop_acceptance() || isolated.is_some(),
            "Manual native acceptance requires an explicit isolated data directory"
        );
        app_data_dir_from_e2e_override(isolated)
    }

    #[cfg(not(feature = "desktop-e2e"))]
    {
        profile_storage::saved_root()
    }
}

/// Build the one delegated host owned by the application.  The verified
/// delegated Explorer graph is enabled when no deployment override is present; an
/// explicit environment variable or persisted setting can still disable it.
/// Trusted sandbox/worktree roots are derived from app-data and constructed as
/// one graph. Missing roots, provider construction errors, or partial
/// readiness always fall back to the durable fail-closed host. This function
/// reuses the AppState DB and never starts a second scheduler loop.
struct DelegationPumpStartup {
    pump: Option<DelegationPumpHostHandle>,
    enabled: bool,
    issue: Option<RuntimeIssue>,
}

fn build_delegation_pump(
    db: &std::sync::Arc<Mutex<Connection>>,
    cached_settings: &settings_cache::SettingsCache,
    model_factory: DeferredKeychainDelegatedModelHostFactory,
) -> DelegationPumpStartup {
    let setting = db.lock().ok().map(|conn| {
        settings_cache::get_setting(&conn, cached_settings, DELEGATED_EXPLORER_SETTING, "")
    });
    let setting = setting.filter(|value| !value.trim().is_empty());
    // The verified Explorer graph is enabled for the production composition
    // by default.  An explicit environment value or persisted setting still
    // wins (including `false`) so operators retain an emergency kill switch.
    let environment = std::env::var(crate::agent::explorer_runtime_config::ENABLE_ENV).ok();
    let opt_in = if environment.is_none() && setting.is_none() {
        crate::agent::explorer_runtime_config::ExplorerOptIn::from_sources(Some("true"), None)
    } else {
        crate::agent::explorer_runtime_config::ExplorerOptIn::from_sources(
            environment.as_deref(),
            setting.as_deref(),
        )
    };
    let shared_db = crate::agent::shared_db::SharedDb::from_arc(db.clone());
    let assembly = if opt_in.enabled {
        let root = app_data_dir();
        (|| {
            let (sandbox, cleanup, provider) =
                crate::agent::concrete_sandbox::production_components_with_provider(
                    shared_db.clone(),
                    &root,
                )
                .map_err(|error| format!("sandbox composition failed: {error:?}"))?;
            #[cfg(feature = "desktop-e2e")]
            {
                let e2e_io =
                    crate::agent::desktop_e2e_explorer_io::DesktopE2eExplorerIo::from_environment()
                        .map_err(|error| {
                            format!("desktop E2E Explorer I/O is unavailable: {error}")
                        })?;
                if let Some(e2e_io) = e2e_io {
                    let (transport, resolver) = e2e_io.into_network_io();
                    let review_executor =
                        Box::new(crate::agent::review_agent::FixedReviewExecutor::new(
                            crate::agent::review_contract::ReviewOutcome {
                                verdict:
                                    crate::agent::review_contract::SemanticReviewVerdict::Passed,
                                summary:
                                    "desktop E2E reviewer verified the bounded fixture evidence"
                                        .into(),
                                findings: vec![],
                                missing_evidence: vec![],
                                evidence_refs: vec![],
                            },
                        ));
                    return crate::agent::delegated_runtime_factory::DelegatedRuntimeFactory::build_with_explorer_runtime_adapters(
                        shared_db.clone(),
                        opt_in,
                        crate::agent::explorer_runtime_config::ExplorerRuntimeDependencies::all_ready(),
                        sandbox,
                        cleanup,
                        provider,
                        model_factory.clone(),
                        transport,
                        resolver,
                        review_executor,
                    )
                    .map_err(|error| format!("delegated runtime composition failed: {error:?}"));
                }
            }
            crate::agent::delegated_runtime_factory::DelegatedRuntimeFactory::build(
                shared_db.clone(),
                opt_in,
                crate::agent::explorer_runtime_config::ExplorerRuntimeDependencies::all_ready(),
                sandbox,
                cleanup,
                provider,
                model_factory.clone(),
            )
            .map_err(|error| format!("delegated runtime composition failed: {error:?}"))
        })()
    } else {
        crate::agent::delegated_runtime_factory::DelegatedRuntimeFactory::fail_closed(
            shared_db.clone(),
        )
        .map_err(|error| format!("fail-closed runtime composition failed: {error:?}"))
    };
    let (assembly, mut issue) = match assembly {
        Ok(assembly) => {
            let issue = if assembly.enabled {
                None
            } else {
                Some(RuntimeIssue::delegation_disabled())
            };
            (assembly, issue)
        }
        Err(error) => {
            eprintln!("[Startup] delegated runtime unavailable: {error}");
            match crate::agent::delegated_runtime_factory::DelegatedRuntimeFactory::fail_closed(
                shared_db,
            ) {
                Ok(assembly) => (
                    assembly,
                    Some(RuntimeIssue::delegation_unavailable(
                        "运行时初始化失败，当前已安全降级为仅主 Agent 模式。",
                    )),
                ),
                Err(fallback) => {
                    eprintln!("[Startup] fail-closed delegated runtime unavailable: {fallback:?}");
                    return DelegationPumpStartup {
                        pump: None,
                        enabled: false,
                        issue: Some(RuntimeIssue::delegation_unavailable(
                            "运行时未能启动，主 Agent 对话仍可使用。",
                        )),
                    };
                }
            }
        }
    };
    let enabled = assembly.enabled;
    let pump = assembly.into_pump();
    if let Err(error) = pump.start() {
        eprintln!("[Startup] delegated pump failed to start: {error:?}");
        issue = Some(RuntimeIssue::delegation_unavailable(
            "调度器未能启动，主 Agent 对话仍可使用。",
        ));
        return DelegationPumpStartup {
            pump: None,
            enabled: false,
            issue,
        };
    }
    DelegationPumpStartup {
        pump: Some(pump),
        enabled,
        issue,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayMenuAction {
    ShowWindow,
    Quit,
    Ignore,
}

fn tray_menu_action(id: &str) -> TrayMenuAction {
    match id {
        "show" => TrayMenuAction::ShowWindow,
        "quit" => TrayMenuAction::Quit,
        _ => TrayMenuAction::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_menu_action_maps_known_menu_ids() {
        assert_eq!(tray_menu_action("show"), TrayMenuAction::ShowWindow);
        assert_eq!(tray_menu_action("quit"), TrayMenuAction::Quit);
        assert_eq!(tray_menu_action("unknown"), TrayMenuAction::Ignore);
    }

    #[test]
    fn runtime_health_exposes_degraded_startup_without_hiding_the_app() {
        let mut state = test_app_state();
        let ready = commands::runtime_health::runtime_health(&state);
        assert_eq!(
            ready.status,
            crate::runtime_health::RuntimeHealthStatus::Ready
        );
        assert!(ready.issues.is_empty());

        state.runtime_health.add(RuntimeIssue::ephemeral_storage());
        let degraded = commands::runtime_health::runtime_health(&state);
        assert_eq!(
            degraded.status,
            crate::runtime_health::RuntimeHealthStatus::Degraded
        );
        assert_eq!(degraded.issues.len(), 1);
        assert_eq!(
            degraded.issues[0].code,
            crate::runtime_health::RuntimeIssueCode::EphemeralStorage
        );
    }

    #[cfg(windows)]
    pub(crate) fn create_test_directory_link(link: &std::path::Path, target: &std::path::Path) {
        use std::io::Write;
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", "[Console]::InputEncoding=[Text.UTF8Encoding]::new($false); $p=[Console]::In.ReadToEnd()|ConvertFrom-Json; New-Item -ItemType Junction -Path $p.link -Target $p.target | Out-Null"])
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).creation_flags(0x08000000);
        crate::child_process_env::apply_minimal_child_environment(&mut command);
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                serde_json::to_string(&serde_json::json!({"link":link,"target":target}))
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap();
        assert!(child.wait_with_output().unwrap().status.success());
    }

    #[cfg(unix)]
    pub(crate) fn create_test_directory_link(link: &std::path::Path, target: &std::path::Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    pub(crate) fn test_app_state() -> AppState {
        AppState {
            db: std::sync::Arc::new(Mutex::new(Connection::open_in_memory().unwrap())),
            runtime_health: RuntimeHealthState::default(),
            delegation_pump: None,
            delegation_available: false,
            delegated_model_factory: DeferredKeychainDelegatedModelHostFactory::new(),
            foreground_automation_pump: Default::default(),
            mcp_manager: std::sync::Arc::new(mcp_client::McpProcessManager::new()),
            file_watcher: file_watcher::FileWatcherManager::new(),
            cached_settings: settings_cache::new_cache(),
            steering: commands::steering::SteeringState::new(),
            skills: commands::skill::SkillState::new(),
            event_emitter: None,
            dev_event_store: Some(std::sync::Arc::new(DevEventStore::default())),
            desktop_adapter: desktop_control::create_mock_adapter(),
            desktop_action_previews: Default::default(),
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dev_http_refuses_native_only_operations_without_side_effects() {
        let state = test_app_state();
        {
            let conn = state.db.lock().unwrap();
            db::migrate(&conn).unwrap();
            conn.execute_batch("INSERT INTO automations
                (id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,executor_kind,
                 script_args,timeout_seconds,next_run_at,created_at,updated_at,workspace_id)
                VALUES ('native-reminder','Native reminder','Owned fixture','once',
                        '1970-01-01T00:00:01Z',1,'','notification','[]',300,1,1,1,'personal');")
                .unwrap();
        }
        let dashboard =
            handle_dev_command("get_automation_dashboard", &serde_json::json!({}), &state);
        assert_eq!(dashboard["ok"], true);
        assert_eq!(dashboard["data"]["items"][0]["id"], "native-reminder");
        assert!(dashboard["data"]["items"][0]["latestRun"].is_null());
        for (command, args) in [
            (
                "export_encrypted_data",
                serde_json::json!({ "password": "test" }),
            ),
            (
                "import_encrypted_data",
                serde_json::json!({ "data": "test", "password": "test" }),
            ),
            ("clear_all_user_data", serde_json::json!({})),
            (
                "confirm_restored_project_directories",
                serde_json::json!({"directories":[]}),
            ),
            ("run_due_automations", serde_json::json!({})),
            (
                "run_automation_now",
                serde_json::json!({"id":"native-reminder"}),
            ),
            (
                "import_data",
                serde_json::json!({ "data": "{}", "mode": "replace" }),
            ),
        ] {
            let result = handle_dev_command(command, &args, &state);
            assert_eq!(result["ok"], false, "{command}");
        }
        let conn = state.db.lock().unwrap();
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT count(*) FROM automation_runs", [], |row| row
                .get(0))
                .unwrap(),
            0
        );
        assert!(conn
            .query_row::<bool, _, _>(
                "SELECT enabled FROM automations WHERE id='native-reminder'",
                [],
                |row| row.get(0)
            )
            .unwrap());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dev_http_requires_local_host_and_json_post() {
        let host = tiny_http::Header::from_bytes("Host", "127.0.0.1:1421").unwrap();
        let content_type =
            tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap();
        assert!(dev_request_has_allowed_host(&[host.clone()]));
        assert!(dev_request_is_json_post(
            &tiny_http::Method::Post,
            &[content_type.clone()]
        ));
        assert!(!dev_request_is_json_post(
            &tiny_http::Method::Get,
            &[content_type]
        ));
        assert!(!dev_request_has_allowed_host(&[
            tiny_http::Header::from_bytes("Host", "attacker.example:1421").unwrap()
        ]));
        assert!(!dev_request_is_json_post(
            &tiny_http::Method::Post,
            &[tiny_http::Header::from_bytes("Content-Type", "text/plain").unwrap()]
        ));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dev_http_skill_commands_share_the_registered_skill_state() {
        let state = test_app_state();
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("SKILL.md"),
            "# Test Skill\nDescription: Loaded through Dev HTTP\n\nVersion: 1.0.0\n",
        )
        .unwrap();

        let loaded = handle_dev_command(
            "load_skills",
            &serde_json::json!({"directory": directory.path()}),
            &state,
        );
        assert_eq!(loaded["ok"], true);
        assert_eq!(loaded["data"], 1);

        let skills = handle_dev_command("get_skills", &serde_json::Value::Null, &state);
        assert_eq!(skills["ok"], true);
        assert_eq!(skills["data"][0]["name"], "Test Skill");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dev_http_learned_habits_share_the_owner_memory_store() {
        let state = test_app_state();
        {
            let conn = state.db.lock().unwrap();
            db::migrate(&conn).unwrap();
            agent::adaptive_constraints::record_preferences(
                &conn,
                Some("s1"),
                &[agent::evolution::NewMemory {
                    category: "preference".into(),
                    content: "喜欢安静简洁的问候".into(),
                    importance: 8,
                }],
                "auto_evolution",
            )
            .unwrap();
        }
        let listed =
            handle_dev_command("get_learned_preferences", &serde_json::Value::Null, &state);
        assert_eq!(listed["ok"], true);
        assert_eq!(listed["data"][0]["ready"], false);
        assert!(listed["data"][0]["updatedAt"].is_number());
        let missing =
            handle_dev_command("dismiss_learned_preference", &serde_json::json!({}), &state);
        assert_eq!(missing["ok"], false);
        let dismissed = handle_dev_command(
            "dismiss_learned_preference",
            &serde_json::json!({"id":listed["data"][0]["id"]}),
            &state,
        );
        assert_eq!(dismissed["ok"], true);
        assert_eq!(
            handle_dev_command("get_learned_preferences", &serde_json::Value::Null, &state)["data"],
            serde_json::json!([])
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dev_http_controls_share_the_active_run_and_reject_invalid_contracts() {
        let state = test_app_state();
        let (_permit, cancellation) = state
            .steering
            .try_start_running_run("owned-session")
            .unwrap();
        for command in ["pause_agent", "resume_agent"] {
            assert_eq!(
                handle_dev_command(
                    command,
                    &serde_json::json!({"sessionId":"owned-session"}),
                    &state
                )["ok"],
                true
            );
            assert!(!cancellation.is_cancelled());
        }
        for mode in ["steer", "follow_up"] {
            assert_eq!(
                handle_dev_command(
                    "submit_in_progress_command",
                    &serde_json::json!({"sessionId":"owned-session","content":"owned instruction","mode":mode}),
                    &state
                )["ok"],
                true
            );
        }
        assert_eq!(
            handle_dev_command(
                "interrupt_agent",
                &serde_json::json!({"session_id":"owned-session"}),
                &state
            )["ok"],
            false
        );
        assert!(!cancellation.is_cancelled());
        assert_eq!(
            handle_dev_command(
                "submit_in_progress_command",
                &serde_json::json!({"sessionId":"owned-session","content":"","mode":"invalid"}),
                &state
            )["ok"],
            false
        );
        assert!(!cancellation.is_cancelled());
        assert_eq!(
            handle_dev_command(
                "interrupt_agent",
                &serde_json::json!({"sessionId":"owned-session"}),
                &state
            )["ok"],
            true
        );
        assert!(cancellation.is_cancelled());
        drop(_permit);
        let (_next, next_signal) = state
            .steering
            .try_start_running_run("owned-session")
            .unwrap();
        assert!(!next_signal.is_cancelled());
        assert_eq!(
            handle_dev_command(
                "submit_in_progress_command",
                &serde_json::json!({"sessionId":"owned-session","content":"","mode":"abort"}),
                &state
            )["ok"],
            true
        );
        assert!(next_signal.is_cancelled());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dev_http_agent_configuration_uses_session_scope_and_permission() {
        let state = test_app_state();
        {
            let conn = state.db.lock().unwrap();
            db::migrate(&conn).unwrap();
            conn.execute(
                "INSERT INTO settings (key, value, updated_at) VALUES ('agent_execution_permission', 'workspace_auto', 1)",
                [],
            )
            .unwrap();
        }

        let work_dir = tempfile::tempdir().unwrap();
        let created = handle_dev_command(
            "create_session",
            &serde_json::json!({"title": "scope test", "work_dir": work_dir.path()}),
            &state,
        );
        assert_eq!(created["ok"], true);
        let session_id = created["data"]["id"].as_str().unwrap().to_string();

        let request = commands::foreground_message_contracts::SendMessageRequest {
            session_id,
            role: "user".to_string(),
            content: "读取参考文件并创建摘要".to_string(),
            text_attachments: Vec::new(),
            personality: None,
            preferences: None,
            thinking_effort: None,
            client_message_id: None,
            web_search_setup: None,
        };
        let config = commands::message::build_agent_config(&state, &request).unwrap();

        // Read operations are now unrestricted; additional_read_dirs no longer exist.
        assert!(config.work_dir.is_some());
        assert_eq!(
            commands::message::agent_execution_permission(&state).unwrap(),
            "ask"
        );
        let changed = handle_dev_command(
            "set_agent_execution_permission",
            &serde_json::json!({"permission": "full_access"}),
            &state,
        );
        assert_eq!(changed["ok"], true);
        assert_eq!(changed["data"], "full_access");
        let loaded = handle_dev_command(
            "get_agent_execution_permission",
            &serde_json::Value::Null,
            &state,
        );
        assert_eq!(loaded["ok"], true);
        assert_eq!(loaded["data"], "full_access");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dev_http_foreground_contracts_preserve_snapshots_and_continuation_shapes() {
        for wrapped in [false, true] {
            let state = test_app_state();
            let request_args = |request: serde_json::Value| {
                if wrapped {
                    serde_json::json!({"req": request})
                } else {
                    request
                }
            };
            {
                let conn = state.db.lock().unwrap();
                db::migrate(&conn).unwrap();
                conn.execute_batch(
                    "INSERT INTO sessions (id, title, created_at, updated_at)
                 VALUES ('session', 'Continuation test', 1, 1);
                 INSERT INTO messages (id, session_id, role, content, created_at)
                 VALUES ('user', 'session', 'user', 'Original task', 1),
                        ('assistant', 'session', 'assistant', 'Continue after review', 2);
                 UPDATE messages SET parent_id = 'user' WHERE id = 'assistant';
                 UPDATE sessions SET leaf_message_id = 'assistant' WHERE id = 'session';",
                )
                .unwrap();
                conn.execute(
                    "UPDATE messages SET metadata = ?1 WHERE id = 'user'",
                    [serde_json::json!({"textAttachments": [{"name": "arrangements.docx", "text": "DOCX body snapshot"}]}).to_string()],
                ).unwrap();
            }

            let history = handle_dev_command(
                "get_messages",
                &serde_json::json!({"sessionId": "session"}),
                &state,
            );
            assert_eq!(history["ok"], true);
            assert_eq!(
                history["data"][0]["textAttachments"][0]["name"],
                "arrangements.docx"
            );
            assert_eq!(
                history["data"][0]["textAttachments"][0]["text"],
                "DOCX body snapshot"
            );
            let invalid_docx = handle_dev_command(
                "extract_document_text",
                &request_args(serde_json::json!({"name": "../private.docx", "dataBase64": "eA=="})),
                &state,
            );
            assert_eq!(invalid_docx["ok"], false);
            assert!(invalid_docx["error"].as_str().unwrap().contains("名称无效"));
            assert!(invalid_docx.get("data").is_none());

            let occupied = state.steering.try_start_run("session").unwrap();
            for (command, args) in [
                (
                    "edit_and_resend_message",
                    request_args(
                        serde_json::json!({"sessionId": "session", "messageId": "user", "content": "replacement"}),
                    ),
                ),
                ("delete_message", serde_json::json!({"messageId": "user"})),
                (
                    "update_message_content",
                    serde_json::json!({"messageId": "user", "content": "changed"}),
                ),
            ] {
                let result = handle_dev_command(command, &args, &state);
                assert_eq!(result["ok"], false, "{command}");
                assert!(result["error"]
                    .as_str()
                    .unwrap()
                    .contains("already running"));
                assert_eq!(
                    handle_dev_command(
                        "get_messages",
                        &serde_json::json!({"sessionId": "session"}),
                        &state
                    ),
                    history
                );
            }
            drop(occupied);
            for command in ["delete_message", "update_message_content"] {
                assert_eq!(
                    handle_dev_command(
                        command,
                        &serde_json::json!({"messageId": "missing", "content": "changed"}),
                        &state
                    )["ok"],
                    false
                );
            }

            for request in [
                serde_json::Value::Null,
                serde_json::json!({}),
                serde_json::json!({"sessionId": "session"}),
                serde_json::json!({"sessionId": 1, "messageId": "assistant"}),
            ] {
                let args = request_args(request);
                let result = handle_dev_command("continue_agent_task", &args, &state);
                assert_eq!(result["ok"], false);
                assert!(result["error"]
                    .as_str()
                    .unwrap()
                    .starts_with("invalid continue_agent_task request:"));
                assert!(result.get("data").is_none());
            }

            let request = serde_json::json!({"sessionId": "session", "messageId": "assistant"});
            let args = request_args(request);
            let result = handle_dev_command("continue_agent_task", &args, &state);
            assert_eq!(
                result,
                serde_json::json!({"ok": false, "error": "Task run not found"})
            );

            {
                let conn = state.db.lock().unwrap();
                conn.execute_batch(
                    "INSERT INTO task_runs (id, session_id, message_id, goal, status, plan,
                    resumable, continuation_context, created_at, updated_at)
                 VALUES ('task', 'session', 'assistant', 'Original task', 'needs_attention',
                    '[]', 1, 'No operation has run.', 1, 2);
                 ALTER TABLE projects RENAME TO unavailable_projects;",
                )
                .unwrap();
            }
            // Fail local context preparation after foreground reservation, before any
            // model or credentials are accessed. Continuation must still not persist
            // its internal prompt as another user turn, even when that slice fails.
            let result = handle_dev_command("continue_agent_task", &args, &state);
            assert_eq!(
                result,
                serde_json::json!({"ok": false, "error": "no such table: projects"})
            );
            let conn = state.db.lock().unwrap();
            let user_turns: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE role = 'user'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(user_turns, 1);
            let incorrect_goals: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM task_runs WHERE goal != 'Original task'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(incorrect_goals, 0);
            let leaf: String = conn
                .query_row(
                    "SELECT leaf_message_id FROM sessions WHERE id = 'session'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_ne!(
                leaf, "assistant",
                "the request must reserve and recover a new slice"
            );
            drop(conn);
            assert_eq!(
                handle_dev_command(
                    "delete_message",
                    &serde_json::json!({"messageId": "user"}),
                    &state
                )["ok"],
                true
            );
            assert_eq!(
                handle_dev_command(
                    "get_messages",
                    &serde_json::json!({"sessionId": "session"}),
                    &state
                )["data"],
                serde_json::json!([])
            );
        }
    }
}

// ─── Dev HTTP Server ─────────────────────────────────────────────────────────

fn load_deployment_environment() {
    // Fixtures must not recover credentials/opt-ins from the developer's .env
    // after their launcher has deliberately sanitized the child environment.
    #[cfg(not(any(test, feature = "desktop-e2e")))]
    {
        let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let _ = dotenvy::from_filename(project_root.join(".env"));
    }
}

#[cfg(debug_assertions)]
fn start_dev_http_server() {
    use tiny_http::{Response, Server};

    let server = match Server::http("127.0.0.1:1421") {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[Dev HTTP] Failed to bind port 1421: {}", e);
            return;
        }
    };
    println!("[Dev HTTP] AngelBot IPC listening on http://127.0.0.1:1421");

    let app_data_dir = app_data_dir();
    std::fs::create_dir_all(&app_data_dir).ok();
    let db_path = app_data_dir.join("data.db");
    let conn = match db::init(&db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[Dev HTTP] DB init failed: {}", e);
            return;
        }
    };
    if let Err(e) = db::migrate(&conn) {
        eprintln!("[Dev HTTP] DB migrate failed: {}", e);
        return;
    }
    if let Ok(restored) = commands::settings::restore_enabled_scheduled_tasks(&conn) {
        if restored > 0 {
            eprintln!("[Dev HTTP] Restored {} scheduled task events", restored);
        }
    }
    // Load .env before resolving the explicit Explorer environment opt-in.
    load_deployment_environment();

    let db = std::sync::Arc::new(Mutex::new(conn));
    let cached_settings = settings_cache::new_cache();
    if let Ok(conn) = db.lock() {
        let _ = settings_cache::load_all(&conn, &cached_settings);
    }
    let state = std::sync::Arc::new(AppState {
        db,
        runtime_health: RuntimeHealthState::default(),
        // The debug HTTP gateway owns a compatibility-only state object.  The
        // Tauri AppState is the sole delegated host owner; keeping this None
        // prevents a second pump/loop over the same logical application.
        delegation_pump: None,
        delegation_available: false,
        delegated_model_factory: DeferredKeychainDelegatedModelHostFactory::new(),
        foreground_automation_pump: Default::default(),
        mcp_manager: std::sync::Arc::new(mcp_client::McpProcessManager::new()),
        file_watcher: file_watcher::FileWatcherManager::new(),
        cached_settings,
        steering: commands::steering::SteeringState::new(),
        skills: commands::skill::SkillState::new(),
        event_emitter: None,
        dev_event_store: Some(std::sync::Arc::new(DevEventStore::default())),
        desktop_adapter: desktop_control::create_dev_adapter(),
        desktop_action_previews: Default::default(),
    });

    for mut request in server.incoming_requests() {
        // The browser development UI uses a same-origin Vite proxy. Never
        // grant arbitrary websites cross-origin access to this local IPC shim.
        if !dev_request_has_allowed_host(request.headers()) {
            let resp = Response::from_string("Forbidden").with_status_code(403);
            let _ = request.respond(resp);
            continue;
        }

        let full_url = request.url().to_string();

        if full_url == "/" {
            let mut resp = Response::from_string("AngelBot Dev OK");
            set_json_headers(&mut resp);
            let _ = request.respond(resp);
            continue;
        }

        if !full_url.starts_with("/__invoke__") {
            let mut resp = Response::from_string("404 Not Found");
            set_json_headers(&mut resp);
            let _ = request.respond(resp);
            continue;
        }

        // GET and simple cross-site form POSTs must not be able to perform
        // commands; the Vite proxy always sends application/json POSTs.
        if !dev_request_is_json_post(request.method(), request.headers()) {
            let resp = Response::from_string("Method Not Allowed").with_status_code(405);
            let _ = request.respond(resp);
            continue;
        }
        let body = {
            let mut bytes = Vec::new();
            request.as_reader().read_to_end(&mut bytes).ok();
            String::from_utf8(bytes).unwrap_or_default()
        };
        let parsed: serde_json::Value =
            serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
        let cmd = parsed
            .get("cmd")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let args_val = parsed
            .get("args")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        let state = std::sync::Arc::clone(&state);
        std::thread::spawn(move || {
            let result = handle_dev_command(&cmd, &args_val, &state);
            let body = serde_json::to_string(&result)
                .unwrap_or_else(|_| r#"{"ok":false,"error":"serialization error"}"#.to_string());
            let mut resp = Response::from_string(&body);
            set_json_headers(&mut resp);
            let _ = request.respond(resp);
        });
    }
}

#[cfg(debug_assertions)]
fn dev_request_has_allowed_host(headers: &[tiny_http::Header]) -> bool {
    headers.iter().any(|header| {
        header
            .field
            .as_str()
            .to_string()
            .eq_ignore_ascii_case("host")
            && matches!(header.value.as_str(), "127.0.0.1:1421" | "localhost:1421")
    })
}

#[cfg(debug_assertions)]
fn dev_request_is_json_post(method: &tiny_http::Method, headers: &[tiny_http::Header]) -> bool {
    method == &tiny_http::Method::Post
        && headers.iter().any(|header| {
            header
                .field
                .as_str()
                .to_string()
                .eq_ignore_ascii_case("content-type")
                && header.value.as_str().starts_with("application/json")
        })
}

#[cfg(debug_assertions)]
fn set_json_headers(resp: &mut tiny_http::Response<std::io::Cursor<Vec<u8>>>) {
    use tiny_http::Header;
    let _ = resp
        .add_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
}

#[cfg(debug_assertions)]
fn handle_dev_command(cmd: &str, args: &serde_json::Value, state: &AppState) -> serde_json::Value {
    use rusqlite::params;

    match cmd {
        "interrupt_agent" | "submit_in_progress_command" | "pause_agent" | "resume_agent" => {
            // Match the typed frontend's camelCase contract and use the same
            // run-local signals as native IPC, including during model waits.
            let result = (|| -> Result<serde_json::Value, String> {
                let session_id = args
                    .get("sessionId")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or("invalid control session")?
                    .to_string();
                match cmd {
                    "interrupt_agent" => {
                        commands::steering::interrupt_agent_impl(&state.steering, session_id)
                            .and_then(|instruction| {
                                serde_json::to_value(instruction).map_err(|error| error.to_string())
                            })
                    }
                    "submit_in_progress_command" => {
                        let content = args
                            .get("content")
                            .and_then(serde_json::Value::as_str)
                            .ok_or("invalid control content")?
                            .to_string();
                        let mode = serde_json::from_value(
                            args.get("mode").cloned().ok_or("invalid control mode")?,
                        )
                        .map_err(|_| "invalid control mode")?;
                        commands::steering::submit_in_progress_command_impl(
                            &state.steering,
                            session_id,
                            content,
                            mode,
                        )?;
                        Ok(serde_json::Value::Null)
                    }
                    _ => {
                        let control = if cmd == "pause_agent" {
                            agent::steering::AgentControlState::Paused
                        } else {
                            agent::steering::AgentControlState::Running
                        };
                        state.steering.set_control_state(session_id, control)?;
                        Ok(serde_json::Value::Null)
                    }
                }
            })();
            match result {
                Ok(data) => serde_json::json!({"ok":true,"data":data}),
                Err(error) => serde_json::json!({"ok":false,"error":error}),
            }
        }
        "get_workspaces" => commands::workspace::get_workspaces_impl(state)
            .map(|v| serde_json::json!({"ok":true,"data":v}))
            .unwrap_or_else(|e| serde_json::json!({"ok":false,"error":e})),
        "open_workspace" => commands::workspace::open_workspace_impl(
            state,
            args.get("id").and_then(|v| v.as_str()).unwrap_or_default(),
        )
        .map(|v| serde_json::json!({"ok":true,"data":v}))
        .unwrap_or_else(|e| serde_json::json!({"ok":false,"error":e})),
        "create_project_workspace" => commands::workspace::create_project_workspace_impl(
            state,
            args.get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            args.get("name").and_then(|v| v.as_str()).map(String::from),
            args.get("agent_provider")
                .and_then(|v| v.as_str())
                .map(String::from),
            args.get("agent_model")
                .and_then(|v| v.as_str())
                .map(String::from),
        )
        .map(|v| serde_json::json!({"ok":true,"data":v}))
        .unwrap_or_else(|e| serde_json::json!({"ok":false,"error":e})),
        "confirm_restored_project_directories" => {
            serde_json::json!({"ok":false,"error":"恢复目录的确认需要在 AngelBot 桌面窗口中进行。"})
        }
        "create_session" => {
            let title = args
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let work_dir = args
                .get("work_dir")
                .and_then(|v| v.as_str())
                .map(String::from);
            let agent_provider = args
                .get("agent_provider")
                .and_then(|v| v.as_str())
                .map(String::from);
            let agent_model = args
                .get("agent_model")
                .and_then(|v| v.as_str())
                .map(String::from);
            commands::session::create_session_impl(
                state,
                title,
                work_dir,
                agent_provider,
                agent_model,
            )
            .map(|session| serde_json::json!({"ok": true, "data": session}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "get_sessions" => commands::session::get_sessions_impl(state)
            .map(|sessions| serde_json::json!({"ok": true, "data": sessions}))
            .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error})),
        "get_context_usage" => {
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            commands::session::get_context_usage_impl(state, session_id)
                .map(|usage| serde_json::json!({"ok": true, "data": usage}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "get_session_usage" => {
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let tracker = crate::llm::UsageTracker::new(state.db.clone());
            tracker
                .get_session_usage(session_id)
                .map(|usage| serde_json::json!({"ok": true, "data": usage}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e.to_string()}))
        }
        "update_session_title" => {
            let id = args
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let title = args
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            commands::session::update_session_title_impl(state, id, title)
                .map(|_| serde_json::json!({"ok": true}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "delete_session" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            commands::session::delete_session_impl(state, id)
                .map(|_| serde_json::json!({"ok": true}))
                .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
        }
        "update_session_model" => {
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let agent_provider = args
                .get("agent_provider")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let agent_model = args
                .get("agent_model")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            commands::session::update_session_model_impl(
                state,
                session_id,
                agent_provider,
                agent_model,
            )
            .map(|_| serde_json::json!({"ok": true}))
            .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
        }
        "send_message" | "continue_agent_task" | "edit_and_resend_message" => {
            let args = args.get("req").unwrap_or(args);
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    return serde_json::json!({
                        "ok": false,
                        "error": format!("failed to start foreground runtime: {error}")
                    });
                }
            };
            let result = runtime.block_on(async {
                if cmd == "send_message" {
                    let request: commands::foreground_message_contracts::SendMessageRequest =
                        serde_json::from_value(args.clone())
                            .map_err(|error| format!("invalid {cmd} request: {error}"))?;
                    commands::message::send_message_from_dev_http(state, request).await
                } else if cmd == "edit_and_resend_message" {
                    let request: commands::message::EditAndResendMessageRequest =
                        serde_json::from_value(args.clone())
                            .map_err(|error| format!("invalid {cmd} request: {error}"))?;
                    commands::message::edit_and_resend_message_impl(state, None, request).await
                } else {
                    let request: commands::message::ContinueAgentTaskRequest =
                        serde_json::from_value(args.clone())
                            .map_err(|error| format!("invalid {cmd} request: {error}"))?;
                    commands::message::continue_agent_task_impl(state, None, request).await
                }
            });
            match result {
                Ok(message) => serde_json::json!({"ok": true, "data": message}),
                Err(error) => serde_json::json!({"ok": false, "error": error}),
            }
        }
        "extract_document_text" => {
            let result = serde_json::from_value::<
                commands::foreground_text_attachments::ExtractDocumentRequest,
            >(args.get("req").unwrap_or(args).clone())
            .map_err(|_| "文档读取请求无效。".to_string())
            .and_then(commands::foreground_text_attachments::extract_document_text_impl);
            match result {
                Ok(snapshot) => serde_json::json!({"ok": true, "data": snapshot}),
                Err(error) => serde_json::json!({"ok": false, "error": error}),
            }
        }
        "get_messages" => {
            let session_id = args
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            match commands::foreground_run_store::ForegroundRunStore::new(state.db.clone())
                .project_session(session_id)
            {
                Ok(messages) => serde_json::json!({"ok": true, "data": messages}),
                Err(error) => serde_json::json!({"ok": false, "error": error}),
            }
        }
        "delete_message" => {
            let message_id = args
                .get("messageId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            commands::message::delete_message_impl(state, None, message_id)
                .map(|_| serde_json::json!({"ok": true}))
                .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
        }
        "cancel_agent_task" => {
            let req = args.get("req").unwrap_or(args);
            let session_id = req
                .get("sessionId")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let message_id = req
                .get("messageId")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            commands::message::cancel_agent_task_impl(state, session_id, message_id)
                .map(|_| serde_json::json!({"ok": true}))
                .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
        }
        "update_message_content" => {
            let message_id = args
                .get("messageId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            commands::message::update_message_content_impl(state, None, message_id, content)
                .map(|_| serde_json::json!({"ok": true}))
                .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
        }
        "resolve_agent_confirmation" => {
            // Production Tauri IPC passes the typed request as `{ req: ... }`.
            // Accept that shape here as well, while retaining compatibility
            // with existing direct dev-HTTP callers.
            let args = args.get("req").unwrap_or(&args);
            let req = commands::message::ResolveAgentConfirmationRequest {
                session_id: args
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                message_id: args
                    .get("messageId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                call_id: args
                    .get("callId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                decision: args
                    .get("decision")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                preview_id: args
                    .get("previewId")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            };
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    return serde_json::json!({"ok": false, "error": format!("failed to start foreground runtime: {error}")})
                }
            };
            runtime
                .block_on(commands::message::resolve_agent_confirmation_and_continue(
                    state, None, req,
                ))
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "preflight_pending_desktop_action" => {
            let args = args.get("req").unwrap_or(&args);
            let session_id = args
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let message_id = args
                .get("messageId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let call_id = args
                .get("callId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            commands::message::preflight_pending_desktop_action_impl(
                state, session_id, message_id, call_id,
            )
            .map(|v| serde_json::json!({"ok": true, "data": v}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "get_env_config" => crate::commands::get_env_config()
            .map(|v| serde_json::json!({"ok": true, "data": v}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e})),
        "check_ollama" => crate::commands::check_ollama()
            .map(|v| serde_json::json!({"ok": true, "data": v}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e})),
        "test_api_connection" => {
            let url = args
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let api_key = args
                .get("apiKey")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
            let provider = args
                .get("provider")
                .and_then(|v| v.as_str())
                .map(String::from);
            crate::commands::mcp::test_api_connection_core(url, api_key, provider)
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "match_personality_direction" => {
            let description = args
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            crate::commands::match_personality_direction(description)
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "export_data" => {
            let scope = args.get("scope").and_then(|v| v.as_str()).map(String::from);
            let opts = commands::settings::ExportOptions { scope };
            commands::settings::export_data_impl(state, Some(opts))
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "export_encrypted_data" => {
            // This development gateway has no desktop keychain authority.
            // A caller-chosen password is not authorization to export secrets.
            serde_json::json!({"ok": false, "error": "加密备份仅可在桌面应用中操作。"})
        }
        "import_data" => {
            let data = args
                .get("data")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let mode = args.get("mode").and_then(|v| v.as_str()).map(String::from);
            if mode.as_deref() == Some("replace") {
                return serde_json::json!({"ok": false, "error": "替换导入仅可在桌面应用中操作。"});
            }
            let opts = commands::settings::ImportOptions { mode };
            commands::settings::import_data_impl(state, data, Some(opts))
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "import_encrypted_data" => {
            serde_json::json!({"ok": false, "error": "加密备份仅可在桌面应用中操作。"})
        }
        "clear_all_user_data" => {
            serde_json::json!({"ok": false, "error": "清空数据仅可在桌面应用中操作。"})
        }
        "read_file" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let work_dir = crate::commands::file::resolve_work_dir(state, None)
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            crate::commands::file::read_file_core(&work_dir, &path)
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "search_messages" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let session_id = args
                .get("sessionId")
                .and_then(|v| v.as_str())
                .map(String::from);
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            let conn = match state.db.lock() {
                Ok(g) => g,
                Err(poisoned) => {
                    eprintln!(
                        "[DEBUG-POISON] {}: db lock was poisoned; recovering inner guard",
                        cmd
                    );
                    poisoned.into_inner()
                }
            };
            crate::fts::search_messages(&conn, &query, session_id.as_deref(), limit)
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "search_memories_by_keyword" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            let conn = match state.db.lock() {
                Ok(g) => g,
                Err(poisoned) => {
                    eprintln!(
                        "[DEBUG-POISON] {}: db lock was poisoned; recovering inner guard",
                        cmd
                    );
                    poisoned.into_inner()
                }
            };
            crate::fts::search_memories_keyword(&conn, &query, limit)
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "rebuild_fts_index" => {
            let conn = match state.db.lock() {
                Ok(g) => g,
                Err(poisoned) => {
                    eprintln!(
                        "[DEBUG-POISON] {}: db lock was poisoned; recovering inner guard",
                        cmd
                    );
                    poisoned.into_inner()
                }
            };
            crate::fts::rebuild_fts_index(&conn)
                .map(|_| serde_json::json!({"ok": true}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "get_work_dir" => {
            let session_id = args
                .get("sessionId")
                .or_else(|| args.get("session_id"))
                .and_then(|v| v.as_str())
                .map(String::from);
            commands::file::resolve_work_dir(state, session_id.as_deref())
                .map(|v| serde_json::json!({"ok": true, "data": commands::file::display_work_dir(&v)}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "get_agent_execution_permission" => commands::message::agent_execution_permission(state)
            .map(|v| serde_json::json!({"ok": true, "data": v}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e})),
        "set_agent_execution_permission" => {
            let permission = args
                .get("permission")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let expires_at = args
                .get("expiresAt")
                .or_else(|| args.get("expires_at"))
                .and_then(|v| v.as_i64());
            let permanent = args
                .get("permanent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            commands::message::set_agent_execution_permission_impl_with_expiry(
                state, permission, expires_at, permanent,
            )
            .map(|v| serde_json::json!({"ok": true, "data": v}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "read_session_file" => {
            let session_id = args
                .get("sessionId")
                .or_else(|| args.get("session_id"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            crate::commands::file::resolve_work_dir(state, Some(session_id))
                .and_then(|work_dir| crate::commands::file::read_file_core(&work_dir, &path))
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "save_session_file" => {
            // The Dev HTTP gateway has no parent native window/picker. Never
            // accept a destination path or pretend a browser download saved it.
            serde_json::json!({"ok": false, "error": "文件另存需要在 AngelBot 桌面窗口中选择位置。"})
        }
        "preview_browser_cleanup" => crate::browser_storage::cleanup::preview(state)
            .map(|data| serde_json::json!({"ok": true, "data": data}))
            .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error})),
        "clear_browser_temp_files" => serde_json::from_value(
            args.get("request")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )
        .map_err(|_| "清理请求格式无效。".to_owned())
        .and_then(|request| crate::browser_storage::cleanup::execute(state, request))
        .map(|data| serde_json::json!({"ok": true, "data": data}))
        .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error})),
        "prepare_browser_mcp" => {
            let session_id = args
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            crate::commands::mcp::prepare_browser_mcp_impl(state, session_id)
                .map(|data| serde_json::json!({"ok": true, "data": data}))
                .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
        }
        "list_work_dir" => {
            let session_id = args
                .get("sessionId")
                .or_else(|| args.get("session_id"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            crate::commands::file::list_work_dir_impl(state, session_id, &path)
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "write_session_file" => {
            let session_id = args
                .get("sessionId")
                .or_else(|| args.get("session_id"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            crate::commands::file::resolve_work_dir(state, Some(session_id))
                .and_then(|work_dir| {
                    crate::commands::file::write_file_core(&work_dir, path, content)
                })
                .map(|_| serde_json::json!({"ok": true, "data": null}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "load_api_config" => {
            let config =
                crate::commands::api_config::load_api_config_internal().unwrap_or_else(|| {
                    crate::llm::ProviderConfig {
                        protocol: None,
                        auth_mode: crate::llm::ModelAuthMode::ApiKey,
                        credential_ref: None,
                        provider: "anthropic".to_string(),
                        model: "claude-sonnet-4-20250514".to_string(),
                        base_url: "https://api.anthropic.com".to_string(),
                        api_key: String::new(),
                        max_tokens: 4096,
                        temperature: 0.7,
                    }
                });
            serde_json::json!({"ok": true, "data": {
                "provider": config.provider,
                "model": config.model,
                "base_url": config.base_url,
                "api_key": "",
                "max_tokens": config.max_tokens,
                "temperature": config.temperature,
                "has_api_key": false,
                "credential_source": "none",
            }})
        }
        "get_learned_preferences" => commands::memory::get_learned_preferences_impl(state)
            .map(|v| serde_json::json!({"ok": true, "data": v}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e})),
        "dismiss_learned_preference" => commands::memory::dismiss_learned_preference_impl(
            state,
            args.get("id").and_then(|v| v.as_str()).unwrap_or_default(),
        )
        .map(|_| serde_json::json!({"ok": true, "data": null}))
        .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e})),
        "get_memories" => commands::memory::get_memories_impl(state)
            .map(|v| serde_json::json!({"ok": true, "data": v}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e})),
        "get_memory_stats" => commands::memory::get_memory_stats_impl(state)
            .map(|v| serde_json::json!({"ok": true, "data": v}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e})),
        "get_activity" => {
            let limit = args.get("limit").and_then(|value| value.as_i64());
            state
                .db
                .lock()
                .map_err(|error| error.to_string())
                .and_then(|conn| commands::activity::get_activity_impl(&conn, limit))
                .map(|entries| serde_json::json!({"ok": true, "data": entries}))
                .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
        }
        "get_workspace_activity_projection" => {
            let workspace_id = args
                .get("workspaceId")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            state
                .db
                .lock()
                .map_err(|error| error.to_string())
                .and_then(|conn| {
                    commands::workspace_activity::get_workspace_activity_projection_impl(
                        &conn,
                        workspace_id,
                    )
                })
                .map(|projection| serde_json::json!({"ok": true, "data": projection}))
                .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
        }
        "get_project_network_approvals" => {
            let workspace_id = args
                .get("workspaceId")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            commands::project_network_approval::get_project_network_approvals_impl(
                state,
                workspace_id,
            )
            .map(|approvals| serde_json::json!({"ok": true, "data": approvals}))
            .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
        }
        "revoke_project_network_approval" => {
            let workspace_id = args
                .get("workspaceId")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let approval_ref = args
                .get("approvalRef")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            commands::project_network_approval::revoke_project_network_approval_impl(
                state,
                workspace_id,
                approval_ref,
            )
            .map(|result| serde_json::json!({"ok": true, "data": result}))
            .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
        }
        "get_runtime_health" => serde_json::json!({
            "ok": true,
            "data": commands::runtime_health::runtime_health(state)
        }),
        "get_skills" => state
            .skills
            .registry
            .lock()
            .map(|registry| {
                registry
                    .all()
                    .iter()
                    .map(|skill| (*skill).clone())
                    .collect::<Vec<_>>()
            })
            .map(|skills| serde_json::json!({"ok": true, "data": skills}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e.to_string()})),
        "load_skills" => {
            let directory = args
                .get("directory")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            state
                .skills
                .registry
                .lock()
                .map_err(|e| e.to_string())
                .and_then(|mut registry| {
                    // Match the desktop IPC command: imports must be copied
                    // into AngelBot's managed store so a dev-server install
                    // survives application restart as well.
                    registry.import_directory(std::path::Path::new(directory))
                })
                .map(|count| serde_json::json!({"ok": true, "data": count}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "import_github_skills" => {
            let repository_url = args
                .get("repositoryUrl")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            state
                .skills
                .registry
                .lock()
                .map_err(|e| e.to_string())
                .and_then(|mut registry| registry.import_github_repository(repository_url))
                .map(|count| serde_json::json!({"ok": true, "data": count}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "set_work_dir" => {
            let session_id = args
                .get("sessionId")
                .or_else(|| args.get("session_id"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let work_dir = args
                .get("workDir")
                .or_else(|| args.get("work_dir"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            commands::file::set_work_dir_impl(state, session_id, work_dir)
                .map(|_| serde_json::json!({"ok": true}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "get_session_file_access" => {
            let session_id = args
                .get("sessionId")
                .or_else(|| args.get("session_id"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            commands::file::get_session_file_access_impl(state, session_id)
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "set_session_read_dirs" => {
            let session_id = args
                .get("sessionId")
                .or_else(|| args.get("session_id"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let directories = args
                .get("directories")
                .and_then(|v| v.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            commands::file::set_session_read_dirs_impl(state, session_id, &directories)
                .map(|_| serde_json::json!({"ok": true}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "get_desktop_trusted_apps" => commands::desktop::get_desktop_trusted_apps_impl(state)
            .map(|value| serde_json::json!({"ok": true, "data": value}))
            .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error})),
        "get_desktop_trusted_app_statuses" => {
            commands::desktop::get_desktop_trusted_app_statuses_impl(state)
                .map(|value| serde_json::json!({"ok": true, "data": value}))
                .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
        }
        "inspect_desktop_draft_targets" => {
            if std::env::var("ANGELBOT_DEV_DESKTOP_CONTROL").as_deref() != Ok("1") {
                serde_json::json!({"ok": false, "error": "Draft target inspection is available only in the desktop app"})
            } else {
                let app_id = args
                    .get("appId")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                commands::desktop::inspect_desktop_draft_targets_from_db(
                    &state.db,
                    state.desktop_adapter.as_ref(),
                    app_id,
                )
                .map(|value| serde_json::json!({"ok": true, "data": value}))
                .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
            }
        }
        "save_desktop_trusted_app" => {
            let app = args.get("app").unwrap_or(args);
            match serde_json::from_value::<commands::desktop::TrustedDesktopApp>(app.clone()) {
                Ok(app) => commands::desktop::save_desktop_trusted_app_impl(state, app)
                    .map(|value| serde_json::json!({"ok": true, "data": value}))
                    .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error})),
                Err(error) => {
                    serde_json::json!({"ok": false, "error": format!("Invalid trusted application: {error}")})
                }
            }
        }
        "confirm_desktop_observation_scope" => {
            match serde_json::from_value::<Vec<commands::desktop::DesktopObservationScopeApp>>(
                args.get("apps").cloned().unwrap_or(serde_json::Value::Null),
            ) {
                Ok(apps) => {
                    commands::desktop::confirm_desktop_observation_scope_from_db(&state.db, &apps)
                        .map(|value| serde_json::json!({"ok": true, "data": value}))
                        .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
                }
                Err(error) => {
                    serde_json::json!({"ok": false, "error": format!("Invalid application scope: {error}")})
                }
            }
        }
        "delete_desktop_trusted_app" => {
            let id = args
                .get("id")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            commands::desktop::delete_desktop_trusted_app_impl(state, id)
                .map(|_| serde_json::json!({"ok": true}))
                .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
        }
        "get_automation_dashboard" => commands::automation::get_automation_dashboard_impl(state)
            .map(|v| serde_json::json!({"ok": true, "data": v}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e})),
        "get_automations" => commands::automation::get_automations_impl(state)
            .map(|v| serde_json::json!({"ok": true, "data": v}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e})),
        "get_workspace_automation_overview" => {
            let workspace_id = args
                .get("workspaceId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            commands::automation::get_workspace_automation_overview_impl(state, workspace_id)
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "set_automation_enabled" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let enabled = args
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            commands::automation::set_automation_enabled_impl(state, id, enabled)
                .map(|_| serde_json::json!({"ok": true, "data": null}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "create_maa_automation" => {
            let timeout_seconds = match args.get("timeoutSeconds") {
                None | Some(serde_json::Value::Null) => Ok(None),
                Some(value) => value
                    .as_u64()
                    .map(Some)
                    .ok_or_else(|| "MAA 超时时间必须是正整数秒".to_string()),
            };
            timeout_seconds
                .and_then(|timeout_seconds| {
                    commands::automation::create_maa_automation_impl(
                        state,
                        args.get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        args.get("triggerValue")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        args.get("workspaceId")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        args.get("appId")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        args.get("launchEmulator")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        timeout_seconds,
                    )
                })
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "create_automation" => {
            let title = args
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let prompt = args
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let trigger_kind = args
                .get("triggerKind")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let trigger_value = args
                .get("triggerValue")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let permission_summary = args
                .get("permissionSummary")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let executor_kind = args
                .get("executorKind")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let workspace_id = args
                .get("workspaceId")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let script_path = args
                .get("scriptPath")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let script_args = args
                .get("scriptArgs")
                .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok());
            let working_dir = args
                .get("workingDir")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let timeout_seconds = args.get("timeoutSeconds").and_then(|v| v.as_u64());
            commands::automation::create_automation_impl(
                state,
                title,
                prompt,
                trigger_kind,
                trigger_value,
                permission_summary,
                executor_kind,
                workspace_id,
                script_path,
                script_args,
                working_dir,
                timeout_seconds,
            )
            .map(|v| serde_json::json!({"ok": true, "data": v}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "delete_automation" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            commands::automation::delete_automation_impl(state, id)
                .map(|_| serde_json::json!({"ok": true, "data": null}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "stop_automation_run" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let run_id = args
                .get("runId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            commands::automation::stop_automation_run_impl(state, id, run_id)
                .map(|_| serde_json::json!({"ok": true, "data": null}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "run_due_automations" | "run_automation_now" => {
            // Dev HTTP has no AppHandle/native notification host. Never consume
            // an occurrence through the test-only, non-delivering core seam.
            serde_json::json!({"ok": false, "error": "自动化执行仅可在桌面应用中操作；开发 HTTP 不会消耗日程。"})
        }
        "get_automation_runs" => {
            let id = args
                .get("automationId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            commands::automation::get_automation_runs_impl(state, id)
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "get_channel_connections" => commands::channel::get_channel_connections_impl(state)
            .map(|v| serde_json::json!({"ok": true, "data": v}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e})),
        "create_channel_connection" => {
            let channel_type = args
                .get("channelType")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let display_name = args
                .get("displayName")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let permission_summary = args
                .get("permissionSummary")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            commands::channel::create_channel_connection_impl(
                state,
                channel_type,
                display_name,
                permission_summary,
            )
            .map(|v| serde_json::json!({"ok": true, "data": v}))
            .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "set_channel_enabled" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let enabled = args
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            commands::channel::set_channel_enabled_impl(state, id, enabled)
                .map(|_| serde_json::json!({"ok": true, "data": null}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "revoke_channel_connection" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            commands::channel::revoke_channel_connection_impl(state, id)
                .map(|_| serde_json::json!({"ok": true, "data": null}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "get_channel_audit" => {
            let id = args
                .get("connectionId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            commands::channel::get_channel_audit_impl(state, id)
                .map(|v| serde_json::json!({"ok": true, "data": v}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "set_channel_proactive_policy" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let reason = args
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let daily_limit = args.get("dailyLimit").and_then(|v| v.as_i64()).unwrap_or(0);
            commands::channel::set_channel_proactive_policy_impl(state, id, reason, daily_limit)
                .map(|_| serde_json::json!({"ok": true}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "confirm_channel_message" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let message = args
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let confirmed = args
                .get("confirmed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            commands::channel::confirm_channel_message_impl(state, id, message, confirmed)
                .map(|_| serde_json::json!({"ok": true, "data": null}))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}))
        }
        "save_channel_credential" | "clear_channel_credential" => {
            serde_json::json!({"ok": false, "error": "Channel credentials require the desktop OS keychain and are unavailable in Dev HTTP"})
        }
        "get_agent_events" => {
            let channel_id = args
                .get("channelId")
                .or_else(|| args.get("channel_id"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let session_id = channel_id.strip_prefix("session-").unwrap_or(channel_id);
            let events = state
                .dev_event_store
                .as_ref()
                .map(|store| store.events_for(session_id))
                .unwrap_or_default();
            serde_json::json!({"ok": true, "data": {"events": events, "count": events.len()}})
        }
        "get_agent_run_events" => {
            let session_id = args
                .get("sessionId")
                .or_else(|| args.get("session_id"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let run_id = args
                .get("runId")
                .or_else(|| args.get("run_id"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let after_sequence = args
                .get("afterSequence")
                .or_else(|| args.get("after_sequence"))
                .and_then(|value| value.as_i64());
            match commands::agent_event::get_agent_run_events_impl(
                state,
                session_id,
                run_id,
                after_sequence,
            ) {
                Ok(events) => serde_json::json!({"ok": true, "data": events}),
                Err(error) => serde_json::json!({"ok": false, "error": error}),
            }
        }
        _ => {
            let err = format!("Unknown command: {}", cmd);
            serde_json::json!({"ok": false, "error": err})
        }
    }
}

/// Hidden native helper dispatch. Must remain ahead of app state initialization.
pub fn window_capture_helper_exit_code() -> Option<i32> {
    window_capture::helper_exit_code()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Some(code) = window_capture_helper_exit_code() {
        std::process::exit(code);
    }
    // Development builds keep the detailed panic hook used to diagnose a
    // poisoned database lock. Release builds retain Rust's concise default
    // hook so local paths and full backtraces are not printed in production.
    #[cfg(debug_assertions)]
    std::panic::set_hook(Box::new(|info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-string panic payload");
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()));
        eprintln!("[DEBUG-PANIC] payload={} location={:?}", payload, location);
        eprintln!(
            "[DEBUG-PANIC] backtrace:\n{}",
            std::backtrace::Backtrace::force_capture()
        );
    }));

    // Register sqlite-vec before validating a captured migration database.
    unsafe {
        crate::vector_store::load_extension_globally();
    }
    #[cfg(feature = "desktop-e2e")]
    let storage = std::env::var_os(DESKTOP_E2E_DATA_DIR_ENV)
        .ok_or_else(|| {
            std::io::Error::other("Desktop acceptance requires an explicit isolated data root")
        })
        .and_then(|_| profile_storage::Lease::isolated(app_data_dir()));
    #[cfg(not(feature = "desktop-e2e"))]
    let storage = profile_storage::Paths::system().and_then(|paths| {
        let args: Vec<_> = std::env::args().collect();
        profile_storage::open(
            &paths,
            args.iter().any(|arg| arg == "--migrate-storage"),
            args.iter().any(|arg| arg == "--rollback-storage"),
        )
    });
    // Both old and new instance handles stay held for the desktop lifetime.
    let storage = match storage {
        Ok(storage) => storage,
        Err(error) => {
            runtime_instance::notify_storage_failure(&error);
            if std::env::args().any(|arg| arg == "--migrate-storage" || arg == "--rollback-storage")
            {
                std::process::exit(2);
            }
            return;
        }
    };
    #[cfg(not(feature = "desktop-e2e"))]
    if std::env::args().any(|arg| arg == "--rollback-storage") {
        eprintln!("[Storage] 迁移已回滚，旧资料已恢复；下次启动使用恢复目录。仅在明确选择后使用 --migrate-storage 重试。");
        runtime_instance::notify_storage_rollback();
        return;
    }
    let app_data_dir = storage.root.clone();
    let mut runtime_health = RuntimeHealthState::default();
    if storage.migrated || storage.legacy_recovery {
        runtime_health.add(RuntimeIssue::storage_migration(storage.legacy_recovery));
    }

    let db_path = app_data_dir.join("data.db");
    let primary = db::init(&db_path).and_then(|conn| {
        db::migrate(&conn)?;
        Ok(conn)
    });
    let mut conn = match primary {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("[Startup] primary database unavailable: {error}");
            runtime_health.add(RuntimeIssue::ephemeral_storage());
            let fallback = Connection::open_in_memory()
                .map_err(|fallback| fallback.to_string())
                .and_then(|conn| {
                    db::migrate(&conn)?;
                    Ok(conn)
                });
            match fallback {
                Ok(conn) => conn,
                Err(fallback) => {
                    eprintln!("[Startup] safe-mode database unavailable: {fallback}");
                    return;
                }
            }
        }
    };

    // A foreground run has no process-independent executor.  If the desktop
    // process exited while one was reserved, promote its hidden provisional
    // reply before the UI loads. Older already-visible replies keep their
    // content and become attention-only, never automatic replays. Delegated
    // attempts recover separately through the single delegation pump below.
    if let Err(error) = commands::foreground_run_store::recover_interrupted_runs_in_conn(
        &mut conn,
        chrono::Utc::now().timestamp(),
    ) {
        eprintln!("[Startup] foreground recovery failed: {error}");
    }
    if let Err(error) =
        commands::automation::recover_interrupted_agent_automation_dispatches_in_conn(
            &mut conn,
            chrono::Utc::now().timestamp(),
        )
    {
        eprintln!("[Startup] automation foreground recovery failed: {error}");
    }
    if let Err(error) =
        commands::delegated_delivery_follow_up::recover_interrupted_reviewed_delivery_follow_ups_in_conn(
            &mut conn,
            chrono::Utc::now().timestamp(),
        )
    {
        eprintln!("[Startup] reviewed delivery foreground recovery failed: {error}");
    }

    // Initialize vector store tables
    if let Err(error) = crate::vector_store::init_vector_table(&conn) {
        eprintln!("[Startup] vector store unavailable: {error}");
        runtime_health.add(RuntimeIssue::semantic_memory_unavailable());
    }

    // Load settings cache at startup
    let cached_settings = settings_cache::new_cache();
    if let Err(error) = settings_cache::load_all(&conn, &cached_settings) {
        eprintln!("[Startup] settings cache unavailable: {error}");
        runtime_health.add(RuntimeIssue::settings_defaults());
    }
    if let Ok(restored) = commands::settings::restore_enabled_scheduled_tasks(&conn) {
        if restored > 0 {
            eprintln!("[Startup] Restored {} scheduled task events", restored);
        }
    }

    // Resolve deployment opt-in before either the debug compatibility thread
    // or the delegated host is started.  This avoids a race where `.env` would
    // be loaded by the debug thread after the production host checked it.
    load_deployment_environment();

    // Start dev HTTP server in debug mode
    #[cfg(debug_assertions)]
    {
        if !manual_desktop_acceptance() {
            std::thread::spawn(start_dev_http_server);
        }
    }

    let db = std::sync::Arc::new(Mutex::new(conn));
    let delegated_model_factory = DeferredKeychainDelegatedModelHostFactory::new();
    let delegation_startup =
        build_delegation_pump(&db, &cached_settings, delegated_model_factory.clone());
    if let Some(issue) = delegation_startup.issue {
        runtime_health.add(issue);
    }
    let delegation_pump = delegation_startup.pump;
    let delegation_available = delegation_startup.enabled;

    let app_builder = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .plugin(
            tauri_plugin_keyring_store::Builder::new()
                .service("com.angelbot.desktop")
                .build(),
        )
        .manage(AppState {
            db,
            runtime_health,
            delegation_pump,
            delegation_available,
            delegated_model_factory,
            foreground_automation_pump: Default::default(),
            mcp_manager: std::sync::Arc::new(mcp_client::McpProcessManager::new()),
            file_watcher: file_watcher::FileWatcherManager::new(),
            cached_settings,
            steering: commands::steering::SteeringState::new(),
            skills: commands::skill::SkillState::new(),
            event_emitter: None,
            dev_event_store: Some(std::sync::Arc::new(DevEventStore::default())),
            desktop_adapter: desktop_control::create_runtime_adapter(),
            desktop_action_previews: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![
            commands::session::create_session,
            commands::session::get_sessions,
            commands::session::delete_session,
            commands::session::update_session_model,
            commands::session::update_session_title,
            commands::session::get_context_usage,
            commands::session::get_history_sessions,
            commands::message::send_message,
            commands::message::get_messages,
            commands::reply_export::save_message_reply,
            commands::reply_export::save_session_file,
            commands::foreground_text_attachments::extract_document_text,
            commands::message::resolve_agent_confirmation,
            commands::message::preflight_pending_desktop_action,
            commands::message::continue_agent_task,
            commands::message::cancel_agent_task,
            commands::message::get_agent_execution_permission,
            commands::message::set_agent_execution_permission,
            commands::message::get_agent_execution_permission_expiry,
            commands::message::delete_message,
            commands::message::update_message_content,
            commands::message::edit_and_resend_message,
            commands::mcp::get_mcp_servers,
            commands::mcp::save_mcp_server,
            commands::mcp::prepare_browser_mcp,
            commands::mcp::preview_browser_cleanup,
            commands::mcp::clear_browser_temp_files,
            commands::mcp::delete_mcp_server,
            commands::mcp::set_mcp_env_var,
            commands::mcp::remove_mcp_env_var,
            commands::mcp::clear_mcp_env,
            commands::mcp::get_env_config,
            commands::mcp::check_ollama,
            commands::mcp::test_api_connection,
            commands::mcp::start_mcp_server,
            commands::mcp::stop_mcp_server,
            commands::mcp::get_mcp_server_status,
            commands::mcp::list_mcp_tools,
            commands::mcp::refresh_mcp_tools,
            commands::memory::get_memories,
            commands::memory::get_learned_preferences,
            commands::memory::dismiss_learned_preference,
            commands::memory::save_memory,
            commands::memory::update_memory,
            commands::memory::delete_memory,
            commands::memory::update_memory_frequency,
            commands::memory::decay_memories,
            commands::memory::get_memories_to_forget,
            commands::memory::set_memory_permanent,
            commands::memory::archive_memory,
            commands::memory::permanently_delete_memory,
            commands::memory::cleanup_archived_memories,
            commands::memory::find_relevant_memories,
            commands::memory::find_relevant_memories_by_time,
            commands::memory::update_memory_embedding,
            commands::memory::batch_update_embeddings,
            commands::memory::update_memory_embeddings_real,
            commands::memory::summarize_memory_content,
            commands::memory::truncate_memory_content,
            commands::memory::process_forgetting_stage,
            commands::memory::run_batch_forgetting,
            commands::memory::get_memory_stats,
            commands::memory::run_evolution_review,
            commands::memory::get_evolution_proposals,
            commands::memory::accept_evolution_proposal,
            commands::memory::reject_evolution_proposal,
            commands::memory::check_evolution_due,
            commands::memory::detect_memory_conflicts,
            commands::memory::get_memory_history,
            commands::memory::merge_memories,
            commands::memory::get_profile,
            commands::memory::save_profile,
            commands::runtime_health::get_runtime_health,
            commands::activity::get_activity,
            commands::workspace_activity::get_workspace_activity_projection,
            commands::task_understanding_view::get_workspace_task_understanding,
            commands::settings::get_scheduled_tasks,
            commands::settings::save_scheduled_task,
            commands::settings::delete_scheduled_task,
            commands::settings::match_personality_direction,
            commands::settings::get_work_directory,
            commands::settings::set_work_directory,
            commands::workspace::get_workspaces,
            commands::workspace::create_project_workspace,
            commands::workspace::open_workspace,
            commands::workspace::confirm_restored_project_directories,
            commands::project_network_approval::get_project_network_approvals,
            commands::project_network_approval::revoke_project_network_approval,
            commands::settings::load_api_config,
            commands::model_connection::chatgpt_plan_status,
            commands::model_connection::chatgpt_plan_sign_in,
            commands::model_connection::chatgpt_plan_change_account,
            commands::model_connection::chatgpt_plan_cancel_sign_in,
            commands::model_connection::chatgpt_plan_disconnect,
            commands::model_connection::chatgpt_plan_models,
            commands::model_connection::test_model_connection,
            commands::settings::save_api_config,
            commands::settings::get_notification_settings,
            commands::settings::save_notification_settings,
            commands::settings::send_notification_channel,
            commands::settings::get_autostart,
            commands::settings::set_autostart,
            commands::settings::start_file_watcher,
            commands::settings::stop_file_watcher,
            commands::settings::is_file_watcher_running,
            commands::settings::export_data,
            commands::settings::export_encrypted_data,
            commands::settings::import_data,
            commands::settings::import_encrypted_data,
            commands::settings::clear_all_user_data,
            commands::web_search::configure_web_search,
            commands::web_search::clear_web_search_config,
            commands::web_search::get_web_search_config,
            commands::web_search::set_web_search_enabled,
            commands::file::read_file,
            commands::file::read_session_file,
            commands::file::write_session_file,
            commands::file::list_work_dir,
            commands::file::get_work_dir,
            commands::file::set_work_dir,
            commands::file::get_session_file_access,
            commands::file::set_session_read_dirs,
            commands::desktop::get_desktop_trusted_apps,
            commands::desktop::get_desktop_trusted_app_statuses,
            commands::desktop::inspect_desktop_draft_targets,
            commands::desktop::save_desktop_trusted_app,
            commands::desktop::confirm_desktop_observation_scope,
            commands::desktop::delete_desktop_trusted_app,
            commands::search::search_messages,
            commands::search::search_memories_by_keyword,
            commands::search::rebuild_fts_index,
            commands::search::hybrid_search_memories,
            commands::agent_event::get_agent_events,
            commands::agent_event::get_agent_run_events,
            commands::branch::create_session_branch,
            commands::branch::get_session_branches,
            commands::branch::get_session_branch_tree,
            commands::branch::switch_to_branch,
            commands::branch::delete_branch,
            commands::branch::get_all_branches,
            commands::session_tree::get_branch_tree,
            commands::session_tree::switch_to_message_branch,
            commands::usage::get_session_usage,
            commands::usage::get_cost_summary,
            commands::usage::get_usage_history,
            commands::usage::record_usage,
            commands::usage::get_usage_last_n_days,
            commands::steering::enqueue_instruction,
            commands::steering::submit_in_progress_command,
            commands::steering::interrupt_agent,
            commands::steering::pause_agent,
            commands::steering::resume_agent,
            commands::steering::get_agent_state,
            commands::steering::get_pending_instructions,
            commands::steering::should_agent_yield,
            commands::steering::cancel_instruction,
            commands::steering::clear_instructions,
            commands::automation::get_automations,
            commands::automation::get_automation_dashboard,
            commands::automation::get_workspace_automation_overview,
            commands::automation::create_automation,
            commands::automation::create_maa_automation,
            commands::automation::set_automation_enabled,
            commands::automation::delete_automation,
            commands::automation::run_automation_now,
            commands::automation::stop_automation_run,
            commands::automation::run_due_automations,
            commands::automation::get_automation_runs,
            commands::channel::get_channel_connections,
            commands::channel::create_channel_connection,
            commands::channel::set_channel_enabled,
            commands::channel::revoke_channel_connection,
            commands::channel::get_channel_audit,
            commands::channel::set_channel_proactive_policy,
            commands::channel::confirm_channel_message,
            commands::channel::save_channel_credential,
            commands::channel::clear_channel_credential,
            commands::skill::get_skills,
            commands::skill::get_skill,
            commands::skill::load_skills,
            commands::skill::import_github_skills,
            commands::skill::register_skill,
            commands::skill::skill_has_action,
            commands::skill::get_skill_ids,
        ])
        .setup(|app| {
            if let Some(state) = app.try_state::<AppState>() {
                let _ = state.delegated_model_factory.install(app.handle().clone());
                // Retired MCP keyring entries are queued in SQLite when a
                // service is changed or removed. Retry cleanup on startup so
                // a prior vault outage does not leave them indefinitely.
                if let Err(error) = state.mcp_manager.with_policy_gate(|| {
                    let conn = state
                        .db
                        .lock()
                        .map_err(|_| "无法访问本地数据库".to_string())?;
                    crate::mcp_credentials::reap_old_references(
                        &conn,
                        &crate::mcp_credentials::KeyringMcpCredentialStore::new(app.handle()),
                    )
                }) {
                    eprintln!("[Startup] MCP credential cleanup pending: {error}");
                }
                if let Some(delegation_pump) = state.delegation_pump.as_ref() {
                    let foreground_follow_up_pump = state.foreground_automation_pump.clone();
                    let app_handle = app.handle().clone();
                    delegation_pump.set_foreground_follow_up_waker(std::sync::Arc::new(
                        move || foreground_follow_up_pump.wake(app_handle.clone()),
                    ));
                }
                // A queue item may have been created before the previous
                // process exited. Startup recovery reconciles its durable
                // boundary above; this wake delivers only items that never
                // crossed a Main-Agent turn.
                state.foreground_automation_pump.wake(app.handle().clone());
            }
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("AngelBot")
                .on_menu_event(|app, event| match tray_menu_action(event.id.as_ref()) {
                    TrayMenuAction::Quit => {
                        if let Some(clock) = app.try_state::<automation_clock::AutomationClock>() {
                            clock.stop();
                        }
                        if let Some(state) = app.try_state::<AppState>() {
                            if let Some(pump) = &state.delegation_pump {
                                let _ = pump.shutdown();
                            }
                        }
                        app.exit(0);
                    }
                    TrayMenuAction::ShowWindow => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    TrayMenuAction::Ignore => {}
                })
                .build(app)?;

            #[cfg(debug_assertions)]
            {
                let window = app.get_webview_window("main").unwrap();
                if manual_desktop_acceptance() {
                    window.set_title("AngelBot · 原生隔离验收")?;
                } else if !model_recovery_acceptance() {
                    window.open_devtools();
                }
            }
            // All startup recovery, managed state and fallible window/tray
            // setup must succeed before due work may be admitted.
            automation_clock::AutomationClock::install(app.handle());
            Ok(())
        });

    #[cfg(feature = "production-updater")]
    let app_builder = app_builder.plugin(tauri_plugin_updater::Builder::new().build());

    #[cfg(feature = "desktop-e2e")]
    let app_builder = if manual_desktop_acceptance() {
        app_builder
    } else {
        app_builder
            .plugin(tauri_plugin_wdio::init())
            .plugin(tauri_plugin_wdio_webdriver::init())
    };

    let context = tauri::generate_context!();
    #[cfg(feature = "desktop-e2e")]
    let context = {
        let mut context = context;
        if model_recovery_acceptance() {
            for window in &mut context.config_mut().app.windows {
                window.focus = false;
                window.title = format!("AngelBot · 恢复隔离验收 {}", std::process::id());
            }
        }
        context
    };
    app_builder
        .build(context)
        .expect("error while building tauri application")
        .run(|app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                if let Some(clock) = app.try_state::<automation_clock::AutomationClock>() {
                    clock.stop();
                }
            }
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    state.mcp_manager.stop_all();
                }
            }
        });
}
