//! Read-only receipt projections. Main's bounded workspace view excludes task
//! configuration and raw evidence; the global settings dashboard includes definitions.

use super::*;

const MAX_ITEMS: usize = 100;
const MAX_TEXT_CHARS: usize = 120;

#[derive(Debug, Serialize)]
pub struct AutomationDashboard {
    pub items: Vec<AutomationDashboardItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationDashboardItem {
    #[serde(flatten)]
    pub automation: Automation,
    pub latest_run: Option<WorkspaceAutomationRunStatus>,
    pub last_skipped_at: Option<i64>,
}

/// One snapshot and one joined receipt read, not a history/output fetch per card.
pub(super) fn automation_dashboard(state: &AppState) -> Result<AutomationDashboard, String> {
    let mut conn = state.db.lock().map_err(|error| error.to_string())?;
    let tx = conn.transaction().map_err(|error| error.to_string())?;
    let sql = format!(
        "SELECT {AUTOMATION_COLUMNS_SQL},run.id,run.status,substr(run.summary,1,401),
                run.exit_code,run.started_at,run.notification_status,
                (SELECT CASE WHEN status='skipped' THEN started_at END
                 FROM automation_runs WHERE automation_id=a.id
                 ORDER BY started_at DESC,rowid DESC LIMIT 1)
         FROM automations a
         LEFT JOIN automation_runs run ON run.id=(
             SELECT run.id FROM automation_runs run
             JOIN automations owner ON owner.id=run.automation_id
             WHERE run.automation_id=a.id AND {EFFECTIVE_RECEIPT_SQL}
             ORDER BY {RECEIPT_PRIORITY_SQL},run.started_at DESC,run.rowid DESC LIMIT 1)
         ORDER BY a.updated_at DESC,a.id"
    );
    let mut items = {
        let mut stmt = tx.prepare(&sql).map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                let latest_run = row
                    .get::<_, Option<String>>(15)?
                    .map(|id| {
                        let summary: String = row.get(17)?;
                        let mut bounded: String = summary.chars().take(400).collect();
                        if summary.chars().count() > 400 {
                            bounded.push('…');
                        }
                        let notification_status: Option<String> = row.get(20)?;
                        bounded
                            .push_str(notification_receipt_summary(notification_status.as_deref()));
                        Ok::<_, rusqlite::Error>(WorkspaceAutomationRunStatus {
                            id,
                            status: row.get(16)?,
                            summary: bounded,
                            exit_code: row.get(18)?,
                            started_at: row.get(19)?,
                            notification_status,
                            stop_control: None,
                        })
                    })
                    .transpose()?;
                Ok(AutomationDashboardItem {
                    automation: automation_from_row(row)?,
                    latest_run,
                    last_skipped_at: row.get(21)?,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
    };
    for item in &mut items {
        if let Some(run) = item
            .latest_run
            .as_mut()
            .filter(|run| run.status == "running")
        {
            let (owned, requested) =
                script_admission::stop_state(&tx, &item.automation.id, &run.id)?;
            run.stop_control = owned.then_some(if requested {
                AutomationStopControl::Requested
            } else {
                AutomationStopControl::Available
            });
        }
    }
    tx.commit().map_err(|error| error.to_string())?;
    Ok(AutomationDashboard { items })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAutomationOverview {
    pub items: Vec<WorkspaceAutomationStatus>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAutomationStatus {
    pub id: String,
    pub title: String,
    pub workspace_id: String,
    pub executor_kind: String,
    pub trigger_kind: String,
    pub trigger_value: String,
    pub enabled: bool,
    pub next_run_at: Option<i64>,
    pub last_run_at: Option<i64>,
    pub latest_run: Option<WorkspaceAutomationRunStatus>,
    pub last_skipped_at: Option<i64>,
    pub maa_config: Option<WorkspaceMaaAutomationConfig>,
}

/// Saved consent parameters, not a claim that the app or its grant is usable now.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMaaAutomationConfig {
    pub app_id: String,
    pub app_display_name: Option<String>,
    pub launch_emulator: bool,
    pub timeout_seconds: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAutomationRunStatus {
    pub id: String,
    pub status: String,
    pub summary: String,
    pub exit_code: Option<i32>,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_control: Option<AutomationStopControl>,
}

fn bounded_text(value: String) -> String {
    value.chars().take(MAX_TEXT_CHARS).collect()
}

/// Keep names revision-bound: a changed or recreated app is not the saved target.
pub(crate) fn maa_config_for_automation(
    conn: &Connection,
    automation_id: &str,
) -> Result<Option<WorkspaceMaaAutomationConfig>, String> {
    conn.query_row(
        "SELECT opts.trusted_app_id,app.display_name,opts.launch_emulator,a.timeout_seconds
         FROM automations a JOIN maa_automation_options opts ON opts.automation_id=a.id
         LEFT JOIN desktop_trusted_apps app ON app.id=opts.trusted_app_id AND app.updated_at=opts.app_revision
         WHERE a.id=?1 AND a.executor_kind='maa'",
        [automation_id],
        |row| {
            Ok(WorkspaceMaaAutomationConfig {
                app_id: row.get(0)?,
                app_display_name: row.get::<_, Option<String>>(1)?.map(bounded_text),
                launch_emulator: row.get(2)?,
                timeout_seconds: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(|error| error.to_string())
}

pub(super) fn notification_receipt_summary(status: Option<&str>) -> &'static str {
    match status {
        Some("requested") => " 已请求系统结果通知，不代表已显示或已读。",
        Some("muted") => " 本次结果通知已关闭，不会自动补发。",
        Some("unavailable") => " 无法请求系统结果通知，请在此核对运行结果；不会自动补发。",
        Some("unconfirmed") => " 系统通知请求结果未知，不会自动补发。",
        _ => "",
    }
}

fn receipt_summary(kind: &str, status: &str) -> &'static str {
    match (kind, status) {
        (_, "attention_required") => "本次结果未知，不可自动重放；请先核对实际状态后明确处理。",
        (_, "running") => "本次执行已登记，尚无结束结果；此回执不能证明进程仍然存活。",
        (_, "queued") => "等待主 Agent 处理；尚未得到完成结果。",
        (_, "awaiting_confirmation") => "主 Agent 需要你在归属工作区对话中确认下一步操作。",
        ("agent", "needs_attention") => "主 Agent 需要你在归属工作区对话中检查并继续处理。",
        (_, "needs_attention") => "自动化配置需要核对；查询不会自动重试。",
        ("maa", "completed") => "MAA 队列已结束；不代表游戏业务目标全部完成。",
        ("script", "completed") => {
            "脚本已结束；回执仅表示脚本执行结果，不代表外部应用的业务目标完成。"
        }
        ("notification", "completed") => "提醒已触发；此回执不证明系统通知已送达。",
        ("notification", "skipped") => "通知已关闭，本次提醒未发送；不会自动补发。",
        ("agent", "completed") => "主 Agent 已结束本次处理；具体结果见归属工作区对话。",
        (_, "completed") => "本次执行已结束；完成范围需在自动化页面核对。",
        ("maa", "failed") => "MAA 执行存在失败；部分操作可能已生效，请核对实际结果。",
        (_, "failed") => "本次执行失败；请核对实际结果，查询不会自动重试。",
        (_, "timed_out") => "本次执行已超时；请核对实际结果，查询不会自动重试。",
        ("script" | "maa", "cancelled") => {
            "本次已停止；已产生的外部操作不会自动回滚，回执不代表业务目标完成。"
        }
        (_, "cancelled") => "本次派发已取消；该回执不表示正在执行的进程已停止。",
        (_, "interrupted") => "本次执行已中断；已发生的操作不能自动回滚，请核对实际结果。",
        _ => "本次状态无法确认；请在自动化页面核对。",
    }
}

/// A read transaction keeps definition/claim/receipts from different moments
/// out of one projection. It does not run recovery or emit scheduling events.
pub(crate) fn workspace_automation_overview(
    db: Arc<Mutex<Connection>>,
    workspace_id: &str,
    query: Option<&str>,
) -> Result<WorkspaceAutomationOverview, String> {
    if workspace_id.trim().is_empty() {
        return Err("查询自动化必须确定归属工作区".into());
    }
    let query = query.map(str::trim).filter(|value| !value.is_empty());
    if query.is_some_and(|value| value.chars().count() > MAX_TEXT_CHARS) {
        return Err("自动化标题查询最多 120 个字符".into());
    }
    let mut conn = db.lock().map_err(|error| error.to_string())?;
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| error.to_string())?;
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
            [workspace_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !exists {
        return Err("归属工作区不存在，不能查询其他工作区或全部自动化".into());
    }
    let definitions = {
        let sql = format!(
            "SELECT a.id,a.title,a.executor_kind,a.trigger_kind,a.trigger_value,a.enabled,a.next_run_at,a.last_run_at,
                (SELECT run.status FROM automation_runs run JOIN automations owner ON owner.id=run.automation_id
                 WHERE run.automation_id=a.id AND {EFFECTIVE_RECEIPT_SQL}
                 ORDER BY {RECEIPT_PRIORITY_SQL},
                          run.started_at DESC,run.rowid DESC LIMIT 1) AS effective_status
             FROM automations a WHERE a.workspace_id=?1 AND (?2 IS NULL OR instr(lower(a.title),lower(?2))>0)
             ORDER BY CASE WHEN effective_status IN ('attention_required','needs_attention') THEN 0
                           WHEN effective_status IN ('running','queued','awaiting_confirmation') THEN 1 ELSE 2 END,
                      a.enabled DESC,a.created_at DESC,a.id ASC LIMIT ?3",
        );
        let mut statement = transaction
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(
                params![workspace_id, query, (MAX_ITEMS + 1) as i64],
                |row| {
                    Ok(WorkspaceAutomationStatus {
                        id: row.get(0)?,
                        title: bounded_text(row.get(1)?),
                        workspace_id: workspace_id.to_string(),
                        executor_kind: bounded_text(row.get(2)?),
                        trigger_kind: bounded_text(row.get(3)?),
                        trigger_value: bounded_text(row.get(4)?),
                        enabled: row.get(5)?,
                        next_run_at: row.get(6)?,
                        last_run_at: row.get(7)?,
                        latest_run: None,
                        last_skipped_at: None,
                        maa_config: None,
                    })
                },
            )
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
    };
    let truncated = definitions.len() > MAX_ITEMS;
    let mut items = Vec::with_capacity(definitions.len().min(MAX_ITEMS));
    let receipt_sql = format!(
        "SELECT run.id,run.status,run.exit_code,run.started_at,run.notification_status FROM automation_runs run
         JOIN automations owner ON owner.id=run.automation_id
         WHERE run.automation_id=?1 AND {EFFECTIVE_RECEIPT_SQL}
         ORDER BY {RECEIPT_PRIORITY_SQL},run.started_at DESC,run.rowid DESC LIMIT 1"
    );
    for mut item in definitions.into_iter().take(MAX_ITEMS) {
        if item.executor_kind == "maa" {
            item.maa_config = maa_config_for_automation(&transaction, &item.id)?;
        }
        // Do not search a capped history page: the owner/active receipt may
        // precede arbitrarily many skipped occurrences or later legacy rows.
        // A muted notification is itself the result, not an overlapping job.
        item.latest_run = transaction
            .query_row(&receipt_sql, [&item.id], |row| {
                let status: String = row.get(1)?;
                let notification_status: Option<String> = row.get(4)?;
                Ok(WorkspaceAutomationRunStatus {
                    id: row.get(0)?,
                    summary: format!(
                        "{}{}",
                        receipt_summary(&item.executor_kind, &status),
                        notification_receipt_summary(notification_status.as_deref())
                    ),
                    status: bounded_text(status),
                    exit_code: row.get(2)?,
                    started_at: row.get(3)?,
                    notification_status,
                    stop_control: None,
                })
            })
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some(run) = item
            .latest_run
            .as_mut()
            .filter(|run| run.status == "running")
        {
            let (owned, requested) = script_admission::stop_state(&transaction, &item.id, &run.id)?;
            run.stop_control = owned.then_some(if requested {
                AutomationStopControl::Requested
            } else {
                AutomationStopControl::Available
            });
        }
        item.last_skipped_at = transaction
            .query_row(
                "SELECT CASE WHEN status='skipped' THEN started_at END FROM automation_runs
             WHERE automation_id=?1 AND ?2!='notification' ORDER BY started_at DESC,rowid DESC LIMIT 1",
                params![item.id, item.executor_kind],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .flatten();
        items.push(item);
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(WorkspaceAutomationOverview { items, truncated })
}

#[cfg(test)]
#[path = "automation_overview_tests.rs"]
mod tests;
