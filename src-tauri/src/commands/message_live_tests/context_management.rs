//! Opt-in context-input ablation. Public benchmark records only; no user memory/tools.
use super::agent_benchmarks::{bounded_provider, row_metrics, Metrics};
use super::companion::message;
use super::*;
use crate::agent::compaction::{CompactionHook, CompactionOutcome, DefaultCompactionHook};
use crate::agent::config::{AgentConfig, MemoryContext};
use crate::agent::tool::ToolHandler;
use crate::llm::{LlmProvider, LlmResponse, Message};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    role: String,
    content: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    history: Vec<Source>,
    question: String,
    question_date: String,
}

fn clean_history(case: &Case) -> Vec<Message> {
    case.history
        .iter()
        .map(|source| message(&source.role, source.content.clone()))
        .collect()
}

fn noisy_history(case: &Case) -> Vec<Message> {
    let mut history = clean_history(case);
    for index in 0..12 {
        history.push(message("user", format!("Unrelated synthetic archive entry {index:02}: station Vega-{index:02} logged calibration batch C{index:02}, shelf marker Q{index:02}, and storage rack R{index:02}. This record concerns a different entity.")));
        history.push(message(
            "assistant",
            "Archived as unrelated background material.".into(),
        ));
    }
    history
}

fn recall(history: &[Message], question: &str, root: &Path) -> (Vec<MemoryContext>, Vec<usize>) {
    let db = Arc::new(Mutex::new(
        crate::db::init(&root.join("memory.db")).unwrap(),
    ));
    crate::db::migrate(&db.lock().unwrap()).unwrap();
    let handler = crate::agent::extensions::memory::RememberFactHandler {
        db: Some(db.clone()),
    };
    let imported_at = Utc::now().timestamp();
    let sources = history
        .iter()
        .enumerate()
        .map(|(index, source)| {
            format!(
                "[source-{index:04}, speaker={}]\n{}",
                source.role, source.content
            )
        })
        .collect::<Vec<_>>();
    for (index, source) in sources.iter().enumerate() {
        assert!(
            handler
                .execute(
                    &serde_json::json!({"content":source,"category":"event","importance":5}),
                    root
                )
                .success
        );
        // Stable owned IDs and one import time avoid random UUID/timing tie breaks.
        db.lock()
            .unwrap()
            .execute(
                "UPDATE memories SET id=?1,created_at=?2,updated_at=?2 WHERE content=?3",
                rusqlite::params![format!("source-{index:04}"), imported_at, source],
            )
            .unwrap();
    }
    let rows = crate::fts::recall_active_memories(
        &db.lock().unwrap(),
        question,
        5,
        crate::fts::MemoryRecallMode::Context,
    )
    .unwrap();
    let indices = rows
        .iter()
        .map(|row| {
            sources
                .iter()
                .position(|source| source == &row.content)
                .unwrap()
        })
        .collect();
    (
        rows.into_iter()
            .map(|row| MemoryContext {
                category: row.category,
                content: row.content,
                importance: row.importance,
            })
            .collect(),
        indices,
    )
}

fn config() -> AgentConfig {
    let mut config = AgentConfig::default();
    config.preferences.use_long_term_memory = true;
    config.preferences.evolution_enabled = false;
    config
}

fn inputs(config: &AgentConfig, history: Vec<Message>, case: &Case) -> Vec<Message> {
    let mut messages = vec![message(
        "system",
        crate::agent::prompt::build_agent_system_prompt(config, &[], &[]),
    )];
    messages.extend(history);
    messages.push(message("user", format!("Historical reference date: {}\n{}\nAnswer only the minimal short answer, without explanation. Use only the supplied records; if unsupported, answer UNKNOWN.",case.question_date,case.question)));
    messages
}

fn emit(
    row: &mut serde_json::Value,
    metrics: &Metrics,
    started: std::time::Instant,
    total: &mut u64,
) -> bool {
    row_metrics(row, metrics, started);
    *total += row["inputTokens"].as_u64().unwrap_or(0);
    row["runningInputTokens"] = serde_json::json!(total);
    if *total >= 120_000 {
        row["budgetStop"] = serde_json::json!(true);
    }
    println!("CONTEXT_EVAL_ROW {row}");
    row["status"] == "completed" && *total < 120_000
}

pub(super) async fn answer(
    provider: &dyn LlmProvider,
    messages: &[Message],
    row: &mut serde_json::Value,
) {
    row["input"] = serde_json::json!(messages
        .iter()
        .map(|m| serde_json::json!({"role":m.role,"content":m.content}))
        .collect::<Vec<_>>());
    match tokio::time::timeout(
        Duration::from_secs(90),
        provider.chat_classified(messages, None, None),
    )
    .await
    {
        Err(_) => row["status"] = serde_json::json!("timed_out"),
        Ok(Err(_)) => {} // Shared instrumentation retains a safe failure class/reference.
        Ok(Ok(response)) => {
            let (response, protocol) = response.into_parts();
            if protocol.is_none_or(|p| p.validate().is_ok()) {
                if let LlmResponse::Text {
                    text,
                    stop_reason: crate::llm::StopReason::EndTurn,
                } = response
                {
                    if !text.trim().is_empty() {
                        row["status"] = serde_json::json!("completed");
                        row["answer"] = serde_json::json!(text);
                    }
                }
            }
        }
    }
}

#[test]
fn context_inputs_exclude_gold_and_recall_only_owned_sources() {
    assert!(serde_json::from_value::<Case>(serde_json::json!({"id":"bad","history":[],"question":"q","question_date":"d","answer":"gold"})).is_err());
    assert!(serde_json::from_value::<Source>(
        serde_json::json!({"role":"user","content":"x","has_answer":true})
    )
    .is_err());
    let case = Case {
        id: "owned".into(),
        history: vec![
            Source {
                role: "user".into(),
                content: "My university degree is Botany.".into(),
            },
            Source {
                role: "assistant".into(),
                content: "Noted.".into(),
            },
        ],
        question: "What university degree did I graduate with?".into(),
        question_date: "2023/01/01".into(),
    };
    let history = noisy_history(&case);
    assert_eq!(history.len(), 26);
    assert!(history[history.len() - 20..]
        .iter()
        .all(|m| !m.content.contains("Botany")));
    let root = tempfile::tempdir().unwrap();
    let (memories, indices) = recall(&history, &case.question, root.path());
    assert!(indices.contains(&0));
    assert!(memories.iter().any(|m| m.content.contains("Botany")));
    assert!(indices.iter().all(|index| *index < history.len()));
    struct SummarySpy;
    #[async_trait::async_trait]
    impl LlmProvider for SummarySpy {
        fn name(&self) -> &str {
            "owned-summary-spy"
        }
        async fn chat(
            &self,
            messages: &[Message],
            _: Option<&[crate::llm::ToolSchema]>,
            _: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            assert!(messages[0].content.contains("Botany"));
            assert!(!messages[0]
                .content
                .contains("What university degree did I graduate with?"));
            Ok(LlmResponse::Text {
                text: "owned checkpoint marker".into(),
                stop_reason: crate::llm::StopReason::EndTurn,
            })
        }
    }
    let mut compacted = history.clone();
    let result = tokio::runtime::Runtime::new().unwrap().block_on(
        DefaultCompactionHook::with_threshold(Arc::new(SummarySpy), 1)
            .maybe_compact(&mut compacted),
    );
    assert!(matches!(result, CompactionOutcome::Compressed { .. }));
    assert_eq!(compacted.len(), 21);
    assert!(compacted[0].content.contains("owned checkpoint marker"));
    assert_eq!(compacted[1].content, history[history.len() - 20].content);
}

#[test]
#[ignore = "explicit scripts/context_eval.py --live; saved Luna, public/synthetic history only"]
fn configured_context_management() {
    assert_eq!(
        std::env::var("ANGELBOT_CONTEXT_EVAL_LIVE").as_deref(),
        Ok("1")
    );
    let selected_arm = std::env::var("ANGELBOT_CONTEXT_EVAL_ARM").unwrap_or_else(|_| "all".into());
    assert!(matches!(
        selected_arm.as_str(),
        "all" | "clean_full" | "noisy_full" | "recent_only" | "checkpoint" | "recall"
    ));
    let root =
        std::path::PathBuf::from(std::env::var_os("ANGELBOT_PRIVATE_CONTEXT_EVAL_ROOT").unwrap());
    let input = std::fs::read(root.join("input.json")).unwrap();
    assert!(input.len() <= 256 * 1024);
    let cases: Vec<Case> = serde_json::from_slice(&input).unwrap();
    assert_eq!(cases.len(), 4);
    assert!(cases.iter().all(|c| c.id.len() <= 80
        && c.question.len() <= 2048
        && c.question_date.len() <= 128
        && c.history.len() <= 48
        && c.history
            .iter()
            .all(|s| matches!(s.role.as_str(), "user" | "assistant") && s.content.len() <= 8192)));
    let app = headless_credential_host().unwrap();
    let mut model =
        crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
            .unwrap();
    assert_eq!(model.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
    assert!(model.model.contains("luna"));
    model.max_tokens = model.max_tokens.min(2048);
    model.temperature = 0.0;
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let mut total=0;
        for (case_index,case) in cases.iter().enumerate() {
            let directory=root.join(format!("case-{case_index}"));
            std::fs::create_dir(&directory).unwrap();
            let noisy=noisy_history(case);
            let recent=noisy[noisy.len()-20..].to_vec();
            let mut checkpoint=None;
            for arm in ["clean_full","noisy_full","recent_only","checkpoint_build","checkpoint","recall"] {
                if selected_arm != "all" && selected_arm != arm && !(selected_arm == "checkpoint" && arm == "checkpoint_build") { continue; }
                let (provider,metrics)=bounded_provider(&model,app.handle(),&root,1);
                let started=std::time::Instant::now();
                let mut row=serde_json::json!({"id":case.id,"arm":arm,"model":model.model,"status":"invalid_response","answer":null,"evidenceSourceIndices":[],"maxTokens":model.max_tokens});
                if arm=="checkpoint_build" {
                    let mut compacted=noisy.clone();
                    row["compactionSourceHistory"]=serde_json::json!(noisy.iter().map(|m|serde_json::json!({"role":m.role,"content":m.content})).collect::<Vec<_>>());
                    let hook=DefaultCompactionHook::with_threshold(provider.clone(),1);
                    match tokio::time::timeout(Duration::from_secs(120),hook.maybe_compact(&mut compacted)).await {
                        Err(_)=>row["status"]=serde_json::json!("timed_out"),
                        Ok(CompactionOutcome::Compressed {before_tokens,after_tokens})=>{
                            row["status"]=serde_json::json!("completed");
                            row["beforeEstimatedTokens"]=serde_json::json!(before_tokens);
                            row["afterEstimatedTokens"]=serde_json::json!(after_tokens);
                            row["checkpointMessages"]=serde_json::json!(compacted.iter().map(|m|serde_json::json!({"role":m.role,"content":m.content})).collect::<Vec<_>>());
                            checkpoint=Some(compacted);
                        },
                        Ok(CompactionOutcome::Failed {reason,..})=>{
                            row["status"]=serde_json::json!("rejected_checkpoint");
                            row["checkpointFailure"]=serde_json::json!(reason);
                        },
                        _=>{},
                    }
                } else {
                    let mut config=config();
                    let history=match arm {
                        "clean_full"=>clean_history(case),
                        "noisy_full"=>noisy.clone(),
                        "recent_only"=>recent.clone(),
                        "checkpoint"=>checkpoint.clone().unwrap(),
                        "recall"=>{
                            let (memories,indices)=recall(&noisy,&case.question,&directory);
                            config.relevant_memories=memories;
                            row["evidenceSourceIndices"]=serde_json::json!(indices);
                            recent.clone()
                        },
                        _=>unreachable!(),
                    };
                    answer(provider.as_ref(),&inputs(&config,history,case),&mut row).await;
                }
                if !emit(&mut row,&metrics,started,&mut total) {return;}
            }
        }
    });
}
