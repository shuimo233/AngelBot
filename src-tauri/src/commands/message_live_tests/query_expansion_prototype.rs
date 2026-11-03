//! Throwaway query-only expansion experiment. No production registration, Main
//! context, ordinary database, tools, desktop or background learning is used.
use super::agent_benchmarks::{boxed_bounded_provider, row_metrics};
use super::*;
use crate::agent::adaptive_constraints::{
    find_learned_preferences, learned_preferences, record_explicit_preference,
};
use crate::llm::{LlmResponse, Message, StopReason};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    key: String,
    content: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    query: String,
    context: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    version: u32,
    live: bool,
    prompt: String,
    records: Vec<Record>,
    cases: Vec<Case>,
}

fn alternatives(text: &str, original: &str) -> Result<Vec<String>, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Expansion {
        queries: Vec<String>,
    }
    let parsed: Expansion =
        serde_json::from_str(text).map_err(|_| "Expansion is not the declared JSON object")?;
    if parsed.queries.is_empty() || parsed.queries.len() > 5 {
        return Err("Expected 1-5 query hints".into());
    }
    let mut seen = HashSet::from([original.trim().to_lowercase()]);
    let mut result = Vec::new();
    for query in parsed.queries {
        let query = query.trim();
        if !(2..=24).contains(&query.chars().count()) {
            return Err("Query hint length is outside 2-24 characters".into());
        }
        if seen.insert(query.to_lowercase()) {
            result.push(query.to_string());
        }
    }
    if result.is_empty() {
        return Err("No alternative query was supplied".into());
    }
    Ok(result)
}

fn message(role: &str, content: &str) -> Message {
    Message {
        role: role.into(),
        content: content.into(),
        tool_calls: None,
        tool_call_id: None,
        protocol_state: None,
        tool_images: Vec::new(),
    }
}

fn snapshot(conn: &rusqlite::Connection) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&learned_preferences(conn).unwrap()).unwrap())
    )
}

fn candidates(
    conn: &rusqlite::Connection,
    keys: &HashMap<String, String>,
    query: &str,
) -> (Vec<Value>, bool) {
    let found = find_learned_preferences(conn, query, 5).unwrap();
    (
        found
            .records
            .into_iter()
            .map(|p| json!({"id":p.id,"key":keys[&p.id],"quote":p.content}))
            .collect(),
        found.partial,
    )
}

#[test]
#[ignore = "query-expansion prototype; explicit offline or saved-Luna run only"]
fn configured_query_expansion_prototype() {
    assert_eq!(
        std::env::var("ANGELBOT_CONTEXT_EVAL_LIVE").as_deref(),
        Ok("1")
    );
    let root = PathBuf::from(std::env::var_os("ANGELBOT_PRIVATE_CONTEXT_EVAL_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    let logs = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("logs")
        .canonicalize()
        .unwrap();
    assert_eq!(root.parent(), Some(logs.as_path()));
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("query-expansion-prototype-"));
    let input: Input =
        serde_json::from_slice(&std::fs::read(root.join("input.json")).unwrap()).unwrap();
    assert_eq!(input.version, 1);
    assert!((1..=5).contains(&input.records.len()) && (1..=5).contains(&input.cases.len()));
    assert!(input.prompt.chars().count() <= 1200);
    assert!(input
        .cases
        .iter()
        .all(|c| !c.query.trim().is_empty() && c.query.chars().count() <= 160));
    assert!(input.cases.iter().all(|c| c
        .context
        .as_ref()
        .map_or(true, |s| !s.trim().is_empty() && s.chars().count() <= 240)));
    assert_eq!(
        input
            .cases
            .iter()
            .map(|c| &c.id)
            .collect::<HashSet<_>>()
            .len(),
        input.cases.len()
    );
    unsafe {
        crate::vector_store::load_extension_globally();
    }
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::db::migrate(&conn).unwrap();
    let keys = {
        conn.execute("INSERT INTO sessions(id,title,created_at,updated_at) VALUES('expansion-owned','Manual prototype records',1,1)",[]).unwrap();
        let mut keys = HashMap::new();
        for record in &input.records {
            conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES(?1,'expansion-owned','user',?2,1)",params![record.key,record.content]).unwrap();
            assert!(record_explicit_preference(
                &conn,
                "expansion-owned",
                &record.key,
                &record.content,
                &record.content
            )
            .unwrap());
            let id: String = conn
                .query_row(
                    "SELECT id FROM adaptive_constraints WHERE constraint_value=?1",
                    [&record.content],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(keys.insert(id, record.key.clone()).is_none());
        }
        keys
    };
    let before = snapshot(&conn);
    // An offline run must not construct a credential host or resolve a model.
    let app = input.live.then(|| headless_credential_host().unwrap());
    let model = app.as_ref().map(|app| {
        let mut model =
            crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
                .unwrap();
        assert_eq!(model.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
        assert!(model.model.contains("luna"));
        model.max_tokens = model.max_tokens.min(512);
        model.temperature = 0.0;
        model
    });
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut used=0;
        for case in &input.cases {
            let (baseline, partial)=candidates(&conn,&keys,&case.query);
            let mut row=json!({"id":case.id,"query":case.query,"baseline":baseline,"baselinePartial":partial,
                "final":baseline,"queries":[],"rejectedPartial":[],"requestCount":0,"inputTokens":0,"outputTokens":0,
                "providerInputs":[],"providerOutputs":[],"status":"skipped_hit"});
            let request_text=if let Some(context)=&case.context {
                let (local,partial)=candidates(&conn,&keys,&format!("{} {}",case.query,context));
                row["localContext"]=json!(local);row["localContextPartial"]=json!(partial);
                json!({"query":case.query,"task_context":context}).to_string()
            } else {case.query.clone()};
            if row["baseline"].as_array().unwrap().is_empty() {
                row["status"]=json!("would_expand");
                if let (Some(app),Some(model))=(&app,&model) {
                    if used>=4 {row["status"]=json!("budget_exceeded"); println!("QUERY_EXPANSION_ROW {row}");break;}
                    let (provider,metrics)=boxed_bounded_provider(model,app.handle(),&root,1);
                    let started=Instant::now();
                    let thinking=crate::llm::resolve_thinking_settings(model,Some("low")).unwrap();
                    let response=provider.chat(&[message("system",&input.prompt),message("user",&request_text)],None,thinking.as_ref()).await;
                    let hints=match response {
                        Ok(response)=>match response.into_parts().0 {
                            LlmResponse::Text{text,stop_reason:StopReason::EndTurn}=>alternatives(&text,&case.query),
                            _=>Err("Expansion did not finish as plain text".into()),
                        },
                        Err(_)=>Err("Provider request failed; no retry".into()),
                    };
                    row["model"]=json!(model.model);
                    row["status"]=json!("completed");
                    match hints {
                        Ok(hints)=>{
                            let mut found=Vec::new();let mut rejected=Vec::new();let mut seen=HashSet::new();
                            for query in &hints {
                                let (records,partial)=candidates(&conn,&keys,query);
                                if partial {rejected.extend(records);continue;}
                                for record in records {if seen.insert(record["id"].as_str().unwrap().to_string()) && found.len()<5 {found.push(record);}}
                            }
                            row["queries"]=json!(hints);row["final"]=json!(found);row["rejectedPartial"]=json!(rejected);
                        },
                        Err(error)=>{row["status"]=json!("invalid_expansion");row["error"]=json!(error);},
                    }
                    row_metrics(&mut row,&metrics,started);used+=row["requestCount"].as_u64().unwrap();
                }
            }
            row["storeUnchanged"]=json!(snapshot(&conn)==before);
            println!("QUERY_EXPANSION_ROW {row}");
            if !matches!(row["status"].as_str(),Some("completed"|"skipped_hit"|"would_expand")) {break;}
        }
    });
}

#[test]
fn query_expansion_hints_are_bounded_data_not_source_statements() {
    assert_eq!(
        alternatives(r#"{"queries":["green tea","绿茶","绿茶"]}"#, "green tea").unwrap(),
        vec!["绿茶"]
    );
    for invalid in [
        r#"{"queries":[]}"#,
        r#"{"queries":["茶"]}"#,
        r#"{"queries":["same"]}"#,
        r#"{"queries":["a1","b2","c3","d4","e5","f6"]}"#,
        r#"{"queries":["绿茶"],"preference":"inferred fact"}"#,
    ] {
        assert!(alternatives(invalid, "same").is_err());
    }
    assert!(alternatives(&json!({"queries":["x".repeat(25)]}).to_string(), "same").is_err());
}
