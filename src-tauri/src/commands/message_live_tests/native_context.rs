//! Saved Luna checkpoint -> durable private ledger -> fresh Main file task.
//! Synthetic files/history only; no desktop, scheduler or ordinary settings writes.
use super::*;
use crate::commands::foreground_context_assembler::{
    ForegroundContextAssembler, ForegroundContextBudget, ForegroundContextRequest,
};
use crate::commands::foreground_response_projector::PersistedTurnOutcome;
use tauri::Manager;

const SOURCE: &str = "item,amount\none,23\ntwo,19\n";

fn message(role: &str, content: String) -> crate::llm::Message {
    crate::llm::Message {
        role: role.into(),
        content,
        tool_calls: None,
        tool_call_id: None,
        protocol_state: None,
        tool_images: Vec::new(),
    }
}

fn request(session: &str, content: &str) -> SendMessageRequest {
    SendMessageRequest {
        session_id: session.into(),
        client_message_id: None,
        role: "user".into(),
        content: content.into(),
        text_attachments: vec![],
        personality: None,
        preferences: None,
        thinking_effort: Some("low".into()),
        web_search_setup: None,
    }
}

pub(super) async fn verify(
    app: &AppHandle,
    config: &crate::llm::ProviderConfig,
    directory: &std::path::Path,
    receipt: &str,
    phase: &str,
) {
    let first = phase == "native-start";
    let workspace_path = directory.join("workspace");
    let mut state = crate::tests::test_app_state();
    state.db = Arc::new(Mutex::new(
        crate::db::init(&directory.join("data.db")).unwrap(),
    ));
    {
        let conn = state.db.lock().unwrap();
        crate::db::migrate(&conn).unwrap();
        if first {
            conn.execute_batch("INSERT OR IGNORE INTO profile(id,updated_at) VALUES(1,1);")
                .unwrap();
        }
    }
    assert!(app.manage(state));
    let state = app.state::<AppState>();
    let opened = if first {
        std::fs::create_dir(&workspace_path).unwrap();
        std::fs::write(workspace_path.join("amounts.csv"), SOURCE).unwrap();
        crate::commands::workspace::create_project_workspace(
            app.state(),
            workspace_path.to_string_lossy().into_owned(),
            Some("Owned native context".into()),
            None,
            None,
        )
        .unwrap()
    } else {
        let project = crate::commands::workspace::get_workspaces(app.state())
            .unwrap()
            .into_iter()
            .find(|w| w.kind == "project")
            .unwrap();
        crate::commands::workspace::open_workspace(app.state(), project.id).unwrap()
    };
    let session = opened.session.id;
    crate::settings_cache::load_all(&state.db.lock().unwrap(), &state.cached_settings).unwrap();
    if first {
        set_agent_execution_permission_impl(&state, "full_access").unwrap();
        crate::commands::memory::resolve_reply_memory_controls(
            &state.db.lock().unwrap(),
            &session,
            Some(false),
            Some(false),
        )
        .unwrap();
    }
    assert_eq!(agent_execution_permission(&state).unwrap(), "full_access");
    if first {
        // Only this owned provider instance lowers its soft threshold. Nothing
        // is saved to the user's model configuration or sent as an output limit.
        let mut bounded = config.clone();
        bounded.max_tokens = (crate::llm::context_window_tokens(&bounded) - 8192) as u32;
        let mut history = vec![
            message("system", "This is an owned synthetic continuity check. Retain the exact owner marker for a later question, ignore repetitive filler, and do not echo the marker until asked.".into()),
            message("user", format!("The exact owner marker to retain is {receipt}. Filler follows:\n{}", "entry ".repeat(6500))),
            message("assistant", "Noted.".into()),
            message("user", "Now reply with only ready.".into()),
        ];
        let provider = crate::llm::create_provider_for_app(&bounded, Some(app)).unwrap();
        let (response, native) = provider
            .chat(&history, None, None)
            .await
            .expect("native compaction must complete without credential/billing fallback")
            .into_parts();
        assert!(
            matches!(response, crate::llm::LlmResponse::Text { ref text, .. } if text.trim() == "ready")
        );
        let native = native.unwrap();
        assert!(
            native
                .output_items
                .iter()
                .any(|item| item["type"] == "compaction"),
            "accepted context_management without an actual checkpoint is not compaction evidence"
        );
        let mut answer = message("assistant", "ready".into());
        answer.protocol_state = Some(native);
        history.push(answer);
        history.remove(0); // Host policy is regenerated, never stored as private conversation data.
        let store = ForegroundRunStore::new(state.db.clone());
        let run = store
            .begin(BeginForegroundTurn {
                session_id: session.clone(),
                user_message_id: Uuid::new_v4().to_string(),
                reply_id: Uuid::new_v4().to_string(),
                role: "user".into(),
                content: "Now reply with only ready.".into(),
                text_attachments: vec![],
                user_turn: ForegroundUserTurn::Append,
                now: Utc::now().timestamp() - 30,
            })
            .unwrap();
        store
            .complete(
                &run,
                PersistedTurnOutcome {
                    content: "ready".into(),
                    private_diagnostic: None,
                    goal: "Now reply with only ready.".into(),
                    tool_calls: None,
                    tool_results: None,
                    task_facts: None,
                    protocol_transcript: Some(history),
                },
            )
            .unwrap();
        println!(
            "LIVE_NATIVE_CONTEXT {}",
            serde_json::json!({
                "phase":phase,"hasNativeCheckpoint":true,"syntheticOnly":true,"calls":1,
                "usageInputTokens":provider.latest_usage().map(|u|u.input_tokens),"session":session
            })
        );
        return;
    }
    let content = if phase == "native-resume" {
        "请回忆之前记录的验收标识，只读取当前项目 amounts.csv，计算两笔金额合计，新建 receipt.json，严格包含 marker（之前记录的完整标识）与 total（数字）两个字段。读回核对后报告实际结果。不要向我重新询问标识。"
    } else {
        "请只读核对当前项目 receipt.json 与 amounts.csv，报告标识和合计是否一致。不新增或改写文件，也不要仅凭之前的回复猜测。"
    };
    let mut input = request(&session, content);
    input.content.push_str("只用项目文件读取／写入工具；仅限这两个文件。禁止运行代码、脚本、终端、联网、应用、自动化或派发子代理。");
    let prepared = ForegroundContextAssembler::prepare(ForegroundContextRequest {
        state: &state,
        app: Some(app),
        request: &input,
        history_before_created_at: i64::MAX,
        budget: ForegroundContextBudget {
            estimated_tool_count: crate::commands::foreground_tool_surface::ESTIMATED_TOOL_COUNT,
        },
        summary_delivery_id: None,
    })
    .await
    .unwrap();
    let registry =
        crate::commands::foreground_tool_surface::ForegroundToolSurface::new(&state, Some(app))
            .with_memory_reading(false)
            .build_for_session(session.clone())
            .unwrap();
    let schemas = registry
        .get_schemas_with_enhanced(prepared.agent_config.enhanced_tool_descriptions)
        .into_iter()
        .map(|v| {
            let v = v.get("function").unwrap_or(&v);
            crate::llm::ToolSchema {
                name: v["name"].as_str().unwrap().into(),
                description: v["description"].as_str().unwrap().into(),
                parameters: v["parameters"].clone(),
            }
        })
        .collect::<Vec<_>>();
    let mut messages =
        crate::agent::foreground_conversation_state::ForegroundConversationState::initial_context(
            &prepared.agent_config,
            &schemas,
            &registry.get_prompt_snippets(),
            &prepared.agent_input,
            crate::agent::foreground_conversation_state::ForegroundHistory::Native(
                &prepared.history,
            ),
        );
    if let Some(context) = prepared.attention_context {
        messages.insert(1, message("system", context));
    }
    let (checkpoint, plaintext, wire_bytes) =
        crate::llm::native_context_shape(config, &messages, receipt).unwrap();
    assert!(
        checkpoint,
        "a fresh process must replay the actual saved checkpoint"
    );
    if phase == "native-resume" {
        assert!(
            !plaintext,
            "the initial Main request must not carry the owner marker in plaintext"
        );
        assert!(
            prepared.history.iter().any(|m| m.content.contains(receipt)),
            "the durable raw branch must still retain its original history"
        );
    }
    let reply = tokio::time::timeout(
        Duration::from_secs(240),
        send_message_impl(
            &state,
            Some(app),
            input,
            None,
            ForegroundUserTurn::Append,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("bounded native-context task timed out")
    .expect("Main task failed");
    let task = reply.task_run.as_ref().unwrap();
    if task.status != "completed" {
        println!(
            "LIVE_NATIVE_INCOMPLETE status={} Main={}",
            task.status, reply.content
        );
    }
    assert_eq!(task.status, "completed");
    assert!(!task.resumable);
    let results = reply.tool_results.as_ref().unwrap();
    assert!(!results.is_empty());
    for result in results {
        assert!(result.success && !result.confirmation_required);
        assert!(matches!(
            result.tool_name.as_str(),
            "read_file" | "write_file" | "list_workspace_items"
        ));
        if phase == "native-check" {
            assert_ne!(result.tool_name, "write_file");
        }
    }
    let output: serde_json::Value =
        serde_json::from_slice(&std::fs::read(workspace_path.join("receipt.json")).unwrap())
            .unwrap();
    assert_eq!(output, serde_json::json!({"marker":receipt,"total":42}));
    assert_eq!(
        std::fs::read_to_string(workspace_path.join("amounts.csv")).unwrap(),
        SOURCE
    );
    assert!(
        results
            .iter()
            .any(|r| r.tool_name == "read_file" && r.output.contains(receipt)),
        "a fresh actual receipt readback is required"
    );
    let writes: i64 = state.db.lock().unwrap().query_row(
        "SELECT count(*) FROM agent_steps WHERE session_id=?1 AND tool_name='write_file' AND success=1",
        [&session], |row| row.get(0),
    ).unwrap();
    assert_eq!(
        writes, 1,
        "restart/readback must not repeat the delivery write"
    );
    println!(
        "LIVE_NATIVE_CONTEXT {}",
        serde_json::json!({
            "phase":phase,"hasNativeCheckpoint":checkpoint,"initialPlaintextMarker":plaintext,
            "wireBytes":wire_bytes,"writes":writes,"total":42,"calls":results.len(),
            "status":task.status,"session":session
        })
    );
}
