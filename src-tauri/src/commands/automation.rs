use crate::{
    agent::supervision::{SupervisorInputKind, WorkspaceSupervisor},
    AppState,
};
use chrono::{TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, State};
use uuid::Uuid;

const DEFAULT_TIMEOUT_SECONDS: u64 = 300;
const MIN_TIMEOUT_SECONDS: u64 = 10;
const MAX_TIMEOUT_SECONDS: u64 = 3600;
const MAX_CAPTURED_OUTPUT_BYTES: usize = 64 * 1024;
const AUTOMATION_CLAIM_TTL_SECONDS: i64 = MAX_TIMEOUT_SECONDS as i64 + 60;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduledAutomationClaim {
    id: String,
    token: String,
}

struct ScheduledAutomationExecution {
    notification: Option<ReminderNotification>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Automation {
    pub id: String,
    pub title: String,
    pub prompt: String,
    #[serde(rename = "triggerKind")]
    pub trigger_kind: String,
    #[serde(rename = "triggerValue")]
    pub trigger_value: String,
    pub enabled: bool,
    #[serde(rename = "permissionSummary")]
    pub permission_summary: String,
    #[serde(rename = "executorKind")]
    pub executor_kind: String,
    #[serde(rename = "workspaceId")]
    pub workspace_id: Option<String>,
    #[serde(rename = "scriptPath")]
    pub script_path: Option<String>,
    #[serde(rename = "scriptArgs")]
    pub script_args: Vec<String>,
    #[serde(rename = "workingDir")]
    pub working_dir: Option<String>,
    #[serde(rename = "timeoutSeconds")]
    pub timeout_seconds: u64,
    #[serde(rename = "nextRunAt")]
    pub next_run_at: Option<i64>,
    #[serde(rename = "lastRunAt")]
    pub last_run_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct AutomationRun {
    pub id: String,
    pub status: String,
    pub summary: String,
    #[serde(rename = "exitCode")]
    pub exit_code: Option<i32>,
    pub output: String,
    #[serde(rename = "startedAt")]
    pub started_at: i64,
}

#[derive(Debug)]
struct ScriptRunResult {
    status: &'static str,
    summary: String,
    exit_code: Option<i32>,
    output: String,
    supervisor_input_id: Option<String>,
}

/// Durable identity bridging one automation run to the Main-Agent turn that
/// consumes it. The reply id is intentionally allocated before the model is
/// called so restart recovery can distinguish "not started" from "do not
/// replay" without inspecting model output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentAutomationDispatch {
    pub automation_run_id: String,
    pub foreground_run_id: String,
    pub supervisor_input_id: String,
}

/// The foreground pump owns only agent-automation follow-ups.  A generic
/// follow-up must be released for its eventual owner, while a lifecycle-cancelled
/// automation input is already terminal and must never be reintroduced into the
/// queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentAutomationDispatchPreparation {
    Dispatch(AgentAutomationDispatch),
    Cancelled,
    Unowned,
}

/// The last durable admission check immediately before the Main-Agent runner is
/// entered.  `AlreadyStarted` is intentionally distinct from cancellation: a
/// task-run row is the non-replay boundary and must be reconciled, not retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentAutomationDispatchAdmission {
    Start,
    Cancelled,
    AlreadyStarted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReminderNotification {
    title: String,
    body: String,
}

/// Calculate the next durable trigger. Repeating schedules retain the existing
/// daily `HH:MM` contract in the system's local timezone. One-time reminders use
/// RFC 3339 so their explicit offset is independent of the system timezone.
fn next_run_at(trigger_kind: &str, trigger_value: &str, now: i64) -> Option<i64> {
    next_run_at_in_timezone(trigger_kind, trigger_value, now, &chrono::Local)
}

fn next_run_at_in_timezone<T: TimeZone>(
    trigger_kind: &str,
    trigger_value: &str,
    now: i64,
    timezone: &T,
) -> Option<i64> {
    if trigger_kind == "once" {
        return chrono::DateTime::parse_from_rfc3339(trigger_value.trim())
            .ok()
            .map(|value| value.timestamp())
            .filter(|value| *value > now);
    }
    if trigger_kind != "schedule" {
        return None;
    }
    // Frequency is part of the contract, not decoration around a time token.
    // Unsupported legacy values must never silently acquire a daily meaning.
    let value = trigger_value.trim();
    let token = value.strip_prefix("每天").unwrap_or(value).trim();
    let (hour, minute) = token.split_once(':')?;
    if !(1..=2).contains(&hour.len())
        || minute.len() != 2
        || !hour.bytes().all(|value| value.is_ascii_digit())
        || !minute.bytes().all(|value| value.is_ascii_digit())
    {
        return None;
    }
    let hour = hour.parse::<u32>().ok()?;
    let minute = minute.parse::<u32>().ok()?;
    let current = chrono::DateTime::<Utc>::from_timestamp(now, 0)?.with_timezone(timezone);
    let time = chrono::NaiveTime::from_hms_opt(hour, minute, 0)?;
    let mut date = current.date_naive();
    // A repeated wall-clock slot uses only its first occurrence (once per local
    // day); a nonexistent slot is skipped. Three civil dates cover today's
    // passed slot plus a DST gap or skipped civil date without an unbounded loop.
    for _ in 0..3 {
        if let Some(candidate) = timezone
            .from_local_datetime(&date.and_time(time))
            .earliest()
        {
            if candidate.timestamp() > now {
                return Some(candidate.timestamp());
            }
        }
        date = date.succ_opt()?;
    }
    None
}

/// Reject unknown or unparsable triggers rather than persisting an enabled item
/// that can never be claimed by the scheduler.
fn validated_next_run_at(trigger_kind: &str, trigger_value: &str, now: i64) -> Result<i64, String> {
    match trigger_kind {
        "schedule" => next_run_at(trigger_kind, trigger_value, now).ok_or_else(|| {
            "目前仅支持每日定时，请使用“每天 09:00”或“09:00”；工作日、每周等频率尚不支持"
                .to_string()
        }),
        "once" => next_run_at(trigger_kind, trigger_value, now).ok_or_else(|| {
            "无法识别提醒时间，或时间已经过去；请使用带时区的 RFC 3339 时间".to_string()
        }),
        _ => Err("仅支持每日定时或一次性提醒触发器".to_string()),
    }
}

fn parse_script_args(raw: &str) -> Result<Vec<String>, String> {
    let args: Vec<String> =
        serde_json::from_str(raw).map_err(|_| "脚本参数必须是字符串数组".to_string())?;
    if args.iter().any(|arg| arg.contains('\0')) {
        return Err("脚本参数不能包含空字符".to_string());
    }
    Ok(args)
}

fn normalize_timeout(timeout_seconds: Option<u64>) -> Result<u64, String> {
    let value = timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS);
    if !(MIN_TIMEOUT_SECONDS..=MAX_TIMEOUT_SECONDS).contains(&value) {
        return Err(format!(
            "脚本超时必须在 {} 到 {} 秒之间",
            MIN_TIMEOUT_SECONDS, MAX_TIMEOUT_SECONDS
        ));
    }
    Ok(value)
}

fn validate_script_path(
    state: &AppState,
    script_path: &str,
    working_dir: Option<&str>,
) -> Result<(PathBuf, PathBuf), String> {
    let root = crate::commands::file::resolve_work_dir(state, None)?;
    let root = std::fs::canonicalize(&root).map_err(|e| format!("无法解析工作目录：{}", e))?;
    let script = std::fs::canonicalize(script_path.trim())
        .map_err(|e| format!("无法解析脚本文件：{}", e))?;
    if !script.is_file() {
        return Err("脚本路径不是文件".to_string());
    }
    if !script.starts_with(&root) {
        return Err("脚本必须位于当前工作目录内".to_string());
    }

    let work_dir = match working_dir.filter(|value| !value.trim().is_empty()) {
        Some(value) => std::fs::canonicalize(value.trim())
            .map_err(|e| format!("无法解析脚本工作目录：{}", e))?,
        None => script
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "脚本缺少父目录".to_string())?,
    };
    if !work_dir.is_dir() || !work_dir.starts_with(&root) {
        return Err("脚本工作目录必须位于当前工作目录内".to_string());
    }
    Ok((script, work_dir))
}

fn script_command(script: &Path, args: &[String], working_dir: &Path) -> Result<Command, String> {
    let extension = script
        .extension()
        .and_then(OsStr::to_str)
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| "仅支持 Python、Node.js、PowerShell 或批处理脚本".to_string())?;
    let mut command = match extension.as_str() {
        "py" => Command::new("python"),
        "js" | "mjs" | "cjs" => Command::new("node"),
        "ps1" => {
            let mut command = Command::new("powershell");
            command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"]);
            command
        }
        "cmd" | "bat" => {
            let mut command = Command::new("cmd");
            command.args(["/d", "/c"]);
            command
        }
        _ => return Err("仅支持 .py、.js、.mjs、.cjs、.ps1、.cmd 或 .bat 脚本".to_string()),
    };
    command
        .arg(script)
        .args(args)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(command)
}

fn capture_stream<R: Read + Send + 'static>(mut stream: R, sender: mpsc::Sender<Vec<u8>>) {
    thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stream.read_to_end(&mut buffer);
        let _ = sender.send(buffer);
    });
}

fn truncate_output(stdout: Vec<u8>, stderr: Vec<u8>) -> String {
    let mut combined = stdout;
    if !stderr.is_empty() {
        if !combined.is_empty() {
            combined.extend_from_slice(b"\n");
        }
        combined.extend_from_slice(b"[stderr]\n");
        combined.extend_from_slice(&stderr);
    }
    let truncated = combined.len() > MAX_CAPTURED_OUTPUT_BYTES;
    combined.truncate(MAX_CAPTURED_OUTPUT_BYTES);
    let mut text = String::from_utf8_lossy(&combined).trim().to_string();
    if truncated {
        text.push_str("\n[输出已截断]");
    }
    text
}

fn execute_script(state: &AppState, automation: &Automation) -> ScriptRunResult {
    let run = || -> Result<ScriptRunResult, String> {
        let script_path = automation
            .script_path
            .as_deref()
            .ok_or_else(|| "未配置脚本路径".to_string())?;
        let (script, work_dir) =
            validate_script_path(state, script_path, automation.working_dir.as_deref())?;
        let mut command = script_command(&script, &automation.script_args, &work_dir)?;
        let mut child = command
            .spawn()
            .map_err(|e| format!("无法启动脚本：{}", e))?;
        let (sender, receiver) = mpsc::channel();
        if let Some(stdout) = child.stdout.take() {
            capture_stream(stdout, sender.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            capture_stream(stderr, sender);
        }

        let started = Instant::now();
        let timeout = Duration::from_secs(automation.timeout_seconds);
        let status = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|e| format!("无法检查脚本状态：{}", e))?
            {
                break (status, false);
            }
            if started.elapsed() >= timeout {
                child
                    .kill()
                    .map_err(|e| format!("无法停止超时脚本：{}", e))?;
                let status = child
                    .wait()
                    .map_err(|e| format!("无法等待脚本停止：{}", e))?;
                break (status, true);
            }
            thread::sleep(Duration::from_millis(100));
        };
        let stdout = receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_default();
        let stderr = receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_default();
        let output = truncate_output(stdout, stderr);
        if status.1 {
            return Ok(ScriptRunResult {
                status: "timed_out",
                summary: format!("脚本超过 {} 秒，已停止", automation.timeout_seconds),
                exit_code: status.0.code(),
                output,
                supervisor_input_id: None,
            });
        }
        let exit_code = status.0.code();
        Ok(if status.0.success() {
            ScriptRunResult {
                status: "completed",
                summary: "脚本执行完成".to_string(),
                exit_code,
                output,
                supervisor_input_id: None,
            }
        } else {
            ScriptRunResult {
                status: "failed",
                summary: format!(
                    "脚本执行失败（退出码 {}）",
                    exit_code.map_or("未知".to_string(), |value| value.to_string())
                ),
                exit_code,
                output,
                supervisor_input_id: None,
            }
        })
    };
    run().unwrap_or_else(|error| ScriptRunResult {
        status: "failed",
        summary: error.clone(),
        exit_code: None,
        output: error,
        supervisor_input_id: None,
    })
}

#[tauri::command]
pub fn get_automations(state: State<AppState>) -> Result<Vec<Automation>, String> {
    get_automations_impl(&state)
}

pub fn get_automations_impl(state: &AppState) -> Result<Vec<Automation>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,executor_kind,script_path,script_args,working_dir,timeout_seconds,next_run_at,last_run_at,workspace_id FROM automations ORDER BY updated_at DESC").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], automation_from_row)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn automation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Automation> {
    let raw_args: String = row.get(9)?;
    Ok(Automation {
        id: row.get(0)?,
        title: row.get(1)?,
        prompt: row.get(2)?,
        trigger_kind: row.get(3)?,
        trigger_value: row.get(4)?,
        enabled: row.get::<_, i64>(5)? != 0,
        permission_summary: row.get(6)?,
        executor_kind: row.get(7)?,
        script_path: row.get(8)?,
        script_args: parse_script_args(&raw_args).unwrap_or_default(),
        working_dir: row.get(10)?,
        timeout_seconds: row.get::<_, i64>(11)? as u64,
        next_run_at: row.get(12)?,
        last_run_at: row.get(13)?,
        workspace_id: row.get(14)?,
    })
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn create_automation(
    state: State<AppState>,
    title: String,
    prompt: String,
    trigger_kind: String,
    trigger_value: String,
    permission_summary: String,
    executor_kind: Option<String>,
    workspace_id: Option<String>,
    script_path: Option<String>,
    script_args: Option<Vec<String>>,
    working_dir: Option<String>,
    timeout_seconds: Option<u64>,
) -> Result<Automation, String> {
    create_automation_impl(
        &state,
        title,
        prompt,
        trigger_kind,
        trigger_value,
        permission_summary,
        executor_kind,
        workspace_id,
        script_path,
        script_args,
        working_dir,
        timeout_seconds,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_automation_impl(
    state: &AppState,
    title: String,
    prompt: String,
    trigger_kind: String,
    trigger_value: String,
    permission_summary: String,
    executor_kind: Option<String>,
    workspace_id: Option<String>,
    script_path: Option<String>,
    script_args: Option<Vec<String>>,
    working_dir: Option<String>,
    timeout_seconds: Option<u64>,
) -> Result<Automation, String> {
    if title.trim().is_empty() {
        return Err("自动化名称不能为空".to_string());
    }
    let executor_kind = executor_kind.unwrap_or_else(|| "agent".to_string());
    if executor_kind != "agent" && executor_kind != "script" && executor_kind != "notification" {
        return Err("不支持的执行方式".to_string());
    }
    let script_args = script_args.unwrap_or_default();
    if script_args.iter().any(|arg| arg.contains('\0')) {
        return Err("脚本参数不能包含空字符".to_string());
    }
    let timeout_seconds = normalize_timeout(timeout_seconds)?;
    let workspace_id = workspace_id.filter(|id| !id.trim().is_empty());
    let script_path = script_path.filter(|path| !path.trim().is_empty());
    let working_dir = working_dir.filter(|path| !path.trim().is_empty());
    if executor_kind == "agent" {
        return create_agent_automation_definition(
            state.db.clone(),
            title,
            prompt,
            trigger_kind,
            trigger_value,
            permission_summary,
            workspace_id.ok_or_else(|| "Agent 自动化需要归属工作区".to_string())?,
            timeout_seconds,
        );
    }

    if executor_kind == "script" {
        let path = script_path
            .as_deref()
            .ok_or_else(|| "脚本任务需要脚本路径".to_string())?;
        validate_script_path(state, path, working_dir.as_deref())?;
    }
    let now = Utc::now().timestamp();
    let next_run_at = Some(validated_next_run_at(&trigger_kind, &trigger_value, now)?);
    let item = Automation {
        id: Uuid::new_v4().to_string(),
        title,
        prompt,
        trigger_kind,
        trigger_value,
        enabled: true,
        permission_summary,
        executor_kind,
        workspace_id,
        script_path,
        script_args,
        working_dir,
        timeout_seconds,
        next_run_at,
        last_run_at: None,
    };
    persist_automation(&state.db, &item, now)?;
    Ok(item)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn create_agent_automation_definition(
    db: Arc<Mutex<Connection>>,
    title: String,
    prompt: String,
    trigger_kind: String,
    trigger_value: String,
    permission_summary: String,
    workspace_id: String,
    timeout_seconds: u64,
) -> Result<Automation, String> {
    if title.trim().is_empty() {
        return Err("自动化名称不能为空".to_string());
    }
    if prompt.trim().is_empty() {
        return Err("Agent 自动化需要任务说明".to_string());
    }
    let now = Utc::now().timestamp();
    let next_run_at = Some(validated_next_run_at(&trigger_kind, &trigger_value, now)?);
    let exists = db
        .lock()
        .map_err(|e| e.to_string())?
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            params![workspace_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())?
        != 0;
    if !exists {
        return Err("归属工作区不存在".to_string());
    }
    let item = Automation {
        id: Uuid::new_v4().to_string(),
        title,
        prompt,
        trigger_kind,
        trigger_value,
        enabled: true,
        permission_summary,
        executor_kind: "agent".to_string(),
        workspace_id: Some(workspace_id),
        script_path: None,
        script_args: Vec::new(),
        working_dir: None,
        timeout_seconds,
        next_run_at,
        last_run_at: None,
    };
    persist_automation(&db, &item, now)?;
    Ok(item)
}

pub(crate) fn create_notification_reminder_definition(
    db: Arc<Mutex<Connection>>,
    title: String,
    body: String,
    when: String,
    workspace_id: String,
) -> Result<Automation, String> {
    if title.trim().is_empty() {
        return Err("提醒名称不能为空".to_string());
    }
    if body.trim().is_empty() {
        return Err("提醒内容不能为空".to_string());
    }
    let now = Utc::now().timestamp();
    let next_run_at = Some(validated_next_run_at("once", &when, now)?);
    let exists = db
        .lock()
        .map_err(|error| error.to_string())?
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            params![workspace_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| error.to_string())?
        != 0;
    if !exists {
        return Err("归属工作区不存在".to_string());
    }
    let item = Automation {
        id: Uuid::new_v4().to_string(),
        title,
        prompt: body,
        trigger_kind: "once".to_string(),
        trigger_value: when,
        enabled: true,
        permission_summary: "到点发送本地提醒".to_string(),
        executor_kind: "notification".to_string(),
        workspace_id: Some(workspace_id),
        script_path: None,
        script_args: Vec::new(),
        working_dir: None,
        timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        next_run_at,
        last_run_at: None,
    };
    persist_automation(&db, &item, now)?;
    Ok(item)
}

fn persist_automation(
    db: &Arc<Mutex<Connection>>,
    item: &Automation,
    now: i64,
) -> Result<(), String> {
    db.lock().map_err(|e| e.to_string())?.execute(
        "INSERT INTO automations (id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,executor_kind,workspace_id,script_path,script_args,working_dir,timeout_seconds,next_run_at,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,1,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14)",
        params![item.id, item.title, item.prompt, item.trigger_kind, item.trigger_value, item.permission_summary, item.executor_kind, item.workspace_id, item.script_path, serde_json::to_string(&item.script_args).map_err(|e| e.to_string())?, item.working_dir, item.timeout_seconds as i64, item.next_run_at, now],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn set_automation_enabled(
    state: State<AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    set_automation_enabled_impl(&state, &id, enabled)
}
pub fn set_automation_enabled_impl(
    state: &AppState,
    id: &str,
    enabled: bool,
) -> Result<(), String> {
    let now = Utc::now().timestamp();
    let mut conn = state.db.lock().map_err(|e| e.to_string())?;
    set_automation_enabled_in_conn(&mut conn, id, enabled, now)
}

#[tauri::command]
pub fn delete_automation(state: State<AppState>, id: String) -> Result<(), String> {
    delete_automation_impl(&state, &id)
}
pub fn delete_automation_impl(state: &AppState, id: &str) -> Result<(), String> {
    let now = Utc::now().timestamp();
    let mut conn = state.db.lock().map_err(|e| e.to_string())?;
    delete_automation_in_conn(&mut conn, id, now)
}

/// Lifecycle changes must settle their own durable delivery inputs before they
/// make the automation ineligible.  Otherwise a `follow_up` that was claimed
/// by the foreground pump can outlive its parent automation and block that
/// Workspace forever.
fn set_automation_enabled_in_conn(
    conn: &mut Connection,
    id: &str,
    enabled: bool,
    now: i64,
) -> Result<(), String> {
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let (kind, value): (String, String) = transaction
        .query_row(
            "SELECT trigger_kind,trigger_value FROM automations WHERE id=?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?;
    let next_run_at = enabled
        .then(|| validated_next_run_at(&kind, &value, now))
        .transpose()?;
    if !enabled {
        cancel_unstarted_agent_automation_dispatches_in_tx(
            &transaction,
            id,
            now,
            "自动化已暂停；不会启动主 Agent",
        )?;
    }
    transaction
        .execute(
            "UPDATE automations
             SET enabled=?2,next_run_at=?3,schedule_claim_token=NULL,schedule_claimed_at=NULL,updated_at=?4
             WHERE id=?1",
            params![id, enabled, next_run_at, now],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(())
}

fn delete_automation_in_conn(conn: &mut Connection, id: &str, now: i64) -> Result<(), String> {
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    // Deletion cascades automation_runs but not supervisor inputs.  Settle every
    // linked delivery first, including a task that already started: that task is
    // independent Main-Agent work now, while its old delivery lease must not be
    // left claimed after the parent record disappears.
    cancel_all_agent_automation_inputs_in_tx(&transaction, id, now)?;
    transaction
        .execute("DELETE FROM automations WHERE id=?1", params![id])
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(())
}

fn cancel_unstarted_agent_automation_dispatches_in_tx(
    transaction: &Transaction<'_>,
    automation_id: &str,
    now: i64,
    summary: &str,
) -> Result<(), String> {
    transaction
        .execute(
            "UPDATE workspace_supervisor_inputs
             SET status = 'cancelled', claim_token = NULL, updated_at = ?1
             WHERE status IN ('queued', 'claimed')
               AND id IN (
                 SELECT run.supervisor_input_id
                 FROM automation_runs run
                 WHERE run.automation_id = ?2
                   AND run.supervisor_input_id IS NOT NULL
                   AND (run.foreground_run_id IS NULL OR NOT EXISTS (
                     SELECT 1 FROM task_runs task WHERE task.id = run.foreground_run_id
                   ))
               )",
            params![now, automation_id],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE automation_runs
             SET status = 'cancelled', summary = ?1, finished_at = ?2
             WHERE automation_id = ?3
               AND supervisor_input_id IS NOT NULL
               AND status IN ('queued', 'running')
               AND (foreground_run_id IS NULL OR NOT EXISTS (
                 SELECT 1 FROM task_runs task WHERE task.id = automation_runs.foreground_run_id
               ))",
            params![summary, now, automation_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn cancel_all_agent_automation_inputs_in_tx(
    transaction: &Transaction<'_>,
    automation_id: &str,
    now: i64,
) -> Result<(), String> {
    transaction
        .execute(
            "UPDATE workspace_supervisor_inputs
             SET status = 'cancelled', claim_token = NULL, updated_at = ?1
             WHERE status IN ('queued', 'claimed')
               AND id IN (
                 SELECT run.supervisor_input_id
                 FROM automation_runs run
                 WHERE run.automation_id = ?2 AND run.supervisor_input_id IS NOT NULL
               )",
            params![now, automation_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn run_automation_now(
    state: State<AppState>,
    app_handle: AppHandle,
    id: String,
) -> Result<(), String> {
    let notification = run_automation_now_with_notification_impl(&state, &id)?;
    if let Some(notification) = notification {
        crate::commands::settings::emit_notification_channel_impl(
            &state,
            &app_handle,
            "reminder".to_string(),
            notification.title,
            notification.body,
        )?;
    }
    state.foreground_automation_pump.wake(app_handle.clone());
    Ok(())
}
pub fn run_automation_now_impl(state: &AppState, id: &str) -> Result<(), String> {
    run_automation_now_with_notification_impl(state, id).map(|_| ())
}

fn run_automation_now_with_notification_impl(
    state: &AppState,
    id: &str,
) -> Result<Option<ReminderNotification>, String> {
    let automation = get_automations_impl(state)?
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| "未找到自动化任务".to_string())?;
    let now = Utc::now().timestamp();
    {
        let mut conn = state.db.lock().map_err(|error| error.to_string())?;
        let transaction = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        if pause_unsupported_schedule_in_tx(&transaction, &automation, now, None)? {
            transaction.commit().map_err(|error| error.to_string())?;
            return Err(
                "自动化使用了不支持的定时频率，已暂停；请重新配置为每日定时或一次性提醒"
                    .to_string(),
            );
        }
        if automation.executor_kind == "agent" {
            persist_agent_automation_run_in_tx(&transaction, &automation, now, None)?;
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(None);
        }
        transaction.commit().map_err(|error| error.to_string())?;
    }
    let result = run_automation_executor(state, &automation)?;
    let notification = notification_for_automation(&automation);
    persist_automation_run(state, &automation, result, now, None)?;
    Ok(notification)
}

fn run_automation_executor(
    state: &AppState,
    automation: &Automation,
) -> Result<ScriptRunResult, String> {
    if automation.executor_kind == "script" {
        Ok(execute_script(state, automation))
    } else if automation.executor_kind == "notification" {
        Ok(notification_run_result())
    } else {
        Err("该执行方式必须通过受控的自动化派发路径运行".to_string())
    }
}

fn persist_agent_automation_run_in_tx(
    transaction: &Transaction<'_>,
    automation: &Automation,
    now: i64,
    claim_token: Option<&str>,
) -> Result<(), String> {
    let input = enqueue_agent_automation_in_tx(transaction, automation, now)?;
    persist_automation_run_in_conn(
        transaction,
        automation,
        ScriptRunResult {
            status: "queued",
            summary: "已加入归属工作区的 AngelBot 队列，等待主 Agent 处理".to_string(),
            exit_code: None,
            output: String::new(),
            supervisor_input_id: Some(input.id),
        },
        now,
        claim_token,
    )
}

fn pause_unsupported_schedule_in_tx(
    transaction: &Transaction<'_>,
    automation: &Automation,
    now: i64,
    claim_token: Option<&str>,
) -> Result<bool, String> {
    if automation.trigger_kind != "schedule"
        || next_run_at(&automation.trigger_kind, &automation.trigger_value, now).is_some()
    {
        return Ok(false);
    }
    cancel_unstarted_agent_automation_dispatches_in_tx(
        transaction,
        &automation.id,
        now,
        "定时频率不受支持，已暂停；不会启动主 Agent",
    )?;
    persist_automation_run_in_conn(
        transaction,
        automation,
        ScriptRunResult {
            status: "needs_attention",
            summary: "定时频率不受支持，已暂停；请重新配置为每日定时或一次性提醒".to_string(),
            exit_code: None,
            output: String::new(),
            supervisor_input_id: None,
        },
        now,
        claim_token,
    )?;
    transaction
        .execute(
            "UPDATE automations SET enabled=0 WHERE id=?1",
            [&automation.id],
        )
        .map_err(|error| error.to_string())?;
    Ok(true)
}

fn notification_run_result() -> ScriptRunResult {
    ScriptRunResult {
        status: "completed",
        summary: "本地提醒已触发".to_string(),
        exit_code: None,
        output: String::new(),
        supervisor_input_id: None,
    }
}

fn notification_for_automation(automation: &Automation) -> Option<ReminderNotification> {
    (automation.executor_kind == "notification").then(|| ReminderNotification {
        title: automation.title.clone(),
        body: automation.prompt.clone(),
    })
}

fn persist_automation_run(
    state: &AppState,
    automation: &Automation,
    result: ScriptRunResult,
    now: i64,
    claim_token: Option<&str>,
) -> Result<(), String> {
    let mut conn = state.db.lock().map_err(|e| e.to_string())?;
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    persist_automation_run_in_conn(&transaction, automation, result, now, claim_token)?;
    transaction.commit().map_err(|error| error.to_string())
}

fn persist_automation_run_in_conn(
    conn: &Connection,
    automation: &Automation,
    result: ScriptRunResult,
    now: i64,
    claim_token: Option<&str>,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO automation_runs (
            id, automation_id, status, summary, exit_code, output, started_at,
            finished_at, supervisor_input_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8)",
        params![
            Uuid::new_v4().to_string(),
            automation.id,
            result.status,
            result.summary,
            result.exit_code,
            result.output,
            now,
            result.supervisor_input_id,
        ],
    )
    .map_err(|e| e.to_string())?;
    let next_run = next_run_at(&automation.trigger_kind, &automation.trigger_value, now);
    let is_one_time = automation.trigger_kind == "once";
    if let Some(token) = claim_token {
        // If the task was disabled or reclaimed while running, preserve that
        // newer scheduling decision. The completed run record remains useful.
        conn.execute(
            "UPDATE automations
             SET last_run_at=?2,next_run_at=?3,enabled=CASE WHEN ?4 THEN 0 ELSE enabled END,schedule_claim_token=NULL,schedule_claimed_at=NULL,updated_at=?2
             WHERE id=?1 AND schedule_claim_token=?5",
            params![automation.id, now, next_run, is_one_time, token],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "UPDATE automations SET last_run_at=?2,next_run_at=?3,enabled=CASE WHEN ?4 THEN 0 ELSE enabled END,updated_at=?2 WHERE id=?1",
            params![automation.id, now, next_run, is_one_time],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Run all enabled schedule definitions which are due while the desktop app is
/// alive. A durable claim prevents overlapping heartbeats from launching the
/// same due task twice; no database lock is retained during execution.
pub fn run_due_automations_impl(state: &AppState, now: i64) -> Result<usize, String> {
    run_due_automations_with_notifications_impl(state, now).map(|outcome| outcome.0)
}

fn run_due_automations_with_notifications_impl(
    state: &AppState,
    now: i64,
) -> Result<(usize, Vec<ReminderNotification>), String> {
    let claims = {
        let mut conn = state.db.lock().map_err(|error| error.to_string())?;
        claim_due_automations(&mut conn, now)?
    };
    run_claimed_automations(state, &claims, now)
}

fn run_claimed_automations(
    state: &AppState,
    claims: &[ScheduledAutomationClaim],
    now: i64,
) -> Result<(usize, Vec<ReminderNotification>), String> {
    let mut notifications = Vec::new();
    let mut executed = 0;
    for claim in claims {
        if let Some(execution) = run_claimed_automation_impl(state, claim, now)? {
            executed += 1;
            if let Some(notification) = execution.notification {
                notifications.push(notification);
            }
        }
    }
    Ok((executed, notifications))
}

fn claim_due_automations(
    conn: &mut Connection,
    now: i64,
) -> Result<Vec<ScheduledAutomationClaim>, String> {
    let stale_before = now - AUTOMATION_CLAIM_TTL_SECONDS;
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let due_ids = {
        let mut statement = transaction
            .prepare(
                "SELECT id FROM automations
             WHERE enabled = 1 AND trigger_kind IN ('schedule', 'once')
               AND next_run_at IS NOT NULL AND next_run_at <= ?1
               AND (schedule_claim_token IS NULL OR schedule_claimed_at <= ?2)
             ORDER BY next_run_at ASC, id ASC",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![now, stale_before], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
    };
    let mut claims = Vec::with_capacity(due_ids.len());
    for id in due_ids {
        let token = format!("automation_{}", Uuid::new_v4().simple());
        let changed = transaction
            .execute(
                "UPDATE automations
                 SET schedule_claim_token = ?1, schedule_claimed_at = ?2, updated_at = ?2
                 WHERE id = ?3 AND enabled = 1 AND trigger_kind IN ('schedule', 'once')
                   AND next_run_at IS NOT NULL AND next_run_at <= ?2
                   AND (schedule_claim_token IS NULL OR schedule_claimed_at <= ?4)",
                params![token, now, id, stale_before],
            )
            .map_err(|error| error.to_string())?;
        if changed == 1 {
            claims.push(ScheduledAutomationClaim { id, token });
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(claims)
}

fn run_claimed_automation_impl(
    state: &AppState,
    claim: &ScheduledAutomationClaim,
    now: i64,
) -> Result<Option<ScheduledAutomationExecution>, String> {
    // A batch claim is not permission to run forever. Earlier items may take
    // time while the user cancels/deletes this definition or a newer heartbeat
    // recovers its claim. Revalidate immediately before crossing execution.
    let automation = {
        let mut conn = state.db.lock().map_err(|error| error.to_string())?;
        let transaction = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let automation = transaction.query_row(
            "SELECT id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,executor_kind,script_path,script_args,working_dir,timeout_seconds,next_run_at,last_run_at,workspace_id
             FROM automations
             WHERE id=?1 AND enabled=1 AND schedule_claim_token=?2
               AND trigger_kind IN ('schedule','once')
               AND next_run_at IS NOT NULL AND next_run_at<=?3",
            params![claim.id, claim.token, now],
            automation_from_row,
        )
        .optional()
        .map_err(|error| error.to_string())?;
        let Some(automation) = automation else {
            return Ok(None);
        };
        if pause_unsupported_schedule_in_tx(&transaction, &automation, now, Some(&claim.token))? {
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(Some(ScheduledAutomationExecution { notification: None }));
        }
        if automation.executor_kind == "agent" {
            // The queue input, its owner receipt, and scheduling advance become
            // visible together; a failure cannot leave an unowned follow-up.
            persist_agent_automation_run_in_tx(&transaction, &automation, now, Some(&claim.token))?;
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(Some(ScheduledAutomationExecution { notification: None }));
        }
        if automation.executor_kind == "notification" {
            // Preparing this local notification has no external side effects.
            // Admission and completion share one lock/transaction: cancellation
            // either wins first, or sees that this reminder has already fired.
            persist_automation_run_in_conn(
                &transaction,
                &automation,
                notification_run_result(),
                now,
                Some(&claim.token),
            )?;
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(Some(ScheduledAutomationExecution {
                notification: notification_for_automation(&automation),
            }));
        }
        // Scripts can take time. Never retain a database lock
        // while executing; cancellation after this admission point cannot undo
        // an external action which has already started.
        transaction.commit().map_err(|error| error.to_string())?;
        automation
    };
    let result = run_automation_executor(state, &automation)?;
    let notification = notification_for_automation(&automation);
    persist_automation_run(state, &automation, result, now, Some(&claim.token))?;
    Ok(Some(ScheduledAutomationExecution { notification }))
}

#[tauri::command]
pub fn run_due_automations(state: State<AppState>, app_handle: AppHandle) -> Result<usize, String> {
    let (count, notifications) =
        run_due_automations_with_notifications_impl(&state, Utc::now().timestamp())?;
    for notification in notifications {
        crate::commands::settings::emit_notification_channel_impl(
            &state,
            &app_handle,
            "reminder".to_string(),
            notification.title,
            notification.body,
        )?;
    }
    // Also wakes previously queued work when this heartbeat finds no new due
    // definitions (for example, a foreground turn was busy on the prior tick).
    state.foreground_automation_pump.wake(app_handle.clone());
    Ok(count)
}

fn enqueue_agent_automation_in_tx(
    transaction: &Transaction<'_>,
    automation: &Automation,
    now: i64,
) -> Result<crate::agent::supervision::SupervisorInput, String> {
    let workspace_id = automation
        .workspace_id
        .as_deref()
        .ok_or_else(|| "Agent 自动化缺少归属工作区".to_string())?;
    let content = format!(
        "这是用户创建的定时自动化「{}」，现在触发。请按当前工作区的上下文与权限处理：\n{}",
        automation.title, automation.prompt,
    );
    WorkspaceSupervisor::submit_in_tx(
        transaction,
        workspace_id,
        SupervisorInputKind::FollowUp,
        &content,
        now,
    )
    .map_err(|error| error.to_string())
}

fn pause_unsupported_agent_dispatch_schedule_in_tx(
    transaction: &Transaction<'_>,
    automation_id: &str,
    now: i64,
) -> Result<bool, String> {
    let automation = transaction
        .query_row(
            "SELECT id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,
                    executor_kind,script_path,script_args,working_dir,timeout_seconds,
                    next_run_at,last_run_at,workspace_id
             FROM automations WHERE id=?1",
            [automation_id],
            automation_from_row,
        )
        .map_err(|error| error.to_string())?;
    pause_unsupported_schedule_in_tx(transaction, &automation, now, None)
}

/// Reserve a stable Main-Agent reply id for one queued automation input. The
/// id is stored before a model call so recovery can tell a never-started turn
/// from one that must not be replayed. `foreground_run_id` is intentionally a
/// soft reference: the task-run row is created by the foreground turn itself.
pub(crate) fn prepare_agent_automation_dispatch(
    state: &AppState,
    supervisor_input_id: &str,
) -> Result<AgentAutomationDispatchPreparation, String> {
    let mut conn = state.db.lock().map_err(|error| error.to_string())?;
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let input_status = transaction
        .query_row(
            "SELECT status FROM workspace_supervisor_inputs WHERE id = ?1",
            params![supervisor_input_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let run = transaction
        .query_row(
            "SELECT run.id, run.automation_id, run.foreground_run_id,
                    run.status, automation.enabled, automation.trigger_kind,
                    EXISTS(SELECT 1 FROM task_runs task WHERE task.id = run.foreground_run_id)
             FROM automation_runs run
             JOIN automations automation ON automation.id = run.automation_id
             WHERE run.supervisor_input_id = ?1 AND automation.executor_kind = 'agent'
             ORDER BY run.started_at DESC, run.id DESC
             LIMIT 1",
            params![supervisor_input_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, bool>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((
        automation_run_id,
        automation_id,
        stored_foreground_run_id,
        run_status,
        enabled,
        trigger_kind,
        task_exists,
    )) = run
    else {
        transaction.commit().map_err(|error| error.to_string())?;
        return Ok(if input_status.as_deref() == Some("cancelled") {
            AgentAutomationDispatchPreparation::Cancelled
        } else {
            AgentAutomationDispatchPreparation::Unowned
        });
    };
    if input_status.as_deref() != Some("claimed")
        || run_status == "cancelled"
        || !agent_automation_run_is_admissible(enabled, &trigger_kind)
    {
        if !agent_automation_run_is_admissible(enabled, &trigger_kind) {
            cancel_unstarted_agent_automation_dispatches_in_tx(
                &transaction,
                &automation_id,
                Utc::now().timestamp(),
                "自动化已暂停；不会启动主 Agent",
            )?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        return Ok(AgentAutomationDispatchPreparation::Cancelled);
    }
    // Upgrade-era queued deliveries may predate the schedule contract and have
    // a future next_run_at. Validate here too, without touching started work.
    if !task_exists
        && pause_unsupported_agent_dispatch_schedule_in_tx(
            &transaction,
            &automation_id,
            Utc::now().timestamp(),
        )?
    {
        transaction.commit().map_err(|error| error.to_string())?;
        return Ok(AgentAutomationDispatchPreparation::Cancelled);
    }
    let foreground_run_id = stored_foreground_run_id
        .unwrap_or_else(|| format!("automation_turn_{}", Uuid::new_v4().simple()));
    transaction
        .execute(
            "UPDATE automation_runs
             SET foreground_run_id = ?1, status = 'running',
                 summary = '主 Agent 正在处理此自动化任务', finished_at = NULL
             WHERE id = ?2",
            params![foreground_run_id, automation_run_id],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(AgentAutomationDispatchPreparation::Dispatch(
        AgentAutomationDispatch {
            automation_run_id,
            foreground_run_id,
            supervisor_input_id: supervisor_input_id.to_string(),
        },
    ))
}

/// Confirm that the claimed delivery is still eligible inside the foreground
/// reservation transaction. The claim token prevents a stale pump from
/// starting a newly reclaimed input. A one-time automation may remain
/// deliverable after its scheduler disabled future triggers; an explicit pause
/// always cancels its unstarted input above.
pub(crate) fn recheck_agent_automation_dispatch_before_begin_in_tx(
    transaction: &Transaction<'_>,
    dispatch: &AgentAutomationDispatch,
    claim_token: &str,
    now: i64,
) -> Result<AgentAutomationDispatchAdmission, String> {
    let linked = transaction
        .query_row(
            "SELECT run.automation_id, run.status, input.status, input.claim_token,
                    automation.enabled, automation.trigger_kind,
                    EXISTS(SELECT 1 FROM task_runs task WHERE task.id = run.foreground_run_id)
             FROM automation_runs run
             JOIN workspace_supervisor_inputs input ON input.id = run.supervisor_input_id
             JOIN automations automation ON automation.id = run.automation_id
             WHERE run.id = ?1 AND run.foreground_run_id = ?2
               AND input.id = ?3 AND automation.executor_kind = 'agent'",
            params![
                dispatch.automation_run_id,
                dispatch.foreground_run_id,
                dispatch.supervisor_input_id,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, bool>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((
        automation_id,
        run_status,
        input_status,
        stored_claim_token,
        enabled,
        trigger_kind,
        task_exists,
    )) = linked
    else {
        return Ok(AgentAutomationDispatchAdmission::Cancelled);
    };
    if task_exists {
        return Ok(AgentAutomationDispatchAdmission::AlreadyStarted);
    }
    if run_status == "cancelled"
        || input_status != "claimed"
        || stored_claim_token.as_deref() != Some(claim_token)
    {
        return Ok(AgentAutomationDispatchAdmission::Cancelled);
    }
    if !agent_automation_run_is_admissible(enabled, &trigger_kind) {
        cancel_unstarted_agent_automation_dispatches_in_tx(
            transaction,
            &automation_id,
            now,
            "自动化已暂停；不会启动主 Agent",
        )?;
        return Ok(AgentAutomationDispatchAdmission::Cancelled);
    }
    if pause_unsupported_agent_dispatch_schedule_in_tx(transaction, &automation_id, now)? {
        return Ok(AgentAutomationDispatchAdmission::Cancelled);
    }
    Ok(AgentAutomationDispatchAdmission::Start)
}

fn agent_automation_run_is_admissible(enabled: bool, trigger_kind: &str) -> bool {
    // `persist_automation_run` disables a successful one-time trigger after it
    // has already queued its single delivery.  That is not an explicit user
    // pause, so the durable queued input remains eligible until it is consumed.
    enabled || trigger_kind == "once"
}

pub(crate) fn foreground_task_status(
    state: &AppState,
    foreground_run_id: &str,
) -> Result<Option<String>, String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    conn.query_row(
        "SELECT status FROM task_runs WHERE id = ?1",
        params![foreground_run_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|error| error.to_string())
}

/// Reflect a persisted foreground outcome in the automation list. Queue
/// settlement is separate: this function never replays or mutates the
/// Main-Agent turn itself.
pub(crate) fn sync_agent_automation_dispatch(
    state: &AppState,
    dispatch: &AgentAutomationDispatch,
    now: i64,
) -> Result<Option<String>, String> {
    let Some(task_status) = foreground_task_status(state, &dispatch.foreground_run_id)? else {
        return Ok(None);
    };
    let Some((status, summary, terminal)) = agent_automation_outcome(&task_status) else {
        return Ok(Some(task_status));
    };
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    conn.execute(
        "UPDATE automation_runs
         SET status = ?1, summary = ?2,
             finished_at = CASE WHEN ?3 THEN ?4 ELSE finished_at END
         WHERE id = ?5 AND foreground_run_id = ?6",
        params![
            status,
            summary,
            terminal,
            now,
            dispatch.automation_run_id,
            dispatch.foreground_run_id,
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(Some(status.to_string()))
}

/// Restore an automation record to a safe queued state when admission failed
/// before `ForegroundRunStore::begin` created the task-run row. The stable
/// reply id remains reserved for the retry; no existing Main-Agent work is
/// ever overwritten.
pub(crate) fn reset_agent_automation_dispatch_to_queued(
    state: &AppState,
    dispatch: &AgentAutomationDispatch,
    now: i64,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    conn.execute(
        "UPDATE automation_runs
         SET status = 'queued', summary = '等待主 Agent 空闲后处理', finished_at = ?1
         WHERE id = ?2 AND foreground_run_id = ?3",
        params![now, dispatch.automation_run_id, dispatch.foreground_run_id],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// A failure before a foreground turn was reserved has no side effects to
/// recover, but retrying a permanent configuration problem every heartbeat is
/// noisy and surprising. Record a clear user-actionable state instead.
pub(crate) fn mark_agent_automation_dispatch_needs_attention(
    state: &AppState,
    dispatch: &AgentAutomationDispatch,
    now: i64,
    _error: &str,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    conn.execute(
        "UPDATE automation_runs
         SET status = 'needs_attention',
             summary = '无法启动主 Agent；请检查模型配置后重新运行此任务',
             finished_at = ?1
         WHERE id = ?2 AND foreground_run_id = ?3",
        params![now, dispatch.automation_run_id, dispatch.foreground_run_id],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// Reconcile dispatch records after foreground-run startup recovery. A linked
/// task row means the prompt crossed the non-replay boundary and the queue can
/// be settled. A reserved id without a task row never reached that boundary,
/// so it is safely made eligible again.
pub(crate) fn recover_interrupted_agent_automation_dispatches_in_conn(
    conn: &mut Connection,
    now: i64,
) -> Result<usize, String> {
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let linked = {
        let mut statement = transaction
            .prepare(
                "SELECT input.id, run.id, run.foreground_run_id, task.status
                 FROM workspace_supervisor_inputs input
                 JOIN automation_runs run ON run.supervisor_input_id = input.id
                 JOIN task_runs task ON task.id = run.foreground_run_id
                 WHERE input.kind = 'follow_up' AND input.status = 'claimed'",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        rows
    };
    let mut reconciled = 0;
    for (input_id, automation_run_id, foreground_run_id, task_status) in linked {
        let Some((status, summary, terminal)) = agent_automation_outcome(&task_status) else {
            continue;
        };
        transaction
            .execute(
                "UPDATE automation_runs
                 SET status = ?1, summary = ?2,
                     finished_at = CASE WHEN ?3 THEN ?4 ELSE finished_at END
                 WHERE id = ?5 AND foreground_run_id = ?6",
                params![
                    status,
                    summary,
                    terminal,
                    now,
                    automation_run_id,
                    foreground_run_id,
                ],
            )
            .map_err(|error| error.to_string())?;
        if terminal {
            transaction
                .execute(
                    "UPDATE workspace_supervisor_inputs
                     SET status = 'completed', updated_at = ?1
                     WHERE id = ?2 AND status = 'claimed'",
                    params![now, input_id],
                )
                .map_err(|error| error.to_string())?;
        }
        reconciled += 1;
    }
    let requeued = transaction
        .execute(
            "UPDATE workspace_supervisor_inputs
             SET status = 'queued', claim_token = NULL, updated_at = ?1
             WHERE status = 'claimed' AND kind = 'follow_up'
               AND EXISTS (
                 SELECT 1 FROM automation_runs run
                 WHERE run.supervisor_input_id = workspace_supervisor_inputs.id
                   AND (run.foreground_run_id IS NULL OR NOT EXISTS (
                     SELECT 1 FROM task_runs task WHERE task.id = run.foreground_run_id
                   ))
               )",
            params![now],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE automation_runs
             SET status = 'queued', summary = '等待主 Agent 处理', finished_at = ?1
             WHERE supervisor_input_id IN (
               SELECT id FROM workspace_supervisor_inputs
               WHERE status = 'queued' AND kind = 'follow_up'
             )
               AND (foreground_run_id IS NULL OR NOT EXISTS (
                 SELECT 1 FROM task_runs task WHERE task.id = automation_runs.foreground_run_id
               ))",
            params![now],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(reconciled + requeued)
}

fn agent_automation_outcome(task_status: &str) -> Option<(&'static str, &'static str, bool)> {
    match task_status {
        "running" => Some(("running", "主 Agent 正在处理此自动化任务", false)),
        "completed" => Some(("completed", "主 Agent 已在归属工作区完成此任务", true)),
        "awaiting_confirmation" => Some((
            "awaiting_confirmation",
            "主 Agent 需要你在归属工作区对话中确认下一步操作",
            true,
        )),
        "needs_attention" | "continue_suggested" | "provider_unavailable" | "stopped" => Some((
            "needs_attention",
            "主 Agent 需要你在归属工作区对话中继续处理",
            true,
        )),
        _ => None,
    }
}

#[tauri::command]
pub fn get_automation_runs(
    state: State<AppState>,
    automation_id: String,
) -> Result<Vec<AutomationRun>, String> {
    get_automation_runs_impl(&state, &automation_id)
}
pub fn get_automation_runs_impl(
    state: &AppState,
    automation_id: &str,
) -> Result<Vec<AutomationRun>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT id,status,summary,exit_code,output,started_at FROM automation_runs WHERE automation_id=?1 ORDER BY started_at DESC LIMIT 20").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![automation_id], |r| {
            Ok(AutomationRun {
                id: r.get(0)?,
                status: r.get(1)?,
                summary: r.get(2)?,
                exit_code: r.get(3)?,
                output: r.get(4)?,
                started_at: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    use crate::agent::shared_db::SharedDb;
    use crate::db::migrate;

    #[test]
    fn script_args_must_be_strings() {
        assert_eq!(
            parse_script_args("[\"--dry-run\", \"today\"]").unwrap(),
            vec!["--dry-run", "today"]
        );
        assert!(parse_script_args("[1]").is_err());
    }

    #[test]
    fn timeout_stays_bounded() {
        assert_eq!(normalize_timeout(None).unwrap(), DEFAULT_TIMEOUT_SECONDS);
        assert!(normalize_timeout(Some(2)).is_err());
        assert!(normalize_timeout(Some(3601)).is_err());
    }

    #[test]
    fn enabled_automation_requires_a_supported_parseable_schedule() {
        assert!(validated_next_run_at("schedule", "每天 09:00", 1_700_000_000).is_ok());
        assert!(validated_next_run_at("schedule", "when convenient", 1_700_000_000).is_err());
        assert!(validated_next_run_at("event", "每天 09:00", 1_700_000_000).is_err());
    }

    #[test]
    fn automation_contract_regression_rejects_non_daily_frequency() {
        let now = 1_700_000_000;
        for value in [
            "工作日 09:00",
            "每周一 09:00",
            "明天 09:00",
            "每月一日 09:00",
            "每天 09:00 或 10:00",
            "09:00 tomorrow",
            "0 9 * * 1",
        ] {
            assert!(
                validated_next_run_at("schedule", value, now).is_err(),
                "unsupported frequency must not silently run every day: {value}"
            );
        }
        for value in ["09:00", "9:00", "每天 09:00", "每天09:00", " 每天 09:00 "] {
            assert!(
                validated_next_run_at("schedule", value, now).is_ok(),
                "{value}"
            );
        }
    }

    #[test]
    fn automation_contract_regression_agent_receipt_failure_leaves_no_orphan_input() {
        for scheduled in [false, true] {
            for failure in [
                "CREATE TRIGGER reject_receipt BEFORE INSERT ON automation_runs
                 BEGIN SELECT RAISE(ABORT,'test receipt failure'); END;",
                "CREATE TRIGGER reject_schedule_advance BEFORE UPDATE OF last_run_at ON automations
                 BEGIN SELECT RAISE(ABORT,'test schedule advance failure'); END;",
            ] {
                let state = crate::tests::test_app_state();
                {
                    let connection = state.db.lock().unwrap();
                    migrate(&connection).unwrap();
                    connection.execute_batch(
                        "INSERT INTO projects (id,name,path,created_at,kind,updated_at)
                             VALUES ('project-a','Project A','C:/project-a',1,'project',1);
                         INSERT INTO automations (
                             id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,
                             executor_kind,workspace_id,script_args,timeout_seconds,next_run_at,created_at,updated_at
                         ) VALUES (
                             'agent-due','Summarize notes','Summarize the local notes','schedule','每天 09:00',1,'',
                             'agent','project-a','[]',300,10,1,1
                         );",
                    ).unwrap();
                    connection.execute_batch(failure).unwrap();
                }
                let outcome = if scheduled {
                    run_due_automations_impl(&state, 15).map(|_| ())
                } else {
                    run_automation_now_impl(&state, "agent-due")
                };
                assert!(outcome.is_err());
                let connection = state.db.lock().unwrap();
                let inputs: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM workspace_supervisor_inputs WHERE workspace_id='project-a'",
                    [], |row| row.get(0),
                ).unwrap();
                assert_eq!(
                    inputs, 0,
                    "failed receipt must not strand an unowned follow-up; scheduled={scheduled}"
                );
                let runs: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM automation_runs WHERE automation_id='agent-due'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(runs, 0);
                let (next_run, last_run): (i64, Option<i64>) = connection
                    .query_row(
                        "SELECT next_run_at,last_run_at FROM automations WHERE id='agent-due'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .unwrap();
                assert_eq!(next_run, 10);
                assert_eq!(last_run, None);
                connection
                    .execute_batch(
                        "DROP TRIGGER IF EXISTS reject_receipt;
                     DROP TRIGGER IF EXISTS reject_schedule_advance;",
                    )
                    .unwrap();
                drop(connection);
                if scheduled {
                    assert_eq!(
                        run_due_automations_impl(&state, 15 + AUTOMATION_CLAIM_TTL_SECONDS + 1)
                            .unwrap(),
                        1
                    );
                } else {
                    run_automation_now_impl(&state, "agent-due").unwrap();
                }
                let connection = state.db.lock().unwrap();
                let owned_inputs: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM workspace_supervisor_inputs input
                     JOIN automation_runs run ON run.supervisor_input_id=input.id
                     WHERE run.automation_id='agent-due' AND run.status='queued'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(
                    owned_inputs, 1,
                    "a successful retry exposes the queue input and owner together"
                );
                let (enabled, next_run): (bool, Option<i64>) = connection
                    .query_row(
                        "SELECT enabled,next_run_at FROM automations WHERE id='agent-due'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .unwrap();
                assert!(enabled, "a valid daily definition must not be paused");
                assert!(next_run.is_some_and(|value| value > 10));
            }
        }
    }

    #[test]
    fn automation_contract_regression_unsupported_legacy_schedule_never_enqueues_agent() {
        for scheduled in [false, true] {
            let state = crate::tests::test_app_state();
            {
                let connection = state.db.lock().unwrap();
                migrate(&connection).unwrap();
                connection.execute_batch(
                    "INSERT INTO projects (id,name,path,created_at,kind,updated_at)
                         VALUES ('project-a','Project A','C:/project-a',1,'project',1);
                     INSERT INTO automations (
                         id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,
                         executor_kind,workspace_id,script_args,timeout_seconds,next_run_at,created_at,updated_at
                     ) VALUES (
                         'legacy-weekly','Summarize notes','Summarize the local notes','schedule','每周一 09:00',1,'',
                         'agent','project-a','[]',300,10,1,1
                     );",
                ).unwrap();
            }
            if scheduled {
                run_due_automations_impl(&state, 15).unwrap();
            } else {
                assert!(run_automation_now_impl(&state, "legacy-weekly").is_err());
            }
            let connection = state.db.lock().unwrap();
            let (value, enabled, next_run): (String, bool, Option<i64>) = connection.query_row(
                "SELECT trigger_value,enabled,next_run_at FROM automations WHERE id='legacy-weekly'",
                [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).unwrap();
            assert_eq!(
                value, "每周一 09:00",
                "preserve the user's definition for correction"
            );
            assert!(!enabled);
            assert_eq!(next_run, None);
            let status: String = connection
                .query_row(
                    "SELECT status FROM automation_runs WHERE automation_id='legacy-weekly'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(status, "needs_attention");
            let inputs: i64 = connection.query_row(
                "SELECT COUNT(*) FROM workspace_supervisor_inputs WHERE workspace_id='project-a'",
                [], |row| row.get(0),
            ).unwrap();
            assert_eq!(
                inputs, 0,
                "invalid schedule must not cross the agent executor boundary"
            );
        }
    }

    #[test]
    fn one_time_trigger_requires_a_future_offset_aware_timestamp() {
        assert_eq!(
            validated_next_run_at("once", "2026-09-19T09:00:00+08:00", 1_700_000_000).unwrap(),
            1_789_779_600
        );
        assert!(validated_next_run_at("once", "2020-01-01T09:00:00+08:00", 1_700_000_000).is_err());
        assert!(validated_next_run_at("once", "tomorrow morning", 1_700_000_000).is_err());
    }

    #[test]
    fn daily_schedule_uses_local_wall_clock_before_today_trigger() {
        let timezone = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-03T08:30:00+08:00")
            .unwrap()
            .timestamp();
        let expected = chrono::DateTime::parse_from_rfc3339("2026-10-03T09:00:00+08:00")
            .unwrap()
            .timestamp();

        assert_eq!(
            next_run_at_in_timezone("schedule", "每天 09:00", now, &timezone),
            Some(expected),
            "每天09:00 must mean local 09:00, not UTC 09:00 (local 17:00)"
        );
    }

    fn test_timestamp(value: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(value)
            .unwrap()
            .timestamp()
    }

    #[test]
    fn daily_schedule_rolls_over_by_local_date_not_utc_date() {
        let timezone = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        for (now, expected) in [
            // Local today is already October 3, while UTC is still October 2.
            ("2026-10-03T00:30:00+08:00", "2026-10-03T09:00:00+08:00"),
            ("2026-10-03T09:00:00+08:00", "2026-10-04T09:00:00+08:00"),
            ("2026-10-03T10:00:00+08:00", "2026-10-04T09:00:00+08:00"),
        ] {
            assert_eq!(
                next_run_at_in_timezone("schedule", "每天 09:00", test_timestamp(now), &timezone),
                Some(test_timestamp(expected)),
                "local rollover from {now}"
            );
        }
    }

    #[test]
    fn daily_schedule_retains_utc_wall_clock_on_utc_systems() {
        for (now, expected) in [
            ("2026-10-03T08:30:00Z", "2026-10-03T09:00:00Z"),
            ("2026-10-03T10:00:00Z", "2026-10-04T09:00:00Z"),
        ] {
            assert_eq!(
                next_run_at_in_timezone("schedule", "每天 09:00", test_timestamp(now), &Utc),
                Some(test_timestamp(expected))
            );
        }
    }

    #[test]
    fn one_time_explicit_offset_is_independent_of_system_timezone() {
        let timezone = chrono::FixedOffset::west_opt(5 * 3600).unwrap();
        let now = test_timestamp("2026-10-03T00:00:00Z");
        let expected = Some(test_timestamp("2026-10-03T01:00:00Z"));
        assert_eq!(
            next_run_at_in_timezone("once", "2026-10-03T09:00:00+08:00", now, &timezone),
            expected
        );
        assert_eq!(
            next_run_at_in_timezone("once", "2026-10-03T09:00:00+08:00", now, &Utc),
            expected
        );
    }

    // Deterministic external timezone boundary: EU-style 2026 spring/fall
    // transitions, without changing the process timezone or adding a tz database.
    #[derive(Clone)]
    struct TestTransitionTimezone;

    impl TimeZone for TestTransitionTimezone {
        type Offset = chrono::FixedOffset;

        fn from_offset(_: &Self::Offset) -> Self {
            Self
        }

        fn offset_from_local_date(
            &self,
            local: &chrono::NaiveDate,
        ) -> chrono::LocalResult<Self::Offset> {
            self.offset_from_local_datetime(&local.and_hms_opt(0, 0, 0).unwrap())
        }

        fn offset_from_local_datetime(
            &self,
            local: &chrono::NaiveDateTime,
        ) -> chrono::LocalResult<Self::Offset> {
            let spring = chrono::NaiveDate::from_ymd_opt(2026, 3, 29).unwrap();
            let fall = chrono::NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
            let two = chrono::NaiveTime::from_hms_opt(2, 0, 0).unwrap();
            let three = chrono::NaiveTime::from_hms_opt(3, 0, 0).unwrap();
            let standard = chrono::FixedOffset::east_opt(3600).unwrap();
            let summer = chrono::FixedOffset::east_opt(2 * 3600).unwrap();
            if local.time() >= two && local.time() < three {
                if local.date() == spring {
                    return chrono::LocalResult::None;
                }
                if local.date() == fall {
                    return chrono::LocalResult::Ambiguous(summer, standard);
                }
            }
            chrono::LocalResult::Single(
                if *local >= spring.and_time(three) && *local < fall.and_time(three) {
                    summer
                } else {
                    standard
                },
            )
        }

        fn offset_from_utc_date(&self, utc: &chrono::NaiveDate) -> Self::Offset {
            self.offset_from_utc_datetime(&utc.and_hms_opt(0, 0, 0).unwrap())
        }

        fn offset_from_utc_datetime(&self, utc: &chrono::NaiveDateTime) -> Self::Offset {
            let timestamp = utc.and_utc().timestamp();
            let summer = timestamp >= test_timestamp("2026-03-29T01:00:00Z")
                && timestamp < test_timestamp("2026-10-25T01:00:00Z");
            chrono::FixedOffset::east_opt(if summer { 2 * 3600 } else { 3600 }).unwrap()
        }
    }

    #[test]
    fn daily_schedule_skips_nonexistent_wall_clock_slots() {
        for now in ["2026-03-28T03:00:00+01:00", "2026-03-29T01:00:00+01:00"] {
            assert_eq!(
                next_run_at_in_timezone(
                    "schedule",
                    "每天 02:30",
                    test_timestamp(now),
                    &TestTransitionTimezone
                ),
                Some(test_timestamp("2026-03-30T02:30:00+02:00"))
            );
        }
    }

    #[test]
    fn daily_schedule_uses_first_ambiguous_slot_only_once_per_local_day() {
        assert_eq!(
            next_run_at_in_timezone(
                "schedule",
                "每天 02:30",
                test_timestamp("2026-10-25T01:00:00+02:00"),
                &TestTransitionTimezone,
            ),
            Some(test_timestamp("2026-10-25T02:30:00+02:00"))
        );
        // The second 02:30 is still in the future, but must not run the same
        // civil-day slot again after its first occurrence has already passed.
        assert_eq!(
            next_run_at_in_timezone(
                "schedule",
                "每天 02:30",
                test_timestamp("2026-10-25T02:45:00+02:00"),
                &TestTransitionTimezone,
            ),
            Some(test_timestamp("2026-10-26T02:30:00+01:00"))
        );
    }

    #[test]
    fn agent_automation_is_enqueued_in_its_workspace() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO projects (id,name,path,created_at,kind,updated_at) VALUES ('project-a','Project A','C:/project-a',1,'project',1)",
                [],
            )
            .unwrap();
        let automation = Automation {
            id: "automation-a".into(),
            title: "整理笔记".into(),
            prompt: "整理今天的笔记并给出摘要".into(),
            trigger_kind: "schedule".into(),
            trigger_value: "每天 21:00".into(),
            enabled: true,
            permission_summary: "每次运行前询问".into(),
            executor_kind: "agent".into(),
            workspace_id: Some("project-a".into()),
            script_path: None,
            script_args: vec![],
            working_dir: None,
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            next_run_at: None,
            last_run_at: None,
        };

        let transaction = connection.transaction().unwrap();
        enqueue_agent_automation_in_tx(&transaction, &automation, 42).unwrap();
        transaction.commit().unwrap();

        let input = WorkspaceSupervisor::new(SharedDb::new(connection))
            .claim_next("project-a", 43)
            .unwrap()
            .unwrap()
            .input;
        assert_eq!(input.kind, SupervisorInputKind::FollowUp);
        assert_eq!(input.workspace_id, "project-a");
        assert!(input.content.contains("整理今天的笔记并给出摘要"));
    }

    #[test]
    fn queued_agent_automation_survives_database_reopen() {
        let database = tempfile::NamedTempFile::new().unwrap();
        let database_path = database.path().to_path_buf();
        let mut connection = Connection::open(&database_path).unwrap();
        migrate(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO projects (id,name,path,created_at,kind,updated_at) VALUES ('project-a','Project A','C:/project-a',1,'project',1)",
                [],
            )
            .unwrap();
        let automation = Automation {
            id: "automation-a".into(),
            title: "整理笔记".into(),
            prompt: "整理今天的笔记并给出摘要".into(),
            trigger_kind: "schedule".into(),
            trigger_value: "每天 21:00".into(),
            enabled: true,
            permission_summary: "每次运行前询问".into(),
            executor_kind: "agent".into(),
            workspace_id: Some("project-a".into()),
            script_path: None,
            script_args: vec![],
            working_dir: None,
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            next_run_at: None,
            last_run_at: None,
        };

        let transaction = connection.transaction().unwrap();
        enqueue_agent_automation_in_tx(&transaction, &automation, 42).unwrap();
        transaction.commit().unwrap();
        drop(connection);

        let reopened = Connection::open(database_path).unwrap();
        migrate(&reopened).unwrap();
        let input = WorkspaceSupervisor::new(SharedDb::new(reopened))
            .claim_next("project-a", 43)
            .unwrap()
            .unwrap()
            .input;
        assert_eq!(input.kind, SupervisorInputKind::FollowUp);
        assert_eq!(input.workspace_id, "project-a");
        assert!(input.content.contains("整理今天的笔记并给出摘要"));
    }

    #[test]
    fn startup_recovery_requeues_only_unstarted_agent_automations() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        connection.execute_batch(
            "
            INSERT INTO sessions (id, title, created_at, updated_at) VALUES ('session-a', 'Main', 1, 1);
            INSERT INTO projects (id, name, path, created_at, kind, active_session_id, updated_at)
                VALUES ('project-a', 'Project A', 'C:/project-a', 1, 'project', 'session-a', 1);
            INSERT INTO automations (id, title, prompt, trigger_kind, trigger_value, enabled, permission_summary, executor_kind, script_args, timeout_seconds, created_at, updated_at)
                VALUES ('automation-a', '整理笔记', '整理今天的笔记', 'manual', '', 1, '', 'agent', '[]', 300, 1, 1);
            INSERT INTO workspace_supervisor_inputs (id, workspace_id, kind, content, priority, status, claim_token, created_at, updated_at)
                VALUES
                    ('input-unstarted', 'project-a', 'follow_up', 'unstarted', 100, 'claimed', 'claim-1', 1, 1),
                    ('input-completed', 'project-a', 'follow_up', 'completed', 100, 'claimed', 'claim-2', 1, 1);
            INSERT INTO automation_runs (id, automation_id, status, summary, output, started_at, finished_at, supervisor_input_id, foreground_run_id)
                VALUES
                    ('run-unstarted', 'automation-a', 'running', '', '', 1, NULL, 'input-unstarted', 'turn-unstarted'),
                    ('run-completed', 'automation-a', 'running', '', '', 1, NULL, 'input-completed', 'turn-completed');
            INSERT INTO messages (id, session_id, role, content, created_at)
                VALUES ('turn-completed', 'session-a', 'assistant', 'done', 1);
            INSERT INTO task_runs (id, session_id, message_id, goal, status, plan, created_at, updated_at)
                VALUES ('turn-completed', 'session-a', 'turn-completed', 'completed work', 'completed', '[]', 1, 1);
            ",
        ).unwrap();

        assert_eq!(
            recover_interrupted_agent_automation_dispatches_in_conn(&mut connection, 10).unwrap(),
            2
        );
        let states: Vec<(String, String, String)> = connection
            .prepare(
                "SELECT input.id, input.status, run.status
                 FROM workspace_supervisor_inputs input
                 JOIN automation_runs run ON run.supervisor_input_id = input.id
                 ORDER BY input.id",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            states,
            vec![
                (
                    "input-completed".into(),
                    "completed".into(),
                    "completed".into()
                ),
                ("input-unstarted".into(), "queued".into(), "queued".into()),
            ]
        );
    }

    #[test]
    fn agent_automation_without_workspace_is_rejected() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        let automation = Automation {
            id: "automation-a".into(),
            title: "整理笔记".into(),
            prompt: "整理今天的笔记".into(),
            trigger_kind: "schedule".into(),
            trigger_value: "每天 21:00".into(),
            enabled: true,
            permission_summary: "每次运行前询问".into(),
            executor_kind: "agent".into(),
            workspace_id: None,
            script_path: None,
            script_args: vec![],
            working_dir: None,
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            next_run_at: None,
            last_run_at: None,
        };

        let transaction = connection.transaction().unwrap();
        assert!(enqueue_agent_automation_in_tx(&transaction, &automation, 42).is_err());
    }

    #[test]
    fn due_scan_claims_only_enabled_scheduled_automations_once() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        for (id, enabled, trigger_kind, next_run_at) in [
            ("earlier", 1, "schedule", Some(10)),
            ("reminder", 1, "once", Some(12)),
            ("later", 1, "schedule", Some(20)),
            ("paused", 0, "schedule", Some(5)),
            ("manual", 1, "manual", Some(5)),
            ("unscheduled", 1, "schedule", None),
        ] {
            connection.execute(
                "INSERT INTO automations (id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,executor_kind,script_args,timeout_seconds,next_run_at,created_at,updated_at) VALUES (?1,?1,'',?2,'',?3,'','script','[]',300,?4,1,1)",
                params![id, trigger_kind, enabled, next_run_at],
            ).unwrap();
        }

        let claims = claim_due_automations(&mut connection, 15).unwrap();
        assert_eq!(claims.len(), 2);
        assert_eq!(claims[0].id, "earlier");
        assert_eq!(claims[1].id, "reminder");
        assert!(claims[0].token.starts_with("automation_"));
        assert!(claim_due_automations(&mut connection, 15)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn cancelled_claimed_reminder_does_not_fire_or_record_a_completed_run() {
        let state = crate::tests::test_app_state();
        let (claim, session_id) = {
            let mut connection = state.db.lock().unwrap();
            migrate(&connection).unwrap();
            let (workspace_id, session_id): (String, String) = connection
                .query_row(
                    "SELECT id,active_session_id FROM projects WHERE kind='personal'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            connection.execute(
                "INSERT INTO automations (id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,executor_kind,workspace_id,script_args,timeout_seconds,next_run_at,created_at,updated_at) VALUES ('reminder','Review draft','Check the draft','once','1970-01-01T00:00:10Z',1,'','notification',?1,'[]',300,10,1,1)",
                [workspace_id],
            ).unwrap();
            let claim = claim_due_automations(&mut connection, 15)
                .unwrap()
                .remove(0);
            (claim, session_id)
        };
        let mut registry = crate::agent::ToolRegistry::new();
        super::super::foreground_automation_control::register(
            &mut registry,
            session_id,
            state.db.clone(),
        );
        let cancelled = registry.execute(
            &crate::agent::tool::ToolCall::new(
                "cancel_reminder".into(),
                serde_json::json!({ "reminder_id": claim.id }),
            ),
            "cancel-reminder",
            Path::new("."),
        );
        assert!(cancelled.result.success, "{}", cancelled.result.content);

        assert!(
            run_claimed_automation_impl(&state, &claim, 17)
                .unwrap()
                .is_none(),
            "a cancelled reminder must not reach the notification channel"
        );
        let connection = state.db.lock().unwrap();
        let completed: i64 = connection
            .query_row("SELECT COUNT(*) FROM automation_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            completed, 0,
            "cancellation must not produce a success receipt"
        );
        let enabled: bool = connection
            .query_row(
                "SELECT enabled FROM automations WHERE id=?1",
                [&claim.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!enabled);
    }

    #[test]
    fn stale_schedule_claim_is_recovered_after_the_execution_bound() {
        let state = crate::tests::test_app_state();
        let recovered_at = 15 + AUTOMATION_CLAIM_TTL_SECONDS + 1;
        let (first, recovered) = {
            let mut connection = state.db.lock().unwrap();
            migrate(&connection).unwrap();
            connection.execute(
                "INSERT INTO automations (id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,executor_kind,script_args,timeout_seconds,next_run_at,created_at,updated_at) VALUES ('due','Due','','once','1970-01-01T00:00:10Z',1,'','notification','[]',300,10,1,1)",
                [],
            ).unwrap();
            let mut first = claim_due_automations(&mut connection, 15).unwrap();
            assert_eq!(first.len(), 1);
            let recovered = claim_due_automations(&mut connection, recovered_at).unwrap();
            assert_eq!(recovered.len(), 1);
            assert_eq!(recovered[0].id, "due");
            assert_ne!(recovered[0].token, first[0].token);
            (first.remove(0), recovered.into_iter().next().unwrap())
        };

        assert!(run_claimed_automation_impl(&state, &first, recovered_at)
            .unwrap()
            .is_none());
        let execution = run_claimed_automation_impl(&state, &recovered, recovered_at)
            .unwrap()
            .unwrap();
        assert_eq!(execution.notification.unwrap().title, "Due");
        assert!(
            run_claimed_automation_impl(&state, &recovered, recovered_at)
                .unwrap()
                .is_none(),
            "a completed claim cannot be replayed"
        );
        assert_eq!(
            run_due_automations_impl(&state, recovered_at + 1).unwrap(),
            0
        );
        let completed: i64 = state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM automation_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(completed, 1);
    }

    #[test]
    fn invalidated_claims_survive_reopen_without_blocking_other_reminders() {
        for invalidation in [
            "DELETE FROM automations WHERE id='obsolete'",
            "UPDATE automations SET next_run_at=100 WHERE id='obsolete'",
            "UPDATE automations SET next_run_at=NULL WHERE id='obsolete'",
            "UPDATE automations SET trigger_kind='manual' WHERE id='obsolete'",
            "UPDATE automations SET enabled=0 WHERE id='obsolete'",
        ] {
            let database = tempfile::NamedTempFile::new().unwrap();
            let mut connection = Connection::open(database.path()).unwrap();
            migrate(&connection).unwrap();
            for (id, due_at) in [("obsolete", 10), ("valid", 11)] {
                connection.execute(
                    "INSERT INTO automations (id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,executor_kind,script_args,timeout_seconds,next_run_at,created_at,updated_at) VALUES (?1,?1,'','once','1970-01-01T00:00:10Z',1,'','notification','[]',300,?2,1,1)",
                    params![id, due_at],
                ).unwrap();
            }
            let claims = claim_due_automations(&mut connection, 15).unwrap();
            assert_eq!(claims.len(), 2);
            connection.execute(invalidation, []).unwrap();
            drop(connection);

            let mut state = crate::tests::test_app_state();
            state.db = Arc::new(Mutex::new(Connection::open(database.path()).unwrap()));
            let (executed, notifications) = run_claimed_automations(&state, &claims, 17).unwrap();
            assert_eq!(executed, 1, "{invalidation}");
            assert_eq!(notifications.len(), 1, "{invalidation}");
            assert_eq!(notifications[0].title, "valid");
            let connection = state.db.lock().unwrap();
            let (count, id): (i64, String) = connection
                .query_row(
                    "SELECT COUNT(*),MIN(automation_id) FROM automation_runs",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!((count, id), (1, "valid".into()), "{invalidation}");
        }
    }

    #[test]
    fn notification_failure_rolls_back_completion() {
        let state = crate::tests::test_app_state();
        let claim = {
            let mut connection = state.db.lock().unwrap();
            migrate(&connection).unwrap();
            connection.execute_batch(
                "INSERT INTO automations (id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,executor_kind,script_args,timeout_seconds,next_run_at,created_at,updated_at) VALUES ('due','Due','','once','1970-01-01T00:00:10Z',1,'','notification','[]',300,10,1,1);
                 CREATE TRIGGER reject_completion BEFORE UPDATE OF last_run_at ON automations
                 BEGIN SELECT RAISE(ABORT,'test completion failure'); END;",
            ).unwrap();
            claim_due_automations(&mut connection, 15)
                .unwrap()
                .remove(0)
        };

        assert!(run_claimed_automation_impl(&state, &claim, 17).is_err());
        let connection = state.db.lock().unwrap();
        let completed: i64 = connection
            .query_row("SELECT COUNT(*) FROM automation_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            completed, 0,
            "a failed completion must not leave a success receipt"
        );
        let (enabled, token): (bool, String) = connection
            .query_row(
                "SELECT enabled,schedule_claim_token FROM automations WHERE id=?1",
                [&claim.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(enabled);
        assert_eq!(token, claim.token);
    }

    fn insert_claimed_agent_dispatch(connection: &Connection) {
        connection
            .execute_batch(
                "
                INSERT INTO projects (id,name,path,created_at,kind,updated_at)
                    VALUES ('project-a','Project A','C:/project-a',1,'project',1);
                INSERT INTO automations (
                    id,title,prompt,trigger_kind,trigger_value,enabled,
                    permission_summary,executor_kind,workspace_id,script_args,
                    timeout_seconds,created_at,updated_at
                ) VALUES (
                    'automation-a','整理笔记','整理今天的笔记','schedule','每天 21:00',1,
                    '','agent','project-a','[]',300,1,1
                );
                INSERT INTO workspace_supervisor_inputs (
                    id,workspace_id,kind,content,priority,status,claim_token,created_at,updated_at
                ) VALUES (
                    'input-a','project-a','follow_up','整理笔记',100,'claimed','claim-a',1,1
                );
                INSERT INTO automation_runs (
                    id,automation_id,status,summary,output,started_at,finished_at,
                    supervisor_input_id,foreground_run_id
                ) VALUES (
                    'run-a','automation-a','running','', '',1,NULL,'input-a','turn-a'
                );
                ",
            )
            .unwrap();
    }

    #[test]
    fn queued_automation_contract_rejects_legacy_frequency_before_reservation() {
        for (value, invalid) in [
            ("每周一 09:00", true),
            ("工作日 09:00", true),
            ("每天 09:00", false),
        ] {
            let state = crate::tests::test_app_state();
            {
                let connection = state.db.lock().unwrap();
                migrate(&connection).unwrap();
                insert_claimed_agent_dispatch(&connection);
                connection.execute(
                    "UPDATE automations SET trigger_value=?1,next_run_at=2000000000 WHERE id='automation-a'",
                    [value],
                ).unwrap();
                connection.execute(
                    "UPDATE automation_runs SET status='queued',foreground_run_id=NULL WHERE id='run-a'", [],
                ).unwrap();
            }
            let preparation = prepare_agent_automation_dispatch(&state, "input-a").unwrap();
            if invalid {
                assert_eq!(
                    preparation,
                    AgentAutomationDispatchPreparation::Cancelled,
                    "{value}"
                );
            } else {
                assert!(matches!(
                    preparation,
                    AgentAutomationDispatchPreparation::Dispatch(_)
                ));
            }
            let connection = state.db.lock().unwrap();
            let (stored, enabled, input_status): (String, bool, String) = connection.query_row(
                "SELECT automation.trigger_value,automation.enabled,input.status
                 FROM automations automation JOIN workspace_supervisor_inputs input ON input.workspace_id=automation.workspace_id
                 WHERE automation.id='automation-a' AND input.id='input-a'",
                [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).unwrap();
            assert_eq!(stored, value);
            assert_eq!(enabled, !invalid);
            assert_eq!(input_status, if invalid { "cancelled" } else { "claimed" });
            let foreground_id: Option<String> = connection
                .query_row(
                    "SELECT foreground_run_id FROM automation_runs WHERE id='run-a'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(foreground_id.is_none(), invalid);
            let attention: i64 = connection.query_row(
                "SELECT COUNT(*) FROM automation_runs WHERE automation_id='automation-a' AND status='needs_attention'",
                [], |row| row.get(0),
            ).unwrap();
            assert_eq!(attention, i64::from(invalid));
        }
    }

    #[test]
    fn queued_automation_contract_rechecks_frequency_at_foreground_admission() {
        for (value, invalid) in [
            ("每周一 09:00", true),
            ("工作日 09:00", true),
            ("每天 09:00", false),
        ] {
            let state = crate::tests::test_app_state();
            {
                let connection = state.db.lock().unwrap();
                migrate(&connection).unwrap();
                insert_claimed_agent_dispatch(&connection);
            }
            let preparation = prepare_agent_automation_dispatch(&state, "input-a").unwrap();
            let AgentAutomationDispatchPreparation::Dispatch(dispatch) = preparation else {
                panic!("valid daily queue must reserve a turn")
            };
            let mut connection = state.db.lock().unwrap();
            connection.execute(
                "UPDATE automations SET trigger_value=?1,next_run_at=2000000000 WHERE id='automation-a'",
                [value],
            ).unwrap();
            let transaction = connection.transaction().unwrap();
            let admission = recheck_agent_automation_dispatch_before_begin_in_tx(
                &transaction,
                &dispatch,
                "claim-a",
                10,
            )
            .unwrap();
            transaction.commit().unwrap();
            assert_eq!(
                admission,
                if invalid {
                    AgentAutomationDispatchAdmission::Cancelled
                } else {
                    AgentAutomationDispatchAdmission::Start
                },
                "{value}"
            );
            let (enabled, input_status): (bool, String) = connection.query_row(
                "SELECT automation.enabled,input.status FROM automations automation
                 JOIN workspace_supervisor_inputs input ON input.workspace_id=automation.workspace_id
                 WHERE automation.id='automation-a' AND input.id='input-a'",
                [], |row| Ok((row.get(0)?, row.get(1)?)),
            ).unwrap();
            assert_eq!(enabled, !invalid);
            assert_eq!(input_status, if invalid { "cancelled" } else { "claimed" });
            let task_count: i64 = connection
                .query_row("SELECT COUNT(*) FROM task_runs", [], |row| row.get(0))
                .unwrap();
            assert_eq!(
                task_count, 0,
                "a rejected admission must not create a foreground task"
            );
        }
    }

    #[test]
    fn queued_automation_contract_preserves_already_started_foreground_identity() {
        let state = crate::tests::test_app_state();
        {
            let connection = state.db.lock().unwrap();
            migrate(&connection).unwrap();
            insert_claimed_agent_dispatch(&connection);
            connection.execute_batch(
                "UPDATE automations SET trigger_value='每周一 09:00',next_run_at=2000000000 WHERE id='automation-a';
                 INSERT INTO sessions (id,title,created_at,updated_at) VALUES ('session-a','Main',1,1);
                 INSERT INTO messages (id,session_id,role,content,created_at) VALUES ('turn-a','session-a','assistant','',1);
                 INSERT INTO task_runs (id,session_id,message_id,goal,status,plan,created_at,updated_at)
                     VALUES ('turn-a','session-a','turn-a','Summarize notes','running','[]',1,1);",
            ).unwrap();
        }
        let AgentAutomationDispatchPreparation::Dispatch(dispatch) =
            prepare_agent_automation_dispatch(&state, "input-a").unwrap()
        else {
            panic!("an already-started turn must retain its reconciliation identity")
        };
        assert_eq!(dispatch.foreground_run_id, "turn-a");
        let mut connection = state.db.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        assert_eq!(
            recheck_agent_automation_dispatch_before_begin_in_tx(
                &transaction,
                &dispatch,
                "claim-a",
                10
            )
            .unwrap(),
            AgentAutomationDispatchAdmission::AlreadyStarted,
        );
        transaction.commit().unwrap();
        let input_status: String = connection
            .query_row(
                "SELECT status FROM workspace_supervisor_inputs WHERE id='input-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(input_status, "claimed");
        let task_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM task_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            task_count, 1,
            "recovery must not replay an already-started turn"
        );
    }

    #[test]
    fn pausing_cancels_an_unstarted_agent_delivery_and_denies_admission() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        insert_claimed_agent_dispatch(&connection);

        set_automation_enabled_in_conn(&mut connection, "automation-a", false, 10).unwrap();

        let stored: (String, Option<String>, String) = connection
            .query_row(
                "SELECT input.status, input.claim_token, run.status
                 FROM workspace_supervisor_inputs input
                 JOIN automation_runs run ON run.supervisor_input_id = input.id
                 WHERE input.id = 'input-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(stored, ("cancelled".into(), None, "cancelled".into()));

        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let admission = recheck_agent_automation_dispatch_before_begin_in_tx(
            &transaction,
            &AgentAutomationDispatch {
                automation_run_id: "run-a".into(),
                foreground_run_id: "turn-a".into(),
                supervisor_input_id: "input-a".into(),
            },
            "claim-a",
            11,
        )
        .unwrap();
        transaction.rollback().unwrap();
        assert_eq!(admission, AgentAutomationDispatchAdmission::Cancelled);
    }

    #[test]
    fn deleting_automation_cancels_claimed_delivery_before_cascade() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        insert_claimed_agent_dispatch(&connection);

        delete_automation_in_conn(&mut connection, "automation-a", 10).unwrap();

        let input: (String, Option<String>) = connection
            .query_row(
                "SELECT status, claim_token FROM workspace_supervisor_inputs WHERE id = 'input-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(input, ("cancelled".into(), None));
        let run_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM automation_runs WHERE id = 'run-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(run_count, 0);
    }
}
