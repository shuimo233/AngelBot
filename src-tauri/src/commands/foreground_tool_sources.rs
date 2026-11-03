//! Main-only, version-bound views over the existing durable result journal.
//! No second receipt store and no replay or authorization from source text.
use crate::agent::{
    tool::{Tool, ToolCategory, ToolHandler, ToolResult},
    ToolRegistry,
};
use crate::agent::{
    tool_context::{self, ToolContextProjector},
    tool_source::{self, SourceRecord},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

pub(crate) struct ForegroundToolSources {
    db: Arc<Mutex<Connection>>,
    session: String,
    current_run: Option<String>,
    allow_history: bool,
}
struct Saved {
    id: String,
    run: String,
    session: String,
    sequence: i64,
    event_type: String,
    payload: String,
    created_at: i64,
}
impl Saved {
    fn record(&self) -> SourceRecord<'_> {
        SourceRecord {
            id: &self.id,
            run_id: &self.run,
            session_id: &self.session,
            sequence: self.sequence,
            event_type: &self.event_type,
            payload: &self.payload,
            created_at: self.created_at,
        }
    }
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            run: row.get(1)?,
            session: row.get(2)?,
            sequence: row.get(3)?,
            event_type: row.get(4)?,
            payload: row.get(5)?,
            created_at: row.get(6)?,
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    source_ref: Option<String>,
    path: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
    format: Option<String>,
    tail: Option<bool>,
}

impl ForegroundToolSources {
    pub(crate) fn new(
        db: Arc<Mutex<Connection>>,
        session: String,
        current_run: Option<String>,
        allow_history: bool,
    ) -> Self {
        Self {
            db,
            session,
            current_run,
            allow_history,
        }
    }

    fn admit(&self, conn: &Connection, run: &str) -> Result<(), String> {
        let (branch, leaf, _) =
            super::foreground_context_recall::source_branch(conn, &self.session)?;
        let owner: Option<(String, bool, Option<String>, String)> = conn
            .query_row(
                "SELECT m.role,m.is_provisional,m.parent_id,t.status FROM task_runs t
             JOIN messages m ON m.id=t.message_id AND m.session_id=t.session_id
             WHERE t.id=?1 AND t.message_id=t.id AND t.session_id=?2",
                params![run, self.session],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((role, hidden, parent, status)) = owner else {
            return Err("Tool source has no foreground owner".into());
        };
        if role != "assistant" {
            return Err("Tool source has no assistant owner".into());
        }
        if self.allow_history && !hidden {
            if branch.iter().any(|id| id == run)
                || super::foreground_history::conversation_branch(conn, &self.session, None)?
                    .anchors
                    .iter()
                    .any(|a| a.id == run)
            {
                return Ok(());
            }
        }
        if self.current_run.as_deref() == Some(run)
            && hidden
            && status == "running"
            && leaf.is_some()
            && (parent == leaf || leaf.as_deref() == Some(run))
        {
            return Ok(());
        }
        Err("Tool source is outside this Main conversation's current branch or run".into())
    }

    fn read(&self, args: &Value) -> Result<String, String> {
        let args: ReadArgs = serde_json::from_value(args.clone())
            .map_err(|e| format!("Invalid source read: {e}"))?;
        let limit = args.limit.unwrap_or(1200);
        if !(1..=2000).contains(&limit) {
            return Err("Invalid bounded source window".into());
        }
        let Some(reference) = args.source_ref.as_deref() else {
            if !args.path.as_deref().unwrap_or("").is_empty()
                || args.offset.unwrap_or(0) != 0
                || args.format.as_deref().unwrap_or("view") != "view"
                || args.tail.unwrap_or(false)
            {
                return Err(
                    "Source windows require source_ref; use an empty object for recent sources"
                        .into(),
                );
            }
            return self.recent(limit.min(3));
        };
        let id = tool_source::record_id(reference)?;
        let raw = match args.format.as_deref().unwrap_or("view") {
            "view" => false,
            "text" => true,
            _ => return Err("Use view or text source format".into()),
        };
        let conn = self
            .db
            .lock()
            .map_err(|_| "Tool source storage is unavailable")?;
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let saved=tx.query_row(
            "SELECT id,run_id,session_id,sequence,event_type,payload,created_at FROM agent_run_events
             WHERE id=?1 AND session_id=?2 AND event_type='ToolExecutionEnd' AND length(CAST(payload AS BLOB))<=?3",
            params![id,self.session,8*tool_source::MAX_SOURCE_BYTES],Saved::from_row)
            .optional().map_err(|e|e.to_string())?.ok_or("Tool source is unavailable in this conversation")?;
        self.admit(&tx, &saved.run)?;
        let source = tool_source::open(&saved.record())?;
        let output = tool_context::view(
            reference,
            &source.tool_name,
            source.reported_success,
            &source.output,
            args.path.as_deref().unwrap_or(""),
            args.offset.unwrap_or(0),
            limit,
            raw,
            args.tail.unwrap_or(false),
        )?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(output)
    }

    fn recent(&self, limit: usize) -> Result<String, String> {
        let conn = self
            .db
            .lock()
            .map_err(|_| "Tool source storage is unavailable")?;
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let (mut allowed, _, branch_incomplete) =
            super::foreground_context_recall::source_branch(&tx, &self.session)?;
        if !self.allow_history {
            allowed.clear();
        }
        if let Some(run) = &self.current_run {
            self.admit(&tx, run)?;
            allowed.push(run.clone());
        }
        allowed.sort();
        allowed.dedup();
        let ids = serde_json::to_string(&allowed).map_err(|e| e.to_string())?;
        let rows = {
            let mut stmt=tx.prepare(
                "SELECT e.id
                 FROM agent_run_events e JOIN json_each(?2) a ON a.value=e.run_id
                 WHERE e.session_id=?1 AND e.event_type='ToolExecutionEnd' AND length(CAST(e.payload AS BLOB))<=?3
                   AND CASE WHEN json_valid(e.payload) THEN json_extract(e.payload,'$.data.tool_name') END!='read_tool_result'
                 ORDER BY e.created_at DESC,e.sequence DESC,e.id LIMIT 32"
            ).map_err(|e|e.to_string())?;
            let rows = stmt
                .query_map(
                    params![self.session, ids, 8 * tool_source::MAX_SOURCE_BYTES],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        let mut items = Vec::new();
        let mut skipped = 0;
        let mut more = rows.len() == 32 || (self.allow_history && branch_incomplete);
        for id in rows {
            // Fetch one original at a time; a small index must not retain 32
            // potentially megabyte-sized receipts just to return three refs.
            let saved = tx.query_row(
                "SELECT id,run_id,session_id,sequence,event_type,payload,created_at FROM agent_run_events WHERE id=?1 AND session_id=?2",
                params![id,self.session], Saved::from_row,
            ).map_err(|e|e.to_string())?;
            self.admit(&tx, &saved.run)?;
            match tool_source::open(&saved.record()) {
                Ok(source) => {
                    if items.len() == limit {
                        more = true;
                        break;
                    }
                    let goal: String = tx
                        .query_row(
                            "SELECT goal FROM task_runs WHERE id=?1 AND session_id=?2",
                            params![saved.run, self.session],
                            |r| r.get(0),
                        )
                        .map_err(|e| e.to_string())?;
                    items.push(json!({"source_ref":tool_source::reference(&saved.id)?,"tool_name":source.tool_name.chars().take(120).collect::<String>(),"goal_excerpt":goal.chars().take(80).collect::<String>(),"reported_success":source.reported_success,"created_at":saved.created_at}));
                }
                Err(_) => skipped += 1,
            }
        }
        let output=json!({"historical_data":true,"recent_sources":items,"index_incomplete":more,"unavailable_candidates":skipped,"note":"At most three verified sources from 32 recent candidates and the bounded active branch; empty is not proof of absence."}).to_string();
        tx.commit().map_err(|e| e.to_string())?;
        if output.len() > tool_context::MAX_VIEW_BYTES {
            return Err("Source index exceeds byte budget".into());
        }
        Ok(output)
    }

    fn current_source(
        &self,
        call_id: &str,
        tool_name: &str,
        output: &str,
    ) -> Result<Option<(String, tool_source::ToolSource)>, String> {
        let Some(run) = &self.current_run else {
            return Ok(None);
        };
        let conn = self
            .db
            .lock()
            .map_err(|_| "Tool source storage is unavailable")?;
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        self.admit(&tx, run)?;
        let rows = {
            let mut stmt=tx.prepare(
                "SELECT id,run_id,session_id,sequence,event_type,payload,created_at FROM agent_run_events
                 WHERE run_id=?1 AND session_id=?2 AND event_type='ToolExecutionEnd'
                   AND CASE WHEN json_valid(payload) THEN json_extract(payload,'$.data.call_id') END=?3
                   AND length(CAST(payload AS BLOB))<=?4 ORDER BY sequence DESC LIMIT 2"
            ).map_err(|e|e.to_string())?;
            let rows = stmt
                .query_map(
                    params![
                        run,
                        self.session,
                        call_id,
                        8 * tool_source::MAX_SOURCE_BYTES
                    ],
                    Saved::from_row,
                )
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        if rows.len() != 1 {
            return Ok(None);
        }
        let source = tool_source::open(&rows[0].record())?;
        if source.call_id != call_id
            || source.tool_name != tool_name
            || source.output != output
            || !source.reported_success
        {
            return Ok(None);
        }
        let reference = tool_source::reference(&rows[0].id)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(Some((reference, source)))
    }
}
impl ToolContextProjector for ForegroundToolSources {
    fn project(
        &self,
        call_id: &str,
        tool_name: &str,
        result: &ToolResult,
    ) -> Result<Option<String>, String> {
        if !result.success
            || result.terminate_batch
            || !result.images.is_empty()
            || tool_name == "read_tool_result"
            || result.content.len() <= 6000
        {
            return Ok(None);
        }
        let Some((reference, source)) = self.current_source(call_id, tool_name, &result.content)?
        else {
            return Ok(None);
        };
        let visible = tool_context::view(
            &reference,
            &source.tool_name,
            source.reported_success,
            &source.output,
            "",
            0,
            1200,
            false,
            false,
        )?;
        if visible.len() + 256 >= result.content.len() {
            return Ok(None);
        }
        Ok(Some(visible))
    }
}
impl ToolHandler for ForegroundToolSources {
    fn name(&self) -> &str {
        "read_tool_result"
    }
    fn execute(&self, args: &Value, _: &Path) -> ToolResult {
        match self.read(args) {
            Ok(output) => ToolResult::success(self.name(), output),
            Err(error) => ToolResult::error(self.name(), error),
        }
    }
}
pub(crate) fn register(registry: &mut ToolRegistry, sources: Arc<ForegroundToolSources>) {
    registry.register(Tool::new("read_tool_result",
        "只读当前Main已有工具结果，不重跑操作。无source_ref时列最近已核验来源；有引用时读原件。先按任务选择字段或文本范围；只需结尾时用tail=true，不从头翻页。部分视图／空索引不代表完整历史、当前状态或授权。输出最多5000字节。",
        json!({"type":"object","additionalProperties":false,"properties":{
            "source_ref":{"type":"string","minLength":24,"maxLength":24,"description":"照抄已返回的引用；省略则列索引，不猜造引用。"},
            "path":{"type":"string","maxLength":512,"description":"view的JSON Pointer，如/content；优先选择所需字段，子字段可继续回查。"},
            "offset":{"type":"integer","minimum":0,"description":"默认0；文本按Unicode字、容器按条目定位。续读用next_offset，不是页码。"},
            "limit":{"type":"integer","minimum":1,"maximum":2000,"description":"默认1200；文本为字数，容器最多16项，来源索引最多3条。"},
            "format":{"type":"string","enum":["view","text"],"description":"默认view，返回字段及omitted_paths；text精确读原文，仅限空path。"},
            "tail":{"type":"boolean","description":"需要文本结尾时设true，直接取末尾limit字，不必从头翻页；仅限文本且offset须为0。索引须为false。"}
        },"required":[]})).with_category(ToolCategory::Extension).with_read_only_budget_hint(true),sources);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::event::{AgentEvent, AgentRunJournal};

    fn seed(conn: &Connection) {
        crate::db::migrate(conn).unwrap();
        conn.execute_batch("INSERT INTO sessions(id,title,created_at,updated_at) VALUES('s','Owned',1,1),('other','Foreign',1,1);
            INSERT INTO messages(id,session_id,role,content,parent_id,is_provisional,created_at) VALUES
            ('u','s','user','Inspect task',NULL,0,1),('r','s','assistant','','u',1,2),
            ('sibling','s','user','Other branch',NULL,0,3);
            INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at)
            VALUES('r','s','r','Inspect task','running','[]',2,2);
            UPDATE sessions SET leaf_message_id='u' WHERE id='s';").unwrap();
    }

    fn append(db: Arc<Mutex<Connection>>, call: &str, output: &str) {
        AgentRunJournal::new(db, "r".into())
            .append(
                "s",
                &AgentEvent::ToolExecutionEnd {
                    call_id: call.into(),
                    tool_name: "inspect".into(),
                    success: true,
                    output: output.into(),
                    error: None,
                },
            )
            .unwrap();
    }

    #[test]
    fn direct_file_windows_keep_exact_partial_receipts_without_rereading_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("archive.txt");
        std::fs::write(&file, format!("{}\nowned-tail", "x".repeat(9000))).unwrap();
        let result = crate::agent::handlers::ReadFileHandler::new().execute(
            &json!({"path":"archive.txt","tail":true,"limit":10}),
            dir.path(),
        );
        assert!(result.success, "{}", result.content);
        let window: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(window["content"], "owned-tail");
        assert_eq!(window["complete"], false);
        let db = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        seed(&db.lock().unwrap());
        AgentRunJournal::new(db.clone(), "r".into())
            .append(
                "s",
                &AgentEvent::ToolExecutionEnd {
                    call_id: "window".into(),
                    tool_name: "read_file".into(),
                    success: true,
                    output: result.content.clone(),
                    error: None,
                },
            )
            .unwrap();
        let id: String = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT id FROM agent_run_events WHERE event_type='ToolExecutionEnd'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let reference = tool_source::reference(&id).unwrap();
        let sources = ForegroundToolSources::new(db.clone(), "s".into(), Some("r".into()), false);
        assert!(sources
            .project("window", "read_file", &result)
            .unwrap()
            .is_none());
        std::fs::write(&file, "newer disk content").unwrap();
        let changes = db.lock().unwrap().total_changes();
        let saved: Value =
            serde_json::from_str(&sources.read(&json!({"source_ref":reference})).unwrap()).unwrap();
        assert_eq!(
            saved["values"]["complete"], false,
            "a complete receipt is not a complete file read"
        );
        assert_eq!(saved["values"]["total_chars"], window["total_chars"]);
        assert_eq!(saved["values"]["resolvedPath"], window["resolvedPath"]);
        let content: Value = serde_json::from_str(
            &sources
                .read(&json!({"source_ref":reference,"path":"/content"}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(content["text"], "owned-tail");
        assert_eq!(db.lock().unwrap().total_changes(), changes);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "newer disk content"
        );
        {
            let conn = db.lock().unwrap();
            conn.execute("UPDATE messages SET is_provisional=0 WHERE id='r'", [])
                .unwrap();
            conn.execute("UPDATE task_runs SET status='completed' WHERE id='r'", [])
                .unwrap();
            conn.execute("UPDATE sessions SET leaf_message_id='r' WHERE id='s'", [])
                .unwrap();
        }
        assert!(
            ForegroundToolSources::new(db.clone(), "s".into(), None, false)
                .read(&json!({"source_ref":reference}))
                .is_err()
        );
        assert!(ForegroundToolSources::new(db, "s".into(), None, true)
            .read(&json!({"source_ref":reference,"path":"/content"}))
            .unwrap()
            .contains("owned-tail"));
    }

    #[test]
    fn durable_sources_survive_reopen_without_replay_and_recheck_branch_and_revision() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.db");
        let db = Arc::new(Mutex::new(Connection::open(&path).unwrap()));
        seed(&db.lock().unwrap());
        let output = json!({"state":"failed","details":{"reason":"window occupied","trace":"诊断🪶".repeat(2500)}}).to_string();
        append(db.clone(), "inspect-1", &output);
        let current = ForegroundToolSources::new(db.clone(), "s".into(), Some("r".into()), false);
        let projected = current
            .project(
                "inspect-1",
                "inspect",
                &ToolResult::success("inspect", &output),
            )
            .unwrap()
            .unwrap();
        assert!(projected.len() < 1000);
        let projected: Value = serde_json::from_str(&projected).unwrap();
        let reference = projected["source_ref"].as_str().unwrap().to_owned();
        assert_eq!(reference.len(), 24);
        assert_eq!(projected["omitted_paths"], json!(["/details"]));
        let changes = db.lock().unwrap().total_changes();
        let details: Value = serde_json::from_str(
            &current
                .read(&json!({"source_ref":reference,"path":"/details"}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(details["values"]["reason"], "window occupied");
        let trace: Value = serde_json::from_str(
            &current
                .read(&json!({"source_ref":reference,"path":"/details/trace","limit":2000}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            trace["has_more"], true,
            "large observations are successful partial reads, not failed actions"
        );
        let tail: Value = serde_json::from_str(&current.read(&json!({"source_ref":reference,"path":"/details/trace","limit":2000,"tail":true})).unwrap()).unwrap();
        let text = tail["text"].as_str().unwrap();
        assert!(text.ends_with('🪶'));
        assert_eq!(
            tail["offset"].as_u64().unwrap() + text.chars().count() as u64,
            7500
        );
        assert_eq!(tail["has_more"], false);
        assert_eq!(tail["complete"], false);
        assert_eq!(tail["omitted_before"], true);
        for args in [
            json!({"tail":true}),
            json!({"source_ref":reference,"path":"/details","tail":true}),
            json!({"source_ref":reference,"path":"/details/trace","tail":true,"offset":1}),
        ] {
            assert!(current.read(&args).is_err());
        }
        assert!(current
            .read(&json!({"source_ref":reference,"path":"/missing"}))
            .is_err());
        assert!(current
            .read(&json!({"source_ref":reference,"offset":99999}))
            .is_err());
        assert!(current
            .read(&json!({"source_ref":reference,"session":"other"}))
            .is_err());
        assert!(
            ForegroundToolSources::new(db.clone(), "other".into(), Some("r".into()), true)
                .read(&json!({"source_ref":reference}))
                .is_err()
        );
        assert!(
            current
                .project(
                    "absent",
                    "inspect",
                    &ToolResult::success("inspect", &output)
                )
                .unwrap()
                .is_none(),
            "journal absence must not mint a dangling reference"
        );
        assert_eq!(
            db.lock().unwrap().total_changes(),
            changes,
            "source reads never execute or mutate work"
        );
        {
            let conn = db.lock().unwrap();
            let raw: String = conn
                .query_row("SELECT payload FROM agent_run_events", [], |r| r.get(0))
                .unwrap();
            let AgentEvent::ToolExecutionEnd { output: saved, .. } =
                serde_json::from_str(&raw).unwrap()
            else {
                panic!()
            };
            assert_eq!(
                saved, output,
                "UI/audit original is not replaced by the model view"
            );
            conn.execute_batch("UPDATE messages SET is_provisional=0,content='Inspected' WHERE id='r'; UPDATE task_runs SET status='completed' WHERE id='r'; UPDATE sessions SET leaf_message_id='r' WHERE id='s';").unwrap();
        }
        drop(current);
        drop(db);
        // New connection and adapter: the reference is journal-owned, not a cache handle.
        let db = Arc::new(Mutex::new(Connection::open(&path).unwrap()));
        let resumed = ForegroundToolSources::new(db.clone(), "s".into(), None, true);
        let recent: Value = serde_json::from_str(&resumed.read(&json!({})).unwrap()).unwrap();
        assert_eq!(recent["recent_sources"][0]["source_ref"], reference);
        let mut exact = String::new();
        let mut offset = 0;
        loop {
            let page: Value = serde_json::from_str(&resumed.read(&json!({"source_ref":reference,"format":"text","offset":offset,"limit":2000})).unwrap()).unwrap();
            exact.push_str(page["text"].as_str().unwrap());
            let Some(next) = page["next_offset"].as_u64() else {
                break;
            };
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(exact, output);
        assert!(
            ForegroundToolSources::new(db.clone(), "s".into(), None, false)
                .read(&json!({"source_ref":reference}))
                .is_err(),
            "memory-disabled cold runs cannot read old receipts"
        );
        db.lock()
            .unwrap()
            .execute(
                "UPDATE sessions SET leaf_message_id='sibling' WHERE id='s'",
                [],
            )
            .unwrap();
        assert!(resumed.read(&json!({"source_ref":reference})).is_err());
        db.lock()
            .unwrap()
            .execute("UPDATE sessions SET leaf_message_id='r' WHERE id='s'", [])
            .unwrap();
        db.lock().unwrap().execute("UPDATE agent_run_events SET payload=replace(payload,'window occupied','forged outcome')", []).unwrap();
        assert!(resumed
            .read(&json!({"source_ref":reference}))
            .unwrap_err()
            .contains("version"));
        let recent: Value = serde_json::from_str(&resumed.read(&json!({})).unwrap()).unwrap();
        assert_eq!(recent["unavailable_candidates"], 1);
        assert_eq!(recent["recent_sources"], json!([]));
    }

    struct FixtureProvider;
    #[async_trait::async_trait]
    impl crate::llm::LlmProvider for FixtureProvider {
        fn name(&self) -> &str {
            "durable-source-fixture"
        }
        async fn chat(
            &self,
            messages: &[crate::llm::Message],
            _: Option<&[crate::llm::ToolSchema]>,
            _: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<crate::llm::LlmResponse, String> {
            use crate::llm::{LlmResponse, StopReason, ToolCall};
            let tools: Vec<_> = messages.iter().filter(|m| m.role == "tool").collect();
            let call = match tools.len() {
                0 => ToolCall {
                    id: "inspect-actual".into(),
                    name: "inspect".into(),
                    arguments: json!({}),
                },
                1 => {
                    let view: Value =
                        serde_json::from_str(tools[0].content.strip_prefix("Result: ").unwrap())
                            .unwrap();
                    assert!(tools[0].content.len() < 1000);
                    assert_eq!(view["omitted_paths"], json!(["/details"]));
                    ToolCall {
                        id: "read-actual".into(),
                        name: "read_tool_result".into(),
                        arguments: json!({"source_ref":view["source_ref"],"path":"/details"}),
                    }
                }
                2 => {
                    let view: Value =
                        serde_json::from_str(tools[1].content.strip_prefix("Result: ").unwrap())
                            .unwrap();
                    assert_eq!(view["values"]["reason"], "window occupied");
                    return Ok(LlmResponse::Text {
                        text: "The window is occupied; no operation was replayed.".into(),
                        stop_reason: StopReason::EndTurn,
                    });
                }
                _ => panic!("deterministic fixture must stop after one source read"),
            };
            Ok(LlmResponse::ToolCalls {
                calls: vec![call],
                text: None,
                stop_reason: StopReason::ToolUse,
            })
        }
    }
    struct FixtureTool {
        output: String,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl ToolHandler for FixtureTool {
        fn name(&self) -> &str {
            "inspect"
        }
        fn execute(&self, _: &Value, _: &Path) -> ToolResult {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            ToolResult::success(self.name(), &self.output)
        }
    }

    #[tokio::test]
    async fn durable_sources_runner_keeps_raw_evidence_and_replays_only_compact_protocol() {
        use crate::agent::event::{DurableEventEmitter, NullEventEmitter};
        use crate::agent::{AgentConfig, AgentResponse, AgentRunner, AgentSliceOutcome};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runner.db");
        let db = Arc::new(Mutex::new(Connection::open(&path).unwrap()));
        seed(&db.lock().unwrap());
        let source = Arc::new(ForegroundToolSources::new(
            db.clone(),
            "s".into(),
            Some("r".into()),
            true,
        ));
        let output=json!({"state":"failed","details":{"reason":"window occupied","trace":"d".repeat(11000)}}).to_string();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = ToolRegistry::empty();
        registry.register(
            Tool::new(
                "inspect",
                "Read synthetic task state",
                json!({"type":"object","properties":{}}),
            )
            .with_read_only_budget_hint(true),
            Arc::new(FixtureTool {
                output: output.clone(),
                calls: calls.clone(),
            }),
        );
        register(&mut registry, source.clone());
        let runner =
            AgentRunner::with_registry(Box::new(FixtureProvider), AgentConfig::default(), registry)
                .with_session_id("s".into())
                .with_emitter(Arc::new(DurableEventEmitter::new(
                    Arc::new(NullEventEmitter),
                    db.clone(),
                    "r".into(),
                )))
                .with_tool_context_projector(source.clone());
        let result = runner
            .run_slice_with_private_transcript(
                "Inspect the task and explain its state without retrying it.",
                &[],
                None,
            )
            .await;
        let AgentSliceOutcome::Completed(AgentResponse::TextWithToolCalls { steps, .. }) =
            &result.outcome
        else {
            panic!("{:?}", result.outcome)
        };
        assert_eq!(steps[0].output, output);
        assert_eq!(steps.len(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let projected = result
            .protocol_transcript
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("inspect-actual"))
            .unwrap();
        assert!(projected.content.len() < 1000);
        let reference: Value =
            serde_json::from_str(projected.content.strip_prefix("Result: ").unwrap()).unwrap();
        {
            let conn = db.lock().unwrap();
            let metadata = super::super::foreground_history::private_transcript_metadata(
                &result.protocol_transcript,
            )
            .unwrap()
            .unwrap();
            conn.execute(
                "UPDATE messages SET is_provisional=0,metadata=?1,content='Inspected' WHERE id='r'",
                [metadata],
            )
            .unwrap();
            conn.execute_batch("UPDATE task_runs SET status='completed' WHERE id='r'; UPDATE sessions SET leaf_message_id='r' WHERE id='s';").unwrap();
        }
        drop(runner);
        drop(source);
        drop(db);
        let db = Arc::new(Mutex::new(Connection::open(path).unwrap()));
        let history =
            super::super::foreground_history::hydrate_llm_history(&db.lock().unwrap(), "s", 10)
                .unwrap();
        let saved = history
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("inspect-actual"))
            .unwrap();
        assert_eq!(
            saved.content, projected.content,
            "reopening does not inflate compact private protocol back to raw audit"
        );
        let resumed = ForegroundToolSources::new(db, "s".into(), None, true);
        let read: Value = serde_json::from_str(
            &resumed
                .read(&json!({"source_ref":reference["source_ref"],"path":"/details"}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(read["values"]["reason"], "window occupied");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn durable_sources_never_project_failures_images_or_ambiguous_calls() {
        let db = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        seed(&db.lock().unwrap());
        let output = "x".repeat(10000);
        append(db.clone(), "same", &output);
        let current = ForegroundToolSources::new(db.clone(), "s".into(), Some("r".into()), true);
        assert_eq!(
            current.read(&json!({})).unwrap(),
            current
                .read(&json!({"format":"view","limit":1200,"offset":0,"path":""}))
                .unwrap(),
            "explicit defaults must be equivalent to omitted defaults"
        );
        for args in [
            json!({"path":"/details"}),
            json!({"offset":1}),
            json!({"limit":0}),
            json!({"limit":2001}),
            json!({"format":"text"}),
        ] {
            assert!(
                current.read(&args).is_err(),
                "do not ignore non-default source window arguments"
            );
        }
        assert!(current
            .project("same", "inspect", &ToolResult::error("inspect", &output))
            .unwrap()
            .is_none());
        let mut result = ToolResult::success("inspect", &output);
        result.terminate_batch = true;
        assert!(current
            .project("same", "inspect", &result)
            .unwrap()
            .is_none());
        assert!(current
            .project(
                "same",
                "wrong-tool",
                &ToolResult::success("wrong-tool", &output)
            )
            .unwrap()
            .is_none());
        append(db.clone(), "same", &output);
        let index: Value = serde_json::from_str(
            &current
                .read(&json!({"format":"view","limit":1500}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(index["recent_sources"].as_array().unwrap().len(), 2);
        let bounded: Value =
            serde_json::from_str(&current.read(&json!({"limit":1})).unwrap()).unwrap();
        assert_eq!(bounded["recent_sources"].as_array().unwrap().len(), 1);
        assert_eq!(
            bounded["index_incomplete"], true,
            "limit is an actual index bound, not a silently ignored option"
        );
        assert!(current
            .project("same", "inspect", &ToolResult::success("inspect", &output))
            .unwrap()
            .is_none());
        db.lock().unwrap().execute_batch("UPDATE sessions SET leaf_message_id=NULL WHERE id='s'; UPDATE messages SET parent_id=NULL WHERE id='r';").unwrap();
        assert!(
            current.read(&json!({})).is_err(),
            "unattached hidden replies are not an active branch"
        );
    }
}
