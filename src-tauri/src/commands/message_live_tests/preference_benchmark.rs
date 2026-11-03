//! Opt-in raw-dialogue -> existing automatic learning -> existing prompt reader.
//! A component experiment, not a full Main/tool or natural multi-day score.
use super::agent_benchmarks::{boxed_bounded_provider, row_metrics};
use super::companion::message;
use super::context_management::answer;
use super::*;
use crate::agent::config::AgentConfig;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    User,
    Assistant,
}

impl Role {
    fn as_str(&self) -> &str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    role: Role,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    history: Vec<Source>,
    question: String,
    options: Vec<String>,
}

impl Case {
    fn source_id(&self, index: usize) -> String {
        format!("{}-source-{index}", self.id)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    arm: String,
    case: Case,
}

#[test]
fn preference_benchmark_input_rejects_truth_and_non_conversational_roles() {
    let input = json!({"arm":"history_reader","case":{"id":"prefeval.owned.explicit.0000",
        "history":[{"role":"user","content":"A user preference"}],"question":"Question","options":["a","b","c","d"]}});
    assert!(serde_json::from_value::<Input>(input.clone()).is_ok());
    let case: Input = serde_json::from_value(input.clone()).unwrap();
    let mut other = input.clone();
    other["case"]["id"] = json!("prefeval.owned.explicit.0001");
    let other: Input = serde_json::from_value(other).unwrap();
    assert_ne!(
        case.case.source_id(0),
        other.case.source_id(0),
        "incremental fixture copies must preserve old source identity without collisions"
    );
    for role in ["system", "tool"] {
        let mut changed = input.clone();
        changed["case"]["history"][0]["role"] = json!(role);
        assert!(serde_json::from_value::<Input>(changed).is_err());
    }
    let mut changed = input;
    changed["case"]["correct_label"] = json!("A");
    assert!(serde_json::from_value::<Input>(changed).is_err());
}

#[test]
#[ignore = "explicit preference_eval.py --live; saved Luna, public synthetic data, no tools"]
fn configured_preference_benchmark() {
    assert_eq!(
        std::env::var("ANGELBOT_CONTEXT_EVAL_LIVE").as_deref(),
        Ok("1")
    );
    let root =
        std::path::PathBuf::from(std::env::var_os("ANGELBOT_PRIVATE_CONTEXT_EVAL_ROOT").unwrap())
            .canonicalize()
            .unwrap();
    let logs = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("logs")
        .canonicalize()
        .unwrap();
    let owner = root.parent().unwrap();
    assert_eq!(owner.parent(), Some(logs.as_path()));
    assert!(owner
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("preference-eval-"));
    let bytes = std::fs::read(root.join("input.json")).unwrap();
    assert!(bytes.len() < 64 * 1024);
    let input: Input = serde_json::from_slice(&bytes).unwrap();
    assert!([
        "no_history",
        "history_reader",
        "stored_defaults",
        "main_grounded",
        "main_memory"
    ]
    .contains(&input.arm.as_str()));
    assert!(input.case.id.starts_with("prefeval.") && input.case.options.len() == 4);
    assert!(!input.case.history.is_empty() && input.case.history.len() <= 20);
    assert!(input
        .case
        .history
        .iter()
        .all(|s| s.content.chars().count() <= 1000));
    let app = headless_credential_host().unwrap();
    let mut model =
        crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
            .unwrap();
    assert_eq!(model.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
    assert!(model.model.contains("luna"));
    model.max_tokens = model.max_tokens.min(2048);
    model.temperature = 0.0;
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut config=AgentConfig::default();
        let mut main_context=None;
        let mut source_session=None;
        config.preferences.use_long_term_memory=true;
        config.preferences.evolution_enabled=["stored_defaults","main_grounded","main_memory"].contains(&input.arm.as_str());
        if config.preferences.evolution_enabled {
            let conn=crate::db::init(&root.join("state.db")).unwrap();
            crate::db::migrate(&conn).unwrap();
            let db=Arc::new(Mutex::new(conn));
            let mut state=crate::tests::test_app_state();state.db=db.clone();
            crate::settings_cache::load_all(&db.lock().unwrap(),&state.cached_settings).unwrap();
            let directory=root.join("workspace");std::fs::create_dir(&directory).unwrap();
            let source_id=crate::commands::workspace::create_project_workspace_impl(&state,directory.to_string_lossy().into(),Some("Owned preference benchmark".into()),None,None).unwrap().session.id;
            {
            let conn=db.lock().unwrap();
            let mut parent:Option<String>=None;
            let source_start:i64=conn.query_row("SELECT COALESCE(MAX(created_at),0)+1 FROM messages",[],|r|r.get(0)).unwrap();
            for (index,source) in input.case.history.iter().enumerate() {
                let id=input.case.source_id(index);
                conn.execute("INSERT INTO messages(id,session_id,role,content,parent_id,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![id,source_id,source.role.as_str(),source.content,parent,index as i64+source_start]).unwrap();
                parent=Some(id);
            }
            conn.execute("UPDATE sessions SET leaf_message_id=?1 WHERE id=?2",params![parent,source_id]).unwrap();
            crate::commands::memory::resolve_reply_memory_controls(&conn,&source_id,Some(true),Some(true)).unwrap();
            }
            let due=crate::commands::memory::automatic_evolution_due(&db.lock().unwrap(),&source_id);
            let (provider,metrics)=boxed_bounded_provider(&model,app.handle(),&root,1);
            let started=Instant::now();
            // Deliberately invokes one bounded review, independently of cadence.
            // This measures extraction/adaptation, not the background scheduler.
            let result=crate::commands::memory::run_evolution_review_impl(db.clone(),&source_id,provider,crate::llm::resolve_thinking_settings(&model,Some("low")).unwrap()).await;
            let prefs=crate::agent::adaptive_constraints::learned_preferences(&db.lock().unwrap()).unwrap();
            let mut row=json!({"id":input.case.id,"arm":input.arm,"phase":"learn","model":model.model,
                "status":if result.is_ok(){"completed"}else{"invalid_review"},"forcedBoundedReview":true,
                "productionReviewDue":due,"storedPreferences":prefs});
            row_metrics(&mut row,&metrics,started);println!("PREFERENCE_EVAL_ROW {row}");
            if row["status"]!="completed" {return;}
            config.adaptive_constraints=crate::agent::adaptive_constraints::active_for_session(&db.lock().unwrap(),&source_id).unwrap();
            source_session=Some(source_id.clone());
            if input.arm=="main_grounded" {main_context=Some((state,source_id));}
            else if input.arm=="main_memory" {
                drop(state);drop(db);
                let mut state=crate::tests::test_app_state();
                state.db=Arc::new(Mutex::new(crate::db::init(&root.join("state.db")).unwrap()));
                crate::settings_cache::load_all(&state.db.lock().unwrap(),&state.cached_settings).unwrap();
                let directory=root.join("consumer");std::fs::create_dir(&directory).unwrap();
                let target=crate::commands::workspace::create_project_workspace_impl(&state,directory.to_string_lossy().into(),Some("Owned preference consumer".into()),None,None).unwrap().session.id;
                main_context=Some((state,target));
            }
        }
        let mut messages=vec![message("system",crate::agent::prompt::build_agent_system_prompt(&config,&[],&[]))];
        if input.arm=="history_reader" {
            messages.extend(input.case.history.iter().map(|s|message(s.role.as_str(),s.content.clone())));
        }
        let options=input.case.options.iter().enumerate().map(|(i,s)|format!("{}. {s}",(b'A'+i as u8) as char)).collect::<Vec<_>>().join("\n");
        let query=format!("{}\n\n{}\n\nReturn only <choice>X</choice>, replacing X with A, B, C or D. Do not explain.",input.case.question,options);
        messages.push(message("user",query.clone()));
        let (provider,metrics)=boxed_bounded_provider(&model,app.handle(),&root,1);
        let started=Instant::now();
        let mut row=json!({"id":input.case.id,"arm":input.arm,"phase":"answer","model":model.model,
            "status":"invalid_response","answer":null,"activePreferenceCount":config.adaptive_constraints.len(),
            "explicitPreferenceCount":config.adaptive_constraints.iter().filter(|p|p.explicit).count(),"toolCount":0});
        if let Some((state,session))=main_context {
            row["sourceSession"]=json!(source_session);row["session"]=json!(session);
            let directory=root.join(if input.arm=="main_memory" {"consumer"} else {"workspace"});
            let provider=super::continuous_main::text_only_provider(provider,directory,&query);
            let request:SendMessageRequest=serde_json::from_value(json!({"sessionId":session,"role":"user","content":query,
                "preferences":{"useLongTermMemory":true,"evolutionEnabled":true}})).unwrap();
            let probe=Mutex::new(serde_json::Value::Null);
            if let Ok(reply)=super::continuous_main::main_turn(&state,Some(app.handle()),&session,request,provider,&model,Some(&probe),None).await {
                row["answer"]=json!(reply.content);
                let conn=state.db.lock().unwrap();
                let status:String=conn.query_row("SELECT status FROM task_runs WHERE id=?1",[&reply.id],|r|r.get(0)).unwrap();
                row["durableStatus"]=json!(status);
                row["toolCount"]=json!(conn.query_row::<i64,_,_>("SELECT count(*) FROM agent_run_events WHERE run_id=?1 AND event_type IN ('ToolExecutionStart','ExternalToolDispatchReserved')",[reply.id],|r|r.get(0)).unwrap());
                if status=="completed" && row["toolCount"]==0 {row["status"]=json!("completed");}
            }
            row["workingView"]=probe.into_inner().unwrap();
        } else {
            answer(provider.as_ref(),&messages,&mut row).await;
        }
        row_metrics(&mut row,&metrics,started);println!("PREFERENCE_EVAL_ROW {row}");
    });
}
