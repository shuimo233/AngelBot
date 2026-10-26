//! One durable admission path for manual and scheduled owned-process effects.

use super::*;
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
};

type OwnerKey = (String, String);
static LIVE_OWNERS: OnceLock<Mutex<HashMap<OwnerKey, (String, Arc<AtomicBool>)>>> = OnceLock::new();

pub(super) fn database_identity(conn: &Connection) -> String {
    match conn.path().filter(|path| !path.is_empty()) {
        Some(path) => {
            let path = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
            let identity = path.to_string_lossy().into_owned();
            #[cfg(windows)]
            let identity = identity.to_lowercase();
            identity
        }
        None => format!("memory:{conn:p}"),
    }
}

fn owner_is_live(database: &str, id: &str) -> Result<bool, String> {
    Ok(LIVE_OWNERS
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|error| error.to_string())?
        .contains_key(&(database.to_string(), id.to_string())))
}

fn stop_signal(
    conn: &Connection,
    id: &str,
    run_id: &str,
) -> Result<Option<Arc<AtomicBool>>, String> {
    let bound: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM automations a JOIN automation_runs run
         ON run.id=a.schedule_claim_token AND run.automation_id=a.id
         WHERE a.id=?1 AND run.id=?2 AND a.executor_kind IN ('script','maa') AND run.status='running')",
        params![id, run_id], |row| row.get(0),
    ).map_err(|_| "无法核验本次执行归属".to_string())?;
    if !bound {
        return Ok(None);
    }
    let owners = LIVE_OWNERS
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| "无法核验本次执行归属".to_string())?;
    Ok(owners
        .get(&(database_identity(conn), id.to_string()))
        .filter(|(owned_run, _)| owned_run == run_id)
        .map(|(_, signal)| signal.clone()))
}

pub(super) fn stop_state(
    conn: &Connection,
    id: &str,
    run_id: &str,
) -> Result<(bool, bool), String> {
    Ok(stop_signal(conn, id, run_id)?
        .map(|signal| (true, signal.load(Ordering::Acquire)))
        .unwrap_or((false, false)))
}

pub(super) fn request_stop(conn: &Connection, id: &str, run_id: &str) -> Result<(), String> {
    let signal = stop_signal(conn, id, run_id)?
        .ok_or_else(|| "本次执行不属于当前应用，未发出停止请求".to_string())?;
    signal.store(true, Ordering::Release);
    Ok(())
}

struct LiveOwner {
    key: OwnerKey,
    run_id: String,
    stop: Arc<AtomicBool>,
}

impl LiveOwner {
    fn reserve(database: String, id: &str, run_id: &str) -> Result<Self, String> {
        let key = (database, id.to_string());
        let stop = Arc::new(AtomicBool::new(false));
        match LIVE_OWNERS
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|error| error.to_string())?
            .entry(key.clone())
        {
            Entry::Occupied(_) => return Err("该自动化仍由当前应用执行，不会重复启动".to_string()),
            Entry::Vacant(entry) => {
                entry.insert((run_id.to_string(), stop.clone()));
            }
        }
        Ok(Self {
            key,
            run_id: run_id.to_string(),
            stop,
        })
    }
}

impl Drop for LiveOwner {
    fn drop(&mut self) {
        if let Ok(mut owners) = LIVE_OWNERS.get_or_init(Mutex::default).lock() {
            if owners
                .get(&self.key)
                .is_some_and(|(run_id, _)| run_id == &self.run_id)
            {
                owners.remove(&self.key);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Origin {
    Manual,
    Scheduled,
}

pub(super) fn requires_attention(conn: &Connection, id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM automations a JOIN automation_runs run
         ON run.id=a.schedule_claim_token AND run.automation_id=a.id
         WHERE a.id=?1 AND a.executor_kind IN ('script','maa') AND run.status='attention_required')",
        [id],
        |row| row.get(0),
    )
    .map_err(|error| error.to_string())
}

/// The bound covers the maximum supported owned-run timeout plus process-tree
/// termination/drain slack. Expiry never proves that an external effect failed.
/// Keep the expired receipt as the owner marker until an explicit manual retry.
pub(super) fn recover_expired(
    transaction: &Transaction<'_>,
    now: i64,
    id: Option<&str>,
    database: &str,
) -> Result<usize, String> {
    let stale_before = now - AUTOMATION_CLAIM_TTL_SECONDS;
    let expired = {
        let mut statement = transaction
            .prepare(
                "SELECT run.id,run.automation_id FROM automation_runs run
                 JOIN automations a ON a.id=run.automation_id
                 WHERE a.executor_kind IN ('script','maa') AND run.status='running'
                   AND run.started_at<=?1 AND (?2 IS NULL OR a.id=?2)",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![stale_before, id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
    };
    let mut recovered = 0;
    for (run_id, automation_id) in &expired {
        // UTC can jump, and a second Connection can inspect the same live DB.
        // Age alone never revokes this process's still-owned execution.
        if owner_is_live(database, automation_id)? {
            continue;
        }
        transaction
            .execute(
                "UPDATE automation_runs SET status='attention_required',
                 summary='上次受控执行结果无法确认；定时已暂停，请检查后明确重试',finished_at=?2
                 WHERE id=?1 AND status='running'",
                params![run_id, now],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE automations SET enabled=0,next_run_at=NULL,
                 schedule_claim_token=?2,schedule_claimed_at=?3,updated_at=?3 WHERE id=?1",
                params![automation_id, run_id, now],
            )
            .map_err(|error| error.to_string())?;
        recovered += 1;
    }
    // Older versions persisted a schedule claim without a started receipt. Do
    // not turn that unknown effect into an automatic retry after upgrading.
    let legacy = {
        let mut statement = transaction
            .prepare(
                "SELECT a.id,a.schedule_claim_token,a.schedule_claimed_at FROM automations a
             WHERE a.executor_kind IN ('script','maa') AND a.schedule_claim_token IS NOT NULL
               AND a.schedule_claimed_at<=?1 AND (?2 IS NULL OR a.id=?2)
               AND NOT EXISTS(SELECT 1 FROM automation_runs run
                              WHERE run.id=a.schedule_claim_token AND run.automation_id=a.id)",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![stale_before, id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
    };
    for (automation_id, token, claimed_at) in legacy {
        if owner_is_live(database, &automation_id)? {
            continue;
        }
        transaction.execute(
            "INSERT INTO automation_runs(id,automation_id,status,summary,started_at,finished_at)
             VALUES(?1,?2,'attention_required','旧执行领取结果无法确认；请检查后明确重试',?3,?4)",
            params![token, automation_id, claimed_at, now],
        ).map_err(|error| error.to_string())?;
        transaction.execute(
            "UPDATE automations SET enabled=0,next_run_at=NULL,schedule_claimed_at=?2,updated_at=?2 WHERE id=?1",
            params![automation_id, now],
        ).map_err(|error| error.to_string())?;
        recovered += 1;
    }
    Ok(recovered)
}

fn consume_occurrence(
    transaction: &Transaction<'_>,
    automation: &Automation,
    now: i64,
) -> Result<(), String> {
    transaction
        .execute(
            "UPDATE automations SET next_run_at=?2,
             enabled=CASE WHEN trigger_kind='once' THEN 0 ELSE enabled END,updated_at=?3 WHERE id=?1",
            params![
                automation.id,
                next_run_at(&automation.trigger_kind, &automation.trigger_value, now),
                now
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn execution_is_busy(conn: &Connection, id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM automation_runs WHERE automation_id=?1 AND status='running')
         OR EXISTS(SELECT 1 FROM automations a WHERE a.id=?1 AND a.schedule_claim_token IS NOT NULL
                   AND NOT EXISTS(SELECT 1 FROM automation_runs run
                                  WHERE run.id=a.schedule_claim_token AND run.automation_id=a.id))",
        [id],
        |row| row.get(0),
    )
    .map_err(|error| error.to_string())
}

/// Consume overlap at scan time, not after earlier batch items finish. The same
/// check at execution admission also covers a manual run starting after a scan.
pub(super) fn skip_busy_due_in_tx(
    transaction: &Transaction<'_>,
    automation: &Automation,
    now: i64,
) -> Result<bool, String> {
    if !execution_is_busy(transaction, &automation.id)? {
        return Ok(false);
    }
    transaction
        .execute(
            "INSERT INTO automation_runs(id,automation_id,status,summary,started_at,finished_at)
             VALUES(?1,?2,'skipped','已有受控进程正在执行，本次定时已跳过；不会稍后补跑',?3,?3)",
            params![Uuid::new_v4().to_string(), automation.id, now],
        )
        .map_err(|error| error.to_string())?;
    consume_occurrence(transaction, automation, now)?;
    Ok(true)
}

pub(super) struct Ticket {
    pub(super) automation: Automation,
    pub(super) run_id: String,
    _live_owner: LiveOwner,
}

pub(super) enum Admission {
    Started(Ticket),
    Consumed,
    NotDue,
}

/// A durable receipt is committed before a ticket can cross into execution.
/// The live-owner guard remains with the ticket even while a worker is queued.
pub(super) fn admit(
    state: &AppState,
    id: &str,
    origin: Origin,
    now: i64,
    on_update: &dyn Fn(),
) -> Result<Admission, String> {
    let (automation, run_id, live_owner) = {
        let mut conn = state.db.lock().map_err(|error| error.to_string())?;
        let database = database_identity(&conn);
        let transaction = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let automation = transaction
            .query_row(
                "SELECT id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,
                 executor_kind,script_path,script_args,working_dir,timeout_seconds,next_run_at,last_run_at,workspace_id
                 FROM automations WHERE id=?1",
                [id],
                automation_from_row,
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let Some(automation) = automation else {
            return if origin == Origin::Scheduled {
                Ok(Admission::NotDue)
            } else {
                Err("未找到自动化任务".to_string())
            };
        };
        if !is_owned_executor(&automation.executor_kind) {
            return Err("受控进程执行方式已改变".to_string());
        }
        if recover_expired(&transaction, now, Some(id), &database)? > 0 {
            transaction.commit().map_err(|error| error.to_string())?;
            drop(conn);
            on_update();
            return if origin == Origin::Scheduled {
                Ok(Admission::NotDue)
            } else {
                Err(
                    "上次受控执行结果无法确认；已暂停定时，请检查后再次明确点击立即运行重试"
                        .to_string(),
                )
            };
        }
        if origin == Origin::Scheduled
            && (!automation.enabled
                || !matches!(automation.trigger_kind.as_str(), "schedule" | "once")
                || !automation.next_run_at.is_some_and(|due| due <= now))
        {
            return Ok(Admission::NotDue);
        }
        if origin == Origin::Scheduled && requires_attention(&transaction, id)? {
            transaction
                .execute(
                    "UPDATE automations SET enabled=0,next_run_at=NULL WHERE id=?1",
                    [id],
                )
                .map_err(|error| error.to_string())?;
            transaction.commit().map_err(|error| error.to_string())?;
            drop(conn);
            on_update();
            return Ok(Admission::NotDue);
        }
        if origin == Origin::Scheduled && skip_busy_due_in_tx(&transaction, &automation, now)? {
            transaction.commit().map_err(|error| error.to_string())?;
            drop(conn);
            on_update();
            return Ok(Admission::Consumed);
        }
        if origin == Origin::Manual && execution_is_busy(&transaction, id)? {
            return Err("该自动化已在执行，不会重复启动".to_string());
        }
        if pause_unsupported_schedule_in_tx(&transaction, &automation, now, None)? {
            transaction.commit().map_err(|error| error.to_string())?;
            drop(conn);
            on_update();
            return if origin == Origin::Scheduled {
                Ok(Admission::Consumed)
            } else {
                Err("自动化定时频率不受支持，已暂停".to_string())
            };
        }
        let run_id = Uuid::new_v4().to_string();
        let live_owner = LiveOwner::reserve(database, id, &run_id)?;
        transaction
            .execute(
                "INSERT INTO automation_runs(id,automation_id,status,summary,started_at,finished_at)
                 VALUES(?1,?2,'running','本次执行已登记；结束结果尚未确认',?3,NULL)",
                params![run_id, id, now],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE automations SET schedule_claim_token=?2,schedule_claimed_at=?3 WHERE id=?1",
                params![id, run_id, now],
            )
            .map_err(|error| error.to_string())?;
        if origin == Origin::Scheduled {
            consume_occurrence(&transaction, &automation, now)?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        (automation, run_id, live_owner)
    };
    on_update();
    Ok(Admission::Started(Ticket {
        automation,
        run_id,
        _live_owner: live_owner,
    }))
}

/// A non-cloneable notification handoff, issued only after the exact run's
/// terminal CAS commits. It is not a replay queue or a delivery receipt.
pub(super) struct OwnedRunOutcome {
    automation_id: String,
    run_id: String,
}
impl OwnedRunOutcome {
    pub(super) fn into_ids(self) -> (String, String) {
        (self.automation_id, self.run_id)
    }
}

pub(super) fn execute_ticket(
    state: &AppState,
    ticket: Ticket,
    on_update: &dyn Fn(),
    execute: &dyn Fn(&AppState, &Automation, &str, &dyn Fn() -> bool) -> ScriptRunResult,
) -> Result<OwnedRunOutcome, String> {
    let cancelled = || ticket._live_owner.stop.load(Ordering::Acquire);
    let result = if cancelled() {
        ScriptRunResult {
            status: "cancelled",
            summary: "本次执行已停止，未启动进程".into(),
            exit_code: None,
            output: String::new(),
            supervisor_input_id: None,
        }
    } else {
        execute(state, &ticket.automation, &ticket.run_id, &cancelled)
    };
    let finalized = finalize(state, &ticket.automation.id, &ticket.run_id, result);
    let outcome = OwnedRunOutcome {
        automation_id: ticket.automation.id.clone(),
        run_id: ticket.run_id.clone(),
    };
    drop(ticket);
    on_update();
    finalized.map(|()| outcome)
}

/// Synchronous callers (manual and development IPC) await the same ticket.
pub(super) fn run(
    state: &AppState,
    id: &str,
    origin: Origin,
    now: i64,
    on_update: &dyn Fn(),
    execute: &dyn Fn(&AppState, &Automation) -> ScriptRunResult,
) -> Result<bool, String> {
    match admit(state, id, origin, now, on_update)? {
        Admission::Started(ticket) => {
            execute_ticket(state, ticket, on_update, &|state, automation, _, _| {
                execute(state, automation)
            })?;
            Ok(true)
        }
        Admission::Consumed => Ok(true),
        Admission::NotDue => Ok(false),
    }
}

fn finalize(
    state: &AppState,
    id: &str,
    run_id: &str,
    result: ScriptRunResult,
) -> Result<(), String> {
    let mut conn = state.db.lock().map_err(|error| error.to_string())?;
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let completed_at = Utc::now().timestamp();
    let changed = transaction
        .execute(
            "UPDATE automation_runs SET status=?2,summary=?3,exit_code=?4,output=?5,finished_at=?6
             WHERE id=?1 AND automation_id=?7 AND status='running'",
            params![
                run_id,
                result.status,
                result.summary,
                result.exit_code,
                result.output,
                completed_at,
                id
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("执行结果回执已失去运行归属；请检查保留的运行记录".to_string());
    }
    let released = transaction
        .execute(
            "UPDATE automations SET last_run_at=?3,
                 enabled=CASE WHEN ?4 THEN 0 ELSE enabled END,
                 next_run_at=CASE WHEN ?4 THEN NULL ELSE next_run_at END,
                 schedule_claim_token=CASE WHEN ?4 THEN schedule_claim_token ELSE NULL END,
                 schedule_claimed_at=CASE WHEN ?4 THEN schedule_claimed_at ELSE NULL END
             WHERE id=?1 AND schedule_claim_token=?2",
            params![
                id,
                run_id,
                completed_at,
                result.status == "attention_required"
            ],
        )
        .map_err(|error| error.to_string())?;
    if released != 1 {
        return Err("执行归属已变化；结果需要核验".to_string());
    }
    transaction.commit().map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "automation_script_admission_tests.rs"]
mod tests;
