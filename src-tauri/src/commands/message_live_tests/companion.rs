//! Small, opt-in memory-to-response probe, not a natural longitudinal study.
//! Source utterances are preloaded fixtures, NOT automatically learned facts.
use super::*;
use crate::agent::tool::ToolHandler;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    sources: Vec<String>,
    question: String,
}

fn prepare(case: &Case, memory: bool) -> (Vec<crate::llm::Message>, Vec<usize>) {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("synthetic.db");
    let mut state = crate::tests::test_app_state();
    state.db = Arc::new(Mutex::new(crate::db::init(&database).unwrap()));
    {
        let conn = state.db.lock().unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute_batch("INSERT INTO sessions(id,title,created_at,updated_at) VALUES('companion-probe','Synthetic companion probe',1,1);").unwrap();
    }
    if memory {
        let handler = crate::agent::extensions::memory::RememberFactHandler {
            db: Some(state.db.clone()),
        };
        for source in &case.sources {
            assert!(
                handler
                    .execute(
                        &serde_json::json!({"content":source,"category":"event","importance":5}),
                        directory.path()
                    )
                    .success
            );
        }
    }
    // Reopen the owned SQLite connection; this is not a fresh-process claim.
    state.db = Arc::new(Mutex::new(crate::db::init(&database).unwrap()));
    assert_eq!(
        state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r
                .get::<_, usize>(0))
            .unwrap(),
        if memory { case.sources.len() } else { 0 }
    );
    let request: SendMessageRequest = serde_json::from_value(serde_json::json!({
        "sessionId":"companion-probe", "role":"user", "content":case.question,
        "preferences":{"useLongTermMemory":memory,"evolutionEnabled":false}
    }))
    .unwrap();
    let config = build_agent_config(&state, &request).unwrap();
    assert_eq!(config.preferences.use_long_term_memory, memory);
    let recalled = config
        .relevant_memories
        .iter()
        .map(|memory| {
            case.sources
                .iter()
                .position(|source| source == &memory.content)
                .expect("only owned source utterances can be recalled")
        })
        .collect::<Vec<_>>();
    let system = crate::agent::prompt::build_agent_system_prompt(&config, &[], &[]);
    let mut messages = vec![message("system", system)];
    if !memory {
        // Oracle-history control: evidence supplied, without retrieval. This
        // is not a matched-token ablation or a claim about an official score.
        for source in &case.sources {
            messages.push(message("user", source.clone()));
            messages.push(message("assistant", "收到。".into()));
        }
    }
    messages.push(message(
        "user",
        format!(
            "{}\n只返回题目要求的短答案或选项，不要解释。没有足够证据时只返回 UNKNOWN。",
            case.question
        ),
    ));
    (messages, recalled)
}

pub(super) fn message(role: &str, content: String) -> crate::llm::Message {
    crate::llm::Message {
        role: role.into(),
        content,
        tool_calls: None,
        tool_call_id: None,
        tool_images: Vec::new(),
        protocol_state: None,
    }
}

#[test]
fn companion_probe_keeps_gold_out_of_the_rust_input_and_reopens_only_owned_memory() {
    assert!(serde_json::from_value::<Case>(serde_json::json!({
        "id":"bad","sources":[],"question":"q","expected":"gold"
    }))
    .is_err());
    let case = Case {
        id: "owned".into(),
        sources: vec!["我喜欢茉莉花茶。".into()],
        question: "我喜欢茉莉花茶".into(),
    };
    let (memory, recalled) = prepare(&case, true);
    assert_eq!(
        recalled.len(),
        1,
        "the exact-query positive control must traverse production recall"
    );
    assert_eq!(
        memory.len(),
        2,
        "memory mode cannot secretly receive oracle history"
    );
    assert!(memory[0].content.contains(&case.sources[0]));
    let (oracle, recalled) = prepare(&case, false);
    assert!(recalled.is_empty());
    assert_eq!(oracle.len(), 4);
    assert!(!oracle[0].content.contains(&case.sources[0]));
    assert_eq!(
        memory.last().unwrap().content,
        oracle.last().unwrap().content
    );
    let natural = Case {
        id: "natural".into(),
        sources: vec!["我早晨通常喝红茶。".into(), "我最喜欢的宠物是仓鼠。".into()],
        question: "早晨我通常喝什么？".into(),
    };
    assert_eq!(
        prepare(&natural, true).1,
        vec![0],
        "a natural question must recall its evidence, not unrelated user facts"
    );
}

#[test]
#[ignore = "explicit scripts/companion_eval.py; live mode borrows only the saved Luna binding"]
fn minimal_companion_probe() {
    let mode = std::env::var("ANGELBOT_COMPANION_MODE").unwrap();
    assert!(matches!(mode.as_str(), "offline" | "live"));
    let input =
        std::fs::read(std::env::var_os("ANGELBOT_PRIVATE_COMPANION_INPUT").unwrap()).unwrap();
    assert!(input.len() <= 64 * 1024);
    let cases: Vec<Case> = serde_json::from_slice(&input).unwrap();
    assert!(!cases.is_empty() && cases.len() <= 12);
    assert!(cases.iter().all(|case| !case.id.is_empty()
        && case.id.len() <= 80
        && !case.question.is_empty()
        && case.question.len() <= 2000
        && case.sources.len() <= 8
        && case.sources.iter().all(|s| s.len() <= 2000)));
    let app = if mode == "live" {
        assert_eq!(
            std::env::var("ANGELBOT_LIVE_MODEL_TEST").as_deref(),
            Ok("1")
        );
        Some(headless_credential_host().expect("headless credential host must initialize"))
    } else {
        None
    };
    let config = app.as_ref().map(|app| {
        let mut config =
            crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
                .expect("saved Luna binding must resolve; no API fallback");
        assert_eq!(config.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
        assert!(config.model.contains("luna"));
        config.max_tokens = config.max_tokens.min(512);
        config
    });
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for case in cases {
        for arm in ["oracle_history", "angelbot_recall"] {
            let (messages, recalled) = prepare(&case, arm == "angelbot_recall");
            let mut row = serde_json::json!({"id":case.id,"arm":arm,"status":"offline","requestCount":0,"toolCount":0,"recalledCount":recalled.len(),"evidenceSourceIndices":recalled,"sourceCount":case.sources.len(),
                "input":messages.iter().map(|m| serde_json::json!({"role":m.role,"content":m.content})).collect::<Vec<_>>(),
                "inputChars":messages.iter().map(|m| m.content.chars().count()).sum::<usize>(),"latencyMs":0,"answer":null,"usage":null,"model":null});
            if let Some(config) = &config {
                row["model"] = serde_json::json!(config.model);
                let provider = crate::llm::create_provider_for_app(
                    config,
                    Some(app.as_ref().unwrap().handle()),
                )
                .expect("saved provider must initialize");
                let started = std::time::Instant::now();
                let response = runtime.block_on(async {
                    tokio::time::timeout(
                        Duration::from_secs(45),
                        provider.chat_classified(&messages, None, None),
                    )
                    .await
                });
                row["requestCount"] = serde_json::json!(1);
                row["latencyMs"] = serde_json::json!(started.elapsed().as_millis());
                let status = match response {
                    Err(_) => "timed_out",
                    Ok(Err(failure)) => {
                        if failure.kind == crate::llm::LlmFailureKind::QuotaExceeded {
                            "quota_exceeded"
                        } else {
                            "unavailable"
                        }
                    }
                    Ok(Ok(response)) => {
                        let (response, protocol) = response.into_parts();
                        match response {
                            crate::llm::LlmResponse::Text {
                                text,
                                stop_reason: crate::llm::StopReason::EndTurn,
                            } if text.len() <= 4096
                                && protocol.is_none_or(|p| p.validate().is_ok()) =>
                            {
                                row["answer"] = serde_json::json!(text);
                                "completed"
                            }
                            _ => "invalid_response",
                        }
                    }
                };
                row["status"] = serde_json::json!(status);
                if let Some(usage) = provider.latest_usage() {
                    row["usage"] = serde_json::json!({"inputTokens":usage.input_tokens,"outputTokens":usage.output_tokens});
                }
                println!("COMPANION_RESULT {row}");
                if status != "completed" {
                    return;
                } // Stop once, never retry to green.
            } else {
                println!("COMPANION_RESULT {row}");
            }
        }
    }
}
