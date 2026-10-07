//! Session-bound, Main-Agent-only task-understanding updates.
//!
//! The model may propose an assumption or an open question through this tool.
//! Program code fixes its provenance and scope, resolves the active Workspace,
//! and rejects every attempt to manufacture a user decision or environment fact.

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent::task_understanding::{
    NewTaskUnderstandingRecord, TaskUnderstanding, UnderstandingKind, UnderstandingScope,
    UnderstandingSource, UnderstandingStatus,
};
use crate::agent::tool::{Tool, ToolCategory, ToolHandler, ToolResult};
use crate::agent::ToolRegistry;

#[derive(Clone)]
struct ForegroundTaskUnderstandingControl {
    session_id: String,
    db: Arc<Mutex<Connection>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordTaskUnderstandingRequest {
    kind: String,
    body: String,
    #[serde(default)]
    affects: Vec<String>,
    #[serde(default)]
    replaces_id: Option<String>,
    #[serde(default)]
    defer: bool,
}

pub(crate) fn register(
    registry: &mut ToolRegistry,
    session_id: String,
    db: Arc<Mutex<Connection>>,
) {
    registry.register(
        Tool::new(
            "record_task_understanding",
            "Record one bounded Main-Agent assumption or unresolved question for the current task. Use only for information that changes the next action. Never record a user decision, confirmed fact, permission, or tool result through this tool.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["kind", "body"],
                "properties": {
                    "kind": {"type": "string", "enum": ["assumption", "open_question"]},
                    "body": {"type": "string", "minLength": 1, "maxLength": 600},
                    "affects": {"type": "array", "maxItems": 8, "items": {"type": "string", "minLength": 1, "maxLength": 160}},
                    "replaces_id": {"type": "string", "minLength": 1, "maxLength": 128},
                    "defer": {"type": "boolean"}
                }
            }),
        )
        .with_category(ToolCategory::Extension)
        .with_label("更新任务理解"),
        Arc::new(ForegroundTaskUnderstandingControl { session_id, db }),
    );
}

impl ToolHandler for ForegroundTaskUnderstandingControl {
    fn name(&self) -> &str {
        "record_task_understanding"
    }

    fn execute(&self, arguments: &Value, _work_dir: &Path) -> ToolResult {
        let request: RecordTaskUnderstandingRequest =
            match serde_json::from_value(arguments.clone()) {
                Ok(request) => request,
                Err(error) => {
                    return ToolResult::error(
                        self.name(),
                        format!("invalid task understanding update: {error}"),
                    )
                }
            };
        let kind = match request.kind.as_str() {
            "assumption" => UnderstandingKind::Assumption,
            "open_question" => UnderstandingKind::OpenQuestion,
            _ => {
                return ToolResult::error(
                    self.name(),
                    "only assumption and open_question are model-writable",
                )
            }
        };
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(error) => {
                return ToolResult::error(
                    self.name(),
                    format!("task understanding store unavailable: {error}"),
                )
            }
        };
        let workspace: Option<(String, String)> = match db
            .query_row(
                "SELECT id, kind FROM projects WHERE active_session_id=?1",
                [&self.session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
        {
            Ok(workspace) => workspace,
            Err(error) => {
                return ToolResult::error(
                    self.name(),
                    format!("could not resolve current workspace: {error}"),
                )
            }
        };
        let Some((workspace_id, workspace_kind)) = workspace else {
            return ToolResult::error(
                self.name(),
                "current session is not the active conversation of a workspace",
            );
        };
        if workspace_kind != "project" {
            return ToolResult::error(
                self.name(),
                "task understanding is only available in a project workspace",
            );
        }
        let update = NewTaskUnderstandingRecord {
            kind,
            source: UnderstandingSource::Agent,
            scope: UnderstandingScope::Task,
            status: if request.defer {
                UnderstandingStatus::Deferred
            } else {
                UnderstandingStatus::Active
            },
            body: request.body,
            affects: request.affects,
            replaces_id: request.replaces_id,
        };
        match TaskUnderstanding::apply_update(
            &db,
            &workspace_id,
            &self.session_id,
            update,
            Utc::now().timestamp(),
        ) {
            Ok(record) => ToolResult::success(
                self.name(),
                json!({
                    "id": record.id,
                    "kind": record.kind,
                    "status": record.status,
                    "scope": record.scope,
                })
                .to_string(),
            ),
            Err(error) => ToolResult::error(self.name(), error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handler() -> ForegroundTaskUnderstandingControl {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE projects (id TEXT PRIMARY KEY, kind TEXT NOT NULL, active_session_id TEXT NOT NULL);
             CREATE TABLE sessions (id TEXT PRIMARY KEY);
             INSERT INTO projects VALUES ('workspace', 'project', 'session');
             INSERT INTO sessions VALUES ('session');",
        ).unwrap();
        db.execute_batch(include_str!("../migrations/069_task_understanding.sql"))
            .unwrap();
        ForegroundTaskUnderstandingControl {
            session_id: "session".to_owned(),
            db: Arc::new(Mutex::new(db)),
        }
    }

    #[test]
    fn model_update_is_bound_to_active_workspace_and_agent_provenance() {
        let handler = handler();
        let result = handler.execute(
            &json!({"kind":"assumption", "body":"use a reversible local draft"}),
            Path::new("."),
        );
        assert!(result.success);
        let db = handler.db.lock().unwrap();
        let projection = TaskUnderstanding::load_projection(&db, "workspace", "session").unwrap();
        assert_eq!(projection.records.len(), 1);
        assert_eq!(projection.records[0].source, UnderstandingSource::Agent);
        assert_eq!(projection.records[0].scope, UnderstandingScope::Task);
    }

    #[test]
    fn model_cannot_claim_a_confirmed_environment_fact() {
        let handler = handler();
        let result = handler.execute(
            &json!({"kind":"environment_fact", "body":"network is available"}),
            Path::new("."),
        );
        assert!(!result.success);
    }
}
