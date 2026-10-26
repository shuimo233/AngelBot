//! Explicit live acceptance, never part of offline verification. Only the
//! configured model/credential is borrowed; files, history and settings are
//! disposable. No application startup or user automation scheduler runs here.
//! The delegation journey starts only its temporary database's production pump.

use super::*;

#[path = "message_live_tests/browser_daily.rs"]
mod browser_daily;
#[path = "message_live_tests/delegation.rs"]
mod delegation;
#[path = "message_live_tests/inference_probe.rs"]
mod inference_probe;
#[path = "message_live_tests/mcp_interrupt.rs"]
mod mcp_interrupt;
#[path = "message_live_tests/model_failure.rs"]
mod model_failure;
#[path = "message_live_tests/native_context.rs"]
mod native_context;

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ModelProbeStatus {
    Available,
    QuotaExceeded,
    Unavailable,
    TimedOut,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelProbeReport {
    status: ModelProbeStatus,
    request_count: u8,
    tool_count: u8,
}

fn probe_report(status: ModelProbeStatus, request_count: u8) -> ModelProbeReport {
    ModelProbeReport {
        status,
        request_count,
        tool_count: 0,
    }
}

fn headless_credential_host() -> Result<tauri::App<tauri::Wry>, &'static str> {
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    tauri::Builder::default()
        .any_thread()
        .plugin(tauri_plugin_keyring_store::init())
        .build(context)
        .map_err(|_| "headless credential host must initialize")
}

async fn probe_model_readiness(
    provider: &dyn crate::llm::LlmProvider,
    budget: Duration,
) -> ModelProbeReport {
    let messages = [crate::llm::Message {
        role: "user".into(),
        content: "Reply exactly OK.".into(),
        tool_calls: None,
        tool_call_id: None,
        tool_images: Vec::new(),
        protocol_state: None,
    }];
    let status =
        match tokio::time::timeout(budget, provider.chat_classified(&messages, None, None)).await {
            Err(_) => ModelProbeStatus::TimedOut,
            Ok(Err(failure)) => match failure.kind {
                crate::llm::LlmFailureKind::QuotaExceeded => ModelProbeStatus::QuotaExceeded,
                crate::llm::LlmFailureKind::Unclassified => ModelProbeStatus::Unavailable,
            },
            Ok(Ok(response)) => {
                let (response, protocol) = response.into_parts();
                if protocol.is_some_and(|state| state.validate().is_err()) {
                    ModelProbeStatus::Unavailable
                } else if matches!(response, crate::llm::LlmResponse::Text {
                text, stop_reason: crate::llm::StopReason::EndTurn,
            } if text.trim() == "OK")
                {
                    ModelProbeStatus::Available
                } else {
                    ModelProbeStatus::Unavailable
                }
            }
        };
    probe_report(status, 1)
}

fn run_model_readiness_probe() -> ModelProbeReport {
    let Ok(app) = headless_credential_host() else {
        return probe_report(ModelProbeStatus::Unavailable, 0);
    };
    let Ok(config) =
        crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
    else {
        return probe_report(ModelProbeStatus::Unavailable, 0);
    };
    if config.auth_mode != crate::llm::ModelAuthMode::ChatgptPlan || !config.model.contains("luna")
    {
        return probe_report(ModelProbeStatus::Unavailable, 0);
    }
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        return probe_report(ModelProbeStatus::Unavailable, 0);
    };
    runtime.block_on(async {
        let Ok(provider) = crate::llm::create_provider_for_app(&config, Some(app.handle())) else {
            return probe_report(ModelProbeStatus::Unavailable, 0);
        };
        probe_model_readiness(provider.as_ref(), Duration::from_secs(45)).await
    })
}

#[tokio::test]
async fn model_probe_is_single_call_without_tools_or_private_output() {
    use crate::llm::{LlmFailure, LlmFailureKind, LlmProvider, LlmResponse, StopReason};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Probe(u8, AtomicUsize);
    #[async_trait::async_trait]
    impl LlmProvider for Probe {
        fn name(&self) -> &str {
            "owned-probe"
        }
        async fn chat(
            &self,
            _: &[crate::llm::Message],
            _: Option<&[crate::llm::ToolSchema]>,
            _: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            panic!("the probe must use the classified seam")
        }
        async fn chat_classified(
            &self,
            messages: &[crate::llm::Message],
            tools: Option<&[crate::llm::ToolSchema]>,
            thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, LlmFailure> {
            assert_eq!(self.1.fetch_add(1, Ordering::SeqCst), 0);
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].role, "user");
            assert_eq!(messages[0].content, "Reply exactly OK.");
            assert!(tools.is_none() && thinking.is_none());
            match self.0 {
                1 | 2 => Err(LlmFailure {
                    kind: if self.0 == 1 {
                        LlmFailureKind::QuotaExceeded
                    } else {
                        LlmFailureKind::Unclassified
                    },
                    diagnostic: "Bearer private-probe-secret insufficient_quota".into(),
                }),
                3 => std::future::pending().await,
                4 => Ok(LlmResponse::ToolCalls {
                    calls: vec![crate::llm::ToolCall {
                        id: "forbidden".into(),
                        name: "write_file".into(),
                        arguments: serde_json::json!({}),
                    }],
                    text: None,
                    stop_reason: StopReason::ToolUse,
                }),
                _ => Ok(LlmResponse::Text {
                    text: if self.0 == 6 {
                        "private-probe-secret"
                    } else {
                        "OK"
                    }
                    .into(),
                    stop_reason: if self.0 == 5 {
                        StopReason::MaxTokens
                    } else {
                        StopReason::EndTurn
                    },
                }),
            }
        }
    }
    for (kind, status) in [
        (0, "available"),
        (1, "quota_exceeded"),
        (2, "unavailable"),
        (3, "timed_out"),
        (4, "unavailable"),
        (5, "unavailable"),
        (6, "unavailable"),
    ] {
        let provider = Probe(kind, AtomicUsize::new(0));
        let report = probe_model_readiness(&provider, Duration::from_millis(1)).await;
        assert_eq!(provider.1.load(Ordering::SeqCst), 1);
        let report = serde_json::to_value(report).unwrap();
        assert_eq!(
            report,
            serde_json::json!({"status":status,"requestCount":1,"toolCount":0})
        );
        assert!(!report.to_string().contains("private-probe-secret"));
    }
}

#[test]
#[ignore = "explicit live model acceptance; requires ANGELBOT_LIVE_MODEL_TEST=1 and an existing ChatGPT authorization"]
fn configured_model_completes_bounded_file_task() {
    assert_eq!(
        std::env::var("ANGELBOT_LIVE_MODEL_TEST").as_deref(),
        Ok("1")
    );
    if std::env::var("ANGELBOT_LIVE_MODEL_JOURNEY").as_deref() == Ok("model-probe") {
        println!(
            "\nMODEL_AVAILABILITY_REPORT {}",
            serde_json::json!(run_model_readiness_probe())
        );
        return;
    }
    if std::env::var("ANGELBOT_LIVE_MODEL_JOURNEY").as_deref() == Ok("recorded-inference") {
        println!("\nRECORDED_INFERENCE_REPORT {}", inference_probe::run());
        return;
    }
    // The parent owns the only writable root. Separate test processes prove
    // that neither provider state, steering nor a settings cache carries the
    // conversation across the restart. This is not a desktop/installer test.
    const PHASE: &str = "ANGELBOT_PRIVATE_LIVE_PHASE";
    const ROOT: &str = "ANGELBOT_PRIVATE_LIVE_ROOT";
    const MARKER: &str = "ANGELBOT_PRIVATE_LIVE_MARKER";
    const RECEIPT: &str = "ANGELBOT_PRIVATE_LIVE_RECEIPT";
    let Ok(phase) = std::env::var(PHASE) else {
        let directory = tempfile::tempdir().unwrap();
        let marker = Uuid::new_v4().to_string();
        let receipt = Uuid::new_v4().to_string();
        let permission =
            std::env::var("ANGELBOT_LIVE_MODEL_PERMISSION").unwrap_or_else(|_| "ask".into());
        let journey =
            std::env::var("ANGELBOT_LIVE_MODEL_JOURNEY").unwrap_or_else(|_| "continuous".into());
        assert!(matches!(
            journey.as_str(),
            "continuous"
                | "delegation"
                | "main-delegation"
                | "main-restart"
                | "main-crash"
                | "main-review-crash"
                | "main-concurrent"
                | "mcp-interrupt"
                | "mcp-disconnect"
                | "model-fault"
                | "browser-daily"
                | "native-context"
        ));
        let phases: &[&str] = if journey == "native-context" {
            assert_eq!(permission, "full_access");
            &["native-start", "native-resume", "native-check"]
        } else if journey == "browser-daily" {
            assert_eq!(permission, "full_access");
            if matches!(
                std::env::var("ANGELBOT_LIVE_BROWSER_TASK").as_deref(),
                Ok("daily-report" | "upload-report")
            ) {
                &["browser-write", "browser-reopen", "browser-restored"]
            } else {
                &["browser-write", "browser-reopen"]
            }
        } else if journey == "model-fault" {
            assert_eq!(permission, "full_access");
            &["mcp-effect-model-fault"]
        } else if journey == "mcp-disconnect" {
            assert_eq!(permission, "full_access");
            &["mcp-effect-disconnect"]
        } else if journey == "mcp-interrupt" {
            assert_eq!(
                permission, "full_access",
                "this journey explicitly verifies trusted MCP auto-execution"
            );
            assert!(
                cfg!(windows),
                "MCP interruption acceptance currently targets Windows"
            );
            &["mcp-effect-crash", "mcp-effect-recover"]
        } else if journey == "main-review-crash" {
            &["main-reviewer-crash", "main-reviewer-resume"]
        } else if journey == "main-crash" {
            &["main-producer-crash", "main-producer-resume"]
        } else if journey == "main-restart" {
            &["main-checkpoint", "main-resume"]
        } else if journey == "main-concurrent" {
            &["main-concurrent"]
        } else if matches!(journey.as_str(), "delegation" | "main-delegation") {
            if journey == "main-delegation" {
                &["main-delegation"]
            } else {
                &["delegation"]
            }
        } else if permission == "ask" {
            &["start", "resume", "learn", "verify-learning"]
        } else {
            &["start", "resume"]
        };
        struct LiveChild(std::process::Child);
        impl Drop for LiveChild {
            fn drop(&mut self) {
                // A failed readiness assertion must not leave the synthetic
                // worker running after its parent or fixture is dropped.
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        for &phase in phases {
            let crash_stage = match phase {
                "main-producer-crash" => Some("producer_after_candidate_write"),
                "main-reviewer-crash" => Some("reviewer_running"),
                "mcp-effect-crash" => Some("mcp_after_effect"),
                _ => None,
            };
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "commands::message::live_tests::configured_model_completes_bounded_file_task",
                    "--exact",
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(PHASE, phase)
                .env(ROOT, directory.path())
                .env(MARKER, &marker)
                .env(RECEIPT, &receipt);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
            }
            let mut child = LiveChild(
                command
                    .spawn()
                    .expect("isolated live acceptance process must start"),
            );
            let started = std::time::Instant::now();
            loop {
                if let Some(crash_stage) = crash_stage {
                    let checkpoint = directory.path().join(if crash_stage == "mcp_after_effect" {
                        "mcp-checkpoint.json"
                    } else {
                        "delegation-checkpoint.json"
                    });
                    if checkpoint.exists() {
                        let ready: serde_json::Value =
                            serde_json::from_slice(&std::fs::read(checkpoint).unwrap()).unwrap();
                        assert_eq!(ready["receipt"], receipt);
                        assert_eq!(ready["process_id"], child.0.id());
                        assert_eq!(ready["crash_stage"], crash_stage);
                        #[cfg(windows)]
                        let mcp_process = if crash_stage == "mcp_after_effect" {
                            use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
                            use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
                            use windows_sys::Win32::System::Threading::{
                                OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
                            };
                            let pid = u32::try_from(ready["mcp_pid"].as_u64().unwrap()).unwrap();
                            let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
                            assert!(
                                !handle.is_null(),
                                "owned MCP process handle must be available"
                            );
                            let process = unsafe { OwnedHandle::from_raw_handle(handle) };
                            assert_eq!(
                                unsafe { WaitForSingleObject(process.as_raw_handle(), 0) },
                                WAIT_TIMEOUT,
                                "owned MCP must still be alive before host loss"
                            );
                            Some(process)
                        } else {
                            None
                        };
                        // Kill this owned child without unwinding StopPump or
                        // running service shutdown. Resume checks the exact
                        // durable attempt/dispatch state before any recovery.
                        child.0.kill().expect("owned crash child must terminate");
                        assert!(!child.0.wait().unwrap().success());
                        #[cfg(windows)]
                        if let Some(process) = mcp_process {
                            use std::os::windows::io::AsRawHandle;
                            use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
                            use windows_sys::Win32::System::Threading::WaitForSingleObject;
                            assert_eq!(
                                unsafe { WaitForSingleObject(process.as_raw_handle(), 5_000) },
                                WAIT_OBJECT_0,
                                "the production MCP job must clean its process after host loss"
                            );
                            println!("LIVE_MCP_PROCESS cleanup verified after forced host exit");
                        }
                        break;
                    }
                }
                if let Some(status) = child.0.try_wait().unwrap() {
                    if !status.success() || crash_stage.is_some() {
                        // Retain only this synthetic fixture for diagnosis. It
                        // contains no copied owner history or raw credentials.
                        let failed_root = directory.keep();
                        panic!(
                            "live acceptance phase {phase} failed; owned fixture retained at {}",
                            failed_root.display()
                        );
                    }
                    break;
                }
                if started.elapsed() > Duration::from_secs(600) {
                    let _ = child.0.kill();
                    let _ = child.0.wait();
                    panic!("live acceptance phase {phase} timed out");
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if crash_stage.is_none() {
                assert_eq!(
                    std::fs::read_to_string(directory.path().join(format!("{phase}.passed")))
                        .unwrap(),
                    receipt,
                    "an empty test-filter match must not count as a successful child phase"
                );
            }
            println!("LIVE_PROCESS {phase} passed");
        }
        if journey == "continuous" {
            println!("LIVE_RESTART verified: independent processes, durable history/settings, next task; permission={permission}");
        }
        return;
    };
    assert!(matches!(
        phase.as_str(),
        "start"
            | "resume"
            | "learn"
            | "verify-learning"
            | "delegation"
            | "main-delegation"
            | "main-checkpoint"
            | "main-resume"
            | "main-producer-crash"
            | "main-producer-resume"
            | "main-reviewer-crash"
            | "main-reviewer-resume"
            | "main-concurrent"
            | "mcp-effect-crash"
            | "mcp-effect-recover"
            | "mcp-effect-disconnect"
            | "mcp-effect-model-fault"
            | "browser-write"
            | "browser-reopen"
            | "browser-prepare"
            | "browser-restored"
            | "browser-continue"
            | "native-start"
            | "native-resume"
            | "native-check"
    ));
    let directory = std::path::PathBuf::from(std::env::var_os(ROOT).unwrap());
    let receipt = std::env::var(RECEIPT).unwrap();
    let marker = std::env::var(MARKER).unwrap();
    Uuid::parse_str(&receipt).unwrap();
    Uuid::parse_str(&marker).unwrap();
    let app = headless_credential_host().expect("headless credential host must initialize");
    let config =
        crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
            .expect("saved model must resolve");
    assert_eq!(config.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
    println!(
        "LIVE_MODEL provider={} model={}",
        config.provider, config.model
    );

    if phase.starts_with("native-") {
        assert!(config.model.contains("luna"));
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(native_context::verify(
                app.handle(),
                &config,
                &directory,
                &receipt,
                &phase,
            ));
        std::fs::write(directory.join(format!("{phase}.passed")), receipt).unwrap();
        return;
    }

    if phase.starts_with("browser-") {
        assert!(config.model.contains("luna"));
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            if phase == "browser-prepare" {
                let source = std::env::var_os("ANGELBOT_PRIVATE_RETAINED_SOURCE").unwrap();
                browser_daily::prepare_retained_restore(std::path::Path::new(&source), &directory);
            } else if phase == "browser-continue" {
                browser_daily::verify_delivery_continuation(
                    app.handle(),
                    &config.provider,
                    &config.model,
                )
                .await;
            } else if phase == "browser-restored" {
                browser_daily::verify_restored(app.handle(), &directory).await;
            } else {
                browser_daily::verify(
                    app.handle(),
                    &directory,
                    &receipt,
                    phase == "browser-reopen",
                )
                .await;
            }
        });
        std::fs::write(directory.join(format!("{phase}.passed")), receipt).unwrap();
        return;
    }

    if phase.starts_with("mcp-effect-") {
        assert!(
            config.model.contains("luna"),
            "this acceptance requires the configured Luna model"
        );
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(mcp_interrupt::verify(
                app.handle(),
                &config,
                &directory,
                &receipt,
                phase == "mcp-effect-recover",
            ));
        std::fs::write(directory.join(format!("{phase}.passed")), receipt).unwrap();
        return;
    }

    let delegated_journey = match phase.as_str() {
        "delegation" => Some(delegation::Journey::Direct),
        "main-delegation" => Some(delegation::Journey::Main),
        "main-checkpoint" => Some(delegation::Journey::Checkpoint),
        "main-resume" => Some(delegation::Journey::Resume),
        "main-producer-crash" => Some(delegation::Journey::ProducerCrash),
        "main-producer-resume" => Some(delegation::Journey::ProducerResume),
        "main-reviewer-crash" => Some(delegation::Journey::ReviewerCrash),
        "main-reviewer-resume" => Some(delegation::Journey::ReviewerResume),
        "main-concurrent" => Some(delegation::Journey::Concurrent),
        _ => None,
    };
    if let Some(journey) = delegated_journey {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(delegation::verify_candidate(
                app.handle(),
                &config,
                &directory,
                &receipt,
                journey,
            ));
        std::fs::write(directory.join(format!("{phase}.passed")), receipt).unwrap();
        return;
    }

    let workspace = directory.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let root = crate::commands::file::display_work_dir(&workspace);
    let original = format!("receipt,{receipt}\nitem,amount,status\nstationery,125.50,paid\nprinting,83.25,paid\npostage,10.00,pending\n");
    let mut state = crate::tests::test_app_state();
    state.db = Arc::new(Mutex::new(
        crate::db::init(&directory.join("data.db")).unwrap(),
    ));
    {
        let mut conn = state.db.lock().unwrap();
        crate::db::migrate(&conn).unwrap();
        if phase == "start" {
            std::fs::write(workspace.join("expenses.csv"), &original).unwrap();
            conn.execute_batch(
                "INSERT INTO projects (id,name,path,created_at,kind,updated_at,active_session_id)
             VALUES ('live-project','Live acceptance','',1,'project',1,NULL);
             INSERT INTO sessions (id,title,created_at,updated_at,work_dir,project_id)
             VALUES ('live-session','Live acceptance',1,1,'','live-project');
             UPDATE projects SET active_session_id='live-session' WHERE id='live-project';",
            )
            .unwrap();
            conn.execute(
                "UPDATE projects SET path=?1 WHERE id='live-project'",
                [&root],
            )
            .unwrap();
            conn.execute(
                "UPDATE sessions SET work_dir=?1 WHERE id='live-session'",
                [&root],
            )
            .unwrap();
        } else {
            let saved_roots: (String, String) = conn.query_row(
                "SELECT p.path,s.work_dir FROM projects p JOIN sessions s ON s.project_id=p.id WHERE s.id='live-session'",
                [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            assert_eq!(
                saved_roots,
                (root.clone(), root.clone()),
                "restart must preserve the workspace binding without repairing it in the test"
            );
            assert_eq!(
                crate::commands::foreground_run_store::recover_interrupted_runs_in_conn(
                    &mut conn,
                    Utc::now().timestamp()
                )
                .unwrap(),
                0,
                "a durable confirmation must not be treated as a running task"
            );
            assert_eq!(
                crate::commands::memory::resolve_reply_memory_controls(
                    &conn,
                    "live-session",
                    None,
                    None
                )
                .unwrap(),
                (false, false),
                "a fresh process must not re-enable memory or learning"
            );
        }
    }
    let permission =
        std::env::var("ANGELBOT_LIVE_MODEL_PERMISSION").unwrap_or_else(|_| "ask".into());
    assert!(matches!(permission.as_str(), "ask" | "workspace_auto"));
    if phase == "start" {
        set_agent_execution_permission_impl(&state, &permission).unwrap();
        crate::commands::memory::resolve_reply_memory_controls(
            &state.db.lock().unwrap(),
            "live-session",
            Some(false),
            Some(false),
        )
        .unwrap();
    } else {
        assert_eq!(agent_execution_permission(&state).unwrap(), permission);
    }
    // Default pump is inert and no AppState is registered on the credential
    // host. This instance cannot start user automations or delegated work.
    let runtime = tokio::runtime::Runtime::new().unwrap();
    if matches!(phase.as_str(), "learn" | "verify-learning") {
        assert_eq!(
            permission, "ask",
            "learning acceptance uses the complete ask-permission journey"
        );
        runtime.block_on(verify_live_learning(
            &state,
            app.handle(),
            phase == "verify-learning",
            &receipt,
        ));
        std::fs::write(directory.join(format!("{phase}.passed")), receipt).unwrap();
        return;
    }
    runtime.block_on(async {
        let request = |content: &str| SendMessageRequest {
            session_id: "live-session".into(),
            client_message_id: None,
            role: "user".into(),
            content: content.into(),
            text_attachments: Vec::new(),
            personality: None,
            preferences: None,
            thinking_effort: Some("low".into()),
            web_search_setup: None,
        };
        if phase == "resume" && permission == "ask" {
            let pending_reply = ForegroundRunStore::new(state.db.clone()).project_session("live-session")
                .unwrap().into_iter().rev().find(|message| message.role == "assistant").unwrap();
            assert_eq!(pending_reply.task_run.as_ref().unwrap().status, "awaiting_confirmation");
            let pending = pending_reply.tool_results.as_ref().unwrap().iter()
                .find(|result| result.confirmation_required).unwrap();
            assert_eq!(pending.tool_name, "write_file");
            assert!(!workspace.join("cancelled.txt").exists());
            cancel_agent_task_impl(&state, "live-session", &pending_reply.id).unwrap();
            assert!(resolve_agent_confirmation_and_continue(&state, Some(app.handle()),
                ResolveAgentConfirmationRequest {
                    session_id: "live-session".into(), message_id: pending_reply.id.clone(),
                    call_id: pending.call_id.clone(), decision: "approved".into(), preview_id: None,
                }).await.is_err(), "stopped confirmation must not replay after restart");
            let stopped = ForegroundRunStore::new(state.db.clone()).project_session("live-session")
                .unwrap().into_iter().find(|message| message.id == pending_reply.id).unwrap();
            assert_eq!(stopped.task_run.as_ref().unwrap().status, "stopped");
            assert!(!workspace.join("cancelled.txt").exists());
            println!("LIVE_STOP verified: pending confirmation survived restart, cancellation prevents replay");
        }
        let goal = if phase == "start" {
            format!("本会话核对代号是 {marker}，只在本会话记住，不保存为记忆或文件。请核对当前项目 expenses.csv 中的办公支出，计算已付、待付和合计金额，保存为新文件 summary.txt。内容为 JSON 对象，字段为原文件 receipt 及 paid、pending、total 三个金额（两位小数字符串）。保存后重新打开结果核对，并向我汇报 receipt 和三个金额。只处理当前目录的这两个文件，不修改原 CSV，不运行命令、不联网、不委派。")
        } else {
            "之前待确认的 cancelled.txt 不再创建。请重新打开 summary.txt，核对 receipt 和三个金额并汇报，也给出前一次交代的会话核对代号（它不在文件里）。不要修改文件，不运行命令、不联网、不委派。".into()
        };
        {
            let mut reply = tokio::time::timeout(
                Duration::from_secs(180),
                send_message_impl(&state, Some(app.handle()), request(&goal), None,
                    ForegroundUserTurn::Append, None, None, None),
            ).await.expect("bounded live turn timed out").expect("Main turn must execute");
            if phase == "start" && permission == "ask" {
                assert_eq!(reply.task_run.as_ref().unwrap().status, "awaiting_confirmation");
                assert!(!workspace.join("summary.txt").exists(), "approval must precede the write");
                let pending = reply.tool_results.as_ref().unwrap().iter()
                    .find(|result| result.confirmation_required).expect("pending write must be visible");
                assert_eq!(pending.tool_name, "write_file");
                let approval = ResolveAgentConfirmationRequest {
                    session_id: "live-session".into(),
                    message_id: reply.id.clone(),
                    call_id: pending.call_id.clone(),
                    decision: "approved".into(),
                    preview_id: None,
                };
                let result = tokio::time::timeout(Duration::from_secs(180),
                    resolve_agent_confirmation_and_continue(&state, Some(app.handle()), approval.clone()))
                    .await.expect("bounded confirmation continuation timed out")
                    .expect("approval continuation must return a receipt");
                assert!(result.success);
                assert_eq!(result.confirmation_status.as_deref(), Some("approved"));
                // A successful tool receipt alone does not prove the model
                // resumed. Read the same durable projection as get_messages.
                reply = ForegroundRunStore::new(state.db.clone()).project_session("live-session")
                    .unwrap().into_iter().rev().find(|message| message.role == "assistant")
                    .expect("continued Main reply must be visible");
                let users: i64 = state.db.lock().unwrap().query_row(
                    "SELECT COUNT(*) FROM messages WHERE session_id='live-session' AND role='user'", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(users, 1, "approval must not create a synthetic user turn");
                let visible_before = ForegroundRunStore::new(state.db.clone()).project_session("live-session").unwrap().len();
                assert!(resolve_agent_confirmation_and_continue(&state, Some(app.handle()), approval).await.is_err(),
                    "the same approval must not replay an already committed write");
                assert_eq!(ForegroundRunStore::new(state.db.clone()).project_session("live-session").unwrap().len(), visible_before);
            }
            let run = reply.task_run.as_ref().expect("durable task result must exist");
            println!("LIVE_TURN {phase} status={} steps={}", run.status, run.step_count);
            for result in reply.tool_results.iter().flatten() {
                println!("LIVE_TOOL name={} success={} confirmation={}",
                    result.tool_name, result.success, result.confirmation_required);
            }
            assert_eq!(run.status, "completed", "live task must complete rather than merely return text");
            for expected in [&receipt, "208.75", "10.00", "218.75"] {
                assert!(reply.content.contains(expected), "reply must contain a verified fixture fact");
            }
            assert!(reply.tool_results.as_ref().is_some_and(|results|
                results.iter().any(|result| result.tool_name == "read_file" && result.success)));
            if phase == "resume" {
                assert!(reply.content.contains(&marker), "fresh process must recall the prior conversation, not just reread the file");
            }
            if phase == "start" && permission == "ask" {
                println!("LIVE_CONFIRMATION approved: model resumed, no synthetic user turn, duplicate approval refused");
            }
        }
        if phase == "start" && permission == "ask" {
            let refused = tokio::time::timeout(Duration::from_secs(180),
                send_message_impl(&state, Some(app.handle()), request(
                    "请在当前项目新建 rejected.txt，内容为 decline-fixture。不要处理其他文件，不运行命令、不联网、不委派。"),
                    None, ForegroundUserTurn::Append, None, None, None))
                .await.expect("bounded refusal preparation timed out").unwrap();
            assert_eq!(refused.task_run.as_ref().unwrap().status, "awaiting_confirmation");
            assert!(!workspace.join("rejected.txt").exists());
            let pending = refused.tool_results.as_ref().unwrap().iter()
                .find(|result| result.confirmation_required).unwrap();
            assert_eq!(pending.tool_name, "write_file");
            let before = ForegroundRunStore::new(state.db.clone()).project_session("live-session").unwrap().len();
            let rejected = resolve_agent_confirmation_and_continue(&state, Some(app.handle()),
                ResolveAgentConfirmationRequest {
                    session_id: "live-session".into(), message_id: refused.id,
                    call_id: pending.call_id.clone(), decision: "rejected".into(), preview_id: None,
                }).await.unwrap();
            assert!(!rejected.success);
            assert_eq!(rejected.confirmation_status.as_deref(), Some("rejected"));
            assert!(!workspace.join("rejected.txt").exists());
            assert_eq!(ForegroundRunStore::new(state.db.clone()).project_session("live-session").unwrap().len(), before,
                "rejection must not start a new model turn");
            let after = tokio::time::timeout(Duration::from_secs(180),
                send_message_impl(&state, Some(app.handle()), request(
                    "刚才拒绝的 rejected.txt 不再创建。请重新打开 summary.txt 并汇报 receipt 和三个金额，不修改文件。"),
                    None, ForegroundUserTurn::Append, None, None, None))
                .await.expect("bounded post-rejection task timed out").unwrap();
            assert_eq!(after.task_run.as_ref().unwrap().status, "completed");
            for expected in [&receipt, "208.75", "10.00", "218.75"] {
                assert!(after.content.contains(expected), "post-rejection reply must contain verified facts");
            }
            assert!(after.tool_results.as_ref().unwrap().iter()
                .any(|result| result.tool_name == "read_file" && result.success));
            assert!(!workspace.join("rejected.txt").exists());
            let writes: i64 = state.db.lock().unwrap().query_row(
                "SELECT COUNT(*) FROM agent_steps WHERE session_id='live-session' AND tool_name='write_file' AND success=1",
                [], |row| row.get(0)).unwrap();
            assert_eq!(writes, 1, "only the one approved write may commit");
            println!("LIVE_REJECTION verified: no file, no automatic model turn, next real task completed");
            let pending = tokio::time::timeout(Duration::from_secs(180),
                send_message_impl(&state, Some(app.handle()), request(
                    "请在当前项目新建 cancelled.txt，内容为 stop-fixture。不要处理其他文件，不运行命令、不联网、不委派。"),
                    None, ForegroundUserTurn::Append, None, None, None))
                .await.expect("bounded restart confirmation preparation timed out").unwrap();
            assert_eq!(pending.task_run.as_ref().unwrap().status, "awaiting_confirmation");
            assert!(!workspace.join("cancelled.txt").exists());
        }
    });
    let output: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(workspace.join("summary.txt")).unwrap())
            .expect("report must use the requested structured format");
    assert_eq!(
        output,
        serde_json::json!({
            "receipt": receipt, "paid": "208.75", "pending": "10.00", "total": "218.75"
        }),
        "amounts must be mapped to the right business fields, not merely mentioned"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("expenses.csv")).unwrap(),
        original
    );
    let conn = state.db.lock().unwrap();
    let binding: (String, String) = conn
        .query_row(
            "SELECT agent_provider,agent_model FROM sessions WHERE id='live-session'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        binding,
        (config.provider, config.model),
        "Main must use the configured model"
    );
    let unsafe_calls: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_steps
             WHERE tool_name NOT IN ('read_file','write_file','list_workspace_items',
                                     'think','check_constraint','verify_result')
                OR (tool_name='list_workspace_items'
                    AND COALESCE(json_extract(tool_input,'$.path'),'') NOT IN ('','.'))",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        unsafe_calls, 0,
        "live acceptance must remain a local-file-only task"
    );
    assert_eq!(
        crate::commands::memory::resolve_reply_memory_controls(&conn, "live-session", None, None)
            .unwrap(),
        (false, false)
    );
    let successful_writes: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_steps WHERE tool_name='write_file' AND success=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        successful_writes, 1,
        "only the original approved write may commit across both processes"
    );
    assert!(!workspace.join("cancelled.txt").exists());
    println!("LIVE_ACCEPTANCE verified: local read/write/readback and persisted next-turn history");
    std::fs::write(directory.join(format!("{phase}.passed")), receipt).unwrap();
}

async fn verify_live_learning(
    state: &AppState,
    app: &AppHandle,
    after_restart: bool,
    receipt: &str,
) {
    if after_restart {
        let conn = state.db.lock().unwrap();
        let disabled: (i64, i64) = conn.query_row(
            "SELECT COUNT(*),SUM(length(constraint_value)) FROM adaptive_constraints WHERE source='user_dismissed' AND status='disabled'",
            [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(
            disabled,
            (1, 0),
            "dismissal and content removal must survive process exit"
        );
        let evidence: i64 = conn.query_row(
            "SELECT COUNT(*) FROM adaptive_constraint_evidence e JOIN adaptive_constraints c USING(constraint_key) WHERE c.status='disabled'",
            [], |r| r.get(0)).unwrap();
        assert_eq!(evidence, 0);
        assert!(
            crate::agent::adaptive_constraints::active_for_session(&conn, "live-session")
                .is_empty()
        );
        println!("LIVE_LEARNING_RESTART verified: dismissal, evidence cleanup and paused controls survived a fresh process");
        return;
    }
    {
        let conn = state.db.lock().unwrap();
        let users: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id='live-session' AND role='user'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            users, 5,
            "real turns, not inserted history or a shifted clock, must reach the review threshold"
        );
        crate::commands::memory::resolve_reply_memory_controls(
            &conn,
            "live-session",
            Some(true),
            Some(true),
        )
        .unwrap();
    }
    for content in [
        "我喜欢聊天先有一句简短结论，再解释，不要给我很长的开场白。请按这个习惯用两句话说明如何开始一天。不要调用工具或替我安排提醒。",
        "今天只想聊个轻松的话题，请用两句话分享一个整理桌面的建议。不需要操作电脑。",
        "之后与我说话也先给简短结论再解释；别把这种表达偏好当成自动执行任务的授权。请按这个方式告诉我如何休息一下。",
        "之前费用表的数据和会话核对代号只用于临时验收，不代表个人消费或工作信息。请简短确认，不保存这些数据。",
        "请按先简短结论再解释的方式，说一句这段交流的总结。不要调用工具或改变文件、提醒和自动化。",
    ] {
        let req = serde_json::from_value::<SendMessageRequest>(serde_json::json!({
            "sessionId":"live-session", "role":"user", "content":content, "thinkingEffort":"low"
        })).unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(180),
            send_message_impl(state, Some(app), req, None, ForegroundUserTurn::Append, None, None, None))
            .await.expect("bounded preference conversation timed out").unwrap();
        assert!(!reply.content.trim().is_empty(), "natural reply was empty: status={:?}, calls={:?}",
            reply.task_run.as_ref().map(|run| run.status.as_str()),
            reply.tool_results.as_ref().map(|calls| calls.iter().map(|call| (&call.tool_name, call.confirmation_required)).collect::<Vec<_>>()));
        assert!(reply.task_run.as_ref().is_none_or(|run| run.status == "completed"),
            "plain conversation need not create a task run, but must not remain interrupted");
        assert!(ForegroundRunStore::new(state.db.clone()).project_session("live-session").unwrap()
            .iter().any(|saved| saved.id == reply.id && saved.content == reply.content),
            "the natural reply must be durably visible before continuing");
        assert!(reply.tool_results.as_ref().is_none_or(|calls| calls.is_empty()),
            "a communication preference does not authorize tools or external actions");
    }
    // The production after-completion hook schedules the review. Do not call
    // extraction directly, insert candidates or advance the cooldown clock.
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let complete: bool = state.db.lock().unwrap().query_row(
                "SELECT json_extract(metadata,'$.automaticEvolution.through_message_id') IS NOT NULL FROM sessions WHERE id='live-session'",
                [], |r| r.get(0)).unwrap();
            if complete { break; }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }).await.expect("the production automatic learning hook must commit a review checkpoint");
    let habits = crate::commands::memory::get_learned_preferences_impl(state).unwrap();
    for habit in &habits {
        // Only the synthetic requests above exist in this disposable DB.
        println!(
            "LIVE_FIXTURE_HABIT ready={} content={}",
            habit.ready, habit.content
        );
    }
    assert!(
        !habits.is_empty(),
        "explicitly repeated communication preferences must produce a tentative observation"
    );
    assert!(
        habits
            .iter()
            .all(|habit| !habit.ready && !habit.content.contains(receipt)),
        "one short conversation is not independent multi-day evidence or personal financial data"
    );
    assert!(
        habits
            .iter()
            .all(|habit| ["结论", "开场白", "简短", "简洁", "两句"]
                .iter()
                .any(|word| habit.content.contains(word))),
        "authorization rules and one-task restrictions must not become global habits"
    );
    {
        let conn = state.db.lock().unwrap();
        let max_evidence: i64 = conn
            .query_row(
                "SELECT MAX(evidence_count) FROM adaptive_constraints",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(max_evidence, 1);
        assert!(
            crate::agent::adaptive_constraints::active_for_session(&conn, "live-session")
                .is_empty()
        );
    }
    let dismissed = &habits[0].id;
    crate::commands::memory::dismiss_learned_preference_impl(state, dismissed).unwrap();
    assert!(crate::commands::memory::get_learned_preferences_impl(state)
        .unwrap()
        .iter()
        .all(|h| &h.id != dismissed));
    crate::commands::memory::resolve_reply_memory_controls(
        &state.db.lock().unwrap(),
        "live-session",
        Some(false),
        Some(false),
    )
    .unwrap();
    println!("LIVE_LEARNING verified: ten real Main turns, automatic real-model review, tentative evidence only, owner dismissal and pause");
}
