//! Isolated real Main/MCP side effect, lost process/receipt, then read-only recovery.
use super::*;
use tauri::Manager;

const SERVER: &str = "live-interrupt";
const SESSION: &str = "personal-main";

const SCRIPT: &str = r#"
const fs = require('fs');
const path = require('path');
const readline = require('readline');
const [root, receipt, provider, model, failureMode] = process.argv.slice(2);
const effects = path.join(root, 'mcp-effects.jsonl');
const checkpoint = path.join(root, 'mcp-checkpoint.json');
const send = (id, result) => process.stdout.write(JSON.stringify({jsonrpc:'2.0', id, result})+'\n');
readline.createInterface({input:process.stdin}).on('line', line => {
  const q = JSON.parse(line);
  if(q.method === 'initialize') return send(q.id, {protocolVersion:'2024-11-05', capabilities:{tools:{}}, serverInfo:{name:'owned-interruption-fixture',version:'1'}});
  if(q.method === 'notifications/initialized') return;
  if(q.method === 'tools/list') return send(q.id, {tools:[
    {name:'record_checkpoint',description:'Synthetic operation: append one receipt. Only run when explicitly requested; never retry an interrupted call.',inputSchema:{type:'object',properties:{receipt:{type:'string',enum:[receipt]}},required:['receipt'],additionalProperties:false}},
    {name:'read_status',description:'Read-only inspection of the synthetic receipt count; never modifies it.',inputSchema:{type:'object',properties:{},additionalProperties:false}}
  ]});
  if(q.method === 'tools/call' && q.params.name === 'record_checkpoint') {
    if(q.params.arguments.receipt !== receipt) return send(q.id, {isError:true,content:[{type:'text',text:'invalid synthetic receipt'}]});
    const fd = fs.openSync(effects, 'a');
    fs.writeSync(fd, JSON.stringify({receipt, pid:process.pid})+'\n'); fs.fsyncSync(fd); fs.closeSync(fd);
    fs.writeFileSync(checkpoint+'.pending', JSON.stringify({receipt, process_id:process.ppid, mcp_pid:process.pid, crash_stage:'mcp_after_effect', provider, model}));
    fs.renameSync(checkpoint+'.pending', checkpoint);
    if(failureMode === 'model-fault') return send(q.id, {content:[{type:'text',text:JSON.stringify({receipt,count:1})}]});
    if(failureMode === 'disconnect') return process.exit(0); // Effect committed, no tools/call receipt.
    return; // Deliberately no acknowledgement: parent terminates the host here.
  }
  if(q.method === 'tools/call' && q.params.name === 'read_status') {
    const rows = fs.existsSync(effects) ? fs.readFileSync(effects,'utf8').trim().split('\n').filter(Boolean) : [];
    return send(q.id, {content:[{type:'text',text:JSON.stringify({receipt,count:rows.length})}]});
  }
  if(q.id !== undefined) send(q.id, {});
});
"#;

pub(super) async fn verify(
    app: &AppHandle,
    config: &crate::llm::ProviderConfig,
    directory: &std::path::Path,
    receipt: &str,
    after_restart: bool,
) {
    let disconnect =
        std::env::var("ANGELBOT_LIVE_MODEL_JOURNEY").as_deref() == Ok("mcp-disconnect");
    let model_fault = std::env::var("ANGELBOT_LIVE_MODEL_JOURNEY").as_deref() == Ok("model-fault");
    let stopped_model = model_fault
        && std::env::var("ANGELBOT_LIVE_MODEL_FAILURE").as_deref() == Ok("stop-pending");
    assert!(!(disconnect || model_fault) || !after_restart);
    let mut state = crate::tests::test_app_state();
    state.db = Arc::new(Mutex::new(
        crate::db::init(&directory.join("data.db")).unwrap(),
    ));
    let script = directory.join("mcp-interruption.cjs");
    let args = serde_json::to_string(&vec![
        script.to_string_lossy().to_string(),
        directory.to_string_lossy().to_string(),
        receipt.to_string(),
        config.provider.clone(),
        config.model.clone(),
        if model_fault {
            "model-fault"
        } else if disconnect {
            "disconnect"
        } else {
            "host-loss"
        }
        .into(),
    ])
    .unwrap();
    {
        let conn = state.db.lock().unwrap();
        crate::db::migrate(&conn).unwrap();
        if !after_restart {
            conn.execute_batch("INSERT OR IGNORE INTO profile(id,updated_at) VALUES(1,1);")
                .unwrap();
            conn.execute(
                "INSERT INTO mcp_servers(id,name,command,args,env,enabled,created_at)
                VALUES(?1,'Owned interruption fixture','node',?2,'',1,1)",
                params![SERVER, args],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO mcp_workspace_enablements(workspace_id,server_id,enabled_at)
                VALUES('personal',?1,1)",
                [SERVER],
            )
            .unwrap();
            std::fs::write(&script, SCRIPT).unwrap();
        } else {
            let saved: (String, String) = conn
                .query_row(
                    "SELECT command,args FROM mcp_servers WHERE id=?1",
                    [SERVER],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(
                saved,
                ("node".into(), args),
                "restart must not repair the MCP configuration"
            );
        }
    }
    crate::settings_cache::load_all(&state.db.lock().unwrap(), &state.cached_settings).unwrap();
    if !after_restart {
        set_agent_execution_permission_impl(&state, "full_access").unwrap();
        crate::commands::memory::resolve_reply_memory_controls(
            &state.db.lock().unwrap(),
            SESSION,
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
    let interrupted_id = if after_restart {
        let checkpoint: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("mcp-checkpoint.json")).unwrap())
                .unwrap();
        assert_eq!(checkpoint["receipt"], receipt);
        assert_eq!(checkpoint["provider"], config.provider);
        assert_eq!(checkpoint["model"], config.model);
        assert_ne!(
            checkpoint["process_id"].as_u64().unwrap(),
            u64::from(std::process::id())
        );
        assert_eq!(effect_count(directory, receipt), 1);
        let mut conn = state.db.lock().unwrap();
        let id: String = conn
            .query_row(
                "SELECT r.id FROM task_runs r JOIN messages m ON m.id=r.message_id
            WHERE r.session_id=?1 AND r.status='running' AND m.is_provisional=1",
                [SESSION],
                |row| row.get(0),
            )
            .unwrap();
        let reserved: i64 = conn
            .query_row(
                "SELECT count(*) FROM agent_run_events
            WHERE run_id=?1 AND event_type='ExternalToolDispatchReserved'",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reserved, 1);
        assert_eq!(
            conn.query_row("SELECT count(*) FROM agent_steps", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "host loss must precede receipt/transcript completion"
        );
        assert_eq!(
            crate::commands::foreground_run_store::recover_interrupted_runs_in_conn(
                &mut conn,
                Utc::now().timestamp(),
            )
            .unwrap(),
            1
        );
        assert_eq!(
            crate::commands::foreground_run_store::recover_interrupted_runs_in_conn(
                &mut conn,
                Utc::now().timestamp(),
            )
            .unwrap(),
            0
        );
        Some(id)
    } else {
        None
    };
    assert!(app.manage(state));
    let state = app.state::<AppState>();
    if let Some(id) = interrupted_id.as_ref() {
        let messages = ForegroundRunStore::new(state.db.clone())
            .project_session(SESSION)
            .unwrap();
        let recovered = messages.iter().find(|message| &message.id == id).unwrap();
        assert!(recovered.content.contains("不会自动重试"));
        assert_eq!(
            recovered.task_run.as_ref().unwrap().status,
            "needs_attention"
        );
        assert!(!recovered.task_run.as_ref().unwrap().resumable);
        assert!(continue_agent_task_impl(
            &state,
            Some(app),
            ContinueAgentTaskRequest {
                session_id: SESSION.into(),
                message_id: id.clone(),
            }
        )
        .await
        .is_err());
        assert_eq!(effect_count(directory, receipt), 1);
    }
    // Start only this explicitly configured synthetic service, through production commands.
    crate::commands::mcp::start_mcp_server(app.clone(), app.state(), SERVER.into()).unwrap();
    assert_eq!(state.mcp_manager.list_tools(SERVER).unwrap().len(), 2);
    let read_request = "请只使用已启用 MCP 服务的 read_status 读取实际状态，禁止 record_checkpoint、写入、重试旧操作、文件、命令、其他应用或自动化。根据真实读数在最终答复写出 observed_count=<实际读数>，并说明此前中断不等于操作失败。";
    let content = if after_restart {
        read_request.to_string()
    } else {
        format!("这是自有合成 MCP 验收，请只调用已启用服务的 record_checkpoint 一次，参数 receipt 为 {receipt}。不要预先检查、重试、调用其他工具、操作文件／应用或创建自动化。该操作可能没有完成回执；若未确认结果，不得重复执行或宣称完成。")
    };
    let mut reply = if model_fault {
        super::model_failure::send_after_known_effect(app, &state, request(content), receipt).await
    } else {
        send(app, &state, content).await
    };
    let failed_status = if stopped_model {
        "stopped"
    } else if model_fault {
        "provider_unavailable"
    } else {
        "needs_attention"
    };
    let disconnected_reply_id = if disconnect || model_fault {
        let failed = reply.task_run.as_ref().unwrap();
        assert_eq!(failed.status, failed_status);
        assert!(!failed.resumable);
        let results = reply.tool_results.as_ref().unwrap();
        assert_eq!(
            results.len(),
            if model_fault && !stopped_model { 2 } else { 1 },
            "a failure must not dispatch another external effect"
        );
        assert_eq!(
            results[0].tool_name,
            crate::agent::handlers::mcp_registry_tool_name(SERVER, "record_checkpoint")
        );
        assert_eq!(results[0].success, model_fault);
        assert!(!results[0].confirmation_required);
        if model_fault && !stopped_model {
            assert_eq!(results[1].tool_name, "agent_loop");
            assert!(!results[1].success);
        }
        assert_eq!(effect_count(directory, receipt), 1);
        assert_eq!(
            state.mcp_manager.get_status(SERVER).unwrap().status,
            if model_fault { "running" } else { "stopped" }
        );
        let id = reply.id.clone();
        assert!(
            continue_agent_task_impl(
                &state,
                Some(app),
                ContinueAgentTaskRequest {
                    session_id: SESSION.into(),
                    message_id: id.clone(),
                }
            )
            .await
            .is_err(),
            "continue cannot repeat a write whose result is unknown"
        );
        assert_eq!(effect_count(directory, receipt), 1);
        reply = send(app, &state, read_request.into()).await;
        Some(id)
    } else {
        None
    };
    assert!(
        after_restart || disconnect || model_fault,
        "the host should be forcibly stopped before any write acknowledgement"
    );
    let results = reply.tool_results.as_ref().unwrap();
    let read_name = crate::agent::handlers::mcp_registry_tool_name(SERVER, "read_status");
    assert_eq!(
        results.len(),
        1,
        "recovery must use exactly one fresh read, not replay the write"
    );
    assert_eq!(results[0].tool_name, read_name);
    assert!(results[0].success);
    let read: serde_json::Value = serde_json::from_str(&results[0].output).unwrap();
    assert_eq!(read["receipt"], receipt);
    assert_eq!(read["count"], 1);
    assert!(reply.content.contains("observed_count=1"));
    assert_eq!(reply.task_run.as_ref().unwrap().status, "completed");
    assert_eq!(effect_count(directory, receipt), 1);
    if let Some(id) = disconnected_reply_id {
        let old = ForegroundRunStore::new(state.db.clone())
            .project_session(SESSION)
            .unwrap()
            .into_iter()
            .find(|message| message.id == id)
            .unwrap();
        assert_eq!(old.task_run.as_ref().unwrap().status, failed_status);
        assert!(!old.task_run.as_ref().unwrap().resumable);
        let next = send(app, &state, "这是新的独立小任务，不需要任何工具：计算 17 乘以 6，只答 next_result=<结果>。不要重试之前的写入。".into()).await;
        assert!(
            next.task_run.is_none(),
            "a text-only reply has no UI task card"
        );
        assert_eq!(
            state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT status FROM task_runs WHERE session_id=?1 AND message_id=?2",
                    params![SESSION, next.id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "completed"
        );
        assert!(next.content.contains("next_result=102"));
        assert!(next.tool_results.as_deref().unwrap_or_default().is_empty());
        assert_eq!(effect_count(directory, receipt), 1);
        assert_eq!(
            state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM task_runs WHERE status='running'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
    state.mcp_manager.stop(SERVER).unwrap();
    assert_eq!(
        state.mcp_manager.get_status(SERVER).unwrap().status,
        "stopped"
    );
    assert_eq!(
        state
            .db
            .lock()
            .unwrap()
            .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    if stopped_model {
        println!("LIVE_MODEL_STOP_RECOVERY verified: real Luna known effect, injected pending inference stopped through native command, fresh normal Main read and next task, one external effect; permission=full_access");
    } else if model_fault {
        println!("LIVE_MODEL_FAILURE verified: real Luna known effect, provider-boundary failure, safe saved reply/events, fresh normal Main read and next task, one external effect; permission=full_access");
    } else if disconnect {
        println!("LIVE_MCP_DISCONNECT verified: one external effect, MCP exit without receipt, no replay or manual reconnect, fresh read and next Main task; permission=full_access");
    } else {
        println!("LIVE_MCP_RECOVERY verified: one external effect, forced host loss, durable no-replay gate, fresh read and Main delivery; permission=full_access");
    }
}

async fn send(app: &AppHandle, state: &AppState, content: String) -> Message {
    tokio::time::timeout(
        Duration::from_secs(180),
        send_message_impl(
            state,
            Some(app),
            request(content),
            None,
            ForegroundUserTurn::Append,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("bounded MCP Main task timed out")
    .expect("MCP Main task must execute")
}

fn request(content: String) -> SendMessageRequest {
    SendMessageRequest {
        session_id: SESSION.into(),
        client_message_id: None,
        role: "user".into(),
        content,
        text_attachments: vec![],
        personality: None,
        preferences: None,
        thinking_effort: Some("low".into()),
        web_search_setup: None,
    }
}

fn effect_count(directory: &std::path::Path, receipt: &str) -> usize {
    let content = std::fs::read_to_string(directory.join("mcp-effects.jsonl")).unwrap();
    let rows: Vec<serde_json::Value> = content
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(rows.iter().all(|row| row["receipt"] == receipt));
    rows.len()
}
