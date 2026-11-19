//! Main-only, version-bound views over the existing durable result journal.
//! No second receipt store and no replay or authorization from source text.
use crate::agent::{
    tool::{Tool, ToolCategory, ToolHandler, ToolResult},
    ToolRegistry,
};
use crate::agent::{
    tool_context::{self, ToolContextProjector},
    tool_source::{self, SourceRecord, ToolSource},
};
use crate::llm::{Message, ModelProtocol};
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
        visible_history: &[Message],
    ) -> Result<Option<String>, String> {
        let contextual = self.allow_history
            && (tool_name == "recall_conversation"
                || (tool_name == "read_file" && result.content.len() >= 512))
            && has_native_history(visible_history);
        if !result.success
            || result.terminate_batch
            || !result.images.is_empty()
            || !(projectable_output(tool_name, &result.content) || contextual)
        {
            return Ok(None);
        }
        let Some((reference, source)) = self.current_source(call_id, tool_name, &result.content)?
        else {
            return Ok(None);
        };
        if contextual {
            if let Some(projected) =
                contextual_model_view(&reference, &source, visible_history, None)
            {
                return Ok(Some(projected));
            }
        }
        bounded_model_view(&reference, &source)
    }
}

fn projectable_output(tool_name: &str, output: &str) -> bool {
    tool_name != "read_tool_result" && output.len() > 6000
}

fn bounded_model_view(reference: &str, source: &ToolSource) -> Result<Option<String>, String> {
    if !source.reported_success || !projectable_output(&source.tool_name, &source.output) {
        return Ok(None);
    }
    let mut visible = tool_context::view(
        reference,
        &source.tool_name,
        source.reported_success,
        &source.output,
        "",
        0,
        1200,
        false,
        false,
    )?;
    if source.tool_name == "run_project_command" {
        if let Some(with_previews) = command_projection_with_previews(&visible, &source.output) {
            visible = with_previews;
        }
    }
    Ok((visible.len() + 256 < source.output.len()).then_some(visible))
}

/// Verify the exact model-visible text against an already opened, sealed source.
pub(crate) fn matches_sealed_model_output(
    reference: &str,
    source: &ToolSource,
    text: &str,
) -> bool {
    if !source.reported_success {
        return false;
    }
    if text == source.output {
        return true;
    }
    text.strip_prefix("Result: ").is_some_and(|result| {
        if result == source.output {
            return text.chars().count() <= tool_context::MAX_TOOL_RESULT_CONTEXT_CHARS;
        }
        bounded_model_view(reference, source).is_ok_and(|view| view.as_deref() == Some(result))
    })
}

pub(crate) fn matches_sealed_model_output_with_history(
    reference: &str,
    source: &ToolSource,
    text: &str,
    visible_history: &[Message],
) -> bool {
    source.reported_success
        && text.strip_prefix("Result: ").is_some_and(|projected| {
            contextual_model_view(reference, source, visible_history, Some(projected)).as_deref()
                == Some(projected)
        })
}

fn contextual_model_view(
    reference: &str,
    source: &ToolSource,
    history: &[Message],
    emitted: Option<&str>,
) -> Option<String> {
    let history = active_native_history(history)?;
    match source.tool_name.as_str() {
        "recall_conversation" => recall_projection(&source.output, history, emitted),
        "read_file" => repeated_read_projection(reference, source, history, emitted),
        _ => None,
    }
}

fn has_native_history(history: &[Message]) -> bool {
    history.iter().any(|message| {
        message
            .protocol_state
            .as_ref()
            .is_some_and(|protocol| protocol.protocol == ModelProtocol::OpenaiResponses)
    })
}

fn active_native_history(history: &[Message]) -> Option<&[Message]> {
    if !has_native_history(history) {
        return None;
    }
    // The Responses adapter discards old input before its latest matching
    // native checkpoint. Cut at any Responses checkpoint conservatively.
    let active_start = history
        .iter()
        .rposition(|message| {
            message.protocol_state.as_ref().is_some_and(|protocol| {
                protocol.protocol == ModelProtocol::OpenaiResponses
                    && protocol
                        .output_items
                        .iter()
                        .any(|item| item["type"] == "compaction")
            })
        })
        .unwrap_or(0);
    Some(&history[active_start..])
}

fn recall_projection(output: &str, history: &[Message], emitted: Option<&str>) -> Option<String> {
    const TEXT_IN_PRIOR_USER: &str = "text_in_prior_user";
    let emitted: Option<Value> = match emitted {
        Some(value) => Some(serde_json::from_str(value).ok()?),
        None => None,
    };
    let mut value: Value = serde_json::from_str(output).ok()?;
    let rows = value.get_mut("sources")?.as_array_mut()?;
    let mut omitted = false;
    for (index, row) in rows.iter_mut().enumerate() {
        // Replay may now contain additional eligible copies. Verify only the
        // exact subset that was omitted in the saved model-visible receipt.
        if emitted
            .as_ref()
            .is_some_and(|saved| !saved["sources"][index][TEXT_IN_PRIOR_USER].is_string())
        {
            continue;
        }
        if row["role"] != "user" || row["complete"] != true {
            continue;
        }
        let Some(prefix) = row["text"]
            .as_str()
            .and_then(|text| prior_user_prefix(text, history))
        else {
            continue;
        };
        let fields = row.as_object_mut()?;
        fields.remove("text");
        fields.remove("complete");
        fields.insert(TEXT_IN_PRIOR_USER.into(), json!(prefix));
        omitted = true;
    }
    if !omitted {
        return None;
    }
    let projected = value.to_string();
    (projected.len() < output.len()).then_some(projected)
}

fn repeated_read_projection(
    reference: &str,
    source: &ToolSource,
    history: &[Message],
    emitted: Option<&str>,
) -> Option<String> {
    const PREVIOUS_READ: &str = "same_content_as_read_call";
    const MIN_SAVINGS_BYTES: usize = 128;
    let mut value: Value = serde_json::from_str(&source.output).ok()?;
    if value["content"].as_str()?.len() < MIN_SAVINGS_BYTES
        || value["resolvedPath"].as_str()?.is_empty()
        || value.get(PREVIOUS_READ).is_some()
        || value.get("source_ref").is_some()
    {
        return None;
    }
    let requested_call = match emitted {
        Some(text) => Some(
            serde_json::from_str::<Value>(text).ok()?[PREVIOUS_READ]
                .as_str()?
                .to_owned(),
        ),
        None => None,
    };
    let prior_call = history
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| {
            let call_id = message.tool_call_id.as_deref()?;
            if message.role != "tool"
                || call_id.is_empty()
                || call_id == source.call_id.as_str()
                || requested_call
                    .as_deref()
                    .is_some_and(|requested| requested != call_id)
                || message.content.strip_prefix("Result: ") != Some(source.output.as_str())
                || !unique_read_call_before_result(history, call_id, index)
            {
                return None;
            }
            Some(call_id)
        })?;
    let fields = value.as_object_mut()?;
    fields.remove("content");
    fields.insert(PREVIOUS_READ.into(), json!(prior_call));
    fields.insert("source_ref".into(), json!(reference));
    let projected = value.to_string();
    (projected.len() + MIN_SAVINGS_BYTES <= source.output.len()).then_some(projected)
}

fn unique_read_call_before_result(history: &[Message], call_id: &str, result_index: usize) -> bool {
    let mut calls = history
        .iter()
        .enumerate()
        .flat_map(|(index, message)| {
            message
                .tool_calls
                .iter()
                .flatten()
                .map(move |call| (index, message.role.as_str(), call))
        })
        .filter(|(_, _, call)| call.id == call_id);
    calls.next().is_some_and(|(index, role, call)| {
        role == "assistant" && index < result_index && call.name == "read_file"
    }) && calls.next().is_none()
        && history
            .iter()
            .filter(|message| message.tool_call_id.as_deref() == Some(call_id))
            .count()
            == 1
}

fn prior_user_prefix(text: &str, history: &[Message]) -> Option<String> {
    let prefix: String = text.chars().take(24).collect();
    if prefix == text {
        return None;
    }
    let mut matches = history
        .iter()
        .filter(|message| message.role == "user" && message.content.starts_with(&prefix));
    (matches.next()?.content == text && matches.all(|message| message.content == text))
        .then_some(prefix)
}

fn command_projection_with_previews(view: &str, output: &str) -> Option<String> {
    const PREVIEW_CHARS: usize = 192;

    let mut view: Value = serde_json::from_str(view).ok()?;
    if view["values"]["code"] != "COMMAND_EXITED" {
        return None;
    }
    let omitted_paths = view["omitted_paths"].as_array()?;
    let output: Value = serde_json::from_str(output).ok()?;
    let mut previews = serde_json::Map::new();
    for path in ["/stdout", "/stderr"] {
        if !omitted_paths
            .iter()
            .any(|omitted| omitted.as_str() == Some(path))
        {
            continue;
        }
        if let Some(captured) = output.pointer(path).and_then(Value::as_str) {
            let start = captured
                .char_indices()
                .rev()
                .nth(PREVIEW_CHARS - 1)
                .map_or(0, |(index, _)| index);
            previews.insert(path.into(), json!(&captured[start..]));
        }
    }
    if previews.is_empty() {
        return None;
    }
    // Each preview ends at the capture boundary. The receipt's truncation
    // flags describe whether that boundary is also the end of the stream.
    view["captured_tail_previews"] = Value::Object(previews);
    let view = view.to_string();
    (view.len() <= tool_context::MAX_VIEW_BYTES).then_some(view)
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
        append_tool(db, call, "inspect", output);
    }

    fn append_tool(db: Arc<Mutex<Connection>>, call: &str, tool_name: &str, output: &str) {
        AgentRunJournal::new(db, "r".into())
            .append(
                "s",
                &AgentEvent::ToolExecutionEnd {
                    call_id: call.into(),
                    tool_name: tool_name.into(),
                    success: true,
                    output: output.into(),
                    error: None,
                },
            )
            .unwrap();
    }

    #[test]
    fn recall_projection_requires_visible_whole_user_originals_and_verifies_sealed_subset() {
        let db = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        seed(&db.lock().unwrap());
        let original = format!(
            "One visible user requirement. {}",
            "保存原始要求。".repeat(32)
        );
        let additional = format!(
            "Another original user message. {}",
            "另外的偏好。".repeat(32)
        );
        let older = format!(
            "Old user message before the checkpoint. {}",
            "之前的约定。".repeat(32)
        );
        let assistant = format!(
            "Assistant suggestion, not a user source. {}",
            "建议。".repeat(32)
        );
        let partial = format!(
            "Partial body with unknown remainder. {}",
            "片段。".repeat(32)
        );
        let revision = "a".repeat(64);
        let row = |id: &str, role: &str, text: &str| {
            json!({
                "message_id":id,"revision":revision,"role":role,"created_at":1,
                "matched":true,"complete":true,"text":text
            })
        };
        let mut fragment = row("partial", "user", &partial);
        fragment.as_object_mut().unwrap().remove("complete");
        fragment["has_more"] = json!(true);
        let mut exact = row("visible", "user", &original);
        exact["attachments"] = json!([{"attachment_index":0,"name":"notes.txt","total_chars":2}]);
        let raw = json!({"historical_data":true,"sources":[
            row("older", "user", &older), exact,
            row("another", "user", &additional), row("assistant", "assistant", &assistant), fragment
        ]});
        let output = raw.to_string();
        let tool = "recall_conversation";
        let result = ToolResult::success(tool, &output);
        append_tool(db.clone(), "recall", tool, &output);
        let sources = ForegroundToolSources::new(db, "s".into(), Some("r".into()), true);
        let message = |role: &str, content: &str| Message {
            role: role.into(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            tool_images: vec![],
            protocol_state: None,
        };
        let mut checkpoint = message("assistant", "");
        checkpoint.protocol_state = Some(crate::llm::ProtocolContinuation {
            protocol: ModelProtocol::OpenaiResponses,
            model: "fixture".into(),
            credential_ref: "fixture".into(),
            output_items: vec![json!({"type":"compaction"})],
        });
        let history = vec![
            message("user", &older),
            checkpoint,
            message("user", &original),
            message("assistant", &additional),
            message("user", &assistant),
            message("user", &partial),
        ];
        let projected = sources
            .project("recall", tool, &result, &history)
            .unwrap()
            .unwrap();
        assert!(projected.len() < output.len());
        let mut compact: Value = serde_json::from_str(&projected).unwrap();
        assert_eq!(
            compact["sources"][1]["text_in_prior_user"],
            original.chars().take(24).collect::<String>()
        );
        assert!(compact["sources"][1].get("text").is_none());
        assert!(compact["sources"][1].get("complete").is_none());
        for field in [
            "message_id",
            "revision",
            "role",
            "created_at",
            "matched",
            "attachments",
        ] {
            assert_eq!(compact["sources"][1][field], raw["sources"][1][field]);
        }
        for index in [0, 2, 3, 4] {
            assert_eq!(compact["sources"][index], raw["sources"][index]);
        }
        assert!(sources
            .project("missing", tool, &result, &history)
            .unwrap()
            .is_none());
        assert!(sources
            .project("recall", tool, &result, &[message("user", &original)])
            .unwrap()
            .is_none());

        let (reference, sealed) = sources
            .current_source("recall", tool, &output)
            .unwrap()
            .unwrap();
        assert_eq!(sealed.output, output);
        let model_result = format!("Result: {projected}");
        assert!(!matches_sealed_model_output(
            &reference,
            &sealed,
            &model_result
        ));
        assert!(matches_sealed_model_output_with_history(
            &reference,
            &sealed,
            &model_result,
            &history
        ));
        let mut replay = history.clone();
        replay.push(message("user", &additional));
        assert!(matches_sealed_model_output_with_history(
            &reference,
            &sealed,
            &model_result,
            &replay
        ));
        replay[2].role = "assistant".into();
        assert!(!matches_sealed_model_output_with_history(
            &reference,
            &sealed,
            &model_result,
            &replay
        ));
        let mut ambiguous = history.clone();
        ambiguous.push(message(
            "user",
            "One visible user requirement. But different.",
        ));
        assert!(sources
            .project("recall", tool, &result, &ambiguous)
            .unwrap()
            .is_none());
        compact["sources"][1]["revision"] = json!("forged");
        assert!(!matches_sealed_model_output_with_history(
            &reference,
            &sealed,
            &format!("Result: {compact}"),
            &history
        ));
    }

    #[test]
    fn repeated_file_projection_keeps_fresh_sealed_read_and_requires_exact_prior_raw() {
        let db = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        seed(&db.lock().unwrap());
        let text = "same file contents 🪶\n".repeat(36);
        let raw = json!({"resolvedPath":"/project/file.txt","content":text,"offset":20,
            "total_chars":1000,"has_more":true,"next_offset":20+text.chars().count()});
        let output = raw.to_string();
        let tool = "read_file";
        let result = ToolResult::success(tool, &output);
        append_tool(db.clone(), "second-read", tool, &output);
        let sources = ForegroundToolSources::new(db, "s".into(), Some("r".into()), true);
        let request = Message {
            role: "assistant".into(),
            content: String::new(),
            tool_calls: Some(vec![crate::llm::ToolCall {
                id: "first-read".into(),
                name: tool.into(),
                arguments: json!({"path":"file.txt","offset":20,"limit":text.chars().count()}),
            }]),
            tool_call_id: None,
            tool_images: vec![],
            protocol_state: Some(crate::llm::ProtocolContinuation {
                protocol: ModelProtocol::OpenaiResponses,
                model: "fixture".into(),
                credential_ref: "fixture".into(),
                output_items: vec![],
            }),
        };
        let previous = Message {
            role: "tool".into(),
            content: format!("Result: {output}"),
            tool_calls: None,
            tool_call_id: Some("first-read".into()),
            tool_images: vec![],
            protocol_state: None,
        };
        let history = vec![request.clone(), previous.clone()];
        let project = |history: &[Message]| {
            sources
                .project("second-read", tool, &result, history)
                .unwrap()
        };
        let projected = project(&history).unwrap();
        assert!(projected.len() + 128 <= output.len());
        let compact: Value = serde_json::from_str(&projected).unwrap();
        let (reference, sealed) = sources
            .current_source("second-read", tool, &output)
            .unwrap()
            .unwrap();
        assert_eq!(sealed.output, output);
        let mut expected = raw.clone();
        expected.as_object_mut().unwrap().remove("content");
        expected["same_content_as_read_call"] = json!("first-read");
        expected["source_ref"] = json!(reference);
        assert_eq!(compact, expected);
        let reread: Value = serde_json::from_str(
            &sources
                .read(&json!({
                    "source_ref":reference,"path":"/content"
                }))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(reread["text"], text);
        let model_result = format!("Result: {projected}");
        assert!(!matches_sealed_model_output(
            &reference,
            &sealed,
            &model_result
        ));
        assert!(matches_sealed_model_output_with_history(
            &reference,
            &sealed,
            &model_result,
            &history
        ));

        let mut changed_range = raw.clone();
        changed_range["offset"] = json!(21);
        let mut changed_content = raw.clone();
        changed_content["content"] = json!("file changed");
        for changed in [changed_range, changed_content, compact.clone()] {
            let mut replay = history.clone();
            replay[1].content = format!("Result: {changed}");
            assert!(project(&replay).is_none());
            assert!(!matches_sealed_model_output_with_history(
                &reference,
                &sealed,
                &model_result,
                &replay
            ));
        }
        let mut ambiguous = history.clone();
        ambiguous.push(previous);
        assert!(project(&ambiguous).is_none());
        let mut wrong_tool = history.clone();
        wrong_tool[0].tool_calls.as_mut().unwrap()[0].name = "other_tool".into();
        assert!(project(&wrong_tool).is_none());
        let mut checkpoint = request;
        checkpoint.tool_calls = None;
        checkpoint.protocol_state.as_mut().unwrap().output_items =
            vec![json!({"type":"compaction"})];
        let mut truncated = history.clone();
        truncated.push(checkpoint);
        assert!(project(&truncated).is_none());
        assert!(!matches_sealed_model_output_with_history(
            &reference,
            &sealed,
            &model_result,
            &truncated
        ));

        let mut forged = compact;
        forged["source_ref"] = json!("forged");
        assert!(!matches_sealed_model_output_with_history(
            &reference,
            &sealed,
            &format!("Result: {forged}"),
            &history
        ));
    }

    #[test]
    fn command_projection_keeps_captured_tails_and_exact_source() {
        let db = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        seed(&db.lock().unwrap());
        let tool = "run_project_command";
        let stdout = format!("build-start\n{}\nbuild-complete", "编译🪶\n".repeat(900));
        let stderr = format!("{}\nwarning-capture-boundary", "\0\"\\".repeat(600));
        let output = json!({
            "code": "COMMAND_EXITED",
            "exit_code": 0,
            "message": "Command completed",
            "stdout": stdout,
            "stdout_truncated": false,
            "stderr": stderr,
            "stderr_truncated": true,
        })
        .to_string();
        append_tool(db.clone(), "cmd-1", tool, &output);
        let sources = ForegroundToolSources::new(db, "s".into(), Some("r".into()), false);
        let result = sources
            .project("cmd-1", tool, &ToolResult::success(tool, &output), &[])
            .unwrap()
            .unwrap();
        assert!(result.len() <= tool_context::MAX_VIEW_BYTES);
        assert!(result.len() + 256 < output.len());
        let view: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(view["values"]["exit_code"], 0);
        assert_eq!(view["values"]["stderr_truncated"], true);
        assert_eq!(view["complete"], false);
        for (path, original) in [("/stdout", stdout), ("/stderr", stderr)] {
            assert!(view["omitted_paths"]
                .as_array()
                .unwrap()
                .contains(&json!(path)));
            let preview = view["captured_tail_previews"][path].as_str().unwrap();
            assert_eq!(preview.chars().count(), 192);
            assert!(original.ends_with(preview));
            let reread: Value = serde_json::from_str(
                &sources
                    .read(&json!({"source_ref":view["source_ref"],"path":path,"tail":true,"limit":192}))
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(reread["text"], preview);
        }
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
            .project("window", "read_file", &result, &[])
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
                &[],
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
                    &ToolResult::success("inspect", &output),
                    &[],
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
            .project(
                "same",
                "inspect",
                &ToolResult::error("inspect", &output),
                &[]
            )
            .unwrap()
            .is_none());
        let mut result = ToolResult::success("inspect", &output);
        result.terminate_batch = true;
        assert!(current
            .project("same", "inspect", &result, &[])
            .unwrap()
            .is_none());
        assert!(current
            .project(
                "same",
                "wrong-tool",
                &ToolResult::success("wrong-tool", &output),
                &[],
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
            .project(
                "same",
                "inspect",
                &ToolResult::success("inspect", &output),
                &[]
            )
            .unwrap()
            .is_none());
        db.lock().unwrap().execute_batch("UPDATE sessions SET leaf_message_id=NULL WHERE id='s'; UPDATE messages SET parent_id=NULL WHERE id='r';").unwrap();
        assert!(
            current.read(&json!({})).is_err(),
            "unattached hidden replies are not an active branch"
        );
    }
}
