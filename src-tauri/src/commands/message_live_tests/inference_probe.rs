//! Opt-in, single provider invocation over a retained synthetic protocol.
//! No AgentRunner loop, browser, tool execution or source database writes.
use super::*;
use tauri::Manager;

pub(super) fn run() -> serde_json::Value {
    let source = std::path::PathBuf::from(std::env::var_os("ANGELBOT_PRIVATE_REPLAY_DB").unwrap());
    let output =
        std::path::PathBuf::from(std::env::var_os("ANGELBOT_PRIVATE_REPLAY_OUTPUT").unwrap());
    let mut report = serde_json::json!({"status":"unavailable", "requestCount":0, "toolCount":0,
        "historyCount":0, "schemaCount":0, "diagnosticRef":null, "reconstructedNotByteExact":true});
    let mut state = crate::tests::test_app_state();
    let conn =
        rusqlite::Connection::open_with_flags(&source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    // Reject accidental selection of ordinary data. These paired UUID markers
    // were emitted by the owned three-process journey, not inferred by name.
    let parent = source.parent().unwrap().parent().unwrap();
    let marker = std::fs::read_to_string(parent.join("browser-write.passed")).unwrap();
    Uuid::parse_str(&marker).unwrap();
    assert_eq!(
        std::fs::read_to_string(parent.join("browser-reopen.passed")).unwrap(),
        marker
    );
    assert!(parent.join("restored-main-history.json").is_file());
    let (provider_name, model, metadata): (String, String, String) = conn
        .query_row(
            "SELECT agent_provider,agent_model,metadata FROM sessions WHERE id='personal-main'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM sessions", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM mcp_servers WHERE id='browser-daily'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let history =
        crate::commands::foreground_history::hydrate_llm_history(&conn, "personal-main", i64::MAX)
            .unwrap();
    assert!(history.last().is_some_and(|m| m.role == "tool"));
    assert!(history.iter().any(|m| m.protocol_state.is_some()));
    report["historyCount"] = serde_json::json!(history.len());
    state.db = Arc::new(Mutex::new(conn));
    crate::settings_cache::load_all(&state.db.lock().unwrap(), &state.cached_settings).unwrap();
    let app = headless_credential_host().unwrap();
    let config =
        crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
            .unwrap();
    let binding: serde_json::Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(
        (config.provider.as_str(), config.model.as_str()),
        (provider_name.as_str(), model.as_str())
    );
    assert_eq!(config.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
    assert!(model.contains("luna"));
    let surface = crate::commands::foreground_tool_surface::ForegroundToolSurface::new(
        &state,
        Some(app.handle()),
    )
    .with_memory_reading(false)
    .build_for_session("personal-main".into())
    .unwrap();
    report["schemaCount"] = serde_json::json!(surface.get_schemas().len());
    assert!(state.mcp_manager.running_servers().is_empty());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let mut probe_state = crate::tests::test_app_state();
        let diagnostics = crate::db::init(&output).unwrap();
        crate::db::migrate(&diagnostics).unwrap();
        probe_state.db = Arc::new(Mutex::new(diagnostics));
        let mut record = |diagnostic: &str| {
            let card = crate::agent::attention::record_recoverable_runtime_failure(
                &probe_state.db.lock().unwrap(),
                "personal-main",
                "diagnostic:recorded-inference",
                "recorded-inference",
                diagnostic,
            )
            .unwrap();
            report["diagnosticRef"] = serde_json::json!(card.evidence_refs[0]);
        };
        let model = match crate::commands::foreground_model_factory::ForegroundModelFactory::new(
            Some(app.handle()),
            &probe_state,
        )
        .resolve(
            crate::commands::foreground_model_factory::ForegroundModelRequest {
                session_id: "personal-main",
                requested_thinking_effort: None,
                now: Utc::now().timestamp(),
            },
        ) {
            Ok(model) => model,
            Err(error) => {
                record(&error.to_string());
                return;
            }
        };
        assert_eq!(
            binding["modelConnectionBinding"]["profileRef"],
            model.identity.provider_profile_ref
        );
        assert_eq!(
            binding["modelConnectionBinding"]["credentialRef"],
            model.identity.credential_handle_ref
        );
        let mut agent_config = crate::agent::config::AgentConfig::default();
        agent_config.work_dir = Some(source.parent().unwrap().join("results"));
        agent_config.preferences.use_long_term_memory = false;
        let runner =
            crate::agent::AgentRunner::with_registry(model.provider, agent_config, surface)
                .with_thinking_settings(model.thinking_settings);
        let result = tokio::time::timeout(
            Duration::from_secs(45),
            runner.probe_recorded_inference(&history),
        )
        .await;
        let (status, diagnostic) = match result {
            Err(_) => (
                "timed_out",
                Some("Recorded inference exceeded its single-call budget".to_string()),
            ),
            Ok(Err(failure)) => (
                if failure.kind == crate::llm::LlmFailureKind::QuotaExceeded {
                    "quota_exceeded"
                } else {
                    "unavailable"
                },
                Some(failure.diagnostic),
            ),
            Ok(Ok(response)) => {
                let (response, protocol) = response.into_parts();
                if let Some(error) = protocol.and_then(|p| p.validate().err()) {
                    ("unavailable", Some(error))
                } else if matches!(
                    response,
                    crate::llm::LlmResponse::Text {
                        stop_reason: crate::llm::StopReason::EndTurn,
                        ..
                    } | crate::llm::LlmResponse::ToolCalls { .. }
                ) {
                    ("available", None)
                } else {
                    (
                        "unavailable",
                        Some("Recorded inference returned an incomplete response".into()),
                    )
                }
            }
        };
        if let Some(error) = diagnostic {
            record(&error);
        }
        report["status"] = serde_json::json!(status);
        report["requestCount"] = serde_json::json!(1);
        assert!(state.mcp_manager.running_servers().is_empty());
    });
    report
}
