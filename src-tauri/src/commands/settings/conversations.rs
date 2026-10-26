//! Portable conversation data, never a scheduler checkpoint or directory grant.
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Serialize, Deserialize)]
pub(super) struct Snapshot {
    projects: Vec<Value>,
    sessions: Vec<Value>,
    messages: Vec<Value>,
    context_summaries: Vec<Value>,
    session_branches: Vec<Value>,
    task_runs: Vec<Value>,
    task_run_facts: Vec<Value>,
    agent_steps: Vec<Value>,
    agent_run_events: Vec<Value>,
}

pub(super) fn snapshot(conn: &Connection) -> Result<Snapshot, String> {
    let read = |table| super::query_table(conn, &format!("SELECT * FROM {table} ORDER BY rowid"));
    let data = Snapshot {
        projects: read("projects")?,
        sessions: read("sessions")?,
        messages: read("messages")?,
        context_summaries: read("context_summaries")?,
        session_branches: read("session_branches")?,
        task_runs: read("task_runs")?,
        task_run_facts: read("task_run_facts")?,
        agent_steps: read("agent_steps")?,
        agent_run_events: read("agent_run_events")?,
    };
    validate(&data)?;
    Ok(data)
}

fn text<'a>(row: &'a Value, key: &str) -> Result<&'a str, String> {
    row.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "备份关联记录不完整，未提交恢复。".to_owned())
}

fn identities(rows: &[Value]) -> Result<HashMap<&str, &Value>, String> {
    let mut result = HashMap::new();
    for row in rows {
        if result.insert(text(row, "id")?, row).is_some() {
            return Err("备份中存在重复记录标识，未提交恢复。".to_owned());
        }
    }
    Ok(result)
}

// Validate graph identity before enabling deferred FK checking for the circular
// project/main and session/leaf pointers. Deferral does not excuse missing data.
fn validate(data: &Snapshot) -> Result<(), String> {
    let projects = identities(&data.projects)?;
    let sessions = identities(&data.sessions)?;
    let messages = identities(&data.messages)?;
    for (id, project) in &projects {
        match text(project, "kind")? {
            "personal" if *id == "personal" && project["path"] == "" => {}
            "project"
                if project["path"]
                    .as_str()
                    .is_some_and(|p| !p.trim().is_empty()) => {}
            _ => return Err("备份工作区类型或目录信息无效。".to_owned()),
        }
        let main = text(project, "active_session_id")?;
        if !sessions.get(main).is_some_and(|s| s["project_id"] == *id) {
            return Err("备份项目的主会话关联无效。".to_owned());
        }
    }
    for session in &data.sessions {
        if let Some(project) = session["project_id"].as_str() {
            if !projects.contains_key(project) {
                return Err("备份会话缺少所属项目。".to_owned());
            }
        }
        if let Some(leaf) = session["leaf_message_id"].as_str() {
            if !messages
                .get(leaf)
                .is_some_and(|m| m["session_id"] == session["id"])
            {
                return Err("备份会话的分支位置无效。".to_owned());
            }
        }
    }
    for message in &data.messages {
        if !sessions.contains_key(text(message, "session_id")?) {
            return Err("备份消息缺少所属会话。".to_owned());
        }
    }
    for (rows, owner) in [(&sessions, "project_id"), (&messages, "session_id")] {
        validate_parents(rows, owner, "parent_id")?;
    }
    let mut branches = data.session_branches.clone();
    for branch in &mut branches {
        let session = sessions
            .get(text(branch, "session_id")?)
            .ok_or("备份分支缺少所属会话。")?;
        branch["project_id"] = session["project_id"].clone();
    }
    validate_parents(&identities(&branches)?, "project_id", "parent_branch_id")?;
    for run in &data.task_runs {
        if !messages
            .get(text(run, "message_id")?)
            .is_some_and(|m| m["session_id"] == run["session_id"])
        {
            return Err("备份任务与消息的关联无效。".to_owned());
        }
    }
    Ok(())
}

fn validate_parents(
    rows: &HashMap<&str, &Value>,
    owner: &str,
    parent_key: &str,
) -> Result<(), String> {
    let mut done = HashSet::new();
    for id in rows.keys() {
        let mut path = HashSet::new();
        let mut cursor = Some(*id);
        while let Some(current) = cursor {
            if done.contains(current) {
                break;
            }
            if !path.insert(current) {
                return Err("备份会话或消息分支存在循环。".to_owned());
            }
            let row = rows.get(current).ok_or("备份分支缺少父记录。")?;
            cursor = row[parent_key].as_str();
            if let Some(parent) = cursor {
                if !rows.get(parent).is_some_and(|p| p[owner] == row[owner]) {
                    return Err("备份分支引用了其他工作区或会话。".to_owned());
                }
            }
        }
        done.extend(path);
    }
    Ok(())
}

/// v1 omitted workspace and tree pointers. Retain its records and path hints,
/// but never interpret a legacy work_dir as a new local authorization.
pub(super) fn restore_legacy(conn: &Connection, import: &Value) -> Result<Value, String> {
    crate::commands::workspace::ensure_personal_workspace(conn)?;
    let legacy_root = import["settings"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["key"] == "work_directory"))
        .and_then(|row| row["value"].as_str())
        .filter(|path| !path.trim().is_empty());
    let mut sessions = import["sessions"].as_array().cloned().unwrap_or_default();
    let mut touched = Vec::new();
    for session in &mut sessions {
        let id = text(session, "id")?.to_owned();
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
                [&id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if exists {
            continue;
        }
        if session["project_id"].is_null() {
            let path = session["work_dir"]
                .as_str()
                .filter(|p| !p.trim().is_empty())
                .or(legacy_root);
            let project = if let Some(path) = path {
                if let Some(id) = conn
                    .query_row(
                        "SELECT id FROM projects WHERE kind='project' AND path=?1",
                        [path],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?
                {
                    id
                } else {
                    let id = uuid::Uuid::new_v4().to_string();
                    let name = std::path::Path::new(path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("恢复的项目");
                    conn.execute("INSERT INTO projects(id,name,path,created_at,kind,updated_at,directory_confirmed) VALUES(?1,?2,?3,0,'project',0,0)",params![id,name,path]).map_err(|e|e.to_string())?;
                    id
                }
            } else {
                "personal".to_owned()
            };
            session["project_id"] = json!(project);
        }
        session["work_dir"] = Value::Null;
        session["additional_read_dirs"] = json!("[]");
        touched.push(id);
    }
    let mut counts = json!({"sessions":super::batch_import(conn,"sessions",&sessions)?});
    let mut messages = import["messages"].as_array().cloned().unwrap_or_default();
    // v1 has no tree pointers. Preserve its array order for messages written in
    // the same second; random UUID order cannot reconstruct the conversation.
    messages.sort_by_key(|message| message["created_at"].as_i64().unwrap_or(0));
    let mut previous = HashMap::new();
    for message in &mut messages {
        let session = text(message, "session_id")?.to_owned();
        let id = text(message, "id")?.to_owned();
        if message.get("parent_id").is_none() {
            message["parent_id"] = previous
                .get(&session)
                .map(|id: &String| json!(id))
                .unwrap_or(Value::Null);
        }
        previous.insert(session, id);
    }
    counts["messages"] = json!(super::batch_import(conn, "messages", &messages)?);
    if let Some(rows) = import["context_summaries"].as_array() {
        counts["context_summaries"] = json!(super::batch_import(conn, "context_summaries", rows)?);
    }
    for (session, leaf) in previous {
        conn.execute(
            "UPDATE sessions SET leaf_message_id=?1 WHERE id=?2 AND leaf_message_id IS NULL",
            params![leaf, session],
        )
        .map_err(|e| e.to_string())?;
    }
    for session in touched {
        conn.execute("UPDATE projects SET active_session_id=?1 WHERE id=(SELECT project_id FROM sessions WHERE id=?1) AND (active_session_id IS NULL OR NOT EXISTS(SELECT 1 FROM messages WHERE session_id=active_session_id))",[session]).map_err(|e|e.to_string())?;
    }
    let orphaned = super::query_table(
        conn,
        "SELECT id FROM projects WHERE active_session_id IS NULL",
    )?;
    for project in orphaned {
        let project = text(&project, "id")?;
        let id = if project == "personal" {
            "personal-main".to_owned()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        conn.execute("INSERT OR IGNORE INTO sessions(id,title,created_at,updated_at,project_id) VALUES(?1,'',0,0,?2)",params![id,project]).map_err(|e|e.to_string())?;
        conn.execute(
            "UPDATE projects SET active_session_id=?1 WHERE id=?2",
            params![id, project],
        )
        .map_err(|e| e.to_string())?;
    }
    // v1 clients may supply tree fields too: validate narrow graph metadata,
    // not another full copy of the user's message bodies and receipts.
    for (table, owner) in [("sessions", "project_id"), ("messages", "session_id")] {
        let rows = super::query_table(conn, &format!("SELECT id,parent_id,{owner} FROM {table}"))?;
        validate_parents(&identities(&rows)?, owner, "parent_id")?;
    }
    Ok(counts)
}

/// Runs inside the ordinary import transaction. Existing populated Main paths
/// win in merge mode; a fresh installation's empty Main may adopt its old leaf.
pub(super) fn restore(conn: &Connection, import: &Value) -> Result<Value, String> {
    let mut data: Snapshot = serde_json::from_value(import.clone())
        .map_err(|_| "备份缺少完整项目、会话或任务记录。".to_owned())?;
    validate(&data)?;
    conn.execute_batch("PRAGMA defer_foreign_keys=ON;")
        .map_err(|e| e.to_string())?;
    let mut counts = json!({});
    let mut empty_projects = HashSet::new();
    for project in &mut data.projects {
        let id = text(project, "id")?;
        let stored: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT kind,active_session_id FROM projects WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((kind, main)) = stored {
            if project["kind"] != kind {
                return Err("现有工作区与备份类型冲突。".to_owned());
            }
            if !conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE session_id=?1)",
                    [main],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(|e| e.to_string())?
            {
                empty_projects.insert(id.to_owned());
            }
        }
        // Never carry a source machine's authority into the receiving machine.
        project["directory_confirmed"] = json!(project["kind"] == "personal");
    }
    counts["projects"] = json!(super::batch_import(conn, "projects", &data.projects)?);
    let mut empty_sessions = HashSet::new();
    for session in &mut data.sessions {
        let id = text(session, "id")?.to_owned();
        let stored: Option<Option<String>> = conn
            .query_row("SELECT project_id FROM sessions WHERE id=?1", [&id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(project) = stored {
            if project.as_deref() != session["project_id"].as_str() {
                return Err("现有会话与备份的项目归属冲突，未提交恢复。".to_owned());
            }
            if !conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE session_id=?1)",
                    [&id],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(|e| e.to_string())?
            {
                empty_sessions.insert(id);
            }
        }
        // A legacy standalone session must not inherit an imported global root.
        // It remains readable, but is attached to the Personal workspace.
        if session["project_id"].is_null() {
            session["project_id"] = json!("personal");
        }
        session["work_dir"] = Value::Null;
        session["additional_read_dirs"] = json!("[]");
    }
    counts["sessions"] = json!(super::batch_import(conn, "sessions", &data.sessions)?);
    for message in &mut data.messages {
        if message["is_provisional"] == 1 {
            message["is_provisional"] = json!(0);
            message["content"] = json!("备份时此任务尚未完成。恢复仅保留历史，不会继续执行或重试；请先核对实际结果，再发起新请求。");
        }
        let id = text(message, "id")?;
        let existing =
            super::query_table_with_params(conn, "SELECT * FROM messages WHERE id=?1", &[&id])?;
        if let Some(existing) = existing.first() {
            if existing != message {
                return Err("备份与现有消息内容冲突，原有历史未改变。".to_owned());
            }
        }
    }
    counts["messages"] = json!(super::batch_import(conn, "messages", &data.messages)?);
    for session in &data.sessions {
        if empty_sessions.contains(text(session, "id")?) {
            conn.execute(
                "UPDATE sessions SET title=?1,context_version=?2,last_compressed_at=?3,metadata=?4,leaf_message_id=?5,agent_provider=?6,agent_model=?7 WHERE id=?8",
                params![session["title"].as_str(), session["context_version"].as_i64().unwrap_or(0),session["last_compressed_at"].as_i64(),session["metadata"].as_str(),session["leaf_message_id"].as_str(),session["agent_provider"].as_str(),session["agent_model"].as_str(),text(session,"id")?],
            ).map_err(|e| e.to_string())?;
        }
    }
    for project in &data.projects {
        if empty_projects.contains(text(project, "id")?) {
            conn.execute(
                "UPDATE projects SET active_session_id=?1 WHERE id=?2",
                params![text(project, "active_session_id")?, text(project, "id")?],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    let mut terminal_run_ids = HashSet::new();
    for run in &mut data.task_runs {
        let id = text(run, "id")?.to_owned();
        let terminal = matches!(
            text(run, "status")?,
            "completed" | "failed" | "stopped" | "cancelled"
        );
        // These are receipts, not resumable execution handles. Pending approvals,
        // queue claims and external worktrees do not migrate with this snapshot.
        run["resumable"] = json!(0);
        if terminal {
            terminal_run_ids.insert(id);
        } else {
            run["status"] = json!("needs_attention");
            run["confirmation_state"] = json!("none");
        }
    }
    for facts in &mut data.task_run_facts {
        let run_id = text(facts, "task_run_id")?;
        let mut value: crate::agent::task_facts::TaskFacts =
            serde_json::from_str(text(facts, "facts_json")?)
                .map_err(|_| "备份任务事实格式无效，未提交恢复。".to_owned())?;
        value.pending_confirmation = None;
        // Older successful receipts can omit this optional fact. Preserve that
        // history only when its original parent run already had a terminal state.
        let terminal_default = value.terminal_reason.is_none() && terminal_run_ids.contains(run_id);
        if !matches!(
            value.terminal_reason,
            Some(
                crate::agent::task_facts::TaskTerminalReason::Completed
                    | crate::agent::task_facts::TaskTerminalReason::Stopped
            )
        ) && !terminal_default
        {
            value.terminal_reason =
                Some(crate::agent::task_facts::TaskTerminalReason::NeedsAttention);
        }
        facts["facts_json"] = json!(serde_json::to_string(&value).map_err(|e| e.to_string())?);
    }
    for (table, rows) in [
        ("context_summaries", &data.context_summaries),
        ("session_branches", &data.session_branches),
        ("task_runs", &data.task_runs),
        ("task_run_facts", &data.task_run_facts),
        ("agent_steps", &data.agent_steps),
        ("agent_run_events", &data.agent_run_events),
    ] {
        counts[table] = json!(super::batch_import(conn, table, rows)?);
    }
    crate::commands::workspace::ensure_personal_workspace(conn)?;
    Ok(counts)
}
