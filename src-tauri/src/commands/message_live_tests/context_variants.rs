//! Opt-in architecture screening: synthetic data, not a production memory policy.
//! Result projection is simulated by an owned read-only fixture Adapter; the
//! actual Runner still owns tool selection, protocol, truncation and closure.
use super::agent_benchmarks::{bounded_provider, boxed_bounded_provider, row_metrics, Metrics};
use super::companion::message;
use super::context_management::answer;
use super::*;
use crate::agent::compaction::{CompactionHook, CompactionOutcome, DefaultCompactionHook};
use crate::agent::config::AgentConfig;
use crate::agent::tool::{Tool, ToolHandler, ToolResult};
use crate::agent::{AgentResponse, AgentSliceOutcome, ToolRegistry};
use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "context_variants/preference_neighborhood.rs"]
mod preference_neighborhood;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    role: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    experiment: String,
    history: Vec<Source>,
    questions: Vec<String>,
    query: Option<String>,
    receipt: Option<serde_json::Value>,
    scope: Option<String>,
}

fn arms(experiment: &str) -> &'static [&'static str] {
    match experiment {
        "result_projection" => &["current", "projection"],
        "neighborhood" => &["isolated", "neighbors"],
        "preference_neighborhood" => &["isolated", "source_neighbors"],
        "preference_grouping" => &["flat", "episode"],
        "memory" => &["full", "checkpoint", "excerpt"],
        "tool_selection" => &["all_tools", "scoped_tools"],
        "conversation_recall" => &["scoped"],
        "receipt_readback" | "receipt_batching" => &["raw", "envelope"],
        "receipt_guidance" => &["envelope", "short_ref", "sizes", "short_sizes"],
        "receipt_budget_catalog" => &["short_sizes", "bounded_catalog"],
        "durable_sources" => &["raw", "compact"],
        _ => panic!("unknown synthetic experiment"),
    }
}

fn request_limit(experiment: &str) -> usize {
    match experiment {
        "conversation_recall"
        | "receipt_readback"
        | "receipt_batching"
        | "receipt_guidance"
        | "receipt_budget_catalog" => 4,
        "durable_sources" => 4,
        "result_projection" | "tool_selection" => 3,
        _ => 1,
    }
}

fn source_history(case: &Case, excerpt: bool) -> Vec<LlmMessage> {
    case.history
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let mut content = source.content.clone();
            if excerpt && content.chars().count() > 200 {
                content = format!(
                    "{}\n[source excerpt; {} characters omitted]",
                    content.chars().take(200).collect::<String>(),
                    content.chars().count() - 200
                );
            }
            message(&source.role, format!("[source-{index:04}]\n{content}"))
        })
        .collect()
}

fn neighboring_history(case: &Case, radius: usize) -> (Vec<LlmMessage>, Vec<String>) {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::migrate(&conn).unwrap();
    conn.execute_batch("INSERT INTO sessions(id,title,created_at,updated_at) VALUES('owned','Synthetic',1,1),('other','Other scope',1,1);").unwrap();
    for (index, source) in case.history.iter().enumerate() {
        conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES(?1,'owned',?2,?3,?4)", params![format!("source-{index:04}"),source.role,source.content,index as i64]).unwrap();
    }
    conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES('outside','other','user',?1,1)", [format!("{} outside-scope sentinel",case.query.as_deref().unwrap())]).unwrap();
    let hits = crate::fts::search_messages(&conn, case.query.as_deref().unwrap(), Some("owned"), 1)
        .unwrap();
    assert!(
        !hits.is_empty(),
        "the frozen exact-entity fixture must have a hit"
    );
    let mut selected = std::collections::BTreeSet::new();
    for hit in hits {
        let index = hit
            .id
            .strip_prefix("source-")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        for i in index.saturating_sub(radius)..=(index + radius).min(case.history.len() - 1) {
            selected.insert(i);
        }
    }
    let ids = selected.iter().map(|i| format!("source-{i:04}")).collect();
    let messages = selected
        .into_iter()
        .map(|i| {
            message(
                &case.history[i].role,
                format!("[source-{i:04}]\n{}", case.history[i].content),
            )
        })
        .collect();
    (messages, ids)
}

fn project_receipt(receipt: &serde_json::Value) -> serde_json::Value {
    let mut projected = receipt.clone();
    if let Some(fields) = projected.as_object_mut() {
        if fields
            .get("details")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v.chars().count() > 512)
        {
            let removed = fields.remove("details").unwrap();
            fields.insert("omittedFields".into(), serde_json::json!([{"name":"details","characters":removed.as_str().unwrap().chars().count()}]));
        }
    }
    projected
}

struct FixtureRead {
    name: String,
    receipt: serde_json::Value,
    target: bool,
    reads: Arc<AtomicUsize>,
}
impl ToolHandler for FixtureRead {
    fn name(&self) -> &str {
        &self.name
    }
    fn execute(&self, _: &serde_json::Value, _: &Path) -> ToolResult {
        if self.target {
            self.reads.fetch_add(1, Ordering::SeqCst);
        }
        ToolResult::success(&self.name, self.receipt.to_string())
    }
}

// Experimental presentation only. The source is immutable and shared by both
// views; this is not a new production store, tool hook or receipt authority.
struct ReceiptProbe {
    original: Arc<serde_json::Value>,
    source_id: String,
    revision: String,
    envelope: bool,
    reader: bool,
    max_paths: usize,
    short_ref: Option<String>,
    sized_catalog: bool,
    bounded_catalog: bool,
    reads: Arc<AtomicUsize>,
}

fn pointer_key(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn receipt_envelope(original: &serde_json::Value) -> serde_json::Value {
    let mut fields = serde_json::Map::new();
    let mut omitted = Vec::new();
    for (key, value) in original.as_object().unwrap() {
        let bytes = value.to_string().len();
        if bytes <= 512 {
            fields.insert(key.clone(), value.clone());
        } else {
            omitted.push(serde_json::json!({"path":format!("/{}",pointer_key(key)),"encoded_bytes":bytes,
                "kind":if value.is_object(){"object"}else if value.is_array(){"array"}else{"scalar"}}));
        }
    }
    serde_json::json!({"fields":fields,"omitted_fields":omitted})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptReadArgs {
    receipt_id: String,
    revision: String,
    paths: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptRefArgs {
    source_ref: String,
    paths: Vec<String>,
}

impl ReceiptProbe {
    fn identity(&self) -> serde_json::Value {
        match &self.short_ref {
            Some(reference) => serde_json::json!({"source_ref":reference}),
            None => serde_json::json!({"receipt_id":self.source_id,"revision":self.revision}),
        }
    }

    fn encode_fields(&self, fields: serde_json::Map<String, serde_json::Value>) -> String {
        let mut output = self.identity();
        output["fields"] = serde_json::Value::Object(fields);
        output.to_string()
    }

    fn run(&self, args: &serde_json::Value) -> Result<String, String> {
        if self.reader {
            // The model selector is not authority. Scope lives in this owned
            // fixture; the full revision is always rechecked by the host.
            if format!("{:x}", Sha256::digest(self.original.to_string().as_bytes()))
                != self.revision
            {
                return Err("Receipt source changed; inspect its new revision".into());
            }
            let paths = if let Some(reference) = &self.short_ref {
                let args: ReceiptRefArgs =
                    serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
                if args.source_ref != *reference {
                    return Err("Source reference is unavailable in this run; inspect again".into());
                }
                args.paths
            } else {
                let args: ReceiptReadArgs =
                    serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
                if args.receipt_id != self.source_id || args.revision != self.revision {
                    return Err("Receipt source or revision is unavailable in this run".into());
                }
                args.paths
            };
            if paths.is_empty() || paths.len() > self.max_paths {
                return Err(if self.max_paths == 4 {
                    "Read one to four original JSON fields"
                } else {
                    "Read one to eight original JSON fields"
                }
                .into());
            }
            let mut fields = serde_json::Map::new();
            for path in paths {
                if path.len() > 128 || !path.starts_with('/') || fields.contains_key(&path) {
                    return Err("Invalid or duplicate JSON pointer".into());
                }
                let value = self
                    .original
                    .pointer(&path)
                    .ok_or("Receipt field does not exist")?;
                fields.insert(path, value.clone());
            }
            let output = self.encode_fields(fields);
            if output.len() > 5000 {
                return Err("Selected fields exceed 5000 encoded bytes; choose narrower scalar child paths from inspection".into());
            }
            return Ok(output);
        }
        if !args.as_object().is_some_and(|args| args.is_empty()) {
            return Err("Inspection accepts only an empty object".into());
        }
        let mut paths = Vec::new();
        for (key, value) in self.original.as_object().unwrap() {
            let path = format!("/{}", pointer_key(key));
            paths.push(path.clone());
            if let Some(children) = value.as_object() {
                paths.extend(
                    children
                        .keys()
                        .map(|key| format!("{path}/{}", pointer_key(key))),
                );
            }
        }
        let mut available = Vec::new();
        let mut unavailable = Vec::new();
        for path in paths {
            if !self.sized_catalog {
                available.push(serde_json::json!(path));
                continue;
            }
            let value = self.original.pointer(&path).unwrap();
            let mut entry =
                serde_json::json!({"path":path,"encoded_bytes":value.to_string().len()});
            if self.bounded_catalog {
                let single = self
                    .encode_fields([(path.clone(), value.clone())].into_iter().collect())
                    .len();
                entry["single_read_bytes"] = serde_json::json!(single);
                if single > 5000 {
                    entry["reason"] = serde_json::json!("single_read_exceeds_budget");
                    unavailable.push(entry);
                    continue;
                }
            }
            available.push(entry);
        }
        let mut source = self.identity();
        source["available_paths"] = serde_json::json!(available);
        if self.bounded_catalog {
            source["unavailable_paths"] = serde_json::json!(unavailable);
            source["max_output_bytes"] = serde_json::json!(5000);
        }
        let view = if self.envelope {
            receipt_envelope(&self.original)
        } else {
            (*self.original).clone()
        };
        let output = format!("Immutable source: {source}\nView:\n{view}");
        if self.envelope && output.len() > 5000 {
            return Err("Structural receipt view exceeds the 5000-byte experiment budget".into());
        }
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(output)
    }
}

impl ToolHandler for ReceiptProbe {
    fn name(&self) -> &str {
        if self.reader {
            "read_receipt_fields"
        } else {
            "inspect_task_receipt"
        }
    }
    fn execute(&self, args: &serde_json::Value, _: &Path) -> ToolResult {
        match self.run(args) {
            Ok(output) => ToolResult::success(self.name(), output),
            Err(error) => ToolResult::error(self.name(), error),
        }
    }
}

fn receipt_registry(case: &Case, arm: &str, reads: Arc<AtomicUsize>) -> ToolRegistry {
    let original = Arc::new(case.receipt.clone().unwrap());
    assert!(original.is_object() && original.as_object().unwrap().len() <= 16);
    assert!(original.to_string().len() <= 128 * 1024);
    let source_id = format!("fixture:{}", case.id);
    let revision = format!("{:x}", Sha256::digest(original.to_string().as_bytes()));
    let max_paths = if matches!(
        case.experiment.as_str(),
        "receipt_batching" | "receipt_guidance" | "receipt_budget_catalog"
    ) {
        8
    } else {
        4
    };
    let short_ref = if matches!(
        case.experiment.as_str(),
        "receipt_guidance" | "receipt_budget_catalog"
    ) && matches!(arm, "short_ref" | "short_sizes" | "bounded_catalog")
    {
        Some(format!(
            "r-{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(uuid::Uuid::new_v4().as_bytes())
        ))
    } else {
        None
    };
    let sized_catalog = matches!(
        case.experiment.as_str(),
        "receipt_guidance" | "receipt_budget_catalog"
    ) && matches!(arm, "sizes" | "short_sizes" | "bounded_catalog");
    let bounded_catalog = case.experiment == "receipt_budget_catalog" && arm == "bounded_catalog";
    let mut registry = ToolRegistry::empty();
    for reader in [false, true] {
        let tool = if reader {
            let mut parameters = serde_json::json!({"type":"object","additionalProperties":false,"properties":{
                    "receipt_id":{"type":"string"},"revision":{"type":"string"},
                    "paths":{"type":"array","minItems":1,"maxItems":max_paths,"items":{"type":"string"}}
                },"required":["receipt_id","revision","paths"]});
            let selector = if short_ref.is_some() {
                parameters["properties"]
                    .as_object_mut()
                    .unwrap()
                    .remove("receipt_id");
                parameters["properties"]
                    .as_object_mut()
                    .unwrap()
                    .remove("revision");
                parameters["properties"]["source_ref"] = serde_json::json!({"type":"string"});
                parameters["required"] = serde_json::json!(["source_ref", "paths"]);
                "source_ref（宿主仍核验完整版本）"
            } else {
                "receipt_id、revision"
            };
            let catalog = if bounded_catalog {
                "available_paths中的path；仅这些项可在单字段预算内读取，single_read_bytes含包装，批量仍有5000字节上限；unavailable_paths已超预算，不可用本工具读取"
            } else if sized_catalog {
                "available_paths中的path；encoded_bytes是字段本身大小，仍须预留回执包装预算"
            } else {
                "available_paths中的JSON Pointer"
            };
            Tool::new("read_receipt_fields",
                format!("只读inspect_task_receipt的同一份不可变原回执。照抄{selector}和{catalog}；可读1至{max_paths}个原字段，总输出至多5000编码字节，大对象应选较小的子字段。不运行原业务、不修改状态、不读其他来源。"),parameters)
        } else {
            Tool::new("inspect_task_receipt",
                "只读检查当前任务的已保存回执，返回观察值、不可变来源和可读字段路径。调用成功不代表业务完成；派发不代表交付。视图被省略或截断时可用read_receipt_fields读原字段，不要重新执行业务。无发送、下载、写文件或调度能力。",
                serde_json::json!({"type":"object","additionalProperties":false,"properties":{},"required":[]}))
        };
        registry.register(
            tool.with_read_only_budget_hint(true),
            Arc::new(ReceiptProbe {
                original: original.clone(),
                source_id: source_id.clone(),
                revision: revision.clone(),
                envelope: arm != "raw",
                reader,
                max_paths,
                short_ref: short_ref.clone(),
                sized_catalog,
                bounded_catalog,
                reads: reads.clone(),
            }),
        );
    }
    assert_eq!(registry.get_schemas().len(), 2);
    registry
}

fn registry(case: &Case, arm: &str, reads: Arc<AtomicUsize>) -> ToolRegistry {
    if matches!(
        case.experiment.as_str(),
        "receipt_readback" | "receipt_batching" | "receipt_guidance" | "receipt_budget_catalog"
    ) {
        return receipt_registry(case, arm, reads);
    }
    // Match the explicit production foreground surface, without built-ins.
    let mut registry = ToolRegistry::empty();
    if case.experiment == "conversation_recall" {
        // Exercise the production read handlers over owned SQLite history,
        // without native foreground/MCP/file authority or preloaded answers.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute("INSERT INTO sessions(id,title,created_at,updated_at) VALUES('owned','Owned history',1,1)", []).unwrap();
        let mut parent: Option<String> = None;
        for (i, source) in case.history.iter().enumerate() {
            let id = format!("source-{i:04}");
            conn.execute("INSERT INTO messages(id,session_id,role,content,parent_id,created_at) VALUES(?1,'owned',?2,?3,?4,?5)", rusqlite::params![id,source.role,source.content,parent,i as i64]).unwrap();
            parent = Some(id);
        }
        conn.execute(
            "UPDATE sessions SET leaf_message_id=?1 WHERE id='owned'",
            [parent],
        )
        .unwrap();
        crate::commands::foreground_context_recall::register(
            &mut registry,
            Arc::new(Mutex::new(conn)),
            "owned".into(),
        );
        assert_eq!(registry.get_schemas().len(), 2);
        return registry;
    }
    let target_scope = case.scope.as_deref().unwrap_or("task");
    let scopes = if case.experiment == "result_projection" {
        vec!["task"]
    } else {
        vec!["mail", "notes", "files", "calendar", "contacts", "browser"]
    };
    for scope in scopes {
        if arm == "scoped_tools" && scope != target_scope {
            continue;
        }
        for action in if scope == "task" {
            vec!["inspect"]
        } else {
            vec!["inspect", "list"]
        } {
            let name = format!(
                "{action}_{scope}_{}",
                if action == "inspect" {
                    "status"
                } else {
                    "items"
                }
            );
            let target = action == "inspect" && scope == target_scope;
            let original = if target {
                case.receipt.clone().unwrap()
            } else {
                serde_json::json!({"items":[],"scope":scope})
            };
            let receipt = if arm == "projection" {
                project_receipt(&original)
            } else {
                original
            };
            registry.register(Tool::new(&name,
                format!("Read-only {action} of the selected {scope} work area. Inspection returns current task metadata and its source reference; listing returns item names. Does not send, write, archive, schedule or execute anything. Use returned state, not dispatch, as evidence."),
                serde_json::json!({"type":"object","properties":{},"required":[],"additionalProperties":false}))
                .with_read_only_budget_hint(true),
                Arc::new(FixtureRead {name,receipt,target,reads:reads.clone()}));
        }
    }
    registry
}

fn config() -> AgentConfig {
    let mut config = AgentConfig::default();
    config.preferences.use_long_term_memory = true;
    config.preferences.evolution_enabled = false;
    config
}

fn emit(
    row: &mut serde_json::Value,
    metrics: &Metrics,
    started: Instant,
    tokens: &mut u64,
    calls: &mut u64,
    limit: u64,
) -> bool {
    row_metrics(row, metrics, started);
    *tokens += row["inputTokens"].as_u64().unwrap();
    *calls += row["requestCount"].as_u64().unwrap();
    assert!(*calls <= limit, "owned experiment call cap");
    row["runningInputTokens"] = serde_json::json!(tokens);
    row["runningProviderCalls"] = serde_json::json!(calls);
    println!("CONTEXT_EVAL_ROW {row}");
    // A completed but incorrect answer is scored later. A local call-budget
    // failure is not retried; independent synthetic conditions may continue.
    matches!(
        row["status"].as_str(),
        Some("completed" | "budget_exceeded")
    ) && *tokens < 120_000
}

#[test]
fn context_variant_projection_contracts_preserve_sources_and_exclude_rubrics() {
    assert!(serde_json::from_value::<Case>(serde_json::json!({"id":"x","experiment":"memory","history":[],"questions":[],"expected":"hidden"})).is_err());
    let receipt = serde_json::json!({"details":"a".repeat(8000),"state":"failed","sent":false,"sourceRef":"fixture:1"});
    let projected = project_receipt(&receipt);
    assert!(receipt["details"].as_str().unwrap().len() > 6000);
    for key in ["state", "sent", "sourceRef"] {
        assert_eq!(receipt[key], projected[key]);
    }
    assert!(projected.get("details").is_none());
    assert_eq!(projected["omittedFields"][0]["characters"], 8000);
    let mut case = Case {
        id: "owned".into(),
        experiment: "neighborhood".into(),
        history: vec![
            Source {
                role: "user".into(),
                content: "EveningRead uses forty five minutes".into(),
            },
            Source {
                role: "assistant".into(),
                content: "noted".into(),
            },
            Source {
                role: "user".into(),
                content: "Changed to twenty minutes".into(),
            },
        ],
        questions: vec!["q".into()],
        query: Some("EveningRead".into()),
        receipt: Some(receipt),
        scope: Some("mail".into()),
    };
    let (isolated, _) = neighboring_history(&case, 0);
    let (neighbors, ids) = neighboring_history(&case, 2);
    assert_eq!(isolated.len(), 1);
    assert!(neighbors.iter().any(|m| m.content.contains("twenty")));
    assert!(!ids.iter().any(|id| id == "outside"));
    case.experiment = "tool_selection".into();
    let reads = Arc::new(AtomicUsize::new(0));
    let all = registry(&case, "all_tools", reads.clone()).get_schemas();
    let scoped = registry(&case, "scoped_tools", reads).get_schemas();
    assert_eq!(all.len(), 12);
    assert_eq!(scoped.len(), 2);
    for tool in &all {
        assert!(!["read_file", "write_file", "run_project_command"]
            .contains(&tool["function"]["name"].as_str().unwrap()));
    }
    for tool in scoped {
        assert!(
            all.contains(&tool),
            "preselection must preserve exact tool identity/schema"
        );
    }
    let json = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/fixtures/context_continuity.json"),
    )
    .unwrap();
    let source: serde_json::Value = serde_json::from_slice(&json).unwrap();
    for case in source["cases"].as_array().unwrap() {
        assert!(case["expected"].is_array());
    }
    case.experiment = "conversation_recall".into();
    let production = registry(&case, "scoped", Arc::new(AtomicUsize::new(0)));
    assert_eq!(production.get_schemas().len(), 2);
    assert!(production.contains("recall_conversation"));
    assert!(production.contains("read_conversation"));
}

#[test]
fn receipt_views_share_contract_and_read_exact_immutable_fields() {
    let original = serde_json::json!({"diagnostics":"log ".repeat(2500),"state":"failed",
        "details":{"trace":"log ".repeat(2000),"reason":"blocked"},"a/b~c":"中🪶"});
    let case = Case {
        id: "receipt-test".into(),
        experiment: "receipt_readback".into(),
        history: vec![],
        questions: vec!["q".into()],
        query: None,
        receipt: Some(original.clone()),
        scope: None,
    };
    let views: Vec<_> = ["raw", "envelope"]
        .iter()
        .map(|arm| registry(&case, arm, Arc::new(AtomicUsize::new(0))))
        .collect();
    let mut schemas: Vec<_> = views.iter().map(ToolRegistry::get_schemas).collect();
    for schema in &mut schemas {
        schema.sort_by_key(serde_json::Value::to_string);
    }
    assert_eq!(
        schemas[0], schemas[1],
        "the view must be the only changed input"
    );
    let call = |tools: &ToolRegistry, name: &str, args| {
        tools
            .execute(
                &crate::agent::tool::ToolCall::new(name.into(), args),
                "test",
                Path::new("."),
            )
            .result
    };
    let mut sources = Vec::new();
    for (i, tools) in views.iter().enumerate() {
        let inspection = call(tools, "inspect_task_receipt", serde_json::json!({}));
        assert!(inspection.success);
        let (header, body) = inspection.content.split_once("\nView:\n").unwrap();
        let source: serde_json::Value =
            serde_json::from_str(header.strip_prefix("Immutable source: ").unwrap()).unwrap();
        assert!(source["available_paths"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("/details/reason")));
        sources.push(source.clone());
        if i == 0 {
            assert!(inspection.content.len() > 6000);
        } else {
            assert!(inspection.content.len() < 5000);
            let body: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!(body["fields"]["state"], "failed");
            assert!(body["omitted_fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["path"] == "/details"));
        }
        let args = serde_json::json!({"receipt_id":source["receipt_id"],"revision":source["revision"],"paths":["/details/reason","/a~1b~0c"]});
        let read = call(tools, "read_receipt_fields", args.clone());
        assert!(read.success, "{}", read.content);
        let read: serde_json::Value = serde_json::from_str(&read.content).unwrap();
        assert_eq!(read["fields"]["/details/reason"], "blocked");
        assert_eq!(read["fields"]["/a~1b~0c"], "中🪶");
        for patch in [
            serde_json::json!({"receipt_id":"other-scope"}),
            serde_json::json!({"revision":"0".repeat(64)}),
            serde_json::json!({"session_id":"other"}),
            serde_json::json!({"paths":["/absent"]}),
            serde_json::json!({"paths":["/details"]}),
            serde_json::json!({"paths":["/state","/state"]}),
        ] {
            let mut bad = args.clone();
            bad.as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(!call(tools, "read_receipt_fields", bad).success);
        }
        assert_eq!(
            case.receipt.as_ref().unwrap(),
            &original,
            "neither view may replace the original"
        );
    }
    assert_eq!(sources[0], sources[1]);
    let wide = serde_json::Value::Object(
        (0..16)
            .map(|i| (format!("field-{i}"), serde_json::json!("v".repeat(500))))
            .collect(),
    );
    let mut wide_case = Case {
        receipt: Some(wide),
        ..case
    };
    let wide_view = registry(&wide_case, "envelope", Arc::new(AtomicUsize::new(0)));
    assert!(
        !call(&wide_view, "inspect_task_receipt", serde_json::json!({})).success,
        "overflow cannot masquerade as a complete bounded view"
    );
    for (experiment, expected) in [("receipt_readback", false), ("receipt_batching", true)] {
        wide_case.experiment = experiment.into();
        let tools = registry(&wide_case, "raw", Arc::new(AtomicUsize::new(0)));
        let read = call(
            &tools,
            "read_receipt_fields",
            serde_json::json!({"receipt_id":"fixture:receipt-test",
            "revision":format!("{:x}",Sha256::digest(wide_case.receipt.as_ref().unwrap().to_string().as_bytes())),
            "paths":["/field-0","/field-1","/field-2","/field-3","/field-4"]}),
        );
        assert_eq!(read.success, expected);
    }
}

#[test]
fn source_guidance_keeps_scope_full_revision_and_exact_size_contracts() {
    let original = serde_json::json!({"diagnostics":"中🪶".repeat(2000),"plan":{"name":"浇水","debug":"x".repeat(8000)}});
    let case = Case {
        id: "guidance-test".into(),
        experiment: "receipt_guidance".into(),
        history: vec![],
        questions: vec!["q".into()],
        query: None,
        receipt: Some(original.clone()),
        scope: None,
    };
    let call = |registry: &ToolRegistry, name: &str, args| {
        registry
            .execute(
                &crate::agent::tool::ToolCall::new(name.into(), args),
                "test",
                Path::new("."),
            )
            .result
    };
    for arm in arms("receipt_guidance") {
        let registry = registry(&case, arm, Arc::new(AtomicUsize::new(0)));
        assert_eq!(registry.get_schemas().len(), 2);
        let result = call(&registry, "inspect_task_receipt", serde_json::json!({}));
        assert!(result.success, "{}", result.content);
        assert!(result.content.len() < 5000);
        let header = result.content.split_once("\nView:\n").unwrap().0;
        let mut identity: serde_json::Value =
            serde_json::from_str(header.strip_prefix("Immutable source: ").unwrap()).unwrap();
        let catalog = identity
            .as_object_mut()
            .unwrap()
            .remove("available_paths")
            .unwrap();
        let short = arm.contains("short");
        let sized = matches!(*arm, "sizes" | "short_sizes");
        assert_eq!(identity.get("source_ref").is_some(), short);
        assert_eq!(identity.get("revision").is_some(), !short);
        if short {
            assert_eq!(identity["source_ref"].as_str().unwrap().len(), 24);
        }
        if sized {
            let field = catalog
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["path"] == "/diagnostics")
                .unwrap();
            assert_eq!(
                field["encoded_bytes"],
                original["diagnostics"].to_string().len()
            );
            assert!(field["encoded_bytes"].as_u64().unwrap() > 5000);
        } else {
            assert!(catalog
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("/diagnostics")));
        }
        let mut args = identity.clone();
        args["paths"] = serde_json::json!(["/plan/name"]);
        let result = call(&registry, "read_receipt_fields", args.clone());
        assert!(result.success, "{}", result.content);
        let read: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(read["fields"]["/plan/name"], "浇水");
        assert_eq!(read.get("revision").is_none(), short);
        if short {
            args["source_ref"] = serde_json::json!("r-foreign");
            assert!(!call(&registry, "read_receipt_fields", args.clone()).success);
            let mut unexpected = identity.clone();
            unexpected["paths"] = serde_json::json!(["/plan/name"]);
            unexpected["revision"] = serde_json::json!("0".repeat(64));
            assert!(!call(&registry, "read_receipt_fields", unexpected).success);
        }
        assert_eq!(case.receipt.as_ref().unwrap(), &original);
    }
    let bounded_case = Case {
        experiment: "receipt_budget_catalog".into(),
        ..case
    };
    let bounded = registry(
        &bounded_case,
        "bounded_catalog",
        Arc::new(AtomicUsize::new(0)),
    );
    let inspected = call(&bounded, "inspect_task_receipt", serde_json::json!({}));
    assert!(inspected.success, "{}", inspected.content);
    let header = inspected.content.split_once("\nView:\n").unwrap().0;
    let source: serde_json::Value =
        serde_json::from_str(header.strip_prefix("Immutable source: ").unwrap()).unwrap();
    assert!(source["unavailable_paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["path"] == "/diagnostics"));
    for path in source["available_paths"].as_array().unwrap() {
        let read = call(
            &bounded,
            "read_receipt_fields",
            serde_json::json!({"source_ref":source["source_ref"],"paths":[path["path"]]}),
        );
        assert!(
            read.success,
            "every advertised single read must fit: {}",
            read.content
        );
        assert_eq!(path["single_read_bytes"], read.content.len());
        assert!(read.content.len() <= 5000);
    }
    assert!(
        !call(
            &bounded,
            "read_receipt_fields",
            serde_json::json!({"source_ref":source["source_ref"],"paths":["/diagnostics"]})
        )
        .success,
        "catalog repair must not bypass the runtime byte guard"
    );
    let other = registry(
        &bounded_case,
        "bounded_catalog",
        Arc::new(AtomicUsize::new(0)),
    );
    assert!(
        !call(
            &other,
            "read_receipt_fields",
            serde_json::json!({"source_ref":source["source_ref"],"paths":["/plan/name"]})
        )
        .success,
        "another registry cannot reuse a previous run's temporary source reference"
    );
    // A short selector must not hide a changed or expired host binding.
    let stale = ReceiptProbe {
        original: Arc::new(original),
        source_id: "owned".into(),
        revision: "0".repeat(64),
        envelope: true,
        reader: true,
        max_paths: 8,
        short_ref: Some("r-owned".into()),
        sized_catalog: true,
        bounded_catalog: false,
        reads: Arc::new(AtomicUsize::new(0)),
    };
    let denied = stale.execute(
        &serde_json::json!({"source_ref":"r-owned","paths":["/plan/name"]}),
        Path::new("."),
    );
    assert!(!denied.success);
    assert!(denied.content.contains("changed"));
}

#[test]
#[ignore = "explicit scripts/context_eval.py --suite continuity --live; saved Luna, synthetic only"]
fn configured_context_variants() {
    assert_eq!(
        std::env::var("ANGELBOT_CONTEXT_EVAL_LIVE").as_deref(),
        Ok("1")
    );
    assert_eq!(
        std::env::var("ANGELBOT_CONTEXT_EVAL_SUITE").as_deref(),
        Ok("continuity")
    );
    let root =
        std::path::PathBuf::from(std::env::var_os("ANGELBOT_PRIVATE_CONTEXT_EVAL_ROOT").unwrap());
    let bytes = std::fs::read(root.join("input.json")).unwrap();
    assert!(bytes.len() <= 256 * 1024);
    let cases: Vec<Case> = serde_json::from_slice(&bytes).unwrap();
    assert!(!cases.is_empty() && cases.len() <= 8);
    let selected_arm = std::env::var("ANGELBOT_CONTEXT_EVAL_ARM").unwrap_or_else(|_| "all".into());
    let case_arms = |experiment: &str| -> Vec<&'static str> {
        arms(experiment)
            .iter()
            .copied()
            .filter(|arm| selected_arm == "all" || selected_arm == *arm)
            .collect()
    };
    let mut limit = 0;
    for case in &cases {
        assert!(case.id.len() <= 80 && !case.questions.is_empty() && case.questions.len() <= 3);
        assert!(case.questions.iter().all(|q| q.len() <= 2048));
        assert!(
            case.history.len() <= 48
                && case.history.iter().all(|s| matches!(
                    s.role.as_str(),
                    "user" | "assistant" | "system"
                ) && s.content.len() <= 16000)
        );
        assert!(!case_arms(&case.experiment).is_empty());
        limit += request_limit(&case.experiment) as u64
            * case_arms(&case.experiment).len() as u64
            * case.questions.len() as u64
            + u64::from(case.experiment == "memory");
    }
    assert!(limit <= 48);
    if let Ok(requested) = std::env::var("ANGELBOT_CONTEXT_EVAL_MAX_CALLS") {
        let requested = requested.parse::<u64>().unwrap();
        assert!(requested > 0 && requested <= limit);
        limit = requested;
    }
    let app = headless_credential_host().unwrap();
    let mut model =
        crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
            .unwrap();
    assert_eq!(model.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
    assert!(model.model.contains("luna"));
    model.max_tokens = model.max_tokens.min(2048);
    model.temperature = 0.0;
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (mut tokens,mut calls)=(0,0);
        // Freeze the clock-bearing system text within the paired screen. A
        // freshly built prompt per arm otherwise changes the experimental input.
        let paired_system=crate::agent::prompt::build_agent_system_prompt(&config(),&[],&[]);
        for (case_index,case) in cases.iter().enumerate() {
            let mut conditions=case_arms(&case.experiment);
            if matches!(case.experiment.as_str(),"receipt_readback"|"receipt_batching"|"receipt_guidance"|"receipt_budget_catalog"|"preference_neighborhood"|"preference_grouping") && case_index%2==1 {conditions.reverse();}
            for arm in &conditions {
                if calls>=limit {return;}
                let durable=(case.experiment=="durable_sources").then(||super::durable_sources::Journey::new(&root,&case.id,arm,case.receipt.as_ref().unwrap()));
                let mut durable_history=Vec::new();
                let mut history=source_history(case,*arm=="excerpt");
                let mut source_ids=Vec::<String>::new();
                if case.experiment=="neighborhood" {(history,source_ids)=neighboring_history(case,if *arm=="neighbors" {2} else {0});}
                if case.experiment=="preference_neighborhood" {(history,source_ids)=preference_neighborhood::history(case,*arm=="source_neighbors");}
                if case.experiment=="preference_grouping" {(history,source_ids)=preference_neighborhood::history(case,true);if *arm=="episode" {preference_neighborhood::group(&mut history);}}
                if *arm=="checkpoint" {
                    let (provider,metrics)=bounded_provider(&model,app.handle(),&root,1);
                    let started=Instant::now();
                    let mut row=serde_json::json!({"id":case.id,"experiment":case.experiment,"arm":"checkpoint_build","turn":0,"model":model.model,"status":"rejected_checkpoint","answer":null});
                    let hook=DefaultCompactionHook::with_token_budget(provider,1,1);
                    match tokio::time::timeout(Duration::from_secs(120),hook.maybe_compact(&mut history)).await {
                        Ok(CompactionOutcome::Compressed{before_tokens,after_tokens})=>{row["status"]=serde_json::json!("completed");row["beforeEstimatedTokens"]=serde_json::json!(before_tokens);row["afterEstimatedTokens"]=serde_json::json!(after_tokens);row["checkpointMessages"]=serde_json::json!(history.iter().map(|m|serde_json::json!({"role":m.role,"content":m.content})).collect::<Vec<_>>());},
                        Err(_)=>row["status"]=serde_json::json!("timed_out"),
                        _=>{},
                    }
                    if !emit(&mut row,&metrics,started,&mut tokens,&mut calls,limit) {return;}
                }
                let system=if matches!(case.experiment.as_str(),"preference_neighborhood"|"preference_grouping") {paired_system.clone()} else {crate::agent::prompt::build_agent_system_prompt(&config(),&[],&[])};
                let mut conversation=vec![message("system",system)];
                conversation.extend(history);
                for (turn,question) in case.questions.iter().enumerate() {
                    if calls>=limit {return;}
                    let runner=request_limit(&case.experiment)>1;
                    let (provider,metrics)=boxed_bounded_provider(&model,app.handle(),&root,request_limit(&case.experiment).min((limit-calls) as usize));
                    let started=Instant::now();
                    let mut row=serde_json::json!({"id":case.id,"experiment":case.experiment,"arm":arm,"turn":turn+1,"model":model.model,"status":"invalid_response","answer":null,"sourceIds":source_ids,"sourceReads":0});
                    if runner {
                        let reads=Arc::new(AtomicUsize::new(0));
                        let tools=if durable.is_some() {ToolRegistry::empty()} else {registry(case,arm,reads.clone())};
                        let mut task_config=config();
                        task_config.preferences.use_long_term_memory=matches!(case.experiment.as_str(),"conversation_recall"|"durable_sources");
                        task_config.work_dir=Some(root.clone());
                        let agent=if let Some(journey)=&durable {journey.runner(provider,task_config,arm,turn)} else {AgentRunner::with_registry(provider,task_config,tools)};
                        match tokio::time::timeout(Duration::from_secs(180),agent.run_slice_with_private_transcript(question,&durable_history,None)).await {
                            Err(_)=>row["status"]=serde_json::json!("timed_out"),
                            Ok(result)=>{
                                if let Some(journey)=&durable {
                                    journey.annotate(&mut row,turn);
                                    if turn==0 && matches!(&result.outcome,AgentSliceOutcome::Completed(_)) {durable_history=journey.settle_and_reopen(&result);}
                                }
                                let outcome=result.outcome;
                                let (status,response)=match outcome {AgentSliceOutcome::Completed(r)=>("completed",r),AgentSliceOutcome::AwaitingConfirmation(r)=>("awaiting_confirmation",r),AgentSliceOutcome::NeedsAttention(r)=>("needs_attention",r),AgentSliceOutcome::Stopped(r)=>("stopped",r)};
                                row["status"]=serde_json::json!(status);
                                match response {AgentResponse::TextWithToolCalls{text,steps}=>{
                                    if case.experiment=="conversation_recall" {reads.store(steps.iter().filter(|s|s.success&&matches!(s.tool_name.as_str(),"recall_conversation"|"read_conversation")).count(),Ordering::SeqCst);}
                                    if durable.is_some() {reads.store(steps.iter().filter(|s|s.success&&matches!(s.tool_name.as_str(),"inspect_task_receipt"|"read_tool_result")).count(),Ordering::SeqCst);}
                                    row["sourceReads"]=serde_json::json!(steps.iter().filter(|s|s.success&&matches!(s.tool_name.as_str(),"read_receipt_fields"|"read_tool_result")).count());
                                    row["answer"]=serde_json::json!(text);row["steps"]=serde_json::json!(steps);
                                },AgentResponse::Text(text)=>row["answer"]=serde_json::json!(text),AgentResponse::Error(_)=>row["status"]=serde_json::json!("runner_error")}
                            }
                        }
                        row["targetReads"]=serde_json::json!(reads.load(Ordering::SeqCst));
                    } else {
                        conversation.push(message("user",question.clone()));
                        answer(provider.as_ref(),&conversation,&mut row).await;
                        if let Some(text)=row["answer"].as_str() {conversation.push(message("assistant",text.into()));}
                    }
                    if !emit(&mut row,&metrics,started,&mut tokens,&mut calls,limit) {return;}
                    if durable.is_some() && row["status"]!="completed" {break;}
                }
            }
        }
    });
}
