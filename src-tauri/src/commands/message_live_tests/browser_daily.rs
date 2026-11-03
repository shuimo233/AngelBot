//! Explicit saved-Luna/official-MCP webpage task; no foreground input or user browser.
use super::*;
use sha2::{Digest, Sha256};
use tauri::Manager;

const SERVER: &str = "browser-daily";
const SESSION: &str = "personal-main";
const UPLOAD_BROWSER_TOOLS: &[&str] = &[
    "browser_navigate",
    "browser_snapshot",
    "browser_wait_for",
    "browser_tabs",
    "browser_close",
    "browser_click",
    "browser_type",
    "browser_press_key",
    "browser_fill_form",
    "browser_select_option",
    "browser_file_upload",
];

// One delivery contract for all file journeys. Display labels in request.json
// are inputs to the form, not replacements for the saved record's raw values.
fn receipt_field_contract(attachment: bool) -> String {
    let fields = "JSON 字段为 draft_id（数字）、status（固定 draft）、title、category（页面保存的值）、location、date、notes、adapter（布尔）";
    format!(
        "{fields}{}。",
        if attachment {
            "，以及 attachment（含 name、bytes、sha256）"
        } else {
            ""
        }
    )
}

#[test]
fn file_journeys_share_saved_value_contract_without_supplying_expected_enum_values() {
    let ordinary = receipt_field_contract(false);
    let upload = receipt_field_contract(true);
    assert!(ordinary.contains("category（页面保存的值）"));
    assert!(upload.starts_with(ordinary.trim_end_matches('。')));
    assert!(!ordinary.contains("attachment"));
    assert!(upload.contains("attachment（含 name、bytes、sha256）"));
    assert!(!upload.contains("equipment"));
}

fn verify_tool_outcomes(reply: &Message) -> Result<(), String> {
    let results = reply
        .tool_results
        .as_deref()
        .filter(|results| !results.is_empty())
        .ok_or("browser acceptance requires actual tool receipts")?;
    let calls = reply.tool_calls.as_deref().unwrap_or_default();
    let arguments_for = |result: &crate::commands::foreground_message_contracts::ToolResultInfo| {
        let mut matching = calls
            .iter()
            .filter(|call| call.id == result.call_id && call.name == result.tool_name);
        let call = matching.next()?;
        if matching.next().is_some() {
            return None;
        }
        serde_json::from_str::<serde_json::Value>(&call.arguments).ok()
    };
    for (index, result) in results.iter().enumerate() {
        if result.success {
            continue;
        }
        // Reuse the host's narrow proof: no dispatch, only schema-proven null
        // corrections, same target and a later successful, separately bound call.
        let repaired = arguments_for(result).is_some_and(|original| {
            results[index + 1..].iter().any(|later| {
                later.success
                    && !later.confirmation_required
                    && later.call_id != result.call_id
                    && later.tool_name == result.tool_name
                    && arguments_for(later).is_some_and(|corrected| {
                        crate::agent::repairs_argument_rejection(
                            &result.call_id,
                            &result.tool_name,
                            &original,
                            &result.output,
                            &corrected,
                        )
                    })
            })
        });
        if !repaired {
            return Err(format!("{}: {}", result.tool_name, result.output));
        }
    }
    Ok(())
}

#[test]
fn browser_acceptance_only_settles_exact_corrected_preflight_rejections() {
    let rejection = serde_json::json!({
        "code":"HOST_ARGUMENTS_REJECTED", "dispatched":false,
        "call_id":"bad", "tool_name":"snapshot", "null_corrections":["depth"]
    })
    .to_string();
    let base = serde_json::json!({
        "id":"reply", "sessionId":"personal-main", "role":"assistant", "createdAt":1,
        "content":"Actual state verified",
        "toolCalls":[
            {"id":"bad","name":"snapshot","arguments":r#"{"target":"owned","depth":null}"#},
            {"id":"good","name":"snapshot","arguments":r#"{"target":"owned","depth":6}"#}
        ],
        "toolResults":[
            {"callId":"bad","toolName":"snapshot","success":false,"output":rejection},
            {"callId":"good","toolName":"snapshot","success":true,"output":"fresh observed state"}
        ]
    });
    let reply: Message = serde_json::from_value(base.clone()).unwrap();
    assert!(
        verify_tool_outcomes(&reply).is_ok(),
        "a proven pre-dispatch correction is not an executed failure"
    );
    assert!(!reply.tool_results.as_ref().unwrap()[0].success);
    let mut omitted = base.clone();
    omitted["toolCalls"][1]["arguments"] = serde_json::json!(r#"{"target":"owned"}"#);
    assert!(verify_tool_outcomes(&serde_json::from_value(omitted).unwrap()).is_ok());
    // Captured 2026-10-23: the official snapshot call used depth:null, then
    // corrected it to 6 without dispatching the first call. Keep its false
    // receipt visible; only this exact host-proven correction may settle it.
    let mut snapshot = base.clone();
    let name = crate::agent::handlers::mcp_registry_tool_name(SERVER, "browser_snapshot");
    for index in 0..2 {
        snapshot["toolCalls"][index]["name"] = serde_json::json!(name);
        snapshot["toolResults"][index]["toolName"] = serde_json::json!(name);
    }
    snapshot["toolCalls"][0]["arguments"] = serde_json::json!(r#"{"depth":null}"#);
    snapshot["toolCalls"][1]["arguments"] = serde_json::json!(r#"{"depth":6}"#);
    let mut proof: serde_json::Value = serde_json::from_str(&rejection).unwrap();
    proof["tool_name"] = serde_json::json!(name);
    snapshot["toolResults"][0]["output"] = serde_json::json!(proof.to_string());
    assert!(verify_tool_outcomes(&serde_json::from_value(snapshot).unwrap()).is_ok());
    for case in [
        "unknown",
        "handler",
        "target",
        "extra",
        "binding",
        "dispatched",
        "unproven",
        "earlier",
        "failed",
        "missing",
        "duplicate",
        "same-id",
        "empty",
        "proposal",
    ] {
        let mut candidate = base.clone();
        match case {
            "unknown" => {
                candidate["toolResults"][0]["output"] =
                    serde_json::json!(r#"Error: {"code":"RESULT_UNKNOWN"}"#)
            }
            "handler" => {
                candidate["toolResults"][0]["output"] =
                    serde_json::json!(format!("Error: {rejection}"))
            }
            "target" => {
                candidate["toolCalls"][1]["arguments"] =
                    serde_json::json!(r#"{"target":"other","depth":6}"#)
            }
            "extra" => {
                candidate["toolCalls"][1]["arguments"] =
                    serde_json::json!(r#"{"target":"owned","depth":6,"new":true}"#)
            }
            "binding" | "dispatched" | "unproven" => {
                let mut receipt: serde_json::Value = serde_json::from_str(&rejection).unwrap();
                match case {
                    "binding" => receipt["call_id"] = serde_json::json!("other"),
                    "dispatched" => receipt["dispatched"] = serde_json::json!(true),
                    _ => receipt["null_corrections"] = serde_json::json!([]),
                }
                candidate["toolResults"][0]["output"] = serde_json::json!(receipt.to_string());
            }
            "earlier" => candidate["toolResults"].as_array_mut().unwrap().reverse(),
            "failed" => candidate["toolResults"][1]["success"] = serde_json::json!(false),
            "missing" => {
                candidate["toolCalls"].as_array_mut().unwrap().remove(1);
            }
            "duplicate" => {
                let duplicate = candidate["toolCalls"][1].clone();
                candidate["toolCalls"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            "same-id" => candidate["toolResults"][1]["callId"] = serde_json::json!("bad"),
            "empty" => candidate["toolResults"] = serde_json::json!([]),
            "proposal" => {
                candidate["toolResults"][1]["confirmationRequired"] = serde_json::json!(true)
            }
            _ => unreachable!(),
        }
        let reply: Message = serde_json::from_value(candidate).unwrap();
        assert!(
            verify_tool_outcomes(&reply).is_err(),
            "{case} must not be settled"
        );
    }
}

/// The restored profile must rediscover and explicitly reauthorize its service.
/// Old receipts are history; only the new upload task may change the webpage.
async fn verify_restored_upload(
    app: &AppHandle,
    state: &AppState,
    root: &std::path::Path,
    before_step: i64,
) -> Message {
    let mut server = crate::commands::mcp::get_mcp_servers(app.clone(), app.state())
        .unwrap()
        .into_iter()
        .find(|server| server.id == SERVER)
        .unwrap();
    assert!(
        server.enabled_workspace_ids.is_empty(),
        "backup must not restore an MCP grant"
    );
    let runtime =
        crate::browser_storage::configured_output(&state.db.lock().unwrap(), SERVER, &server.args)
            .unwrap()
            .expect("restored browser output must be rebound to the new profile");
    assert!(runtime.starts_with(root));
    assert!(runtime.is_dir());
    assert!(
        std::fs::read_dir(&runtime).unwrap().next().is_none(),
        "a backup must not carry browser runtime files"
    );
    // Renew this one service's authorization explicitly, just as the settings
    // page does. This is not evidence of portable grants or automatic trust.
    server.enabled = true;
    server.enabled_workspace_ids = vec!["personal".into()];
    crate::commands::mcp::save_mcp_server(app.clone(), app.state(), server).unwrap();
    crate::commands::mcp::start_mcp_server(app.clone(), app.state(), SERVER.into()).unwrap();
    crate::commands::mcp::stop_mcp_server(app.state(), SERVER.into()).unwrap();
    let surface =
        crate::commands::foreground_tool_surface::ForegroundToolSurface::new(state, Some(app))
            .build_for_session(SESSION.into())
            .unwrap();
    assert!(
        surface.contains(&crate::agent::handlers::mcp_registry_tool_name(
            SERVER,
            "browser_file_upload"
        ))
    );
    assert_eq!(
        state.mcp_manager.get_status(SERVER).unwrap().status,
        "stopped"
    );
    let url = reqwest::Url::parse(&std::env::var("ANGELBOT_LIVE_BROWSER_URL").unwrap()).unwrap();
    assert_eq!(url.scheme(), "http");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert!(url.port().is_some() && url.username().is_empty() && url.password().is_none());
    let client = reqwest::Client::new();
    let before: serde_json::Value = client
        .get(url.join("api/state").unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before["saveAttempts"], 1);
    assert_eq!(before["uploadAttempts"], 1);
    assert_eq!(before["drafts"].as_array().unwrap().len(), 1);
    let content = format!("这是加密备份恢复后的新任务，不是重试旧任务。请读取当前日常结果区的 request.json、receipt.json 和 borrow-items.csv，使用原申请的字段新建一份标题前加“恢复后复核：”的借用申请草稿；附件只用当前日常结果区的 borrow-items.csv。应用为 {url}，只保存草稿，不发送。请实际打开保存后的新草稿，核对全部字段及附件名称、字节数、服务端 SHA256 后，新建 restored-upload.json 并读回交付 angelbot-file:restored-upload.json。{} 只用现有文件读取／写入和这个浏览器 MCP 的页面导航、快照、输入、点击及选择文件；旧文件只读，新回执只写一次。若回执未知先核对，不重复保存或上传。不要重做旧申请、下载、运行代码、命令、自动化、委派、打开其他网站或访问其他文件。", receipt_field_contract(true));
    let reply = tokio::time::timeout(
        Duration::from_secs(240),
        send_message_impl(
            state,
            Some(app),
            SendMessageRequest {
                session_id: SESSION.into(),
                client_message_id: None,
                role: "user".into(),
                content,
                text_attachments: vec![],
                personality: None,
                preferences: None,
                thinking_effort: None,
                web_search_setup: None,
            },
            None,
            ForegroundUserTurn::Append,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("restored upload timed out")
    .expect("restored upload Main failed");
    assert_eq!(
        reply.task_run.as_ref().map(|run| run.status.as_str()),
        Some("completed"),
        "{}",
        reply.content
    );
    verify_tool_outcomes(&reply).unwrap();
    let results = reply.tool_results.as_deref().unwrap();
    let allowed = UPLOAD_BROWSER_TOOLS
        .iter()
        .map(|name| crate::agent::handlers::mcp_registry_tool_name(SERVER, name))
        .collect::<Vec<_>>();
    for result in results {
        assert!(
            matches!(
                result.tool_name.as_str(),
                "read_file" | "write_file" | "list_workspace_items"
            ) || allowed.contains(&result.tool_name),
            "unexpected restored upload tool {}",
            result.tool_name
        );
    }
    let old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("receipt.json")).unwrap()).unwrap();
    let title = format!("恢复后复核：{}", old["title"].as_str().unwrap());
    let mut expected = old.clone();
    expected["draft_id"] = serde_json::json!(2);
    expected["title"] = serde_json::json!(title);
    let local: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("restored-upload.json")).unwrap()).unwrap();
    assert_eq!(local, expected);
    let saved = results
        .iter()
        .position(|r| {
            r.output.contains("已保存的草稿")
                && r.output.contains(&title)
                && r.output
                    .contains(old["attachment"]["sha256"].as_str().unwrap())
        })
        .unwrap();
    let written = results
        .iter()
        .position(|r| r.tool_name == "write_file")
        .unwrap();
    assert!(
        saved < written,
        "delivery must follow fresh server readback"
    );
    let after: serde_json::Value = client
        .get(url.join("api/state").unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["saveAttempts"], 2);
    assert_eq!(after["mutations"], 2);
    assert_eq!(after["uploadAttempts"], 2);
    assert_eq!(after["downloadAttempts"], 0);
    assert_eq!(after["drafts"].as_array().unwrap().len(), 2);
    assert_eq!(
        after["drafts"][0], before["drafts"][0],
        "the original draft is immutable"
    );
    expected.as_object_mut().unwrap().remove("status");
    expected.as_object_mut().unwrap().remove("draft_id");
    expected["id"] = serde_json::json!(2);
    assert_eq!(after["drafts"][1], expected);
    let conn = state.db.lock().unwrap();
    let name = crate::agent::handlers::mcp_registry_tool_name(SERVER, "browser_file_upload");
    let mut uploads = conn
        .prepare("SELECT tool_input FROM agent_steps WHERE rowid>?1 AND tool_name=?2 AND success=1")
        .unwrap();
    let inputs = uploads
        .query_map(params![before_step, name], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        inputs.len(),
        1,
        "new task must select its attachment once, without replay"
    );
    let input: serde_json::Value = serde_json::from_str(&inputs[0]).unwrap();
    let paths = input["paths"].as_array().unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(
        std::path::Path::new(paths[0].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        root.join("borrow-items.csv")
    );
    drop(uploads);
    drop(conn);
    assert_eq!(
        state.mcp_manager.get_status(SERVER).unwrap().status,
        "running"
    );
    crate::commands::mcp::stop_mcp_server(app.state(), SERVER.into()).unwrap();
    reply
}

/// Reuse the already verified source without replaying its model/tool phases.
/// Keep both the original source and the failed restored target read-only.
pub(super) fn prepare_retained_restore(source: &std::path::Path, directory: &std::path::Path) {
    let marker = std::fs::read_to_string(source.join("browser-write.passed")).unwrap();
    Uuid::parse_str(&marker).unwrap();
    assert_eq!(
        std::fs::read_to_string(source.join("browser-reopen.passed")).unwrap(),
        marker
    );
    let mut state = crate::tests::test_app_state();
    state.db = Arc::new(Mutex::new(
        rusqlite::Connection::open_with_flags(
            source.join("data.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap(),
    ));
    let history = ForegroundRunStore::new(state.db.clone())
        .project_session(SESSION)
        .unwrap();
    assert_eq!(
        serde_json::to_value(&history).unwrap(),
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(source.join("restored-main-history.json")).unwrap()
        )
        .unwrap()
    );
    prepare_restored_profile(&state, directory, "borrow-items.csv");
    std::fs::create_dir(directory.join("results")).unwrap();
    for name in ["request.json", "receipt.json", "borrow-items.csv"] {
        std::fs::copy(
            source.join("results").join(name),
            directory.join("results").join(name),
        )
        .unwrap();
    }
}

fn prepare_restored_profile(state: &AppState, directory: &std::path::Path, attachment: &str) {
    let history = ForegroundRunStore::new(state.db.clone())
        .project_session(SESSION)
        .unwrap();
    assert!(!history.is_empty());
    let password = "synthetic daily backup password".to_owned();
    let encrypted =
        crate::commands::settings::export_encrypted_data_impl(state, password.clone(), None)
            .unwrap();
    assert!(!encrypted.contains(attachment));
    let restored = directory.join("restored");
    std::fs::create_dir(&restored).unwrap();
    let mut target = crate::tests::test_app_state();
    let conn = crate::db::init(&restored.join("data.db")).unwrap();
    crate::db::migrate(&conn).unwrap();
    target.db = Arc::new(Mutex::new(conn));
    crate::commands::settings::import_encrypted_data_impl(&target, encrypted, password, None)
        .unwrap();
    assert_eq!(
        serde_json::to_value(
            ForegroundRunStore::new(target.db.clone())
                .project_session(SESSION)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&history).unwrap()
    );
    let leaf = |state: &AppState| {
        state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT leaf_message_id FROM sessions WHERE id=?1",
                [SESSION],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    };
    assert_eq!(leaf(state), leaf(&target));
    std::fs::write(
        directory.join("restored-main-history.json"),
        serde_json::to_vec(&history).unwrap(),
    )
    .unwrap();
    let source_root = crate::commands::file::resolve_work_dir(state, Some(SESSION)).unwrap();
    for name in ["request.json", "receipt.json", attachment] {
        let bytes = std::fs::read(source_root.join(name)).unwrap();
        assert_eq!(
            std::fs::read(restored.join("results").join(name)).unwrap(),
            bytes
        );
    }
}

#[test]
fn retained_restore_uses_encrypted_import_without_mutating_source_or_failed_target() {
    let source = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let conn = crate::db::init(&source.path().join("data.db")).unwrap();
    crate::db::migrate(&conn).unwrap();
    conn.execute_batch("INSERT OR IGNORE INTO profile(id,updated_at) VALUES(1,1);
        INSERT INTO messages(id,session_id,role,content,created_at) VALUES('retained-message','personal-main','user','Retained synthetic history',1);
        UPDATE sessions SET leaf_message_id='retained-message' WHERE id='personal-main';").unwrap();
    let mut state = crate::tests::test_app_state();
    state.db = Arc::new(Mutex::new(conn));
    let history = ForegroundRunStore::new(state.db.clone())
        .project_session(SESSION)
        .unwrap();
    std::fs::write(
        source.path().join("restored-main-history.json"),
        serde_json::to_vec(&history).unwrap(),
    )
    .unwrap();
    drop(state);
    std::fs::create_dir(source.path().join("results")).unwrap();
    for name in ["request.json", "receipt.json", "borrow-items.csv"] {
        std::fs::write(source.path().join("results").join(name), name.as_bytes()).unwrap();
    }
    std::fs::create_dir(source.path().join("restored")).unwrap();
    std::fs::write(
        source.path().join("restored/data.db"),
        b"prior failure evidence",
    )
    .unwrap();
    let marker = Uuid::new_v4().to_string();
    for name in ["browser-write.passed", "browser-reopen.passed"] {
        std::fs::write(source.path().join(name), &marker).unwrap();
    }
    let before = std::fs::read(source.path().join("data.db")).unwrap();
    prepare_retained_restore(source.path(), output.path());
    assert_eq!(
        std::fs::read(source.path().join("data.db")).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(source.path().join("restored/data.db")).unwrap(),
        b"prior failure evidence"
    );
    let restored = crate::db::init(&output.path().join("restored/data.db")).unwrap();
    assert_eq!(
        restored
            .query_row(
                "SELECT content FROM messages WHERE id='retained-message'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "Retained synthetic history"
    );
    for name in ["request.json", "receipt.json", "borrow-items.csv"] {
        assert_eq!(
            std::fs::read(output.path().join("results").join(name)).unwrap(),
            name.as_bytes()
        );
    }
    let rejected = tempfile::tempdir().unwrap();
    std::fs::write(
        source.path().join("browser-reopen.passed"),
        Uuid::new_v4().to_string(),
    )
    .unwrap();
    assert!(
        std::panic::catch_unwind(|| prepare_retained_restore(source.path(), rejected.path()))
            .is_err()
    );
    assert!(!rejected.path().join("restored").exists());
}

/// Continue the actual retained Main through the same command as the UI.
/// Previously saved artifacts are evidence, never instructions to replay them.
pub(super) async fn verify_delivery_continuation(app: &AppHandle, provider: &str, model: &str) {
    let profile = std::path::PathBuf::from(
        std::env::var_os("ANGELBOT_PRIVATE_CONTINUATION_PROFILE").unwrap(),
    );
    let marker =
        std::fs::read_to_string(profile.parent().unwrap().join("browser-prepare.passed")).unwrap();
    Uuid::parse_str(&marker).unwrap();
    let mut state = crate::tests::test_app_state();
    state.db = Arc::new(Mutex::new(
        crate::db::init(&profile.join("data.db")).unwrap(),
    ));
    crate::db::migrate(&state.db.lock().unwrap()).unwrap();
    crate::settings_cache::load_all(&state.db.lock().unwrap(), &state.cached_settings).unwrap();
    let (source_id, goal, before_step): (String, String, i64) = {
        let conn = state.db.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT agent_provider,agent_model FROM sessions WHERE id='personal-main'",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            )
            .unwrap(),
            (provider.to_owned(), model.to_owned())
        );
        conn.query_row(
            "SELECT r.message_id,r.goal,(SELECT coalesce(max(rowid),0) FROM agent_steps)
            FROM task_runs r JOIN sessions s ON s.id=r.session_id AND s.leaf_message_id=r.message_id
            WHERE s.id='personal-main' AND r.status='needs_attention' AND r.resumable=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    let root = crate::commands::file::resolve_work_dir(&state, Some(SESSION)).unwrap();
    assert_eq!(root, profile.join("results").canonicalize().unwrap());
    let originals = [
        "request.json",
        "receipt.json",
        "borrow-items.csv",
        "restored-upload.json",
    ]
    .map(|name| (name, std::fs::read(root.join(name)).unwrap()));
    assert!(app.manage(state));
    let state = app.state::<AppState>();
    let server = crate::commands::mcp::get_mcp_servers(app.clone(), app.state())
        .unwrap()
        .into_iter()
        .find(|server| server.id == SERVER)
        .unwrap();
    assert!(server.enabled && server.enabled_workspace_ids == ["personal"]);
    assert!(server.command == "node" || server.command == "node.exe");
    let args: Vec<String> = serde_json::from_str(&server.args).unwrap();
    let runtime =
        crate::browser_storage::configured_output(&state.db.lock().unwrap(), SERVER, &server.args)
            .unwrap()
            .unwrap();
    assert!(runtime.starts_with(&root));
    let cli = std::path::PathBuf::from(std::env::var_os("ANGELBOT_LIVE_BROWSER_CLI").unwrap());
    assert_eq!(
        std::path::Path::new(&args[0]).canonicalize().unwrap(),
        cli.canonicalize().unwrap()
    );
    assert!(
        args[1..]
            == [
                "--browser",
                "msedge",
                "--headless",
                "--isolated",
                "--no-webmcp",
                "--output-dir",
                &crate::commands::file::display_work_dir(&runtime),
                "--image-responses",
                "omit"
            ],
        "continuation requires the unchanged isolated headless browser preset"
    );
    let prior = get_messages(app.state(), SESSION.into())
        .unwrap()
        .into_iter()
        .find(|message| message.id == source_id)
        .unwrap();
    let calls = prior.tool_calls.as_deref().unwrap();
    let receipts = prior.tool_results.as_deref().unwrap();
    let artifact_call =
        |receipt: &crate::commands::foreground_message_contracts::ToolResultInfo, tool: &str| {
            receipt.success
                && receipt.tool_name == tool
                && calls.iter().any(|call| {
                    call.id == receipt.call_id
                        && call.name == tool
                        && serde_json::from_str::<serde_json::Value>(&call.arguments)
                            .ok()
                            .and_then(|args| args["path"].as_str().map(str::to_owned))
                            .and_then(|path| root.join(path).canonicalize().ok())
                            .is_some_and(|path| path == root.join("restored-upload.json"))
                })
        };
    let written = receipts
        .iter()
        .position(|r| artifact_call(r, "write_file"))
        .unwrap();
    assert_eq!(
        receipts
            .iter()
            .filter(|r| artifact_call(r, "write_file"))
            .count(),
        1
    );
    let read = receipts
        .iter()
        .enumerate()
        .find(|(index, r)| *index > written && artifact_call(r, "read_file"))
        .unwrap()
        .1;
    let snapshot: serde_json::Value = serde_json::from_str(&read.output).unwrap();
    assert_eq!(
        snapshot["content"].as_str().unwrap().as_bytes(),
        originals[3].1
    );
    assert_eq!(
        snapshot["resolvedPath"],
        crate::commands::file::display_work_dir(&root.join("restored-upload.json"))
    );
    let reply = tokio::time::timeout(
        Duration::from_secs(180),
        continue_agent_task_impl(
            &state,
            Some(app),
            ContinueAgentTaskRequest {
                session_id: SESSION.into(),
                message_id: source_id.clone(),
            },
        ),
    )
    .await
    .expect("Main delivery continuation timed out")
    .expect("Main delivery continuation failed");
    // Text-only replies intentionally omit the UI task card. Completion is
    // durable state, not the presence of optional presentation metadata.
    let task: (String, String, String) = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT r.status,r.goal,s.leaf_message_id FROM task_runs r
             JOIN sessions s ON s.id=r.session_id
             WHERE r.message_id=?1 AND r.session_id=?2",
            params![&reply.id, SESSION],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(task, ("completed".into(), goal, reply.id.clone()));
    assert_eq!(reply.session_id, SESSION);
    assert_eq!(reply.parent_id.as_deref(), Some(source_id.as_str()));
    assert_ne!(reply.id, source_id);
    assert!(reply.content.contains("angelbot-file:restored-upload.json"));
    let results = reply.tool_results.as_deref().unwrap_or_default();
    // A grounded continuation may deliver from its preserved real write/read
    // receipts. Do not force another tool merely to increase the call count.
    if !results.is_empty() {
        verify_tool_outcomes(&reply).unwrap();
    }
    let owned_url =
        reqwest::Url::parse(&std::env::var("ANGELBOT_LIVE_BROWSER_URL").unwrap()).unwrap();
    for result in results {
        assert!(
            matches!(
                result.tool_name.as_str(),
                "read_file" | "list_workspace_items" | "verify_result" | "think"
            ) || [
                "browser_navigate",
                "browser_snapshot",
                "browser_wait_for",
                "browser_tabs",
                "browser_close",
                "browser_click"
            ]
            .iter()
            .any(|tool| result.tool_name
                == crate::agent::handlers::mcp_registry_tool_name(SERVER, tool)),
            "continuation must not repeat effects: {}",
            result.tool_name
        );
        if result.tool_name
            == crate::agent::handlers::mcp_registry_tool_name(SERVER, "browser_navigate")
        {
            let call = reply
                .tool_calls
                .as_deref()
                .unwrap()
                .iter()
                .find(|call| call.id == result.call_id)
                .unwrap();
            let args: serde_json::Value = serde_json::from_str(&call.arguments).unwrap();
            let target = reqwest::Url::parse(args["url"].as_str().unwrap()).unwrap();
            assert!(
                target.origin() == owned_url.origin()
                    && target.path().starts_with(owned_url.path())
                    && target.username().is_empty()
                    && target.password().is_none(),
                "continuation left its owned website"
            );
        }
    }
    let conn = state.db.lock().unwrap();
    let mut statement = conn
        .prepare("SELECT tool_name FROM agent_steps WHERE rowid>?1")
        .unwrap();
    for name in statement
        .query_map([before_step], |r| r.get::<_, String>(0))
        .unwrap()
    {
        let name = name.unwrap();
        assert!(
            results.iter().any(|r| r.tool_name == name),
            "unreported continuation step {name}"
        );
    }
    drop(statement);
    drop(conn);
    for (name, bytes) in originals {
        assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes);
    }
    let mcp_started = !state.mcp_manager.running_servers().is_empty();
    if mcp_started {
        crate::commands::mcp::stop_mcp_server(app.state(), SERVER.into()).unwrap();
    }
    println!(
        "LIVE_BROWSER_CONTINUED_REPORT {}",
        serde_json::json!({
            "sessionId":SESSION,"sourceMessageId":source_id,"replyMessageId":reply.id,
            "taskCompleted":true,"goalUnchanged":true,"calls":results.len(),"mcpStarted":mcp_started
        })
    );
}

/// A third process opens the imported database, not the source database or its caches.
pub(super) async fn verify_restored(app: &AppHandle, directory: &std::path::Path) {
    let task = std::env::var("ANGELBOT_LIVE_BROWSER_TASK").unwrap();
    assert!(matches!(task.as_str(), "daily-report" | "upload-report"));
    let upload = task == "upload-report";
    let output = if upload {
        "restored-upload.json"
    } else {
        "restore-check.md"
    };
    let attachment = if upload {
        "borrow-items.csv"
    } else {
        "borrow-record.csv"
    };
    let restored = directory.join("restored");
    let mut state = crate::tests::test_app_state();
    state.db = Arc::new(Mutex::new(
        crate::db::init(&restored.join("data.db")).unwrap(),
    ));
    crate::db::migrate(&state.db.lock().unwrap()).unwrap();
    crate::settings_cache::load_all(&state.db.lock().unwrap(), &state.cached_settings).unwrap();
    assert!(app.manage(state));
    let state = app.state::<AppState>();
    let opened =
        crate::commands::workspace::open_workspace_impl(&state, "personal".into()).unwrap();
    assert_eq!(opened.workspace.active_session_id, SESSION);
    assert_eq!(opened.session.id, SESSION);
    let history = get_messages(app.state(), SESSION.into()).unwrap();
    assert_eq!(
        serde_json::to_value(&history).unwrap(),
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(directory.join("restored-main-history.json")).unwrap()
        )
        .unwrap(),
        "the new process must retain the complete exported Main history"
    );
    let root = crate::commands::file::resolve_work_dir(&state, Some(SESSION)).unwrap();
    assert_eq!(root, restored.join("results").canonicalize().unwrap());
    assert!(!root.join(output).exists());
    let originals = ["request.json", "receipt.json", attachment].map(|name| {
        let source = std::fs::read(directory.join("results").join(name)).unwrap();
        assert_eq!(std::fs::read(root.join(name)).unwrap(), source);
        (name, source)
    });
    let before_step = {
        let conn = state.db.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM task_runs WHERE status='completed'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM task_runs WHERE status='running' OR resumable=1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        conn.query_row("SELECT coalesce(max(rowid),0) FROM agent_steps", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    // Explicit authorization for this new task, in the disposable app only;
    // this is not a test of portable credentials or restored permissions.
    set_agent_execution_permission_impl(&state, "full_access").unwrap();
    let reply = if upload {
        verify_restored_upload(app, &state, &root, before_step).await
    } else {
        tokio::time::timeout(Duration::from_secs(180), send_message_impl(
        &state, Some(app), SendMessageRequest {
            session_id: SESSION.into(), client_message_id: None, role: "user".into(),
            content: "这是备份恢复后的新任务，不是重试之前的任务。请读取日常结果区的 receipt.json 和 borrow-record.csv，核对编号及状态一致后，仅新建 restore-check.md。文件固定四行：标题：<实际标题>、日期：<实际日期>、草稿编号：<实际编号>、状态：<实际英文状态>，每项独占一行，不加 Markdown 装饰。写好后读回核验并给我 angelbot-file:restore-check.md 文件链接。不要修改原文件，不要打开网页、下载、执行命令、自动化或委派，不要重做旧任务。".into(),
            text_attachments: vec![], personality: None, preferences: None,
            thinking_effort: None, web_search_setup: None,
        }, None, ForegroundUserTurn::Append, None, None, None,
        )).await.expect("restored new task timed out").expect("restored Main failed")
    };
    assert_eq!(
        reply.task_run.as_ref().map(|run| run.status.as_str()),
        Some("completed"),
        "{}",
        reply.content
    );
    let results = reply.tool_results.as_deref().unwrap();
    assert!(!results.is_empty());
    verify_tool_outcomes(&reply).unwrap();
    for result in results {
        assert!(
            upload
                || matches!(
                    result.tool_name.as_str(),
                    "read_file" | "write_file" | "list_workspace_items"
                )
        );
    }
    let report = std::fs::read_to_string(root.join(output)).unwrap();
    if !upload {
        assert_eq!(
            report.trim().lines().collect::<Vec<_>>(),
            [
                "标题：已有设备借用",
                "日期：2026-10-23",
                "草稿编号：1",
                "状态：draft"
            ]
        );
    }
    assert!(reply.content.contains(&format!("angelbot-file:{output}")));
    let mut reads = std::collections::HashSet::new();
    let mut writes = 0;
    {
        let conn = state.db.lock().unwrap();
        let mut statement = conn.prepare("SELECT tool_name,tool_input,success,tool_output FROM agent_steps WHERE rowid>?1 ORDER BY rowid").unwrap();
        for row in statement
            .query_map([before_step], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .unwrap()
        {
            let (name, input, success, step_output) = row.unwrap();
            let input: serde_json::Value = serde_json::from_str(&input).unwrap();
            if !success {
                // verify_tool_outcomes already proved a separately bound,
                // successful correction with no dispatch of the rejected call.
                // A different durable failure cannot borrow that proof.
                assert!(results.iter().any(|result| !result.success
                    && result.tool_name == name
                    && result.output == step_output
                    && reply
                        .tool_calls
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .any(|call| call.id == result.call_id
                            && call.name == name
                            && serde_json::from_str::<serde_json::Value>(&call.arguments)
                                .ok()
                                .as_ref()
                                == Some(&input))));
                continue;
            }
            if upload && name.starts_with("mcp_") {
                assert!(UPLOAD_BROWSER_TOOLS.iter().any(
                    |tool| name == crate::agent::handlers::mcp_registry_tool_name(SERVER, tool)
                ));
                continue; // The helper also checks the exact selected file.
            }
            let path = root
                .join(input["path"].as_str().unwrap_or("."))
                .canonicalize()
                .unwrap();
            match name.as_str() {
                "write_file" => {
                    assert_eq!(path, root.join(output));
                    writes += 1;
                }
                "read_file" => {
                    let runtime = crate::browser_storage::owned_namespace(&conn).unwrap();
                    assert!(
                        ["receipt.json", attachment, output]
                            .iter()
                            .any(|file| path == root.join(file))
                            || (upload && path == root.join("request.json"))
                            || (upload
                                && runtime.as_ref().is_some_and(|dir| path.starts_with(dir)))
                    );
                    if path == root.join(output) {
                        assert_eq!(
                            writes, 1,
                            "the generated report must be read after its write"
                        );
                    }
                    reads.insert(path);
                }
                "list_workspace_items" => {
                    let runtime = crate::browser_storage::owned_namespace(&conn).unwrap();
                    assert!(
                        path == root
                            || (upload
                                && runtime.as_ref().is_some_and(|dir| path.starts_with(dir)))
                    );
                }
                _ => panic!("unexpected restored tool {name}"),
            }
        }
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM task_runs WHERE status='completed'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            3
        );
    }
    assert_eq!(writes, 1);
    for name in ["receipt.json", attachment, output] {
        assert!(reads.contains(&root.join(name)), "must read {name}");
    }
    if upload {
        assert!(reads.contains(&root.join("request.json")));
    }
    let reopened_history = get_messages(app.state(), SESSION.into()).unwrap();
    assert!(reopened_history.len() > history.len());
    for old in history {
        let retained = reopened_history.iter().find(|m| m.id == old.id).unwrap();
        assert_eq!(
            serde_json::to_value(retained).unwrap(),
            serde_json::to_value(old).unwrap()
        );
    }
    for (name, bytes) in originals {
        assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes);
        assert_eq!(
            std::fs::read(directory.join("results").join(name)).unwrap(),
            bytes
        );
    }
    assert_eq!(
        state.mcp_manager.get_status(SERVER).unwrap().status,
        "stopped"
    );
    println!(
        "LIVE_BROWSER_RESTORED_REPORT {}",
        serde_json::json!({
            "sessionId":SESSION,"sameMain":true,"oldHistoryUnchanged":true,
            "sourceFilesUnchanged":true,"successfulWrites":writes,"calls":results.len(),
            "report":report,"newTaskCompleted":true,"mcpStarted":upload,
            "mcpAuthorizationRenewed":upload,"thinkingEffort":"default-medium"
        })
    );
}

pub(super) async fn verify(
    app: &AppHandle,
    directory: &std::path::Path,
    receipt: &str,
    after_restart: bool,
) {
    let task = std::env::var("ANGELBOT_LIVE_BROWSER_TASK").unwrap_or_default();
    assert!(matches!(
        task.as_str(),
        "" | "form" | "form-report" | "daily-report" | "upload-report"
    ));
    let form = !task.is_empty();
    let project_report = task == "form-report";
    let daily_report = task == "daily-report";
    let upload_report = task == "upload-report";
    let daily_files = daily_report || upload_report;
    let file_report = project_report || daily_files;
    // The caller owns this loopback fixture. Never navigate to an inherited
    // user URL or use the ordinary browser profile in this acceptance.
    let url = reqwest::Url::parse(&std::env::var("ANGELBOT_LIVE_BROWSER_URL").unwrap()).unwrap();
    assert_eq!(url.scheme(), "http");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert!(url.port().is_some());
    assert!(url.username().is_empty() && url.password().is_none());
    let first = if daily_report {
        "已有设备借用".to_owned()
    } else {
        format!(
            "{}-{}",
            if form { "设备借用" } else { "核对清单" },
            &receipt[..8]
        )
    };
    let second = if form {
        "二楼会议室".to_string()
    } else {
        format!("准备资料-{}", &receipt[..8])
    };
    let workspace_path = directory.join(if daily_files { "results" } else { "workspace" });
    let mut source = serde_json::json!({"title":first,"category":"显示设备","location":second,"date":"2026-10-23",
        "notes":"周五会议使用，只保存草稿，不发送通知。","adapter":true});
    let upload_bytes = format!("物品,数量,说明\nHDMI 转接线,1,{first}\n").into_bytes();
    let attachment = serde_json::json!({"name":"borrow-items.csv","bytes":upload_bytes.len(),
        "sha256":hex::encode(Sha256::digest(&upload_bytes))});
    if upload_report {
        source["attachment_path"] = serde_json::json!(workspace_path.join("borrow-items.csv"));
    }
    let cli = std::path::PathBuf::from(std::env::var_os("ANGELBOT_LIVE_BROWSER_CLI").unwrap());
    assert!(cli.is_absolute() && cli.is_file());
    let args = serde_json::to_string(&vec![
        cli.to_string_lossy().to_string(),
        "--browser".into(),
        "msedge".into(),
        "--headless".into(),
        "--isolated".into(),
        "--no-webmcp".into(),
        "--output-dir".into(),
        directory
            .join(if daily_files {
                "results"
            } else {
                "browser-output"
            })
            .to_string_lossy()
            .to_string(),
        "--image-responses".into(),
        "omit".into(),
    ])
    .unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let status_url = url.join("api/state").unwrap();
    let before: serde_json::Value = client
        .get(status_url.clone())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let mutations = if form { 1 } else { 3 };
    assert_eq!(
        before["mutations"],
        if after_restart || daily_report {
            mutations
        } else {
            0
        }
    );
    let mut state = crate::tests::test_app_state();
    state.db = Arc::new(Mutex::new(
        crate::db::init(&directory.join("data.db")).unwrap(),
    ));
    {
        let conn = state.db.lock().unwrap();
        crate::db::migrate(&conn).unwrap();
        if !after_restart {
            conn.execute_batch("INSERT OR IGNORE INTO profile(id,updated_at) VALUES(1,1);")
                .unwrap();
        }
    }
    assert!(app.manage(state));
    let state = app.state::<AppState>();
    let args = if daily_files {
        // Same managed allocation and connection path as the UI preset.
        let output =
            crate::browser_storage::prepare_output(&state.db.lock().unwrap(), SERVER).unwrap();
        let mut args: Vec<String> = serde_json::from_str(&args).unwrap();
        let index = args.iter().position(|arg| arg == "--output-dir").unwrap() + 1;
        args[index] = crate::commands::file::display_work_dir(&output);
        serde_json::to_string(&args).unwrap()
    } else {
        args
    };
    if daily_files {
        let root = crate::commands::file::resolve_work_dir(&state, Some(SESSION)).unwrap();
        assert_eq!(root, workspace_path.canonicalize().unwrap());
        assert!(crate::commands::file::resolve_delegation_workspace_root(
            &state.db.lock().unwrap(),
            SESSION
        )
        .unwrap()
        .is_none());
        if !after_restart {
            std::fs::write(
                root.join("request.json"),
                serde_json::to_vec_pretty(&source).unwrap(),
            )
            .unwrap();
            if upload_report {
                std::fs::write(root.join("borrow-items.csv"), &upload_bytes).unwrap();
            }
        }
    }
    let (workspace_id, session) = if project_report {
        let opened = if after_restart {
            let project = crate::commands::workspace::get_workspaces(app.state())
                .unwrap()
                .into_iter()
                .find(|w| w.kind == "project")
                .expect("the saved project must exist");
            crate::commands::workspace::open_workspace(app.state(), project.id).unwrap()
        } else {
            std::fs::create_dir(&workspace_path).unwrap();
            std::fs::write(
                workspace_path.join("request.json"),
                serde_json::to_vec_pretty(&source).unwrap(),
            )
            .unwrap();
            crate::commands::workspace::create_project_workspace(
                app.state(),
                workspace_path.to_string_lossy().into_owned(),
                Some("Owned daily workflow".into()),
                None,
                None,
            )
            .unwrap()
        };
        assert_eq!(
            std::path::Path::new(opened.workspace.root_path.as_ref().unwrap())
                .canonicalize()
                .unwrap(),
            workspace_path.canonicalize().unwrap()
        );
        (opened.workspace.id, opened.session.id)
    } else {
        ("personal".into(), SESSION.into())
    };
    {
        let mut conn = state.db.lock().unwrap();
        crate::db::migrate(&conn).unwrap();
        if !after_restart {
            conn.execute(
                "INSERT INTO mcp_servers(id,name,command,args,env,enabled,created_at)
                 VALUES(?1,'Owned browser acceptance','node',?2,'',1,1)",
                params![SERVER, args],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO mcp_workspace_enablements(workspace_id,server_id,enabled_at)
                 VALUES(?1,?2,1)",
                params![workspace_id, SERVER],
            )
            .unwrap();
        } else {
            assert_eq!(
                conn.query_row(
                    "SELECT command,args FROM mcp_servers WHERE id=?1",
                    [SERVER],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                )
                .unwrap(),
                ("node".into(), args.clone())
            );
            assert_eq!(
                crate::commands::foreground_run_store::recover_interrupted_runs_in_conn(
                    &mut conn,
                    Utc::now().timestamp()
                )
                .unwrap(),
                0
            );
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM task_runs WHERE status='completed'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
        }
    }
    crate::settings_cache::load_all(&state.db.lock().unwrap(), &state.cached_settings).unwrap();
    if !after_restart {
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
    assert_eq!(
        state.mcp_manager.get_status(SERVER).unwrap().status,
        "stopped"
    );
    if !after_restart {
        // Settings discovery is explicit only once. A new Main process must
        // expose the durable catalog and connect on its first actual call.
        crate::commands::mcp::start_mcp_server(app.clone(), app.state(), SERVER.into()).unwrap();
        crate::commands::mcp::stop_mcp_server(app.state(), SERVER.into()).unwrap();
    }
    {
        let surface =
            crate::commands::foreground_tool_surface::ForegroundToolSurface::new(&state, Some(app))
                .build_for_session(session.clone())
                .unwrap();
        assert!(
            surface.contains(&crate::agent::handlers::mcp_registry_tool_name(
                SERVER,
                "browser_navigate"
            ))
        );
        if upload_report {
            assert!(
                surface.contains(&crate::agent::handlers::mcp_registry_tool_name(
                    SERVER,
                    "browser_file_upload"
                ))
            );
        }
        assert_eq!(
            state.mcp_manager.get_status(SERVER).unwrap().status,
            "stopped",
            "building schemas must not start a process"
        );
    }
    let receipt_fields = receipt_field_contract(upload_report);
    let content = if upload_report && !after_restart {
        format!("请读取日常结果区 request.json，在应用 {url} 按其中信息创建一份借用申请草稿，并通过页面的附件控件上传其中 attachment_path 指向的那个文件，仅保存草稿，不发送。若保存回执不明确，先从草稿箱核对，不要重复保存或上传。保存后重新打开已保存草稿，核对全部字段和附件文件名、实际字节数、服务端 SHA256，再新建 receipt.json。{receipt_fields}读回回执后用 angelbot-file:receipt.json 交付；不要创建项目。")
    } else if daily_report && !after_restart {
        format!("请先读取日常结果区的 request.json，再打开 {} 查看这份已有草稿，核对全部字段。下载页面提供的 CSV 并读回核对；将网页实际信息保存为 receipt.json。{receipt_fields}读回后交付这两个文件。不要创建、填写或改写网页记录。", url.join("drafts/1").unwrap())
    } else if file_report && after_restart {
        format!("请读取{} receipt.json，再重新打开应用 {url} 的草稿箱，只读核对报告与服务端编号、状态及所有字段是否一致。不要重新创建、填写、保存、上传、发送或改写任何文件；报告实际结果。", if daily_files { "日常结果区的" } else { "项目" })
    } else if file_report {
        format!("请读取当前项目 request.json，在应用 {url} 按其中信息创建一份借用申请草稿。只保存草稿，不发送或提交申请。如果保存回执不明确，先到草稿箱核对，不能重复保存。保存后从草稿箱实际核对全部字段，并在当前项目新建 receipt.json。{receipt_fields}读取 receipt.json 验证后向我报告实际结果和草稿编号。")
    } else if form && after_restart {
        format!("请重新打开应用 {url} ，只读核对之前保存的“{first}”申请草稿，报告实际读到的编号、状态及所有字段。不要重新创建、填写、保存或发送，不依赖旧回执猜测结果。")
    } else if form {
        format!("请在应用 {url} 创建一份借用申请草稿：申请标题“{first}”，借用类别“显示设备”，使用地点“{second}”，使用日期“2026-10-23”，用途说明“周五会议使用，只保存草稿，不发送通知。”，需要 HDMI 转接线。仅保存草稿，不发送或提交申请。如果保存回执不明确，请先到草稿箱查看已保存记录，不要盲目重试导致重复。完成后核对全部字段并告诉我实际结果及草稿编号。")
    } else if after_restart {
        format!("请重新打开这个网页 {url} ，只读核对之前的两个事项是否保留。不要重新添加、勾选或修改任何内容，不依赖旧回执猜测状态。请报告实际读到的事项名称和总数、已完成及未完成数。")
    } else {
        format!("请打开 {url} ，在网页中新建两个事项“{first}”和“{second}”，仅把第一个标为已完成。然后从网页实际核对并告诉我事项名称和总数、已完成及未完成数。")
    } + if upload_report {
        "只用日常结果区的文件读取／写入工具和这个已启用的浏览器 MCP 的导航、快照、输入、点击及选择文件。文件仅限 request.json、borrow-items.csv（只读）和 receipt.json（仅新建一次）；只允许上传 borrow-items.csv 到这个隔离网页。禁止运行代码、脚本、终端、访问其他文件、网站、应用、自动化或派发子代理。"
    } else if daily_report {
        "只用日常结果区的文件工具及这个已启用的浏览器 MCP 的导航、快照、输入和点击；request.json 和 borrow-record.csv 只读，只新建一次 receipt.json。禁止运行代码、脚本、终端、访问其他文件、网站、应用、自动化或派发子代理。"
    } else if file_report {
        "只用项目文件读取／写入工具及这个已启用的浏览器 MCP 的导航、快照、输入和点击；文件仅限 request.json（只读）和 receipt.json（回执）。禁止运行代码、脚本、终端、访问其他文件、网站、应用、自动化或派发子代理。"
    } else {
        "只使用这个已启用的浏览器 MCP，按页面的导航、快照、输入和点击操作；禁止运行代码、脚本、终端、文件、其他网站、应用或自动化。"
    } + if daily_report {
        if after_restart {
            "这里是 AngelBot 日常，不新建项目；上文的文件均指当前日常结果区。另外只读核对已保存的 borrow-record.csv，不能再次下载。用 angelbot-file:receipt.json 和 angelbot-file:borrow-record.csv 链接交付实际文件。"
        } else {
            "这里是 AngelBot 日常，不新建项目；上文的文件均指当前日常结果区。核对保存的草稿后，点击一次页面上的“下载草稿 CSV”，把下载文件保留为正式结果区的 borrow-record.csv 并读回，不要用 write_file 代替下载，也不要重复下载。可只读浏览器返回的本轮临时快照和下载，用现有文件整理工具复制这一个下载文件。用 angelbot-file:receipt.json 和 angelbot-file:borrow-record.csv 链接交付实际文件。"
        }
    } else if upload_report {
        "核对后用 angelbot-file:receipt.json 链接交付实际回执。重开阶段只能读取现有文件和网页，不能重复上传或改写。"
    } else {
        ""
    };
    let reply = tokio::time::timeout(
        Duration::from_secs(240),
        send_message_impl(
            &state,
            Some(app),
            SendMessageRequest {
                session_id: session.clone(),
                client_message_id: None,
                role: "user".into(),
                content,
                text_attachments: vec![],
                personality: None,
                preferences: None,
                // Daily delivery uses the product's default (medium), not the
                // older low-effort browser stress journey. Failures at low
                // effort remain evidence, not silently counted as a pass.
                thinking_effort: if daily_files {
                    None
                } else {
                    Some("low".into())
                },
                web_search_setup: None,
            },
            None,
            ForegroundUserTurn::Append,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("bounded browser task timed out")
    .expect("Main browser task failed");
    let status = reply.task_run.as_ref().map(|run| run.status.as_str());
    if status != Some("completed") {
        println!(
            "LIVE_BROWSER_INCOMPLETE status={}; Main={}",
            status.unwrap_or("missing-task-receipt"),
            reply.content
        );
        for result in reply.tool_results.as_deref().unwrap_or_default() {
            println!(
                "LIVE_BROWSER_CALL {} success={} output={}",
                result.tool_name,
                result.success,
                result.output.chars().take(240).collect::<String>()
            );
        }
    }
    assert_eq!(status, Some("completed"), "{}", reply.content);
    let results = reply.tool_results.as_ref().unwrap();
    assert!(
        !results.is_empty(),
        "a text-only answer is not browser acceptance"
    );
    verify_tool_outcomes(&reply).unwrap();
    let mut allowed = if after_restart {
        let mut read_tools = vec![
            "browser_navigate",
            "browser_snapshot",
            "browser_wait_for",
            "browser_tabs",
            "browser_close",
        ];
        if form {
            read_tools.push("browser_click");
        }
        read_tools
    } else {
        vec![
            "browser_navigate",
            "browser_snapshot",
            "browser_wait_for",
            "browser_tabs",
            "browser_close",
            "browser_click",
            "browser_type",
            "browser_press_key",
            "browser_fill_form",
            "browser_select_option",
        ]
    };
    if upload_report && !after_restart {
        allowed.push("browser_file_upload");
    }
    for result in results {
        if file_report
            && matches!(
                result.tool_name.as_str(),
                "read_file" | "write_file" | "list_workspace_items" | "organize_workspace_item"
            )
        {
            if result.tool_name == "organize_workspace_item" {
                assert!(daily_report && !after_restart);
            }
            assert!(
                !after_restart
                    || !matches!(
                        result.tool_name.as_str(),
                        "write_file" | "organize_workspace_item"
                    ),
                "reopen must be read-only"
            );
            continue;
        }
        if form
            && after_restart
            && result.tool_name
                == crate::agent::handlers::mcp_registry_tool_name(SERVER, "browser_click")
        {
            // Following a link is not a business mutation. Restrict these
            // receipts to the actual read pages, then compare durable state.
            let pages = result
                .output
                .lines()
                .filter_map(|line| line.trim().strip_prefix("- Page URL: "))
                .collect::<Vec<_>>();
            let read_pages = [
                url.to_string(),
                url.join("drafts").unwrap().to_string(),
                url.join("drafts/1").unwrap().to_string(),
            ];
            assert!(
                !pages.is_empty()
                    && pages
                        .iter()
                        .all(|page| read_pages.iter().any(|allowed| allowed == page)),
                "reopen clicks must only navigate owned read pages: {}",
                result.output
            );
        }
        assert!(
            allowed.iter().any(|name| result.tool_name
                == crate::agent::handlers::mcp_registry_tool_name(SERVER, name)),
            "unexpected tool {}",
            result.tool_name
        );
    }
    if file_report {
        assert!(
            reply.content.contains("receipt.json"),
            "Main must identify the actual local delivery"
        );
        if daily_files {
            assert!(reply.content.contains("angelbot-file:receipt.json"));
            if daily_report {
                assert!(reply.content.contains("angelbot-file:borrow-record.csv"));
            }
        }
    } else {
        assert!(reply.content.contains(&first) && reply.content.contains(&second));
    }
    let after: serde_json::Value = client
        .get(status_url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    if form {
        assert!(results.iter().any(|result|result.output.contains("已保存的草稿")
            && result.output.contains(&first) && result.output.contains(&second)
            && result.output.contains("2026-10-23")),
            "completion requires a fresh saved-record receipt, not the filled form or dispatch alone");
        let drafts = after["drafts"].as_array().unwrap();
        assert_eq!(
            drafts.len(),
            1,
            "a lost receipt must not cause duplicate drafts"
        );
        let mut expected_draft = serde_json::json!({
            "id":1,"title":first,"category":"equipment","location":second,"date":"2026-10-23",
            "notes":"周五会议使用，只保存草稿，不发送通知。","adapter":true,
        });
        if upload_report {
            expected_draft["attachment"] = attachment.clone();
        }
        assert_eq!(drafts[0], expected_draft);
        assert_eq!(
            after["saveAttempts"], 1,
            "never blindly repeat an uncertain save"
        );
        assert_eq!(after["receiptLost"], true);
        if !file_report {
            assert!(reply.content.contains("2026-10-23"));
        }
        if file_report {
            let data = std::fs::read(workspace_path.join("receipt.json"))
                .expect("Main must create the local receipt");
            let local: serde_json::Value =
                serde_json::from_slice(&data).expect("receipt must be JSON");
            let mut expected_local = serde_json::json!({"draft_id":1,"status":"draft","title":first,
                "category":"equipment","location":second,"date":"2026-10-23",
                "notes":"周五会议使用，只保存草稿，不发送通知。","adapter":true});
            if upload_report {
                expected_local["attachment"] = attachment.clone();
            }
            assert_eq!(local, expected_local);
            assert_eq!(
                std::fs::read(workspace_path.join("request.json")).unwrap(),
                serde_json::to_vec_pretty(&source).unwrap(),
                "source must remain byte-for-byte unchanged"
            );
            assert!(
                results.iter().any(|r| r.tool_name == "read_file"),
                "must read project data, not infer it from chat"
            );
            if upload_report {
                assert_eq!(
                    std::fs::read(workspace_path.join("borrow-items.csv")).unwrap(),
                    upload_bytes
                );
                assert_eq!(after["uploadAttempts"], 1);
                assert_eq!(after["downloadAttempts"], 0);
                assert!(results.iter().any(|r| r.output.contains("已保存的草稿")
                    && r.output.contains(attachment["sha256"].as_str().unwrap())),
                    "Main must inspect the saved attachment receipt, not infer it from the selected file");
            }
            if !after_restart {
                let saved = results
                    .iter()
                    .position(|r| r.output.contains("已保存的草稿") && r.output.contains(&first))
                    .unwrap();
                let write = results
                    .iter()
                    .position(|r| r.tool_name == "write_file")
                    .expect("report needs a real file write");
                assert!(
                    saved < write,
                    "local delivery must follow verification of the saved webpage record"
                );
                assert!(
                    results[write + 1..]
                        .iter()
                        .any(|r| r.tool_name == "read_file"),
                    "must read back the materialized report"
                );
            }
            let conn = state.db.lock().unwrap();
            let runtime = if daily_files {
                crate::browser_storage::configured_output(&conn, SERVER, &args).unwrap()
            } else {
                None
            };
            let mut statement = conn.prepare("SELECT tool_name,tool_input FROM agent_steps WHERE session_id=?1 AND success=1 AND tool_name IN ('read_file','write_file','list_workspace_items') ORDER BY rowid").unwrap();
            let files = statement
                .query_map([&session], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .unwrap();
            for file in files {
                let (name, arguments) = file.unwrap();
                let arguments: serde_json::Value = serde_json::from_str(&arguments).unwrap();
                let path = arguments["path"].as_str().unwrap_or(".");
                let target = workspace_path.join(path).canonicalize().unwrap();
                assert!(
                    target == workspace_path.join("receipt.json").canonicalize().unwrap()
                        || (name == "read_file"
                            && target
                                == workspace_path.join("request.json").canonicalize().unwrap())
                        || (daily_report
                            && name == "read_file"
                            && target
                                == workspace_path
                                    .join("borrow-record.csv")
                                    .canonicalize()
                                    .unwrap())
                        || (upload_report
                            && name == "read_file"
                            && target
                                == workspace_path
                                    .join("borrow-items.csv")
                                    .canonicalize()
                                    .unwrap())
                        || (name == "list_workspace_items"
                            && target == workspace_path.canonicalize().unwrap())
                        || (matches!(name.as_str(), "read_file" | "list_workspace_items")
                            && runtime
                                .as_ref()
                                .is_some_and(|runtime| target.starts_with(runtime))),
                    "unexpected file scope"
                );
            }
            let writes: i64 = conn.query_row("SELECT count(*) FROM agent_steps WHERE session_id=?1 AND tool_name='write_file' AND success=1", [&session], |r| r.get(0)).unwrap();
            assert_eq!(writes, 1, "reopen must not rewrite the receipt");
            if upload_report {
                let name =
                    crate::agent::handlers::mcp_registry_tool_name(SERVER, "browser_file_upload");
                let mut uploads = conn.prepare("SELECT tool_input FROM agent_steps WHERE session_id=?1 AND tool_name=?2 AND success=1 ORDER BY rowid").unwrap();
                let inputs = uploads
                    .query_map(params![session, name], |row| row.get::<_, String>(0))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                assert_eq!(
                    inputs.len(),
                    1,
                    "must use the file chooser exactly once, without replay after reopening"
                );
                let input: serde_json::Value = serde_json::from_str(&inputs[0]).unwrap();
                let paths = input["paths"].as_array().unwrap();
                assert_eq!(paths.len(), 1);
                let selected = std::path::Path::new(paths[0].as_str().unwrap());
                assert!(selected.is_absolute());
                assert_eq!(
                    selected.canonicalize().unwrap(),
                    workspace_path
                        .join("borrow-items.csv")
                        .canonicalize()
                        .unwrap()
                );
            }
            if daily_report {
                assert_eq!(after["downloadAttempts"], 1);
                let runtime = crate::browser_storage::configured_output(&conn, SERVER, &args)
                    .unwrap()
                    .unwrap();
                let mut copies = conn.prepare("SELECT tool_input FROM agent_steps WHERE session_id=?1 AND tool_name='organize_workspace_item' AND success=1").unwrap();
                let copies = copies
                    .query_map([&session], |row| row.get::<_, String>(0))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                assert_eq!(
                    copies.len(),
                    1,
                    "download publication is unique and never replayed"
                );
                let copy: serde_json::Value = serde_json::from_str(&copies[0]).unwrap();
                assert_eq!(copy["operation"], "copy");
                assert_eq!(
                    workspace_path
                        .join(copy["source"].as_str().unwrap())
                        .canonicalize()
                        .unwrap(),
                    runtime.join("borrow-record.csv").canonicalize().unwrap()
                );
                assert_eq!(
                    workspace_path
                        .join(copy["destination"].as_str().unwrap())
                        .canonicalize()
                        .unwrap(),
                    workspace_path
                        .join("borrow-record.csv")
                        .canonicalize()
                        .unwrap()
                );
                assert_eq!(
                    std::fs::read(workspace_path.join("borrow-record.csv")).unwrap(),
                    b"id,status\n1,draft\n"
                );
                assert_eq!(
                    conn.query_row(
                        "SELECT count(*) FROM projects WHERE kind='project'",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                    0
                );
            }
            drop(statement);
            drop(conn);
            if daily_files && after_restart {
                let filename = if upload_report {
                    "borrow-items.csv"
                } else {
                    "borrow-record.csv"
                };
                prepare_restored_profile(&state, directory, filename);
            }
            println!(
                "LIVE_BROWSER_PROJECT_REPORT {}",
                serde_json::json!({
                "phase":if after_restart {"reopen-read-only"} else {"source-to-web-to-report"},
                "receipt":local,"sourceUnchanged":true,"successfulWrites":writes,"sessionId":session,
                "calls":results.len(),"scope":if daily_files {"owned Daily results and loopback webpage"} else {"owned temporary project and loopback webpage"},
                "backupRestoreVerified": daily_files && after_restart,
                "thinkingEffort": if daily_files { "default-medium" } else { "low" }})
            );
        }
    } else {
        let items = after["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], first);
        assert_eq!(items[0]["done"], true);
        assert_eq!(items[1]["name"], second);
        assert_eq!(items[1]["done"], false);
    }
    assert_eq!(
        after["mutations"], mutations,
        "restart must not replay webpage changes"
    );
    if after_restart {
        assert_eq!(before, after);
    }
    state.mcp_manager.stop(SERVER).unwrap();
    assert_eq!(
        state.mcp_manager.get_status(SERVER).unwrap().status,
        "stopped"
    );
    if form {
        println!("LIVE_BROWSER_FORM verified: phase={}, drafts=1 saveAttempts=1 mutations=1 receiptLost=true calls={}; Main={}",
            if after_restart { "reopen-read-only" } else { "write-and-reconcile" }, results.len(), reply.content);
    } else {
        println!("LIVE_BROWSER verified: phase={}, total=2 completed=1 active=1 mutations=3 calls={}; Main={}",
        if after_restart { "reopen-read-only" } else { "write-and-read" }, results.len(), reply.content);
    }
}
