//! Session-bound automation tool exposed only to the user-facing Main Agent.
//!
//! The workspace is resolved from the durable session. It is deliberately not
//! accepted from model arguments, preventing a tool call from crossing the
//! current workspace boundary.

use crate::agent::{
    shared_db::SharedDb,
    supervision::WorkspaceSupervisor,
    tool::{Tool, ToolCategory, ToolHandler, ToolResult},
    ToolRegistry,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

const DEFAULT_AGENT_AUTOMATION_TIMEOUT_SECONDS: u64 = 300;

struct CreateAutomationHandler {
    session_id: String,
    db: Arc<Mutex<Connection>>,
}

struct ScheduleReminderHandler {
    session_id: String,
    db: Arc<Mutex<Connection>>,
}

struct ListRemindersHandler {
    session_id: String,
    db: Arc<Mutex<Connection>>,
}

struct CancelReminderHandler {
    session_id: String,
    db: Arc<Mutex<Connection>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReminderSummary {
    id: String,
    title: String,
    body: String,
    when: String,
    next_run_at: i64,
}

fn workspace_id_for_session(
    db: &Arc<Mutex<Connection>>,
    session_id: &str,
) -> Result<String, String> {
    WorkspaceSupervisor::new(SharedDb::from_arc(db.clone()))
        .workspace_for_session(session_id)
        .map_err(|error| format!("无法确定当前工作区: {error}"))
}

impl ToolHandler for CreateAutomationHandler {
    fn name(&self) -> &str {
        "create_automation"
    }

    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            title: String,
            prompt: String,
            schedule: String,
        }

        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::error(self.name(), format!("自动化参数解析失败: {error}"))
            }
        };
        let workspace_id = match workspace_id_for_session(&self.db, &self.session_id) {
            Ok(workspace_id) => workspace_id,
            Err(error) => return ToolResult::error(self.name(), error),
        };
        match super::automation::create_agent_automation_definition(
            self.db.clone(),
            args.title,
            args.prompt,
            "schedule".to_string(),
            args.schedule,
            "按当前 Agent 操作权限执行".to_string(),
            workspace_id,
            DEFAULT_AGENT_AUTOMATION_TIMEOUT_SECONDS,
        ) {
            Ok(automation) => ToolResult::success(
                self.name(),
                format!(
                    "已创建自动化「{}」，下次运行时间为 {}。",
                    automation.title,
                    automation
                        .next_run_at
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "未安排".to_string())
                ),
            ),
            Err(error) => ToolResult::error(self.name(), error),
        }
    }
}

impl ToolHandler for ScheduleReminderHandler {
    fn name(&self) -> &str {
        "schedule_reminder"
    }

    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            title: String,
            body: String,
            when: String,
        }

        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::error(self.name(), format!("提醒参数解析失败: {error}"))
            }
        };
        let workspace_id = match workspace_id_for_session(&self.db, &self.session_id) {
            Ok(workspace_id) => workspace_id,
            Err(error) => return ToolResult::error(self.name(), error),
        };
        match super::automation::create_notification_reminder_definition(
            self.db.clone(),
            args.title,
            args.body,
            args.when,
            workspace_id,
        ) {
            Ok(reminder) => ToolResult::success(
                self.name(),
                format!(
                    "已创建一次性提醒「{}」，将在 {} 触发。",
                    reminder.title, reminder.trigger_value
                ),
            ),
            Err(error) => ToolResult::error(self.name(), error),
        }
    }
}

impl ToolHandler for ListRemindersHandler {
    fn name(&self) -> &str {
        "list_reminders"
    }

    fn execute(&self, _arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        let workspace_id = match workspace_id_for_session(&self.db, &self.session_id) {
            Ok(workspace_id) => workspace_id,
            Err(error) => return ToolResult::error(self.name(), error),
        };
        let reminders = {
            let conn = match self.db.lock() {
                Ok(conn) => conn,
                Err(error) => return ToolResult::error(self.name(), error.to_string()),
            };
            let mut statement = match conn.prepare(
                "SELECT id,title,prompt,trigger_value,next_run_at
                 FROM automations
                 WHERE workspace_id=?1
                   AND executor_kind='notification'
                   AND trigger_kind='once'
                   AND enabled=1
                   AND next_run_at IS NOT NULL
                 ORDER BY next_run_at ASC
                 LIMIT 20",
            ) {
                Ok(statement) => statement,
                Err(error) => return ToolResult::error(self.name(), error.to_string()),
            };
            let rows = match statement.query_map(params![workspace_id], |row| {
                Ok(ReminderSummary {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    body: row.get(2)?,
                    when: row.get(3)?,
                    next_run_at: row.get(4)?,
                })
            }) {
                Ok(rows) => rows,
                Err(error) => return ToolResult::error(self.name(), error.to_string()),
            };
            match rows.collect::<Result<Vec<_>, _>>() {
                Ok(reminders) => reminders,
                Err(error) => return ToolResult::error(self.name(), error.to_string()),
            }
        };
        ToolResult::success(
            self.name(),
            serde_json::json!({ "reminders": reminders }).to_string(),
        )
    }
}

impl ToolHandler for CancelReminderHandler {
    fn name(&self) -> &str {
        "cancel_reminder"
    }

    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            reminder_id: String,
        }

        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::error(self.name(), format!("取消提醒参数解析失败: {error}"))
            }
        };
        let workspace_id = match workspace_id_for_session(&self.db, &self.session_id) {
            Ok(workspace_id) => workspace_id,
            Err(error) => return ToolResult::error(self.name(), error),
        };
        let now = chrono::Utc::now().timestamp();
        let conn = match self.db.lock() {
            Ok(conn) => conn,
            Err(error) => return ToolResult::error(self.name(), error.to_string()),
        };
        let title = match conn
            .query_row(
                "SELECT title FROM automations
                 WHERE id=?1 AND workspace_id=?2
                   AND executor_kind='notification'
                   AND trigger_kind='once' AND enabled=1",
                params![args.reminder_id, workspace_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
        {
            Ok(Some(title)) => title,
            Ok(None) => {
                return ToolResult::error(
                    self.name(),
                    "当前工作区中没有找到这个待提醒事项；请先查询提醒列表。",
                )
            }
            Err(error) => return ToolResult::error(self.name(), error.to_string()),
        };
        match conn.execute(
            "UPDATE automations
             SET enabled=0,next_run_at=NULL,schedule_claim_token=NULL,
                 schedule_claimed_at=NULL,updated_at=?3
             WHERE id=?1 AND workspace_id=?2
               AND executor_kind='notification'
               AND trigger_kind='once' AND enabled=1",
            params![args.reminder_id, workspace_id, now],
        ) {
            Ok(1) => ToolResult::success(self.name(), format!("已取消提醒「{title}」。")),
            Ok(_) => ToolResult::error(self.name(), "提醒状态已发生变化，请重新查询。"),
            Err(error) => ToolResult::error(self.name(), error.to_string()),
        }
    }
}

pub(crate) fn register(
    registry: &mut ToolRegistry,
    session_id: String,
    db: Arc<Mutex<Connection>>,
) {
    registry.register(
        Tool::new(
            "list_reminders",
            "列出当前工作区尚未触发的一次性提醒。用户询问已有提醒、想取消或核对提醒时先调用；结果只包含当前工作区，不能查看其他工作区。",
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {}
            }),
        )
        .with_category(ToolCategory::Extension)
        .with_prompt_snippet("查询或取消提醒前，先用 list_reminders 获取当前工作区中的准确提醒 ID。"),
        Arc::new(ListRemindersHandler {
            session_id: session_id.clone(),
            db: db.clone(),
        }),
    );
    registry.register(
        Tool::new(
            "cancel_reminder",
            "取消当前工作区中一个尚未触发的一次性提醒。只能使用 list_reminders 返回的准确 reminder_id；如果用户指代不明确，先查询并澄清。",
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "reminder_id": { "type": "string", "minLength": 1, "description": "list_reminders 返回的提醒 ID" }
                },
                "required": ["reminder_id"]
            }),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true)
        .with_prompt_snippet("取消提醒是副作用：先准确定位提醒，再用 cancel_reminder 并遵守确认策略。"),
        Arc::new(CancelReminderHandler {
            session_id: session_id.clone(),
            db: db.clone(),
        }),
    );
    registry.register(
        Tool::new(
            "schedule_reminder",
            "创建一次性本地提醒。适用于“明天提醒我”“稍后叫我”等明确的单次提醒；when 必须根据系统提示中的当前时间换算成带时区的 RFC 3339 时间。时间含义不明确时先询问用户，不要猜测。",
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "title": { "type": "string", "minLength": 1, "maxLength": 120, "description": "通知标题" },
                    "body": { "type": "string", "minLength": 1, "maxLength": 1000, "description": "到点显示的提醒内容" },
                    "when": { "type": "string", "format": "date-time", "description": "未来的 RFC 3339 时间，必须包含时区偏移，例如 2026-09-19T09:00:00+08:00" }
                },
                "required": ["title", "body", "when"]
            }),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true)
        .with_prompt_snippet(
            "单次提醒使用 schedule_reminder；重复任务才使用 create_automation。",
        ),
        Arc::new(ScheduleReminderHandler {
            session_id: session_id.clone(),
            db: db.clone(),
        }),
    );
    registry.register(
        Tool::new(
            "create_automation",
            "在当前工作区创建每天定时运行的 Agent 自动化。适用于需要重复处理、整理或检查的任务。",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "简短、可识别的自动化名称" },
                    "prompt": { "type": "string", "description": "每次触发时交给主 Agent 的完整任务说明" },
                    "schedule": { "type": "string", "description": "每天的运行时间，格式如“每天 09:00”" }
                },
                "required": ["title", "prompt", "schedule"]
            }),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true)
        .with_prompt_snippet(
            "用户要求重复执行任务时，用 create_automation 创建绑定当前工作区的定时自动化。",
        ),
        Arc::new(CreateAutomationHandler { session_id, db }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrate;

    #[test]
    fn tool_persists_an_agent_automation_in_the_current_workspace() {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        let (workspace_id, session_id): (String, String) = connection
            .query_row(
                "SELECT id, active_session_id FROM projects WHERE kind = 'personal'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let db = Arc::new(Mutex::new(connection));
        let handler = CreateAutomationHandler {
            session_id,
            db: db.clone(),
        };

        let result = handler.execute(
            &serde_json::json!({
                "title": "整理收件箱",
                "prompt": "检查待处理邮件并生成行动清单，不要发送邮件。",
                "schedule": "每天 09:00"
            }),
            Path::new("."),
        );

        assert!(result.success, "{}", result.content);
        let stored: (String, String, String) = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT title, executor_kind, workspace_id FROM automations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(stored, ("整理收件箱".into(), "agent".into(), workspace_id));
    }

    #[test]
    fn reminder_tool_persists_a_one_time_local_notification() {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        let (workspace_id, session_id): (String, String) = connection
            .query_row(
                "SELECT id, active_session_id FROM projects WHERE kind = 'personal'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let db = Arc::new(Mutex::new(connection));
        let handler = ScheduleReminderHandler {
            session_id,
            db: db.clone(),
        };
        let when = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();

        let result = handler.execute(
            &serde_json::json!({
                "title": "查看会议资料",
                "body": "记得查看明天会议的材料。",
                "when": when
            }),
            Path::new("."),
        );

        assert!(result.success, "{}", result.content);
        let stored: (String, String, String, String) = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT title, trigger_kind, executor_kind, workspace_id FROM automations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            stored,
            (
                "查看会议资料".into(),
                "once".into(),
                "notification".into(),
                workspace_id
            )
        );
    }

    #[test]
    fn reminder_list_is_read_only_and_scoped_to_the_current_workspace() {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        let (workspace_id, session_id): (String, String) = connection
            .query_row(
                "SELECT id, active_session_id FROM projects WHERE kind = 'personal'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let db = Arc::new(Mutex::new(connection));
        let when = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        db.lock()
            .unwrap()
            .execute(
                "INSERT INTO projects (id,name,path,created_at,kind,updated_at,active_session_id)
                 VALUES ('other-workspace','Other','',1,'project',1,NULL)",
                [],
            )
            .unwrap();
        let visible = super::super::automation::create_notification_reminder_definition(
            db.clone(),
            "可见提醒".into(),
            "属于当前空间".into(),
            when.clone(),
            workspace_id,
        )
        .unwrap();
        let hidden = super::super::automation::create_notification_reminder_definition(
            db.clone(),
            "其他空间提醒".into(),
            "不能泄漏".into(),
            when,
            "other-workspace".into(),
        )
        .unwrap();
        let handler = ListRemindersHandler { session_id, db };

        let result = handler.execute(&serde_json::json!({}), Path::new("."));

        assert!(result.success, "{}", result.content);
        assert!(result.content.contains(&visible.id));
        assert!(result.content.contains("可见提醒"));
        assert!(!result.content.contains(&hidden.id));
        assert!(!result.content.contains("其他空间提醒"));
    }

    #[test]
    fn cancel_reminder_preserves_evidence_and_refuses_cross_workspace_access() {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        let (workspace_id, session_id): (String, String) = connection
            .query_row(
                "SELECT id, active_session_id FROM projects WHERE kind = 'personal'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let db = Arc::new(Mutex::new(connection));
        let when = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        db.lock()
            .unwrap()
            .execute(
                "INSERT INTO projects (id,name,path,created_at,kind,updated_at,active_session_id)
                 VALUES ('other-workspace','Other','',1,'project',1,NULL)",
                [],
            )
            .unwrap();
        let reminder = super::super::automation::create_notification_reminder_definition(
            db.clone(),
            "待取消提醒".into(),
            "保留记录".into(),
            when.clone(),
            workspace_id,
        )
        .unwrap();
        let foreign = super::super::automation::create_notification_reminder_definition(
            db.clone(),
            "其他空间提醒".into(),
            "不能取消".into(),
            when,
            "other-workspace".into(),
        )
        .unwrap();
        let handler = CancelReminderHandler {
            session_id,
            db: db.clone(),
        };

        let refused = handler.execute(
            &serde_json::json!({ "reminder_id": foreign.id }),
            Path::new("."),
        );
        assert!(!refused.success);

        let result = handler.execute(
            &serde_json::json!({ "reminder_id": reminder.id }),
            Path::new("."),
        );
        assert!(result.success, "{}", result.content);
        let stored: (i64, Option<i64>, i64) = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT enabled,next_run_at,COUNT(*) FROM automations WHERE id=?1",
                params![reminder.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(stored, (0, None, 1));
    }
}
