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

struct ListAutomationsHandler {
    session_id: String,
    db: Arc<Mutex<Connection>>,
}

struct PauseAutomationHandler {
    session_id: String,
    db: Arc<Mutex<Connection>>,
}

struct StopAutomationRunHandler {
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
        #[serde(deny_unknown_fields)]
        struct Args {
            title: String,
            #[serde(default)]
            prompt: String,
            schedule: String,
            #[serde(rename = "executorKind", default)]
            executor_kind: Option<String>,
            #[serde(rename = "appId", default)]
            app_id: String,
            #[serde(rename = "launchEmulator", default)]
            launch_emulator: bool,
            #[serde(rename = "timeoutSeconds", default)]
            timeout_seconds: Option<u64>,
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
        let definition = match args.executor_kind.as_deref().unwrap_or("agent") {
            "agent" => super::automation::create_agent_automation_definition(
                self.db.clone(),
                args.title,
                args.prompt,
                "schedule".to_string(),
                args.schedule,
                "按当前 Agent 操作权限执行".to_string(),
                workspace_id,
                DEFAULT_AGENT_AUTOMATION_TIMEOUT_SECONDS,
            ),
            "maa" => super::automation::create_maa_automation_definition(
                self.db.clone(),
                args.title,
                args.schedule,
                workspace_id,
                args.app_id,
                args.launch_emulator,
                args.timeout_seconds,
            ),
            _ => Err(
                "仅支持 agent 或 maa 自动化；MAA 只能选择已可信并授予打开权限的应用".to_string(),
            ),
        };
        match definition {
            Ok(automation) => {
                let maa_config = if automation.executor_kind == "maa" {
                    self.db
                        .lock()
                        .map_err(|error| error.to_string())
                        .and_then(|conn| {
                            super::automation::maa_config_for_automation(&conn, &automation.id)
                        })
                } else {
                    Ok(None)
                };
                // Creation is already committed; readback failure must not invite a retry.
                let (maa_config, warning) = match maa_config {
                    Ok(value) => (value, None),
                    Err(_) => (None, Some("暂时无法回读已保存配置；请先用 list_automations 查询核对，不要重复创建。")),
                };
                let mut reply = serde_json::json!({
                    "id": automation.id, "title": automation.title,
                    "workspaceId": automation.workspace_id, "executorKind": automation.executor_kind,
                    "enabled": automation.enabled, "nextRunAt": automation.next_run_at,
                    "summary": "已在当前工作区创建每日自动化；创建不是立即执行，也不代表任务已完成。",
                    "permissionSummary": automation.permission_summary,
                    "maaConfig": maa_config,
                });
                if let Some(warning) = warning {
                    reply["readbackWarning"] = serde_json::json!(warning);
                }
                ToolResult::success(self.name(), reply.to_string())
            }
            Err(error) => ToolResult::error(self.name(), error),
        }
    }
}

impl ToolHandler for ListAutomationsHandler {
    fn name(&self) -> &str {
        "list_automations"
    }

    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            #[serde(default)]
            query: Option<String>,
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(args) => args,
            Err(_) => {
                return ToolResult::error(
                    self.name(),
                    "自动化查询参数无效；只能按标题查询当前工作区",
                )
            }
        };
        let workspace_id = match workspace_id_for_session(&self.db, &self.session_id) {
            Ok(value) => value,
            Err(error) => return ToolResult::error(self.name(), error),
        };
        match super::automation::workspace_automation_overview(
            self.db.clone(),
            &workspace_id,
            args.query.as_deref(),
        ) {
            Ok(overview) => {
                ToolResult::success(self.name(), serde_json::json!(overview).to_string())
            }
            Err(error) => ToolResult::error(self.name(), error),
        }
    }
}

impl ToolHandler for PauseAutomationHandler {
    fn name(&self) -> &str {
        "pause_automation"
    }

    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            automation_id: String,
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(args) => args,
            Err(_) => {
                return ToolResult::error(
                    self.name(),
                    "暂停参数无效；需要当前工作区的准确自动化 ID",
                )
            }
        };
        let workspace_id = match workspace_id_for_session(&self.db, &self.session_id) {
            Ok(value) => value,
            Err(error) => return ToolResult::error(self.name(), error),
        };
        match super::automation::pause_workspace_automation(self.db.clone(), &workspace_id, &args.automation_id) {
            Ok(()) => ToolResult::success(self.name(), serde_json::json!({
                "id": args.automation_id, "workspaceId": workspace_id, "enabled": false,
                "summary": "已暂停后续自动调度；正在执行的脚本、MAA 或主 Agent 不会因此停止。未启动的 Agent 派发按既有规则取消。",
            }).to_string()),
            Err(error) => ToolResult::error(self.name(), error),
        }
    }
}

impl ToolHandler for StopAutomationRunHandler {
    fn name(&self) -> &str {
        "stop_automation_run"
    }

    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            automation_id: String,
            run_id: String,
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(args) => args,
            Err(_) => {
                return ToolResult::error(
                    self.name(),
                    "停止参数无效；需要当前工作区的准确自动化和运行 ID",
                )
            }
        };
        let workspace_id = match workspace_id_for_session(&self.db, &self.session_id) {
            Ok(value) => value,
            Err(error) => return ToolResult::error(self.name(), error),
        };
        match super::automation::stop_workspace_automation_run(self.db.clone(), &workspace_id, &args.automation_id, &args.run_id) {
            Ok(()) => ToolResult::success(self.name(), serde_json::json!({
                "id": args.automation_id, "runId": args.run_id, "workspaceId": workspace_id,
                "stopControl": "requested",
                "summary": "已请求停止本次受控运行，尚需等待清理和结束回执。已发生的操作不会撤回；后续计划未因此改变，异常仍可能暂停计划。请用 list_automations 核对结果，不要据此声称已经停止。",
            }).to_string()),
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
        Tool::new("list_automations",
            "只读查询当前工作区的自动化、一次性提醒及脱敏运行回执，包括已触发、已暂停、失败和未发送的结果。可按标题字面包含筛选；最多100项，truncated=true时缩小query。maaConfig仅是已保存绑定参数，不证明当前授权或程序可用；appDisplayName为空时不得编造名称。未知结果不自动重试，running回执不证明进程存活，completed按执行方式解释，不证明通知已送达或游戏成功。",
            serde_json::json!({"type":"object","additionalProperties":false,
                "properties":{"query":{"type":"string","maxLength":120,"description":"可选：自动化标题的字面包含查询，不是通配符"}}}),
        ).with_category(ToolCategory::Extension)
            .with_prompt_snippet("查询提醒是否已触发、通知结果或日常自动化进度／结果时，用 list_automations 读取当前工作区的持久回执，可按标题 query；不要只查待提醒列表。暂停、停止前也先查询准确状态和 ID。latestRun.stopControl 仅在本应用持有本次运行时出现；缺少它不能停止历史运行。触发不证明通知送达，completed 不承诺游戏目标全部完成。"),
        Arc::new(ListAutomationsHandler { session_id: session_id.clone(), db: db.clone() }),
    );
    registry.register(
        Tool::new("pause_automation",
            "暂停当前工作区自动化的后续调度。只接受 list_automations 返回的准确 automation_id；这是确认后产生的副作用，不停止当前运行、不重试、不重新启用或删除。",
            serde_json::json!({"type":"object","additionalProperties":false,
                "properties":{"automation_id":{"type":"string","minLength":1,"description":"list_automations 返回的当前工作区自动化 ID"}},
                "required":["automation_id"]}),
        ).with_category(ToolCategory::Extension).with_confirmation(true)
            .with_prompt_snippet("暂停自动化先准确查询 ID 并遵守确认策略。暂停只影响未来调度，不意味着停止脚本、MAA 队列或已经开始的主 Agent；结果未知时先指导核对实际状态，绝不自动重放。"),
        Arc::new(PauseAutomationHandler { session_id: session_id.clone(), db: db.clone() }),
    );
    registry.register(
        Tool::new("stop_automation_run",
            "请求停止当前工作区中本应用持有的这一次脚本或 MAA 执行。先查询 list_automations，仅使用 latestRun.stopControl=available/requested 的准确 ID。只设停止信号，等待执行端清理；不停止主 Agent、不接管其他程序、不撤回外部操作或暂停未来计划。",
            serde_json::json!({"type":"object","additionalProperties":false,
                "properties":{
                    "automation_id":{"type":"string","minLength":1},
                    "run_id":{"type":"string","minLength":1,"description":"list_automations 返回的 latestRun.id"}},
                "required":["automation_id","run_id"]}),
        ).with_category(ToolCategory::Extension).with_confirmation(true)
            .with_prompt_snippet("用户要停止当前自动化而非暂停未来计划时，先准确查询，再遵守统一确认策略调用 stop_automation_run。requested 只代表请求已接受，须查询结束回执；MAA 停止后可能需要人工核对，不自动重试。"),
        Arc::new(StopAutomationRunHandler { session_id: session_id.clone(), db: db.clone() }),
    );
    registry.register(
        Tool::new(
            "list_reminders",
            "仅列出当前工作区尚未触发的一次性提醒，用于查看待办或取消前定位。空列表不代表提醒未触发；查询已触发、失败、未发送等历史结果请用 list_automations。不能查看其他工作区。",
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {}
            }),
        )
        .with_category(ToolCategory::Extension)
        .with_prompt_snippet("查看待提醒或取消前用 list_reminders 获取准确 ID；查询提醒是否触发、通知是否成功或历史结果用 list_automations，不从待提醒空列表推断结果。"),
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
            "在当前工作区创建每日自动化：默认 agent 执行重复任务；maa 按需启动已可信 MAA 的当前 GUI 勾选队列。创建 MAA 是批准以后无人值守执行及本次自有进程回收，不是立即运行或修改任务设置；可选启动已配置模拟器，绝不接管已打开 MAA。队列结束不等于游戏日常全部完成。",
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "title": { "type": "string", "description": "简短、可识别的自动化名称" },
                    "prompt": { "type": "string", "description": "agent 模式必填，每次触发时交给主 Agent 的任务说明；maa 不使用此字段" },
                    "schedule": { "type": "string", "description": "每天的运行时间，格式如“每天 18:00”" },
                    "executorKind": { "type": "string", "enum": ["agent", "maa"], "default": "agent", "description": "maa 是专用按需预设，不通过通用脚本启动常驻 GUI" },
                    "appId": { "type": "string", "description": "maa 必填：inspect_desktop_capabilities 返回的准确可信 MAA id，必须已启用并授予 launch；不能用路径替代或扩展应用授权" },
                    "launchEmulator": { "type": "boolean", "default": false, "description": "仅 maa：是否明确批准本次启动其已配置模拟器，不改 MAA 设置" },
                    "timeoutSeconds": { "type": "integer", "minimum": 10, "maximum": 3600, "description": "仅 maa：最大等待秒数，默认 3600；超时回收本次自有进程并要求检查" }
                },
                "required": ["title", "schedule"]
            }),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true)
        .with_prompt_snippet(
            "用户要求重复任务时用 create_automation。MAA 日常选 executorKind=maa，先用 inspect_desktop_capabilities 核对准确可信 appId，再明确每日时间、是否启动已配置模拟器，并确认这是以后无人值守的当前 GUI 队列；遵守既有副作用确认，不修改或凭空授予应用权限。",
        ),
        Arc::new(CreateAutomationHandler { session_id, db }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrate;

    fn maa_fixture() -> (tempfile::TempDir, Arc<Mutex<Connection>>, String, String) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("config")).unwrap();
        let executable = directory.path().join("MAA.exe");
        std::fs::write(&executable, b"fixture only; never execute").unwrap();
        let config = serde_json::json!({
            "Current": "daily", "Timers": {"List": [{"IsEnabled": false}]},
            "Configurations": {"daily": {"TaskQueue": [{"IsEnable":true}], "Gui": {
                "RemoteControl": {}, "StartUpSettings": {"RunDirectly": false}
            }}}
        });
        std::fs::write(
            directory.path().join("config/gui.new.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        let (workspace_id, session_id) = connection
            .query_row(
                "SELECT id,active_session_id FROM projects WHERE kind='personal'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap();
        connection.execute(
            "INSERT INTO desktop_trusted_apps(id,display_name,executable_path,capabilities,enabled,created_at,updated_at)
             VALUES('maa-app','Fixture MAA',?1,'[\"launch\"]',1,1,7)",
            [executable.to_string_lossy().as_ref()],
        ).unwrap();
        (
            directory,
            Arc::new(Mutex::new(connection)),
            workspace_id,
            session_id,
        )
    }

    #[test]
    fn maa_tool_creates_only_the_current_workspace_preset_and_keeps_confirmation() {
        let (directory, db, workspace_id, session_id) = maa_fixture();
        let config = directory.path().join("config/gui.new.json");
        let original = std::fs::read(&config).unwrap();
        let mut registry = ToolRegistry::empty();
        register(&mut registry, session_id.clone(), db.clone());
        let tool = registry.get("create_automation").unwrap();
        assert!(tool.requires_confirmation);
        assert!(tool.description.contains("以后无人值守"));
        assert!(tool.description.contains("自有进程回收"));
        assert!(tool.parameters["properties"].get("workspaceId").is_none());
        let handler = CreateAutomationHandler {
            session_id,
            db: db.clone(),
        };
        let result = handler.execute(
            &serde_json::json!({
                "title": "MAA 18:00", "schedule": "每天 18:00", "executorKind": "maa",
                "appId": "maa-app", "launchEmulator": true
            }),
            Path::new("."),
        );
        assert!(result.success, "{}", result.content);
        assert!(result.content.contains("当前 GUI 勾选队列"));
        assert!(result.content.contains("队列结束不等于游戏日常全部完成"));
        let created: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(created.get("readbackWarning").is_none());
        assert_eq!(
            created["maaConfig"],
            serde_json::json!({
                "appId": "maa-app", "appDisplayName": "Fixture MAA",
                "launchEmulator": true, "timeoutSeconds": 3600,
            })
        );
        let overview = super::super::automation::workspace_automation_overview(
            db.clone(),
            &workspace_id,
            None,
        )
        .unwrap();
        assert_eq!(
            created["maaConfig"],
            serde_json::to_value(&overview.items[0].maa_config).unwrap()
        );
        for field in ["prompt", "scriptPath", "scriptArgs", "workingDir", "output"] {
            assert!(created.get(field).is_none());
            assert!(created["maaConfig"].get(field).is_none());
        }
        let stored: (String, String, u64, String, i64, bool) = db.lock().unwrap().query_row(
            "SELECT a.executor_kind,a.workspace_id,a.timeout_seconds,o.trusted_app_id,o.app_revision,o.launch_emulator
             FROM automations a JOIN maa_automation_options o ON o.automation_id=a.id", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        ).unwrap();
        assert_eq!(
            stored,
            ("maa".into(), workspace_id, 3600, "maa-app".into(), 7, true)
        );
        assert_eq!(
            std::fs::read(config).unwrap(),
            original,
            "creation must be read-only outside AngelBot"
        );
    }

    #[test]
    fn maa_tool_cannot_expand_trust_or_accept_a_model_selected_workspace() {
        for rejected in [
            "missing_app",
            "revoked",
            "workspace",
            "path",
            "executor",
            "timeout",
            "auto_run",
        ] {
            let (directory, db, _, session_id) = maa_fixture();
            let mut args = serde_json::json!({
                "title": "MAA 18:00", "schedule": "18:00", "executorKind": "maa", "appId": "maa-app"
            });
            match rejected {
                "missing_app" => args["appId"] = serde_json::json!("unknown"),
                "revoked" => {
                    db.lock()
                        .unwrap()
                        .execute("UPDATE desktop_trusted_apps SET capabilities='[]'", [])
                        .unwrap();
                }
                "workspace" => args["workspaceId"] = serde_json::json!("foreign-workspace"),
                "path" => {
                    args["appId"] =
                        serde_json::json!(directory.path().join("MAA.exe").to_string_lossy())
                }
                "executor" => args["executorKind"] = serde_json::json!("script"),
                "timeout" => args["timeoutSeconds"] = serde_json::json!(-1),
                "auto_run" => {
                    let config = directory.path().join("config/gui.new.json");
                    let mut value: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
                    value["Configurations"]["daily"]["Gui"]["StartUpSettings"]["RunDirectly"] =
                        serde_json::json!(true);
                    std::fs::write(config, serde_json::to_vec(&value).unwrap()).unwrap();
                }
                _ => unreachable!(),
            }
            let handler = CreateAutomationHandler {
                session_id,
                db: db.clone(),
            };
            let result = handler.execute(&args, Path::new("."));
            assert!(!result.success, "{rejected}: {}", result.content);
            let count: i64 = db
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM automations", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 0, "{rejected}");
        }
    }

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
        let created: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(created["id"].as_str().is_some_and(|id| !id.is_empty()));
        assert_eq!(created["workspaceId"], workspace_id);
        assert_eq!(created["enabled"], true);
        assert!(created["maaConfig"].is_null());
        assert!(created.get("readbackWarning").is_none());
        assert!(created["nextRunAt"].as_i64().is_some());
        assert!(created["summary"]
            .as_str()
            .unwrap()
            .contains("创建不是立即执行"));
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
    fn result_tools_are_workspace_scoped_read_only_or_confirmed_future_only_pause() {
        let (_directory, db, workspace_id, session_id) = maa_fixture();
        let mut registry = ToolRegistry::empty();
        register(&mut registry, session_id.clone(), db.clone());
        assert!(
            !registry
                .get("list_automations")
                .unwrap()
                .requires_confirmation
        );
        assert!(
            registry
                .get("pause_automation")
                .unwrap()
                .requires_confirmation
        );
        assert!(!crate::agent::guards::is_side_effect(
            &registry,
            "list_automations"
        ));
        assert!(crate::agent::guards::is_side_effect(
            &registry,
            "pause_automation"
        ));
        assert!(
            registry
                .get("stop_automation_run")
                .expect("Main can stop an exact owned run")
                .requires_confirmation
        );
        assert!(crate::agent::guards::is_side_effect(
            &registry,
            "stop_automation_run"
        ));
        let created = CreateAutomationHandler { session_id: session_id.clone(), db: db.clone() }.execute(
            &serde_json::json!({"title":"Daily review","prompt":"Summarize notes","schedule":"18:00"}), Path::new("."),
        );
        assert!(created.success, "{}", created.content);
        let created: serde_json::Value = serde_json::from_str(&created.content).unwrap();
        let id = created["id"].as_str().unwrap();
        db.lock().unwrap().execute(
            "INSERT INTO automation_runs(id,automation_id,status,summary,output,started_at,finished_at)
             VALUES('result',?1,'completed','secret-path-error','secret-output',20,20)", [id],
        ).unwrap();
        let list = ListAutomationsHandler {
            session_id: session_id.clone(),
            db: db.clone(),
        };
        let before = db.lock().unwrap().total_changes();
        let result = list.execute(&serde_json::json!({"query":"Daily"}), Path::new("."));
        assert!(result.success, "{}", result.content);
        let overview: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(overview["items"][0]["id"], id);
        assert_eq!(
            overview["items"][0]["latestRun"]["summary"],
            "主 Agent 已结束本次处理；具体结果见归属工作区对话。"
        );
        assert!(!result.content.contains("secret"));
        assert_eq!(db.lock().unwrap().total_changes(), before);
        let invalid = list.execute(&serde_json::json!({"workspaceId":"other"}), Path::new("."));
        assert!(!invalid.success);
        let pause = PauseAutomationHandler {
            session_id,
            db: db.clone(),
        };
        for _ in 0..2 {
            let result = pause.execute(&serde_json::json!({"automation_id":id}), Path::new("."));
            assert!(result.success, "{}", result.content);
            assert!(result.content.contains("不会因此停止"));
        }
        assert!(
            !pause
                .execute(
                    &serde_json::json!({"automation_id":id,"enabled":true}),
                    Path::new(".")
                )
                .success
        );
        let overview =
            super::super::automation::workspace_automation_overview(db, &workspace_id, None)
                .unwrap();
        assert!(!overview.items[0].enabled);
        assert_eq!(
            overview.items[0].latest_run.as_ref().unwrap().status,
            "completed"
        );
    }

    #[test]
    fn result_tools_cannot_list_or_pause_another_workspace_or_an_unbound_session() {
        let (_directory, db, _, session_id) = maa_fixture();
        db.lock().unwrap().execute_batch(
            "INSERT INTO projects(id,name,path,created_at,kind,updated_at) VALUES('other-workspace','Other','',1,'project',1);
             INSERT INTO automations(id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,
                 executor_kind,workspace_id,script_args,timeout_seconds,next_run_at,created_at,updated_at)
             VALUES('foreign','Foreign secret','secret','schedule','18:00',1,'','script','other-workspace','[]',300,10,1,1);",
        ).unwrap();
        let list = ListAutomationsHandler {
            session_id: session_id.clone(),
            db: db.clone(),
        };
        let result = list.execute(&serde_json::json!({}), Path::new("."));
        assert!(result.success, "{}", result.content);
        assert!(!result.content.contains("foreign"));
        assert!(!result.content.contains("Foreign secret"));
        for session in [session_id.as_str(), "missing-session"] {
            let stop = StopAutomationRunHandler {
                session_id: session.into(),
                db: db.clone(),
            };
            let result = stop.execute(
                &serde_json::json!({"automation_id":"foreign","run_id":"foreign-run"}),
                Path::new("."),
            );
            assert!(!result.success);
            assert!(!result.content.contains("Foreign secret"));
        }
        let pause = PauseAutomationHandler {
            session_id,
            db: db.clone(),
        };
        assert!(
            !pause
                .execute(
                    &serde_json::json!({"automation_id":"foreign"}),
                    Path::new(".")
                )
                .success
        );
        let enabled: bool = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT enabled FROM automations WHERE id='foreign'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(enabled);
        let unbound = ListAutomationsHandler {
            session_id: "missing-session".into(),
            db,
        };
        assert!(
            !unbound
                .execute(&serde_json::json!({}), Path::new("."))
                .success
        );
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
        let mut registry = ToolRegistry::empty();
        register(&mut registry, session_id.clone(), db.clone());
        assert!(registry
            .get("list_reminders")
            .unwrap()
            .prompt_snippet
            .as_ref()
            .unwrap()
            .contains("list_automations"));
        let handler = ListRemindersHandler {
            session_id: session_id.clone(),
            db: db.clone(),
        };

        let result = handler.execute(&serde_json::json!({}), Path::new("."));

        assert!(result.success, "{}", result.content);
        assert!(result.content.contains(&visible.id));
        assert!(result.content.contains("可见提醒"));
        assert!(!result.content.contains(&hidden.id));
        assert!(!result.content.contains("其他空间提醒"));

        {
            let conn = db.lock().unwrap();
            conn.execute(
                "UPDATE automations SET enabled=0,next_run_at=NULL,last_run_at=20 WHERE id=?1",
                [&visible.id],
            )
            .unwrap();
            conn.execute("INSERT INTO automation_runs(id,automation_id,status,summary,output,started_at,finished_at)
                VALUES('triggered',?1,'completed','private receipt','',20,20)", [&visible.id]).unwrap();
        }
        let before = db.lock().unwrap().total_changes();
        let pending = handler.execute(&serde_json::json!({}), Path::new("."));
        assert!(!pending.content.contains(&visible.id));
        let history = ListAutomationsHandler {
            session_id,
            db: db.clone(),
        }
        .execute(&serde_json::json!({"query":"可见提醒"}), Path::new("."));
        assert!(history.success, "{}", history.content);
        let history: serde_json::Value = serde_json::from_str(&history.content).unwrap();
        assert_eq!(history["items"][0]["id"], visible.id);
        assert_eq!(history["items"][0]["latestRun"]["status"], "completed");
        assert!(history["items"][0]["latestRun"]["summary"]
            .as_str()
            .unwrap()
            .contains("不证明系统通知已送达"));
        assert_eq!(db.lock().unwrap().total_changes(), before);
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
