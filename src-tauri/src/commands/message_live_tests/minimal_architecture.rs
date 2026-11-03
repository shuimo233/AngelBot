//! Throwaway combined core experiment, not a new production memory framework.
//! One real evolution review, then cold-open AgentRunner + production read-only
//! memory tools. No coordinator, background cadence, desktop or business writes.
use super::agent_benchmarks::{boxed_bounded_provider, row_metrics};
use super::*;
use crate::agent::adaptive_constraints::{
    learned_preferences, project_for_session, project_for_task, record_explicit_preference,
};
use crate::agent::config::{AdaptiveConstraint, AgentConfig};
use crate::agent::{AgentResponse, AgentSliceOutcome, ToolRegistry};
use crate::llm::{LlmFailure, LlmProvider, LlmResponse, Message, ThinkingSettings, ToolSchema};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Instant;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    question: String,
    clock: Option<String>,
    prefix: Option<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    version: u32,
    sources: Vec<String>,
    noise: Vec<String>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SequenceStage {
    id: String,
    sources: Vec<String>,
    probes: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SequenceInput {
    version: u32,
    noise: Vec<String>,
    stages: Vec<SequenceStage>,
    model: Option<String>,
}

impl SequenceInput {
    fn validate(&self) -> (bool, bool) {
        assert!([2, 5, 7, 8, 9].contains(&self.version));
        let transfer = self.version == 5;
        let temporal = [7, 8, 9].contains(&self.version);
        assert_eq!(self.noise.len(), if temporal { 0 } else { 36 });
        assert_eq!(
            self.stages.len(),
            if temporal {
                2
            } else if transfer {
                4
            } else {
                5
            }
        );
        let source_count = match self.version {
            8 | 9 => 3,
            7 => 2,
            5 => 8,
            _ => 5,
        };
        assert_eq!(
            self.stages.iter().map(|s| s.sources.len()).sum::<usize>(),
            source_count
        );
        assert_eq!(
            self.stages.iter().map(|s| s.probes.len()).sum::<usize>(),
            if temporal { 6 } else { 7 }
        );
        assert!(self
            .stages
            .iter()
            .flat_map(|s| &s.sources)
            .chain(&self.noise)
            .all(|s| s.chars().count() <= 160));
        assert!(self
            .stages
            .iter()
            .flat_map(|s| &s.probes)
            .all(|c| c.question.chars().count() <= 300
                && if temporal {
                    c.clock.as_ref().is_some_and(|clock| {
                        clock.len() <= 40 && chrono::DateTime::parse_from_rfc3339(clock).is_ok()
                    })
                } else {
                    c.clock.is_none()
                }));
        if temporal {
            assert!(self
                .model
                .as_ref()
                .is_some_and(|m| !m.is_empty() && m.len() <= 200));
        }
        (transfer, temporal)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StaleInput {
    version: u32,
    live: bool,
    model: String,
    cases: Vec<Case>,
    counter_sources: Option<Vec<CounterSource>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CounterSource {
    id: String,
    quotes: [String; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderInput {
    version: u32,
    live: bool,
    model: String,
    frame: serde_json::Value,
}

fn order_frame(frame: &serde_json::Value, matched_first: bool) -> serde_json::Value {
    assert!(frame.to_string().len() <= 64 * 1024);
    assert_eq!(
        frame["resolvedThinking"],
        json!({"protocol":"openai_responses","effort":"medium","budgetTokens":null})
    );
    assert_eq!(
        frame["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["system", "user", "assistant", "tool"]
    );
    assert_eq!(
        frame["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "read_conversation",
            "recall_conversation",
            "recall_memories"
        ]
    );
    let calls = frame["messages"][2]["toolCalls"].as_array().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["name"], "recall_conversation");
    assert_eq!(calls[0]["id"], frame["messages"][3]["toolCallId"]);
    let mut result = frame.clone();
    let output = frame["messages"][3]["content"]
        .as_str()
        .unwrap()
        .strip_prefix("Result: ")
        .unwrap();
    let mut output: serde_json::Value = serde_json::from_str(output).unwrap();
    assert_eq!(output["historical_data"], true);
    let sources = output["sources"].as_array_mut().unwrap();
    assert!((2..=10).contains(&sources.len()));
    assert!(sources
        .iter()
        .all(|s| s["matched"].is_boolean() && s["text"].is_string()));
    if matched_first {
        sources.sort_by_key(|s| s["matched"] != true); // Stable within each group.
        result["messages"][3]["content"] = json!(format!("Result: {output}"));
    }
    result
}

fn complete_order_response(response: LlmResponse) -> bool {
    let (response, protocol) = response.into_parts();
    if protocol.is_some_and(|p| p.validate().is_err()) {
        return false;
    }
    match response {
        LlmResponse::Text {
            stop_reason: crate::llm::StopReason::EndTurn,
            ..
        } => true,
        LlmResponse::ToolCalls {
            calls,
            stop_reason: crate::llm::StopReason::EndTurn | crate::llm::StopReason::ToolUse,
            ..
        } => !calls.is_empty(),
        _ => false,
    }
}

const LOOKUP_CURRENT: &str = "When current or historical preferences matter and authorized lookup is available, use recall_memories (user-stated preferences are included by default); set include_learned_habits=true only to inspect inferred habits.";
const LOOKUP_AWARE: &str = "A host lookup for this request already placed up to two matching user-stated candidates first; the rest are recent records. Use relevant shown originals when sufficient for this task; use recall_memories for missing details or unresolved later corrections and conditions. Do not repeat lookup solely because the view has omissions. User-stated preferences are included by default; set include_learned_habits=true only to inspect inferred habits.";

fn completed_lookup_contract(system: &str) -> String {
    assert_eq!(system.matches(LOOKUP_CURRENT).count(), 1);
    system.replace(LOOKUP_CURRENT, LOOKUP_AWARE)
}

fn lookup_frame(frame: &serde_json::Value, aware: bool) -> serde_json::Value {
    assert!(frame.to_string().len() <= 64 * 1024);
    assert_eq!(
        frame["resolvedThinking"],
        json!({"protocol":"openai_responses","effort":"medium","budgetTokens":null})
    );
    assert_eq!(
        frame["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["system", "user"]
    );
    assert_eq!(
        frame["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "read_conversation",
            "recall_conversation",
            "recall_memories"
        ]
    );
    let system = frame["messages"][0]["content"].as_str().unwrap();
    assert_eq!(system.matches(LOOKUP_CURRENT).count(), 1);
    let mut result = frame.clone();
    if aware {
        result["messages"][0]["content"] = json!(completed_lookup_contract(system));
    }
    result
}

fn run_order_view(root: &Path, input: OrderInput) {
    assert!([14, 19].contains(&input.version));
    let frames = [false, true].map(|changed| {
        if input.version == 19 {
            lookup_frame(&input.frame, changed)
        } else {
            order_frame(&input.frame, changed)
        }
    });
    if !input.live {
        return;
    }
    let app = headless_credential_host().unwrap();
    let model = source_model(app.handle(), &input.model);
    let thinking = crate::llm::resolve_thinking_settings(&model, Some("medium"))
        .unwrap()
        .expect("The recorded order screen requires actual medium thinking");
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut total_input = 0;
        for (index, arm) in [0, 1, 1, 0].into_iter().enumerate() {
            if total_input >= 30_000 { return; }
            let frame = &frames[arm];
            let messages = frame["messages"].as_array().unwrap().iter().map(|m| serde_json::from_value::<Message>(json!({
                "role":m["role"],"content":m["content"],"tool_calls":m["toolCalls"],"tool_call_id":m["toolCallId"]
            })).unwrap()).collect::<Vec<_>>();
            let tools: Vec<ToolSchema> = serde_json::from_value(frame["tools"].clone()).unwrap();
            let (provider, metrics) = boxed_bounded_provider(&model, app.handle(), root, 1);
            let started = Instant::now();
            // Reconstruct only the recorded public frame, not hidden protocol.
            // Exactly one inference; returned tool calls are never dispatched.
            let result = tokio::time::timeout(Duration::from_secs(90), provider.chat_classified(&messages, Some(&tools), Some(&thinking))).await;
            let complete = match result { Ok(Ok(response)) => complete_order_response(response), _ => false };
            let name = if input.version==19 {if arm==0 {"current_contract"}else{"lookup_aware"}}else if arm==0 {"chronological"}else{"matched_first"};
            let mut row = json!({"id":index,"arm":name,"model":model.model,"status":if complete {"completed"}else{"unavailable"},"toolExecutions":0});
            row_metrics(&mut row, &metrics, started);
            total_input += row["inputTokens"].as_u64().unwrap();
            let metered = row["reportedUsageCount"] == row["requestCount"];
            println!("MINIMAL_ARCHITECTURE_ROW {row}");
            if row["status"] != "completed" || !metered { return; }
        }
    });
}

fn validate_stability_input(input: &StaleInput) {
    assert!([11, 12, 13, 15, 16, 17, 18, 20].contains(&input.version));
    let source_readers = input.version >= 12;
    assert_eq!(
        input.cases.len(),
        match input.version {
            15..=18 => 4,
            13 => 3,
            12 => 2,
            _ => 6,
        }
    );
    assert!(!input.model.is_empty() && input.model.len() <= 200);
    let mut ids = std::collections::HashSet::new();
    assert!(input.cases.iter().all(|case| {
        !case.id.is_empty()
            && ids.insert(&case.id)
            && !case.question.is_empty()
            && case.question.chars().count() <= 300
            && if source_readers && input.version < 16 {
                case.prefix == Some(1)
            } else {
                matches!(case.prefix, Some(0 | 1))
            }
            && case.clock.as_ref().is_some_and(|clock| {
                clock.len() <= 40 && chrono::DateTime::parse_from_rfc3339(clock).is_ok()
            })
    }));
    if matches!(input.version, 16..=18) {
        let sources = input.counter_sources.as_ref().unwrap();
        assert_eq!(sources.len(), 2);
        assert_ne!(sources[0].id, sources[1].id);
        for source in sources {
            assert!(
                !source.id.is_empty()
                    && source.id.len() <= 64
                    && source
                        .id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            );
            assert_ne!(source.quotes[0], source.quotes[1]);
            assert!(source
                .quotes
                .iter()
                .all(|s| (4..=160).contains(&s.chars().count())));
            let pair = [0, 1].map(|p| {
                input
                    .cases
                    .iter()
                    .find(|c| c.id == format!("{}_{}", source.id, p) && c.prefix == Some(p))
                    .unwrap()
            });
            assert_eq!(pair[0].question, pair[1].question);
            assert_eq!(pair[0].clock, pair[1].clock);
        }
    } else {
        assert!(input.counter_sources.is_none());
    }
}

/// Manually nominate synthetic declarations through the existing source guard.
/// Both profiles clone the same archive; nothing touches its original files.
fn prepare_counter_profiles(root: &Path, input: &StaleInput) -> serde_json::Value {
    validate_stability_input(input);
    assert!(matches!(input.version, 16..=18));
    let mut prepared = serde_json::Map::new();
    for prefix in 0..2 {
        let directory = root
            .join(format!("prefix-{prefix}"))
            .canonicalize()
            .unwrap();
        assert_eq!(directory.parent(), Some(root));
        let path = directory.join("state.db").canonicalize().unwrap();
        assert_eq!(path, directory.join("state.db"));
        let conn = crate::db::init(&path).unwrap();
        let session: String = conn
            .query_row(
                "SELECT session_id FROM messages WHERE id='seq-0-0'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let start: i64 = conn
            .query_row(
                "SELECT created_at+1 FROM messages WHERE id='temporal-1-0'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut parent: Option<String> = conn
            .query_row(
                "SELECT leaf_message_id FROM sessions WHERE id=?1",
                [&session],
                |r| r.get(0),
            )
            .unwrap();
        let mut proofs = serde_json::Map::new();
        let mut ids = Vec::new();
        let mut messages = Vec::new();
        for (index, source) in input.counter_sources.as_ref().unwrap().iter().enumerate() {
            let quote = &source.quotes[prefix];
            conn.execute("INSERT INTO messages(id,session_id,role,content,parent_id,created_at) VALUES(?1,?2,'user',?3,?4,?5)",params![source.id,session,quote,parent,start+index as i64]).unwrap();
            assert!(record_explicit_preference(&conn, &session, &source.id, quote, quote).unwrap());
            let id: String = conn.query_row("SELECT id FROM adaptive_constraints WHERE constraint_value=?1 AND source='explicit_user_preference' AND status='active'",[quote],|r|r.get(0)).unwrap();
            proofs.insert(
                format!("{}_{prefix}", source.id),
                json!({"sourceIds":[id],"messageIds":[source.id],"quote":quote}),
            );
            ids.push(id);
            messages.push(source.id.clone());
            parent = Some(source.id.clone());
        }
        conn.execute(
            "UPDATE sessions SET leaf_message_id=?1 WHERE id=?2",
            params![parent, session],
        )
        .unwrap();
        let projection = project_for_session(&conn, &session).unwrap();
        assert_eq!(projection.records.len(), 12);
        assert!(proofs.values().all(|p| projection
            .records
            .iter()
            .all(|r| !r.value.contains(p["quote"].as_str().unwrap()))));
        prepared.insert(prefix.to_string(),json!({"sha256":archive_hash(&conn),"sourceIds":ids,"messageIds":messages,"caseProofs":proofs}));
    }
    json!(prepared)
}

fn read_only_registry(db: Arc<Mutex<rusqlite::Connection>>) -> ToolRegistry {
    let mut registry = ToolRegistry::empty();
    crate::agent::extensions::memory::register(&mut registry, db);
    for name in ["remember_fact", "update_memory", "forget_memory"] {
        registry.unregister(name);
    }
    assert_eq!(registry.len(), 1);
    assert!(registry.contains("recall_memories"));
    registry
}

fn source_read_only_registry(db: Arc<Mutex<rusqlite::Connection>>, source: &str) -> ToolRegistry {
    let mut registry = read_only_registry(db.clone());
    crate::commands::foreground_context_recall::register(&mut registry, db, source.into());
    assert_eq!(registry.len(), 3);
    registry
}

const PREFERENCE_STATE_QUERIES: [&str; 3] = [
    "SELECT json_group_array(json_object('id',id,'scope',scope,'category',category,'text',content,'importance',importance,'frequency',frequency,'stage',forget_stage,'superseded',superseded_by,'time',updated_at)) FROM (SELECT * FROM memories ORDER BY id)",
    "SELECT json_group_array(json_object('id',id,'key',constraint_key,'text',constraint_value,'scope',scope,'expires',expires_at,'confidence',confidence,'status',status,'source',source,'time',updated_at)) FROM (SELECT * FROM adaptive_constraints ORDER BY id)",
    "SELECT json_group_array(json_object('id',id,'key',constraint_key,'session',session_id,'source',source,'time',observed_at)) FROM (SELECT * FROM adaptive_constraint_evidence ORDER BY id)",
];

fn state_hash(
    conn: &rusqlite::Connection,
    queries: impl IntoIterator<Item = &'static str>,
) -> String {
    let mut hash = Sha256::new();
    for sql in queries {
        let data: String = conn.query_row(sql, [], |r| r.get(0)).unwrap();
        hash.update(data.as_bytes());
        hash.update([0]);
    }
    format!("{:x}", hash.finalize())
}

fn preference_state_hash(conn: &rusqlite::Connection) -> String {
    // Review cursors may advance without changing any retained preference.
    state_hash(conn, PREFERENCE_STATE_QUERIES)
}

fn archive_hash(conn: &rusqlite::Connection) -> String {
    // Reader/prefix evidence also includes control state and the source graph.
    state_hash(conn, [
        "SELECT json_group_array(json_object('id',id,'leaf',leaf_message_id,'metadata',metadata)) FROM (SELECT * FROM sessions ORDER BY id)",
        "SELECT json_group_array(json_object('id',id,'session',session_id,'parent',parent_id,'role',role,'text',content,'time',created_at)) FROM (SELECT * FROM messages ORDER BY id)",
    ].into_iter().chain(PREFERENCE_STATE_QUERIES))
}

fn frozen_clock(text: &str, clock: &str) -> String {
    let Some((before, rest)) = text.split_once("## 当前时间\n本机当前时间为 ") else {
        return text.into();
    };
    let (_, after) = rest.split_once("。解释").expect("known clock layer");
    format!("{before}## 当前时间\n本机当前时间为 {clock}。解释{after}")
}

fn local_clock(clock: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(clock)
        .expect("validated source instant")
        .format("%Y年%m月%d日%H时%M分%S秒（UTC%:z）")
        .to_string()
}

struct PairedClock {
    inner: Box<dyn LlmProvider>,
    clock: String,
    lookup_aware: bool,
}

pub(super) fn with_frozen_clock(inner: Box<dyn LlmProvider>, clock: &str) -> Box<dyn LlmProvider> {
    Box::new(PairedClock {
        inner,
        clock: clock.into(),
        lookup_aware: false,
    })
}

#[async_trait::async_trait]
impl LlmProvider for PairedClock {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn latest_usage(&self) -> Option<crate::llm::ProviderUsage> {
        self.inner.latest_usage()
    }
    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, String> {
        self.chat_classified(messages, tools, thinking)
            .await
            .map_err(|e| e.diagnostic)
    }
    async fn chat_classified(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, LlmFailure> {
        let mut messages = messages.to_vec();
        for m in &mut messages {
            if m.role == "system" {
                m.content = frozen_clock(&m.content, &self.clock);
                if self.lookup_aware {
                    m.content = completed_lookup_contract(&m.content);
                }
            }
        }
        // Freeze before instrumentation, so receipts are the actual sent input.
        self.inner.chat_classified(&messages, tools, thinking).await
    }
}

/// Shared by both frozen experiments; only the projection size varies. Every
/// reader cold-opens the same owned database and gets the real read-only tool.
struct Reader<'a> {
    root: &'a Path,
    source: &'a str,
    model: &'a crate::llm::ProviderConfig,
    app: &'a tauri::AppHandle,
    clock: &'a str,
    work_dir: Option<&'a Path>,
    task_view: bool,
}

impl Reader<'_> {
    async fn probe(&self, case: &Case, arm: &str, limit: Option<usize>) -> serde_json::Value {
        self.probe_effort(case, arm, limit, "low").await
    }

    async fn probe_effort(
        &self,
        case: &Case,
        arm: &str,
        limit: Option<usize>,
        effort: &str,
    ) -> serde_json::Value {
        self.probe_surface(case, arm, limit, effort, false, false)
            .await
    }

    async fn probe_surface(
        &self,
        case: &Case,
        arm: &str,
        limit: Option<usize>,
        effort: &str,
        source_readers: bool,
        lookup_aware: bool,
    ) -> serde_json::Value {
        assert!(
            !lookup_aware || self.task_view,
            "Cannot claim a lookup without performing it for this task"
        );
        let db = Arc::new(Mutex::new(
            crate::db::init(&self.root.join("state.db")).unwrap(),
        ));
        let before = archive_hash(&db.lock().unwrap());
        let mut config = AgentConfig::default();
        config.work_dir = Some(self.work_dir.unwrap_or(self.root).into());
        config.preferences.use_long_term_memory = true;
        config.preferences.evolution_enabled = true;
        let all = learned_preferences(&db.lock().unwrap())
            .unwrap()
            .into_iter()
            .filter(|p| p.ready)
            .collect::<Vec<_>>();
        let mut projection = if self.task_view {
            project_for_task(&db.lock().unwrap(), self.source, &case.question).unwrap()
        } else {
            project_for_session(&db.lock().unwrap(), self.source).unwrap()
        };
        assert!(all.len() < 50 && all.len() == projection.records.len() + projection.omitted);
        if let Some(limit) = limit {
            assert!((1..=12).contains(&limit));
            projection.records.truncate(limit);
            config.adaptive_constraints = projection.records;
            config.adaptive_constraints_omitted = all.len() - config.adaptive_constraints.len();
        } else {
            config.adaptive_constraints = all
                .iter()
                .map(|p| AdaptiveConstraint {
                    key: format!("interaction_preference:{}", p.id),
                    value: p.content.clone(),
                    scope: "global".into(),
                    confidence: 1.0,
                    explicit: p.explicit,
                })
                .collect();
        }
        let visible = config
            .adaptive_constraints
            .iter()
            .map(|p| p.value.clone())
            .collect::<Vec<_>>();
        let omitted = config.adaptive_constraints_omitted;
        let registry = if source_readers {
            source_read_only_registry(db.clone(), self.source)
        } else {
            read_only_registry(db.clone())
        };
        // The source surface can require search, memory lookup, correction
        // lookup, then one final answer. The original single-tool study keeps 3.
        let call_budget = if source_readers { 4 } else { 3 };
        let (provider, metrics) =
            boxed_bounded_provider(self.model, self.app, self.root, call_budget);
        let runner = AgentRunner::with_registry(
            Box::new(PairedClock {
                inner: provider,
                clock: case.clock.as_deref().unwrap_or(self.clock).into(),
                lookup_aware,
            }),
            config,
            registry,
        )
        .with_thinking_settings(
            crate::llm::resolve_thinking_settings(self.model, Some(effort)).unwrap(),
        );
        let started = Instant::now();
        let mut row = json!({"id":case.id,"arm":arm,"model":self.model.model,"status":"timed_out",
            "visible":visible,"omitted":omitted,"archiveRecords":all.len(),"coldOpened":true,"archiveSha256":before.clone(),"requestedEffort":effort,"callBudget":call_budget,
            "clock":case.clock.as_deref().unwrap_or(self.clock),"historyMessages":0,"text":null,"steps":[]});
        if source_readers {
            row["sourceReaders"] = json!(true);
        }
        row["taskView"] = json!(self.task_view);
        row["lookupAware"] = json!(lookup_aware);
        if let Ok(outcome) = tokio::time::timeout(
            Duration::from_secs(120),
            runner.run_slice(&case.question, &[]),
        )
        .await
        {
            let (status, response) = match outcome {
                AgentSliceOutcome::Completed(r) => ("completed", r),
                AgentSliceOutcome::AwaitingConfirmation(r) => ("awaiting_confirmation", r),
                AgentSliceOutcome::NeedsAttention(r) => ("needs_attention", r),
                AgentSliceOutcome::Stopped(r) => ("stopped", r),
            };
            row["status"] = json!(status);
            match response {
                AgentResponse::TextWithToolCalls { text, steps } => {
                    row["text"] = json!(text);
                    row["steps"] = json!(steps);
                }
                AgentResponse::Text(text) => row["text"] = json!(text),
                AgentResponse::Error(_) => row["status"] = json!("runner_error"),
            }
        }
        row["archiveUnchanged"] = json!(before == archive_hash(&db.lock().unwrap()));
        row_metrics(&mut row, &metrics, started);
        row
    }
}

#[test]
fn minimal_architecture_contract_is_read_only_and_freezes_only_system_clock() {
    assert_eq!(
        local_clock("2026-11-05T18:29:00+08:00"),
        "2026年11月05日18时29分00秒（UTC+08:00）"
    );
    assert_eq!(
        local_clock("2026-11-05T03:59:00-06:30"),
        "2026年11月05日03时59分00秒（UTC-06:30）"
    );
    assert!(
        serde_json::from_value::<Case>(json!({"id":"x","question":"q","expected":"gold"})).is_err()
    );
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let registry = read_only_registry(Arc::new(Mutex::new(conn)));
    assert_eq!(
        registry.get("recall_memories").unwrap().name,
        "recall_memories"
    );
    let source_registry = source_read_only_registry(
        Arc::new(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        "host-selected-source",
    );
    assert_eq!(source_registry.len(), 3);
    for name in [
        "recall_memories",
        "recall_conversation",
        "read_conversation",
    ] {
        assert!(!source_registry.get(name).unwrap().requires_confirmation);
    }
    for name in ["recall_conversation", "read_conversation"] {
        assert!(source_registry.get(name).unwrap().read_only_budget_hint);
    }
    for name in ["remember_fact", "update_memory", "forget_memory"] {
        assert!(!source_registry.contains(name));
    }
    let original = "prefix\n## 当前时间\n本机当前时间为 old。解释relative\n\npolicy";
    assert_eq!(
        frozen_clock(original, "fixed"),
        "prefix\n## 当前时间\n本机当前时间为 fixed。解释relative\n\npolicy"
    );
    assert_eq!(
        frozen_clock("unrelated source", "fixed"),
        "unrelated source"
    );
    for version in [7, 8, 9] {
        let probes = (0..3)
            .map(|i| json!({"id":i.to_string(),"question":"q","clock":"2026-11-01T00:00:00+08:00"}))
            .collect::<Vec<_>>();
        let mut input: SequenceInput = serde_json::from_value(
            json!({"version":version,"noise":[],"model":"fixture-luna","stages":[
            {"id":"future","sources":vec!["source";if version>=8 {2}else{1}],"probes":probes},
            {"id":"withdraw","sources":["cancel"],"probes":probes}]}),
        )
        .unwrap();
        assert_eq!(input.validate(), (false, true));
        input.stages[0].probes[0].clock = None;
        assert!(std::panic::catch_unwind(|| input.validate()).is_err());
    }
    let stable = json!({"version":11,"live":false,"model":"fixture-luna","cases":
        (0..6).map(|i| json!({"id":format!("stable-{i}"),"question":"q","prefix":i%2,
            "clock":"2026-11-05T18:29:00+08:00"})).collect::<Vec<_>>()});
    validate_stability_input(&serde_json::from_value(stable.clone()).unwrap());
    let mut guards = stable.clone();
    guards["version"] = json!(20);
    validate_stability_input(&serde_json::from_value(guards.clone()).unwrap());
    guards["counter_sources"] = json!([]);
    assert!(std::panic::catch_unwind(|| validate_stability_input(
        &serde_json::from_value(guards).unwrap()
    ))
    .is_err());
    for (version, count) in [(12, 2), (13, 3), (15, 4)] {
        let source_stable = json!({"version":version,"live":false,"model":"fixture-luna","cases":
            (0..count).map(|i|json!({"id":format!("source-{i}"),"question":"q","prefix":1,
                "clock":"2026-11-06T12:00:00+08:00"})).collect::<Vec<_>>()});
        validate_stability_input(&serde_json::from_value(source_stable.clone()).unwrap());
        for fault in ["count", "prefix"] {
            let mut invalid = source_stable.clone();
            if fault == "count" {
                invalid["cases"].as_array_mut().unwrap().pop();
            } else {
                invalid["cases"][0]["prefix"] = json!(0);
            }
            let input: StaleInput = serde_json::from_value(invalid).unwrap();
            assert!(std::panic::catch_unwind(|| validate_stability_input(&input)).is_err());
        }
    }
    for fault in [
        "version",
        "count",
        "duplicate",
        "prefix",
        "missing_prefix",
        "clock",
        "missing_clock",
    ] {
        let mut invalid = stable.clone();
        match fault {
            "version" => invalid["version"] = json!(10),
            "count" => {
                invalid["cases"].as_array_mut().unwrap().pop();
            }
            "duplicate" => invalid["cases"][1]["id"] = invalid["cases"][0]["id"].clone(),
            "prefix" => invalid["cases"][0]["prefix"] = json!(2),
            "missing_prefix" => invalid["cases"][0]["prefix"] = json!(null),
            "clock" => invalid["cases"][0]["clock"] = json!("not an instant"),
            _ => invalid["cases"][0]["clock"] = json!(null),
        }
        let input: StaleInput = serde_json::from_value(invalid).unwrap();
        assert!(
            std::panic::catch_unwind(|| validate_stability_input(&input)).is_err(),
            "{fault}"
        );
    }
    let order = json!({"messages":[{"role":"system","content":"unchanged policy"},{"role":"user","content":"unchanged question"},
        {"role":"assistant","toolCalls":[{"id":"search-1","name":"recall_conversation","arguments":{"query":"original"}}]},
        {"role":"tool","toolCallId":"search-1","content":format!("Result: {}",json!({"historical_data":true,"sources":[
            {"message_id":"context","matched":false,"text":"original context"},
            {"message_id":"hit","matched":true,"text":"original hit"},
            {"message_id":"later","matched":false,"text":"original later context"}]}))}],
        "tools":[{"name":"read_conversation"},{"name":"recall_conversation"},{"name":"recall_memories"}],
        "resolvedThinking":{"protocol":"openai_responses","effort":"medium","budgetTokens":null}});
    assert_eq!(order_frame(&order, false), order);
    let mut lookup = order.clone();
    lookup["messages"].as_array_mut().unwrap().truncate(2);
    let mut config = AgentConfig::default();
    config.adaptive_constraints.push(AdaptiveConstraint {
        key: "interaction_preference:fixture".into(),
        value: "原话保持不变".into(),
        scope: "global".into(),
        confidence: 1.0,
        explicit: true,
    });
    config.adaptive_constraints_omitted = 34;
    lookup["messages"][0]["content"] = json!(crate::agent::prompt::build_agent_system_prompt(
        &config,
        &[],
        &[]
    ));
    assert_eq!(lookup_frame(&lookup, false), lookup);
    assert!(std::panic::catch_unwind(|| completed_lookup_contract(
        "No completed lookup contract here"
    ))
    .is_err());
    let mut annotated = lookup_frame(&lookup, true);
    assert!(annotated["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("unresolved later corrections and conditions"));
    annotated["messages"][0]["content"] = lookup["messages"][0]["content"].clone();
    assert_eq!(
        annotated, lookup,
        "No data, scope or tool change in the lookup-contract screen"
    );
    let changed = order_frame(&order, true);
    let output: serde_json::Value = serde_json::from_str(
        changed["messages"][3]["content"]
            .as_str()
            .unwrap()
            .strip_prefix("Result: ")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        output["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["message_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["hit", "context", "later"]
    );
    assert_eq!(changed["tools"], order["tools"]);
    for i in 0..3 {
        assert_eq!(changed["messages"][i], order["messages"][i]);
    }
    let text_response = || LlmResponse::Text {
        text: "original answer".into(),
        stop_reason: crate::llm::StopReason::EndTurn,
    };
    assert!(complete_order_response(text_response()));
    for stop_reason in [
        crate::llm::StopReason::EndTurn,
        crate::llm::StopReason::ToolUse,
        crate::llm::StopReason::MaxTokens,
    ] {
        assert_eq!(
            complete_order_response(LlmResponse::ToolCalls {
                calls: vec![crate::llm::ToolCall {
                    id: "lookup".into(),
                    name: "read_conversation".into(),
                    arguments: json!({})
                }],
                text: None,
                stop_reason
            }),
            stop_reason != crate::llm::StopReason::MaxTokens
        );
    }
    let protocol = crate::llm::ProtocolContinuation {
        protocol: crate::llm::ModelProtocol::OpenaiResponses,
        model: "fixture-luna".into(),
        credential_ref: "fixture-credential".into(),
        output_items: vec![json!({"type":"message"})],
    };
    assert!(complete_order_response(LlmResponse::WithProtocol {
        response: Box::new(text_response()),
        protocol: protocol.clone()
    }));
    let mut invalid = protocol;
    invalid.output_items.clear();
    assert!(!complete_order_response(LlmResponse::WithProtocol {
        response: Box::new(text_response()),
        protocol: invalid
    }));
    assert!(!complete_order_response(LlmResponse::Text {
        text: "incomplete".into(),
        stop_reason: crate::llm::StopReason::MaxTokens
    }));
    let mut stale_call = order.clone();
    stale_call["messages"][3]["toolCallId"] = json!("foreign-call");
    assert!(std::panic::catch_unwind(|| order_frame(&stale_call, true)).is_err());
    unsafe {
        crate::vector_store::load_extension_globally();
    }
    let data = tempfile::tempdir().unwrap();
    let conn = crate::db::init(&data.path().join("state.db")).unwrap();
    crate::db::migrate(&conn).unwrap();
    let original_hash = archive_hash(&conn);
    conn.execute("INSERT INTO memories(id,scope,category,content,importance,source,frequency,is_permanent,forget_stage,created_at,updated_at) VALUES('hash-probe','global','fact','probe',4,'user',1,0,'active',1,1)",[]).unwrap();
    assert_ne!(original_hash, archive_hash(&conn));
    conn.execute(
        "INSERT INTO sessions(id,title,created_at,updated_at) VALUES('hash-session','probe',1,1)",
        [],
    )
    .unwrap();
    for id in ["hash-a", "hash-b"] {
        conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES(?1,'hash-session','user','probe',1)",[id]).unwrap();
    }
    let root_hash = archive_hash(&conn);
    conn.execute(
        "UPDATE messages SET parent_id='hash-a' WHERE id='hash-b'",
        [],
    )
    .unwrap();
    assert_ne!(root_hash, archive_hash(&conn));
    let before_control = archive_hash(&conn);
    let before_preference = preference_state_hash(&conn);
    conn.execute("UPDATE sessions SET metadata=json_object('automaticEvolution',json_object('through_message_id','hash-b')) WHERE id='hash-session'",[]).unwrap();
    assert_ne!(before_control, archive_hash(&conn));
    assert_eq!(before_preference, preference_state_hash(&conn));
    let before_edit = archive_hash(&conn);
    conn.execute(
        "UPDATE memories SET category='preference',content='先给结论' WHERE id='hash-probe'",
        [],
    )
    .unwrap();
    assert_ne!(before_edit, archive_hash(&conn));
    assert_ne!(before_preference, preference_state_hash(&conn));
    let snapshots = tempfile::tempdir().unwrap();
    let snapshot = snapshots.path().join("prefix.db");
    conn.execute("VACUUM INTO ?1", [snapshot.to_str().unwrap()])
        .unwrap();
    let copied = rusqlite::Connection::open_with_flags(
        &snapshot,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(archive_hash(&conn), archive_hash(&copied));
    assert_eq!(preference_state_hash(&conn), preference_state_hash(&copied));
}

#[test]
#[ignore = "explicit saved Luna core evaluation; isolated data, no desktop"]
fn configured_minimal_architecture() {
    assert_eq!(
        std::env::var("ANGELBOT_CONTEXT_EVAL_LIVE").as_deref(),
        Ok("1")
    );
    let root =
        std::path::PathBuf::from(std::env::var("ANGELBOT_PRIVATE_CONTEXT_EVAL_ROOT").unwrap())
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
        .starts_with("minimal-architecture-"));
    let input: Input =
        serde_json::from_slice(&std::fs::read(root.join("input.json")).unwrap()).unwrap();
    assert_eq!(input.version, 1);
    assert_eq!(input.sources.len(), 3);
    assert_eq!(input.noise.len(), 36);
    assert_eq!(input.cases.len(), 4);
    assert!(input
        .sources
        .iter()
        .chain(&input.noise)
        .all(|s| s.chars().count() <= 160));
    assert!(input
        .cases
        .iter()
        .all(|c| c.question.chars().count() <= 300));
    let app = headless_credential_host().unwrap();
    let mut model =
        crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
            .unwrap();
    assert_eq!(model.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
    assert!(model.model.contains("luna"));
    model.max_tokens = model.max_tokens.min(2048);
    model.temperature = 0.0;
    let database = root.join("state.db");
    let clock = chrono::Local::now().to_rfc3339();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut state = crate::tests::test_app_state();
        state.db = Arc::new(Mutex::new(crate::db::init(&database).unwrap()));
        crate::db::migrate(&state.db.lock().unwrap()).unwrap();
        crate::settings_cache::load_all(&state.db.lock().unwrap(), &state.cached_settings).unwrap();
        let directory = root.join("source"); std::fs::create_dir(&directory).unwrap();
        let source = crate::commands::workspace::create_project_workspace_impl(&state, directory.to_string_lossy().into(), Some("Owned combined core".into()), None, None).unwrap().session.id;
        let start = chrono::Utc::now().timestamp() - 3600;
        {
            let conn = state.db.lock().unwrap();
            let mut parent = None::<String>;
            for (i, content) in input.sources.iter().enumerate() {
                let id = format!("original-{i}");
                conn.execute("INSERT INTO messages(id,session_id,role,content,parent_id,created_at) VALUES(?1,?2,'user',?3,?4,?5)",params![id,source,content,parent,start+i as i64]).unwrap();
                parent = Some(id);
            }
            conn.execute("UPDATE sessions SET leaf_message_id=?1 WHERE id=?2",params![parent,source]).unwrap();
            crate::commands::memory::resolve_reply_memory_controls(&conn,&source,Some(true),Some(true)).unwrap();
        }
        let (provider, metrics) = boxed_bounded_provider(&model, app.handle(), &root, 1);
        let started = Instant::now();
        let review = crate::commands::memory::run_evolution_review_impl(state.db.clone(), &source, provider, crate::llm::resolve_thinking_settings(&model,Some("low")).unwrap()).await;
        let learned = learned_preferences(&state.db.lock().unwrap()).unwrap();
        let mut row = json!({"id":"learn","status":if review.is_ok()&&learned.len()>=2{"completed"}else{"needs_work"},"model":model.model,"storedPreferences":learned,"forcedBoundedReview":true});
        row_metrics(&mut row, &metrics, started);
        println!("MINIMAL_ARCHITECTURE_ROW {row}");
        let mut total_input = row["inputTokens"].as_u64().unwrap();
        if row["status"]!="completed" {return;}
        {
            let conn = state.db.lock().unwrap();
            for (i, content) in input.noise.iter().enumerate() {
                let id = format!("manual-noise-{i}");
                conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES(?1,?2,'user',?3,?4)",params![id,source,content,start+100+i as i64]).unwrap();
                assert!(record_explicit_preference(&conn,&source,&id,content,content).unwrap());
            }
        }
        let original_hash = archive_hash(&state.db.lock().unwrap());
        drop(state); // Readers must open the persisted archive, not retained objects.
        let reader = Reader { root: &root, source: &source, model: &model, app: app.handle(), clock: &clock, work_dir:None,task_view:false };
        for (index, case) in input.cases.iter().enumerate() {
            // Counterbalance order without sharing responses/history between arms.
            let arms = if index%2==0 {["full_snapshot","bounded_recall"]} else {["bounded_recall","full_snapshot"]};
            for arm in arms {
                if total_input>=120_000 {return;}
                let mut row = reader.probe(case,arm,if arm=="full_snapshot" {None}else{Some(12)}).await;
                let conn = crate::db::init(&database).unwrap();
                row["archiveUnchanged"]=json!(row["archiveUnchanged"]==true && archive_hash(&conn)==original_hash);
                total_input+=row["inputTokens"].as_u64().unwrap();
                println!("MINIMAL_ARCHITECTURE_ROW {row}");
                // A new failure is evidence; do not compensate with another trial.
                if row["status"]!="completed" {return;}
            }
        }
    });
}

#[test]
#[ignore = "explicit saved Luna sequential configuration evaluation; no desktop"]
fn configured_minimal_architecture_sequence() {
    assert_eq!(
        std::env::var("ANGELBOT_CONTEXT_EVAL_LIVE").as_deref(),
        Ok("1")
    );
    let root =
        std::path::PathBuf::from(std::env::var("ANGELBOT_PRIVATE_CONTEXT_EVAL_ROOT").unwrap())
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
        .starts_with("minimal-architecture-"));
    let input: SequenceInput =
        serde_json::from_slice(&std::fs::read(root.join("input.json")).unwrap()).unwrap();
    let (transfer, temporal) = input.validate();
    let budget_pair = input.version == 9;
    let app = headless_credential_host().unwrap();
    let mut model =
        crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
            .unwrap();
    assert_eq!(model.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
    assert!(model.model.contains("luna"));
    if temporal {
        assert_eq!(input.model.as_deref(), Some(model.model.as_str()));
    }
    model.max_tokens = model.max_tokens.min(2048);
    model.temperature = 0.0;
    let clock = chrono::Local::now().to_rfc3339();
    let database = root.join("state.db");
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut state = crate::tests::test_app_state();
        state.db = Arc::new(Mutex::new(crate::db::init(&database).unwrap()));
        crate::db::migrate(&state.db.lock().unwrap()).unwrap();
        crate::settings_cache::load_all(&state.db.lock().unwrap(), &state.cached_settings).unwrap();
        let source = if temporal {
            state.db.lock().unwrap().query_row("SELECT session_id FROM messages WHERE id='seq-0-0' AND role='user'",[],|r|r.get::<_,String>(0)).unwrap()
        } else {
            let directory = root.join("source"); std::fs::create_dir(&directory).unwrap();
            let source = crate::commands::workspace::create_project_workspace_impl(&state, directory.to_string_lossy().into(), Some("Owned sequential core".into()), None, None).unwrap().session.id;
            crate::commands::memory::resolve_reply_memory_controls(&state.db.lock().unwrap(), &source, Some(true), Some(true)).unwrap();
            source
        };
        drop(state);
        let reader = Reader {root:&root,source:&source,model:&model,app:app.handle(),clock:&clock,work_dir:None,task_view:false};
        let start = if temporal { chrono::DateTime::parse_from_rfc3339("2026-10-29T00:00:00+08:00").unwrap().timestamp() } else {chrono::Utc::now().timestamp()-5*86400};
        let soft_input_stop = if budget_pair {100_000}else if temporal {60_000}else if transfer {80_000}else{120_000};
        let mut total_input = 0;
        let mut total_output = 0;
        let mut probe_index = 0;
        for (stage_index, stage) in input.stages.iter().enumerate() {
            let mut deferred_review = None;
            if total_input>=soft_input_stop || (budget_pair && total_input+total_output>=120_000) {return;}
            // Close/reopen between stages too. Clock passage is simulated only
            // in original source dates and this owned session's cooldown, never
            // by resetting its through-message cursor or production settings.
            let db = Arc::new(Mutex::new(crate::db::init(&database).unwrap()));
            let mut latest = String::new();
            {
                let conn = db.lock().unwrap();
                let mut parent: Option<String> = conn.query_row("SELECT leaf_message_id FROM sessions WHERE id=?1",[&source],|r|r.get(0)).unwrap();
                for (i, content) in stage.sources.iter().enumerate() {
                    let id = format!("{}-{stage_index}-{i}",if temporal {"temporal"}else{"seq"});
                    conn.execute("INSERT INTO messages(id,session_id,role,content,parent_id,created_at) VALUES(?1,?2,'user',?3,?4,?5)",params![id,source,content,parent,start+stage_index as i64*86400+i as i64]).unwrap();
                    parent=Some(id.clone()); latest=id;
                }
                conn.execute("UPDATE sessions SET leaf_message_id=?1 WHERE id=?2",params![parent,source]).unwrap();
                if !stage.sources.is_empty() {
                    conn.execute("UPDATE sessions SET metadata=json_set(metadata,'$.automaticEvolution.last_attempt_at',0) WHERE id=?1",[&source]).unwrap();
                }
            }
            if !stage.sources.is_empty() {
                let before = preference_state_hash(&db.lock().unwrap());
                let (provider,metrics) = boxed_bounded_provider(&model,app.handle(),&root,1);
                let started=Instant::now();
                let reviewed=crate::commands::memory::run_evolution_review_impl(db.clone(),&source,provider,crate::llm::resolve_thinking_settings(&model,Some("low")).unwrap()).await;
                let prefs=learned_preferences(&db.lock().unwrap()).unwrap();
                let admitted = !budget_pair || prefs.iter().any(|p| p.explicit && p.ready && stage.sources.first().is_some_and(|s| p.content==*s));
                let unchanged=before==preference_state_hash(&db.lock().unwrap());
                let cursor_advanced=db.lock().unwrap().query_row::<String,_,_>("SELECT json_extract(metadata,'$.automaticEvolution.through_message_id') FROM sessions WHERE id=?1",[&source],|r|r.get(0)).unwrap()==latest;
                let mut row=json!({"id":stage.id,"phase":"learn","model":model.model,"status":if reviewed.is_ok(){"completed"}else{"invalid_review"},"storedPreferences":prefs,"simulatedElapsedTime":true,"forcedBoundedReview":true,
                    "newCandidates":reviewed.as_ref().map(|r|r.new_memories.len()).ok(),"newMemories":reviewed.as_ref().map(|r|&r.new_memories).ok(),"fixtureAdmitted":admitted,
                    "preferenceStateUnchanged":unchanged,"cursorAdvanced":cursor_advanced});
                row_metrics(&mut row,&metrics,started);
                let messages=stage.sources.iter().map(|s|("user".to_string(),s.clone())).collect::<Vec<_>>();
                row["onlyNewSources"]=json!(row["providerInputs"][0]["messages"][0]["content"]==crate::agent::evolution::build_extraction_prompt(&messages,None));
                total_input+=row["inputTokens"].as_u64().unwrap();
                total_output+=row["outputTokens"].as_u64().unwrap();
                let metered = !budget_pair || row["reportedUsageCount"]==row["requestCount"];
                if !metered {row["studyStopReason"]=json!("missing_usage");}
                if !admitted {row["studyStopReason"]=json!("learning_admission");}
                let ready = row["status"]=="completed" && row["requestCount"]==1 && admitted && metered;
                if budget_pair && ready {deferred_review=Some(row);}else{println!("MINIMAL_ARCHITECTURE_ROW {row}");}
                if !ready {return;}
            }
            drop(db);
            if (transfer || temporal) && !stage.probes.is_empty() {
                // Only synthetic noise is newer. Preserve actual learned
                // source chronology; every target must require tool recall.
                let conn=crate::db::init(&database).unwrap();
                let noise_time=if temporal {start+2*86400+1000}else{chrono::Utc::now().timestamp()-300};
                assert_eq!(conn.execute("UPDATE messages SET created_at=?1+rowid WHERE id LIKE 'manual-noise-%' AND role='user'",[noise_time]).unwrap(),36);
            }
            if let Some(mut row) = deferred_review {
                let conn=crate::db::init(&database).unwrap();
                row["prefixSha256"]=json!(archive_hash(&conn));
                let file=format!("prefix-{stage_index}.db");
                let snapshot=root.join(&file);
                conn.execute("VACUUM INTO ?1",[snapshot.to_str().unwrap()]).unwrap();
                let snapshot_hash=format!("{:x}",Sha256::digest(std::fs::read(&snapshot).unwrap()));
                row["prefixSnapshot"]=json!({"file":file,"sha256":snapshot_hash});
                println!("MINIMAL_ARCHITECTURE_ROW {row}");
            }
            for case in &stage.probes {
                // Check soft limits before the whole pair, never drop its
                // second arm just because the first arm consumed more tokens.
                if total_input>=soft_input_stop || (budget_pair && total_input+total_output>=120_000) {return;}
                let arms=if budget_pair {if probe_index%2==0 {vec![("low",12),("medium",12)]}else{vec![("medium",12),("low",12)]}}else if transfer || temporal {vec![("current",12)]}else if probe_index%2==0 {vec![("view12",12),("view6",6)]}else{vec![("view6",6),("view12",12)]};
                for (arm,limit) in arms {
                    if !budget_pair && total_input>=soft_input_stop {return;}
                    let mut row=if budget_pair {reader.probe_effort(case,arm,Some(limit),arm).await}else{reader.probe(case,arm,Some(limit)).await};
                    row["stage"]=json!(stage.id);
                    total_input+=row["inputTokens"].as_u64().unwrap();
                    total_output+=row["outputTokens"].as_u64().unwrap();
                    let metered = !budget_pair || row["reportedUsageCount"]==row["requestCount"];
                    if !metered {row["studyStopReason"]=json!("missing_usage");}
                    let unchanged = row["archiveUnchanged"]==true;
                    if !unchanged {row["studyStopReason"]=json!("archive_changed");}
                    println!("MINIMAL_ARCHITECTURE_ROW {row}");
                    if row["status"]!="completed" || !unchanged || !metered {return;}
                }
                probe_index+=1;
            }
            if stage_index==0 && !temporal {
                let conn=crate::db::init(&database).unwrap();
                for (i,content) in input.noise.iter().enumerate() {
                    let id=format!("manual-noise-{i}");
                    conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES(?1,?2,'user',?3,?4)",params![id,source,content,start+100+i as i64]).unwrap();
                    assert!(record_explicit_preference(&conn,&source,&id,content,content).unwrap());
                }
            }
        }
    });
}

#[test]
#[ignore = "owned legacy-option query experiment; live model only with input.live"]
fn configured_minimal_architecture_stale() {
    use crate::agent::tool::ToolHandler;
    assert_eq!(
        std::env::var("ANGELBOT_CONTEXT_EVAL_LIVE").as_deref(),
        Ok("1")
    );
    let root =
        std::path::PathBuf::from(std::env::var("ANGELBOT_PRIVATE_CONTEXT_EVAL_ROOT").unwrap())
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
        .starts_with("minimal-architecture-"));
    let input: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("input.json")).unwrap()).unwrap();
    if matches!(input["version"].as_u64(), Some(14 | 19)) {
        run_order_view(&root, serde_json::from_value(input).unwrap());
        return;
    }
    let input: StaleInput = serde_json::from_value(input).unwrap();
    if input.version == 10 {
        run_clock_view(&root, input);
        return;
    }
    if [11, 12, 13, 15, 16, 17, 18, 20].contains(&input.version) {
        run_stability(&root, input);
        return;
    }
    assert!([3, 4, 6].contains(&input.version));
    let natural = input.version == 6;
    assert_eq!(input.cases.len(), if natural { 3 } else { 4 });
    assert!(input
        .cases
        .iter()
        .all(|c| c.question.chars().count() <= 300));
    let db = Arc::new(Mutex::new(crate::db::init(&root.join("state.db")).unwrap()));
    let source: String = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT session_id FROM messages WHERE id='seq-0-0' AND role='user'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let count=db.lock().unwrap().execute("UPDATE messages SET created_at=?1+rowid WHERE id LIKE 'manual-noise-%' AND role='user'",[chrono::Utc::now().timestamp()-300]).unwrap();
    assert_eq!(count, 36); // Reorder manual noise only, never original learned sources.
    let handler = crate::agent::extensions::memory::RecallMemoriesHandler {
        db: Some(db.clone()),
    };
    let (old_term, topic, targets) = if natural {
        ("地铁", "通勤", ["地铁", "自行车", "打车"])
    } else {
        ("乌龙茶", "饮品", ["乌龙茶", "白茶", "温水"])
    };
    let old = handler.execute(
        &json!({"query":old_term,"limit":5,"include_learned_habits":true}),
        &root,
    );
    let topic = handler.execute(
        &json!({"query":topic,"limit":5,"include_learned_habits":true}),
        &root,
    );
    let projection = project_for_session(&db.lock().unwrap(), &source).unwrap();
    let visible = projection
        .records
        .iter()
        .map(|p| p.value.clone())
        .collect::<Vec<_>>();
    let ready = old.success
        && topic.success
        && old.content.contains(targets[0])
        && (natural || !old.content.contains(targets[1]))
        && !old.content.contains(targets[2])
        && targets.iter().all(|word| topic.content.contains(word))
        && visible.iter().all(|s| {
            !targets.iter().any(|word| s.contains(word))
                && !s.contains(if natural { "汇报" } else { "小榆" })
        });
    let row = json!({"id":"lookup_diagnostic","status":if ready{"prepared"}else{"invalid_fixture"},"oldResult":old.content,"topicResult":topic.content,"visible":visible,"omitted":projection.omitted,"manualNoiseReordered":count,"requestCount":0,"inputTokens":0,"outputTokens":0});
    println!("MINIMAL_ARCHITECTURE_ROW {row}");
    drop(handler);
    drop(db);
    if !ready || !input.live {
        return;
    }
    let app = headless_credential_host().unwrap();
    let mut model =
        crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
            .unwrap();
    assert_eq!(model.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
    assert!(model.model.contains("luna"));
    assert_eq!(
        model.model, input.model,
        "Never substitute the learned fixture model"
    );
    model.max_tokens = model.max_tokens.min(2048);
    model.temperature = 0.0;
    let clock = chrono::Local::now().to_rfc3339();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let reader = Reader {
            root: &root,
            source: &source,
            model: &model,
            app: app.handle(),
            clock: &clock,
            work_dir: None,
            task_view: false,
        };
        for case in &input.cases {
            let row = reader.probe(case, "current", Some(12)).await;
            println!("MINIMAL_ARCHITECTURE_ROW {row}");
            if row["status"] != "completed" {
                return;
            }
        }
    });
}

/// Format-only screening on an already learned prefix. No extraction, noise
/// reordering, semantic time parsing or production configuration changes.
fn run_clock_view(root: &Path, input: StaleInput) {
    assert_eq!(input.version, 10);
    assert_eq!(input.cases.len(), 2);
    assert_eq!(input.cases[0].question, input.cases[1].question);
    assert!(input.cases.iter().all(|c| {
        c.question.chars().count() <= 300
            && c.clock
                .as_ref()
                .is_some_and(|s| s.len() <= 40 && chrono::DateTime::parse_from_rfc3339(s).is_ok())
    }));
    // Offline planning never needs the credential host.
    if !input.live {
        return;
    }
    let db = crate::db::init(&root.join("state.db")).unwrap();
    let source: String = db
        .query_row(
            "SELECT session_id FROM messages WHERE id='seq-0-0' AND role='user'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(db);
    let app = headless_credential_host().unwrap();
    let model = source_model(app.handle(), &input.model);
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let reader = Reader {
            root,
            source: &source,
            model: &model,
            app: app.handle(),
            clock: "unused",
            work_dir: None,
            task_view: false,
        };
        let mut total_input = 0_u64;
        for (index, case) in input.cases.iter().enumerate() {
            // A soft stop is evaluated only before a whole pair; it is not a
            // remote output/compute cap, and cannot discard a costly partner.
            if total_input >= 35_000 {
                return;
            }
            let arms = if index == 0 {
                ["iso", "local"]
            } else {
                ["local", "iso"]
            };
            for arm in arms {
                let original = case.clock.as_ref().unwrap();
                let rendered = if arm == "local" {
                    local_clock(original)
                } else {
                    original.clone()
                };
                let viewed = Case {
                    id: case.id.clone(),
                    question: case.question.clone(),
                    clock: Some(rendered),
                    prefix: None,
                };
                let mut row = reader.probe_effort(&viewed, arm, Some(12), "medium").await;
                row["sourceClock"] = json!(original);
                total_input += row["inputTokens"].as_u64().unwrap();
                let metered = row["reportedUsageCount"] == row["requestCount"];
                let unchanged = row["archiveUnchanged"] == true;
                if !metered {
                    row["studyStopReason"] = json!("missing_usage");
                }
                if !unchanged {
                    row["studyStopReason"] = json!("archive_changed");
                }
                println!("MINIMAL_ARCHITECTURE_ROW {row}");
                if row["status"] != "completed" || !metered || !unchanged {
                    return;
                }
            }
        }
    });
}

fn source_model(app: &tauri::AppHandle, expected: &str) -> crate::llm::ProviderConfig {
    let mut model =
        crate::commands::foreground_model_factory::resolve_model_config(Some(app)).unwrap();
    assert_eq!(model.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
    assert!(model.model.contains("luna"));
    assert_eq!(model.model, expected, "Never substitute the source model");
    model.max_tokens = model.max_tokens.min(2048);
    model.temperature = 0.0;
    model
}

#[test]
fn counterfactual_profiles_keep_only_source_values_variable() {
    unsafe {
        crate::vector_store::load_extension_globally();
    }
    let data = tempfile::tempdir().unwrap();
    let root = data.path().canonicalize().unwrap();
    let base = root.join("base.db");
    let conn = crate::db::init(&base).unwrap();
    crate::db::migrate(&conn).unwrap();
    conn.execute_batch("INSERT INTO sessions(id,title,created_at,updated_at) VALUES('s','Synthetic',1,1);
        INSERT INTO messages(id,session_id,role,content,created_at) VALUES('seq-0-0','s','user','已有对话。',99),('temporal-1-0','s','user','已有更正。',100);
        UPDATE sessions SET leaf_message_id='temporal-1-0' WHERE id='s';").unwrap();
    for (i,quote) in ["从北京时间2026年11月5日18点30分开始，无雨的上班出行优先步行；那之前继续骑自行车，遇上下雨照旧打车。", "以后给我安排日常通勤，默认坐地铁。"].into_iter().enumerate() {
        let id=format!("older-broad-{i}");
        conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES(?1,'s','user',?2,?3)",params![id,quote,90+i]).unwrap();
        assert!(record_explicit_preference(&conn,"s",&id,quote,quote).unwrap());
    }
    for i in 0..36 {
        let id = format!("noise-{i}");
        let quote = format!("以后介绍兴趣{i}时，先给一个例子。");
        conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES(?1,'s','user',?2,?3)",params![id,quote,200+i]).unwrap();
        assert!(record_explicit_preference(&conn, "s", &id, &quote, &quote).unwrap());
    }
    drop(conn);
    for prefix in 0..2 {
        let directory = root.join(format!("prefix-{prefix}"));
        std::fs::create_dir(&directory).unwrap();
        std::fs::copy(&base, directory.join("state.db")).unwrap();
    }
    let sources = json!([{"id":"counter_rain","quotes":["以后小雨上班优先打车。","以后小雨上班优先地铁。"]},
        {"id":"counter_tea","quotes":["以后下午茶优先乌龙茶。","以后下午茶优先白茶。"]}]);
    let cases = ["counter_rain","counter_tea"].into_iter().flat_map(|id| (0..2).map(move |p|json!({"id":format!("{id}_{p}"),"question":format!("{id} task"),"prefix":p,"clock":"2026-11-07T12:00:00+08:00"}))).collect::<Vec<_>>();
    let raw = json!({"version":16,"live":false,"model":"fixture-luna","cases":cases,"counter_sources":sources});
    let input: StaleInput = serde_json::from_value(raw.clone()).unwrap();
    let prepared = prepare_counter_profiles(&root, &input);
    let views = [0, 1].map(|p| {
        let conn = crate::db::init(&root.join(format!("prefix-{p}/state.db"))).unwrap();
        assert_eq!(archive_hash(&conn), prepared[p.to_string()]["sha256"]);
        assert_eq!(
            prepared[p.to_string()]["caseProofs"]
                .as_object()
                .unwrap()
                .len(),
            2
        );
        for (id,query) in [("counter_rain","我现在要去公司，外面下小雨。帮我安排这趟出行，只输出JSON：{\"选择\":\"交通方式名称\",\"说明\":\"一句准备建议\"}。"),
            ("counter_tea","下午休息时想喝点茶，帮我选一种，只输出JSON：{\"选择\":\"饮品名称\",\"说明\":\"一句准备建议\"}。")]
        {
            let projection=project_for_task(&conn,"s",query).unwrap();
            let quote=prepared[p.to_string()]["caseProofs"][format!("{id}_{p}")]["quote"].as_str().unwrap();
            assert!(projection.records.iter().take(2).any(|r|r.value==quote),"local task query must retain the matching declaration in either window: {id}/{p}");
            assert_eq!(projection.records.len(),12);
            assert_eq!(projection.omitted,28);
            assert_eq!(archive_hash(&conn),prepared[p.to_string()]["sha256"]);
        }
        project_for_session(&conn, "s")
            .unwrap()
            .records
            .into_iter()
            .map(|r| (r.key, r.value, r.scope, r.confidence, r.explicit))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        views[0], views[1],
        "same visible records, including original IDs"
    );
    assert_ne!(prepared["0"]["sha256"], prepared["1"]["sha256"]);
    // The window-size screen uses new topics, not the tuned rain/tea questions.
    let window_root = root.join("window");
    std::fs::create_dir(&window_root).unwrap();
    for prefix in 0..2 {
        let directory = window_root.join(format!("prefix-{prefix}"));
        std::fs::create_dir(&directory).unwrap();
        std::fs::copy(&base, directory.join("state.db")).unwrap();
    }
    let window_sources = json!([
        {"id":"window_shirt","quotes":["以后工作日挑衬衫，颜色优先深蓝色。","以后工作日挑衬衫，颜色优先浅灰色。"]},
        {"id":"window_relax","quotes":["以后下班后放松，优先听爵士乐。","以后下班后放松，优先看纪录片。"]}]);
    let window_queries = [
        ("window_shirt","帮我选明天工作日穿的衬衫颜色，只输出JSON：{\"选择\":\"颜色名称\",\"说明\":\"一句搭配建议\"}。"),
        ("window_relax","今天忙完下班了，帮我安排一个放松活动，只输出JSON：{\"选择\":\"活动名称\",\"说明\":\"一句准备建议\"}。")];
    let window_cases = window_queries.iter().flat_map(|(id,query)| (0..2).map(move |p|json!({"id":format!("{id}_{p}"),"question":query,"prefix":p,"clock":"2026-11-07T12:00:00+08:00"}))).collect::<Vec<_>>();
    let window_input: StaleInput = serde_json::from_value(json!({"version":18,"live":false,"model":"fixture-luna","cases":window_cases,"counter_sources":window_sources})).unwrap();
    let window_prepared = prepare_counter_profiles(&window_root, &window_input);
    for p in 0..2 {
        let conn = crate::db::init(&window_root.join(format!("prefix-{p}/state.db"))).unwrap();
        for (id, query) in window_queries {
            let view = project_for_task(&conn, "s", query).unwrap();
            let quote = window_prepared[p.to_string()]["caseProofs"][format!("{id}_{p}")]["quote"]
                .as_str()
                .unwrap();
            assert!(
                view.records.iter().take(2).any(|r| r.value == quote),
                "new-topic source missing from lean view: {id}/{p}"
            );
            assert_eq!(
                archive_hash(&conn),
                window_prepared[p.to_string()]["sha256"]
            );
        }
    }
    let conn = crate::db::init(&root.join("prefix-0/state.db")).unwrap();
    for (id, update) in [
        ("counter_rain", "status='disabled'"),
        ("counter_tea", "expires_at=0"),
    ] {
        let quote = prepared["0"]["caseProofs"][format!("{id}_0")]["quote"]
            .as_str()
            .unwrap();
        // Only fixture state changes here; selection must not revive either.
        conn.execute(
            &format!("UPDATE adaptive_constraints SET {update} WHERE constraint_value=?1"),
            [quote],
        )
        .unwrap();
        let view = project_for_task(&conn, "s", quote).unwrap();
        assert!(!view.records.iter().any(|r| r.value == quote));
    }
    drop(conn);
    for fault in ["source", "question", "prefix"] {
        let mut invalid = raw.clone();
        match fault {
            "source" => {
                invalid["counter_sources"][0]["quotes"][1] =
                    invalid["counter_sources"][0]["quotes"][0].clone()
            }
            "question" => invalid["cases"][1]["question"] = json!("different task"),
            _ => invalid["cases"][1]["prefix"] = json!(0),
        }
        let input: StaleInput = serde_json::from_value(invalid).unwrap();
        assert!(
            std::panic::catch_unwind(|| validate_stability_input(&input)).is_err(),
            "{fault}"
        );
    }
}

/// Exactly two cold rounds on owned, previously learned prefixes. Wrong answers
/// remain observations; they neither trigger retries nor shorten this matrix.
fn run_stability(root: &Path, input: StaleInput) {
    validate_stability_input(&input);
    let source_readers = input.version >= 12;
    if !input.live {
        return;
    }
    if matches!(input.version, 16..=18) {
        let prepared = prepare_counter_profiles(root, &input);
        std::fs::write(
            root.join("counter-prefixes.json"),
            serde_json::to_vec_pretty(&prepared).unwrap(),
        )
        .unwrap();
    }
    let prefixes = [0, 1].map(|prefix| {
        let directory = root
            .join(format!("prefix-{prefix}"))
            .canonicalize()
            .unwrap();
        assert_eq!(directory.parent(), Some(root));
        let database = directory.join("state.db").canonicalize().unwrap();
        assert_eq!(database, directory.join("state.db"));
        let conn = rusqlite::Connection::open_with_flags(
            database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let source: String = conn
            .query_row(
                "SELECT session_id FROM messages WHERE id='seq-0-0' AND role='user'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        (directory, source)
    });
    let app = headless_credential_host().unwrap();
    let model = source_model(app.handle(), &input.model);
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut total_input = 0_u64;
        for trial in 1..=2 {
            for offset in 0..input.cases.len() {
                // Rounds are separated, so this soft stop may truncate a pair;
                // the scorer must reject an incomplete matrix, never drop it.
                if total_input >= 100_000 {
                    return;
                }
                let index = if trial == 1 {
                    offset
                } else {
                    input.cases.len() - 1 - offset
                };
                let case = &input.cases[index];
                let prefix = case.prefix.unwrap();
                let (directory, source) = &prefixes[prefix as usize];
                let reader = Reader {
                    root: directory,
                    source,
                    model: &model,
                    app: app.handle(),
                    clock: "unused",
                    work_dir: matches!(input.version, 16..=18).then_some(root),
                    task_view: matches!(input.version, 17 | 18 | 20),
                };
                // Each new-topic task gets both windows once; the profiles
                // counterbalance which size comes first. No tool/prompt change.
                let slots = if input.version == 18 && (index + trial) % 2 == 0 {
                    2
                } else {
                    12
                };
                let mut row = reader
                    .probe_surface(
                        case,
                        "stable",
                        Some(slots),
                        "medium",
                        source_readers,
                        input.version == 20 && (index + trial) % 2 == 0,
                    )
                    .await;
                if input.version == 18 {
                    row["viewSlots"] = json!(slots);
                }
                row["trial"] = json!(trial);
                row["prefix"] = json!(prefix);
                total_input += row["inputTokens"].as_u64().unwrap();
                let metered = row["reportedUsageCount"] == row["requestCount"];
                let unchanged = row["archiveUnchanged"] == true;
                if !metered {
                    row["studyStopReason"] = json!("missing_usage");
                }
                if !unchanged {
                    row["studyStopReason"] = json!("archive_changed");
                }
                println!("MINIMAL_ARCHITECTURE_ROW {row}");
                if row["status"] != "completed" || !metered || !unchanged {
                    return;
                }
            }
        }
    });
}
