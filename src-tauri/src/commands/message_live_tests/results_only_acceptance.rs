//! Single opt-in live resume on an owned copy of completed synthetic receipts.
use super::*;

#[test]
#[ignore = "requires the configured authorized Luna model and the isolated paused-release copy"]
fn configured_results_only_resume() {
    assert_eq!(
        std::env::var("ANGELBOT_CONTEXT_EVAL_LIVE").as_deref(),
        Ok("1")
    );
    let root =
        std::path::PathBuf::from(std::env::var_os("ANGELBOT_PRIVATE_CONTEXT_EVAL_ROOT").unwrap());
    let conn = crate::db::init(&root.join("main-data.db")).unwrap();
    crate::db::migrate(&conn).unwrap();
    let (session, message): (String, String) = conn.query_row(
        "SELECT s.id,s.leaf_message_id FROM sessions s JOIN projects p ON p.active_session_id=s.id
         JOIN task_runs t ON t.session_id=s.id AND t.message_id=s.leaf_message_id
         WHERE p.name='Owned continuous A' AND p.kind='project' AND t.goal='继续。'
           AND t.status='provider_unavailable' AND t.completed_step_count=3",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    let app = headless_credential_host().unwrap();
    let model = crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
        .unwrap();
    assert_eq!(model.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
    assert!(model.model.contains("luna"));
    let mut state = crate::tests::test_app_state();
    crate::settings_cache::load_all(&conn, &state.cached_settings).unwrap();
    state.db = Arc::new(Mutex::new(conn));
    set_agent_execution_permission_impl(&state, "ask").unwrap();
    assert!(app.manage(state));
    let request: ContinueAgentTaskInput = serde_json::from_value(serde_json::json!({
        "sessionId":session, "messageId":message, "resultsOnly":true,
        "interaction":{"thinkingEffort":"low"},
    }))
    .unwrap();
    assert!(request.results_only);
    assert_eq!(
        request.continuation.interaction.thinking_effort.as_deref(),
        Some("low")
    );
    let outcome = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(continue_agent_task_from_input(
            &app.state::<AppState>(),
            Some(app.handle()),
            request,
        ));
    let payload = match outcome {
        Ok(reply) => serde_json::json!({
            "status":"returned", "session":session, "sourceMessage":message, "replyId":reply.id,
            "answer":reply.content, "taskRun":reply.task_run, "toolResults":reply.tool_results,
        }),
        Err(error) => serde_json::json!({"status":"command_error","session":session,"error":error}),
    };
    println!("RESULTS_ONLY_ROW {payload}");
}
