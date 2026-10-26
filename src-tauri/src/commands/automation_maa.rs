//! The MAA preset binds routine consent to one trusted application revision.
//! Protocol/process ownership stays in maa_routine; scheduling stays here.

use super::*;
use crate::commands::desktop::{self, TrustedDesktopApp, CAPABILITY_LAUNCH};

struct Options {
    trusted_app_id: String,
    app_revision: i64,
    launch_emulator: bool,
}

fn trusted_maa_app(conn: &Connection, id: &str) -> Result<TrustedDesktopApp, String> {
    let mut app = desktop::get_trusted_desktop_app_from_conn(conn, id)?
        .ok_or_else(|| "MAA 不在可信应用列表中".to_string())?;
    if !app.enabled
        || !app
            .capabilities
            .iter()
            .any(|value| value == CAPABILITY_LAUNCH)
    {
        return Err("MAA 已停用或未授予打开应用权限".to_string());
    }
    let executable = std::fs::canonicalize(&app.executable_path)
        .map_err(|error| format!("MAA 可执行文件不可用：{error}"))?;
    if !executable.is_file()
        || !executable
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.eq_ignore_ascii_case("MAA.exe"))
    {
        return Err("MAA 预设需要可信的 MAA.exe".to_string());
    }
    app.executable_path = executable.to_string_lossy().into_owned();
    Ok(app)
}

pub fn create_maa_automation_impl(
    state: &AppState,
    title: String,
    trigger_value: String,
    workspace_id: String,
    app_id: String,
    launch_emulator: bool,
    timeout_seconds: Option<u64>,
) -> Result<Automation, String> {
    create_maa_automation_definition(
        state.db.clone(),
        title,
        trigger_value,
        workspace_id,
        app_id,
        launch_emulator,
        timeout_seconds,
    )
}

/// The Main Agent and UI share creation; only the caller resolves its workspace.
pub(crate) fn create_maa_automation_definition(
    db: Arc<Mutex<Connection>>,
    title: String,
    trigger_value: String,
    workspace_id: String,
    app_id: String,
    launch_emulator: bool,
    timeout_seconds: Option<u64>,
) -> Result<Automation, String> {
    create_with_validation(
        &db,
        title,
        trigger_value,
        workspace_id,
        app_id,
        launch_emulator,
        timeout_seconds,
        &crate::maa_routine::validate_install,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_with_validation(
    db: &Arc<Mutex<Connection>>,
    title: String,
    trigger_value: String,
    workspace_id: String,
    app_id: String,
    launch_emulator: bool,
    timeout_seconds: Option<u64>,
    validate: &dyn Fn(&str) -> Result<(), String>,
) -> Result<Automation, String> {
    if title.trim().is_empty() {
        return Err("自动化名称不能为空".to_string());
    }
    let now = Utc::now().timestamp();
    let next_due = validated_next_run_at("schedule", &trigger_value, now)?;
    let timeout_seconds = normalize_timeout(Some(timeout_seconds.unwrap_or(MAX_TIMEOUT_SECONDS)))?;
    let mut conn = db.lock().map_err(|error| error.to_string())?;
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
            [&workspace_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !exists {
        return Err("归属工作区不存在".to_string());
    }
    let app = trusted_maa_app(&transaction, &app_id)?;
    // Read-only structural validation. Creating a preset never edits MAA files.
    validate(&app.executable_path)?;
    let item = Automation {
        id: Uuid::new_v4().to_string(), title: title.trim().to_string(), prompt: String::new(),
        trigger_kind: "schedule".to_string(), trigger_value, enabled: true,
        permission_summary: format!(
            "明确批准无人值守按需启动可信 MAA「{}」的当前 GUI 勾选队列；{}；核验队列结束与部分失败，清理本次启动的 MAA 和模拟器进程树，不接管原先打开的 MAA 或关闭原先运行的模拟器进程；游戏操作仍由 MAA 当前队列执行，队列结束不等于游戏日常全部完成。",
            app.display_name, if launch_emulator { "按需启动 MAA 已配置的模拟器" } else { "不启动模拟器" }),
        executor_kind: "maa".to_string(), workspace_id: Some(workspace_id),
        script_path: None, script_args: Vec::new(), working_dir: None,
        timeout_seconds, next_run_at: Some(next_due), last_run_at: None,
    };
    persist_automation_in_conn(&transaction, &item, now)?;
    transaction.execute(
        "INSERT INTO maa_automation_options(automation_id,trusted_app_id,app_revision,launch_emulator)
         VALUES(?1,?2,?3,?4)",
        params![item.id, app.id, app.updated_at, launch_emulator],
    ).map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(item)
}

fn binding(conn: &Connection, id: &str) -> Result<(Options, TrustedDesktopApp), String> {
    let options = conn.query_row(
        "SELECT trusted_app_id,app_revision,launch_emulator FROM maa_automation_options WHERE automation_id=?1",
        [id], |row| Ok(Options { trusted_app_id: row.get(0)?, app_revision: row.get(1)?, launch_emulator: row.get(2)? }),
    ).map_err(|error| format!("MAA 预设绑定不可用：{error}"))?;
    let app = trusted_maa_app(conn, &options.trusted_app_id)?;
    if app.updated_at != options.app_revision {
        return Err("MAA 可信应用授权已变化；请重新创建预设确认授权".to_string());
    }
    Ok((options, app))
}

pub(super) fn execute(
    state: &AppState,
    automation: &Automation,
    run_id: &str,
    cancelled: &dyn Fn() -> bool,
) -> ScriptRunResult {
    execute_with(
        state,
        automation,
        run_id,
        cancelled,
        &crate::maa_routine::validate_install,
        &crate::maa_routine::run_once,
    )
}

type Runner<'a> =
    dyn Fn(&str, bool, &str, Duration, &dyn Fn() -> bool) -> crate::maa_routine::MaaRunResult + 'a;

fn execute_with(
    state: &AppState,
    automation: &Automation,
    run_id: &str,
    cancelled: &dyn Fn() -> bool,
    validate: &dyn Fn(&str) -> Result<(), String>,
    runner: &Runner<'_>,
) -> ScriptRunResult {
    let checked = (|| {
        let conn = state.db.lock().map_err(|error| error.to_string())?;
        binding(&conn, &automation.id)
    })();
    let (options, app) = match checked {
        Ok(value) => value,
        Err(error) => return attention(error),
    };
    if let Err(error) = validate(&app.executable_path) {
        return attention(error);
    }
    // Pausing the definition only stops future scheduling. Revoking its trusted
    // application grant/revision does request cleanup of this owned invocation.
    let revocation_observed = std::cell::Cell::new(false);
    let revoked = || {
        let current = state
            .db
            .lock()
            .map_err(|error| error.to_string())
            .and_then(|conn| binding(&conn, &automation.id));
        let changed = current
            .map(|(current_options, current)| {
                current.executable_path != app.executable_path
                    || current_options.trusted_app_id != options.trusted_app_id
                    || current_options.app_revision != options.app_revision
                    || current_options.launch_emulator != options.launch_emulator
            })
            .unwrap_or(true);
        if changed {
            revocation_observed.set(true);
        }
        changed || cancelled()
    };
    let result = runner(
        &app.executable_path,
        options.launch_emulator,
        run_id,
        Duration::from_secs(automation.timeout_seconds),
        &revoked,
    );
    if result.status == "cancelled" && revocation_observed.get() {
        return attention("MAA 授权已撤销，已请求清理本次运行；请检查后重新确认授权".into());
    }
    ScriptRunResult {
        status: result.status,
        summary: result.summary,
        exit_code: result.exit_code,
        output: result.output,
        supervisor_input_id: None,
    }
}

fn attention(summary: String) -> ScriptRunResult {
    ScriptRunResult {
        status: "attention_required",
        summary,
        exit_code: None,
        output: String::new(),
        supervisor_input_id: None,
    }
}

#[cfg(test)]
#[path = "automation_maa_tests.rs"]
mod tests;
