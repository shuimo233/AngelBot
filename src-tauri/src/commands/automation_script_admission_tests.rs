use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};

fn setup() -> Arc<AppState> {
    let state = Arc::new(crate::tests::test_app_state());
    {
        let conn = state.db.lock().unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO automations(id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,
             executor_kind,script_args,timeout_seconds,next_run_at,created_at,updated_at)
             VALUES('script','Script','','schedule','每天 09:00',1,'','script','[]',300,10,1,1)", [],
        ).unwrap();
    }
    state
}

fn completed(_: &AppState, _: &Automation) -> ScriptRunResult {
    ScriptRunResult {
        status: "completed",
        summary: "脚本执行完成".into(),
        exit_code: Some(0),
        output: "fixture".into(),
        supervisor_input_id: None,
    }
}

fn execute(state: &AppState, origin: Origin, now: i64) -> Result<bool, String> {
    run(state, "script", origin, now, &|| {}, &completed)
}

fn owner(state: &AppState) -> Option<String> {
    state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT schedule_claim_token FROM automations WHERE id='script'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn next(state: &AppState) -> Option<i64> {
    state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT next_run_at FROM automations WHERE id='script'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn start_blocked(
    state: &Arc<AppState>,
) -> (thread::JoinHandle<Result<bool, String>>, mpsc::Sender<()>) {
    let (ready_sender, ready_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let state = state.clone();
    let worker = thread::spawn(move || {
        run(
            &state,
            "script",
            Origin::Manual,
            15,
            &|| {},
            &|state, automation| {
                ready_sender.send(()).unwrap();
                release_receiver
                    .recv_timeout(Duration::from_secs(30))
                    .unwrap();
                completed(state, automation)
            },
        )
    });
    ready_receiver
        .recv_timeout(Duration::from_secs(30))
        .unwrap();
    (worker, release_sender)
}

#[test]
fn manual_overlap_is_rejected_and_the_same_started_receipt_is_finalized() {
    let state = setup();
    let (worker, release) = start_blocked(&state);
    let run_id = owner(&state).unwrap();
    let started: (String, Option<i64>) = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT status,finished_at FROM automation_runs WHERE id=?1",
            [&run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(started, ("running".into(), None));
    assert!(execute(&state, Origin::Manual, 16)
        .unwrap_err()
        .contains("不会重复启动"));
    release.send(()).unwrap();
    worker.join().unwrap().unwrap();
    let conn = state.db.lock().unwrap();
    let terminal: (String, Option<i32>, i64, i64) = conn
        .query_row(
            "SELECT status,exit_code,started_at,finished_at FROM automation_runs WHERE id=?1",
            [&run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        (terminal.0.as_str(), terminal.1, terminal.2),
        ("completed", Some(0), 15)
    );
    assert!(terminal.3 > terminal.2);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM automation_runs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let last: i64 = conn
        .query_row(
            "SELECT last_run_at FROM automations WHERE id='script'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(last, terminal.3);
    drop(conn);

    execute(&state, Origin::Manual, 15).unwrap();
    let runs = get_automation_runs_impl(&state, "script").unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!((runs[0].started_at, runs[1].started_at), (15, 15));
    assert_ne!(runs[0].id, run_id);
    assert_eq!(runs[1].id, run_id);

    let state = setup();
    let (workspace_id, session_id): (String, String) = {
        let conn = state.db.lock().unwrap();
        let scope: (String, String) = conn
            .query_row(
                "SELECT id,active_session_id FROM projects WHERE kind='personal'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        conn.execute(
            "UPDATE automations SET workspace_id=?1 WHERE id='script'",
            [&scope.0],
        )
        .unwrap();
        scope
    };
    let Admission::Started(ticket) = admit(&state, "script", Origin::Manual, 15, &|| {}).unwrap()
    else {
        panic!("expected one owned ticket");
    };
    let run_id = ticket.run_id.clone();
    let available = get_automation_runs_impl(&state, "script").unwrap();
    assert_eq!(
        available[0].stop_control,
        Some(AutomationStopControl::Available)
    );
    assert_eq!(
        serde_json::to_value(&available[0]).unwrap()["stopControl"],
        "available"
    );
    let foreign = setup();
    {
        let conn = foreign.db.lock().unwrap();
        conn.execute("INSERT INTO automation_runs(id,automation_id,status,summary,started_at) VALUES(?1,'script','running','foreign fixture',15)", [&run_id]).unwrap();
        conn.execute(
            "UPDATE automations SET schedule_claim_token=?1 WHERE id='script'",
            [&run_id],
        )
        .unwrap();
        assert_eq!(
            stop_state(&conn, "script", &run_id).unwrap(),
            (false, false)
        );
    }
    assert!(stop_automation_run_impl(&foreign, "script", &run_id).is_err());
    for (id, target) in [("other", run_id.as_str()), ("script", "old-run")] {
        assert!(stop_automation_run_impl(&state, id, target).is_err());
    }
    for change in [
        "UPDATE automation_runs SET status='attention_required'",
        "UPDATE automations SET executor_kind='agent'",
        "UPDATE automations SET schedule_claim_token='new-owner'",
    ] {
        state.db.lock().unwrap().execute_batch(change).unwrap();
        assert!(stop_automation_run_impl(&state, "script", &run_id).is_err());
        let conn = state.db.lock().unwrap();
        conn.execute(
            "UPDATE automation_runs SET status='running' WHERE id=?1",
            [&run_id],
        )
        .unwrap();
        conn.execute("UPDATE automations SET executor_kind='script',schedule_claim_token=?1 WHERE id='script'", [&run_id]).unwrap();
    }
    assert_eq!(
        stop_state(&state.db.lock().unwrap(), "script", &run_id).unwrap(),
        (true, false)
    );
    assert!(
        stop_workspace_automation_run(state.db.clone(), "other-workspace", "script", &run_id)
            .is_err()
    );
    let overview = workspace_automation_overview(state.db.clone(), &workspace_id, None).unwrap();
    assert_eq!(
        overview.items[0].latest_run.as_ref().unwrap().stop_control,
        Some(AutomationStopControl::Available)
    );
    let mut registry = crate::agent::ToolRegistry::empty();
    crate::commands::foreground_automation_control::register(
        &mut registry,
        session_id,
        state.db.clone(),
    );
    for args in [
        serde_json::json!({"automation_id":"script","run_id":"old-run"}),
        serde_json::json!({"automation_id":"script","run_id":run_id,"workspace_id":workspace_id}),
    ] {
        let call = crate::agent::tool::ToolCall::new("stop_automation_run".into(), args);
        assert!(
            !registry
                .execute(&call, "stop", Path::new("."))
                .result
                .success
        );
    }
    assert_eq!(
        stop_state(&state.db.lock().unwrap(), "script", &run_id).unwrap(),
        (true, false)
    );
    let call = crate::agent::tool::ToolCall::new(
        "stop_automation_run".into(),
        serde_json::json!({"automation_id":"script","run_id":run_id}),
    );
    let result = registry.execute(&call, "stop", Path::new(".")).result;
    assert!(result.success, "{}", result.content);
    let content: serde_json::Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(content["stopControl"], "requested");
    assert!(
        content.get("status").is_none(),
        "a request must not fabricate a terminal state"
    );
    let overview = workspace_automation_overview(state.db.clone(), &workspace_id, None).unwrap();
    assert_eq!(
        overview.items[0].latest_run.as_ref().unwrap().stop_control,
        Some(AutomationStopControl::Requested)
    );
    assert!(
        stop_automation_run_impl(&state, "script", &run_id).is_ok(),
        "a live exact owned run accepts a stop request"
    );
    assert!(
        stop_automation_run_impl(&state, "script", &run_id).is_ok(),
        "the same request is idempotent"
    );
    assert_eq!(
        stop_state(&state.db.lock().unwrap(), "script", &run_id).unwrap(),
        (true, true)
    );
    assert_eq!(
        get_automation_runs_impl(&state, "script").unwrap()[0].status,
        "running",
        "requesting stop is not proof of cleanup"
    );
    let requested = get_automation_runs_impl(&state, "script").unwrap();
    assert_eq!(
        requested[0].stop_control,
        Some(AutomationStopControl::Requested)
    );
    assert_eq!(
        serde_json::to_value(&requested[0]).unwrap()["stopControl"],
        "requested"
    );
    assert_eq!(
        next(&state),
        Some(10),
        "stop this run does not pause future scheduling"
    );
    let effects = AtomicUsize::new(0);
    execute_ticket(&state, ticket, &|| {}, &|state, automation, _, _| {
        effects.fetch_add(1, Ordering::SeqCst);
        completed(state, automation)
    })
    .unwrap();
    assert_eq!(
        effects.load(Ordering::SeqCst),
        0,
        "a stopped queued ticket must not start an executor"
    );
    let runs = get_automation_runs_impl(&state, "script").unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "cancelled");
    let overview = workspace_automation_overview(state.db.clone(), &workspace_id, None).unwrap();
    assert!(overview.items[0]
        .latest_run
        .as_ref()
        .unwrap()
        .stop_control
        .is_none());
    assert!(serde_json::to_value(&runs[0])
        .unwrap()
        .get("stopControl")
        .is_none());
    assert!(stop_automation_run_impl(&state, "script", &run_id).is_err());
    assert_eq!(owner(&state), None);
    assert_eq!(next(&state), Some(10));
    assert_eq!(
        stop_state(&state.db.lock().unwrap(), "script", &run_id).unwrap(),
        (false, false)
    );
}

#[test]
fn due_overlap_is_skipped_once_without_replacing_the_manual_owner() {
    let state = setup();
    let claims = {
        let mut conn = state.db.lock().unwrap();
        claim_due_automations(&mut conn, 15).unwrap()
    };
    assert_eq!(claims.len(), 1);
    assert!(!claims[0].skipped);
    let (worker, release) = start_blocked(&state);
    let run_id = owner(&state);
    assert_eq!(
        run_claimed_automations(&state, &claims, 16).unwrap().count,
        1
    );
    assert_eq!(owner(&state), run_id);
    let next_due = next(&state);
    assert!(next_due.unwrap() > 16);
    assert_eq!(run_due_automations_impl(&state, 16).unwrap(), 0);
    release.send(()).unwrap();
    worker.join().unwrap().unwrap();
    assert_eq!(next(&state), next_due);
    let skipped: i64 = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM automation_runs WHERE status='skipped'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(skipped, 1);

    // Scan while the manual owner is live, then finish it before the batch
    // reaches this item. The consumed occurrence must not be admitted again.
    let state = setup();
    let (worker, release) = start_blocked(&state);
    let run_id = owner(&state);
    let claims = {
        let mut conn = state.db.lock().unwrap();
        claim_due_automations(&mut conn, 16).unwrap()
    };
    assert_eq!(claims.len(), 1);
    assert!(claims[0].skipped);
    assert_eq!(owner(&state), run_id);
    let next_due = next(&state);
    assert!(next_due.unwrap() > 16);
    release.send(()).unwrap();
    worker.join().unwrap().unwrap();
    assert_eq!(
        run_claimed_automations(&state, &claims, 17).unwrap().count,
        1
    );
    assert_eq!(run_due_automations_impl(&state, 17).unwrap(), 0);
    assert_eq!(next(&state), next_due);
    let conn = state.db.lock().unwrap();
    let statuses: Vec<String> = conn
        .prepare("SELECT status FROM automation_runs ORDER BY rowid")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(statuses, ["completed", "skipped"]);
}

#[test]
fn pause_reenable_and_forward_clock_jump_cannot_revoke_live_work() {
    let state = setup();
    let (worker, release) = start_blocked(&state);
    let run_id = owner(&state);
    let jumped = 15 + AUTOMATION_CLAIM_TTL_SECONDS + 1;
    {
        let mut conn = state.db.lock().unwrap();
        set_automation_enabled_in_conn(&mut conn, "script", false, jumped).unwrap();
        set_automation_enabled_in_conn(&mut conn, "script", true, jumped).unwrap();
    }
    assert_eq!(owner(&state), run_id);
    assert!(execute(&state, Origin::Manual, jumped).is_err());
    assert_eq!(run_due_automations_impl(&state, jumped).unwrap(), 0);
    let status: String = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT status FROM automation_runs WHERE id=?1",
            [run_id.unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "running");
    let rescheduled = next(&state);
    release.send(()).unwrap();
    worker.join().unwrap().unwrap();
    assert_eq!(next(&state), rescheduled);
}

#[test]
fn manual_run_does_not_consume_future_schedule_and_scheduled_run_consumes_at_start() {
    let state = setup();
    state
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE automations SET next_run_at=100 WHERE id='script'",
            [],
        )
        .unwrap();
    execute(&state, Origin::Manual, 15).unwrap();
    assert_eq!(next(&state), Some(100));
    let effects = AtomicUsize::new(0);
    run(
        &state,
        "script",
        Origin::Scheduled,
        100,
        &|| {},
        &|state, automation| {
            effects.fetch_add(1, Ordering::SeqCst);
            assert!(next(state).unwrap() > 100);
            completed(state, automation)
        },
    )
    .unwrap();
    assert!(!execute(&state, Origin::Scheduled, 100).unwrap());
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[test]
fn admission_write_failure_rolls_back_without_any_external_effect() {
    let state = setup();
    state
        .db
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_started BEFORE INSERT ON automation_runs WHEN NEW.status='running'
         BEGIN SELECT RAISE(ABORT,'fixture admission failure'); END;",
        )
        .unwrap();
    let effects = AtomicUsize::new(0);
    assert!(run(
        &state,
        "script",
        Origin::Scheduled,
        15,
        &|| {},
        &|state, automation| {
            effects.fetch_add(1, Ordering::SeqCst);
            completed(state, automation)
        }
    )
    .is_err());
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert_eq!(owner(&state), None);
    assert_eq!(next(&state), Some(10));
    let count: i64 = state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM automation_runs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn finalization_write_failure_keeps_the_original_started_receipt() {
    let state = setup();
    state.db.lock().unwrap().execute_batch(
        "CREATE TRIGGER fail_terminal BEFORE UPDATE ON automation_runs WHEN NEW.status='completed'
         BEGIN SELECT RAISE(ABORT,'fixture finalization failure'); END;",
    ).unwrap();
    let effects = AtomicUsize::new(0);
    assert!(run(
        &state,
        "script",
        Origin::Manual,
        15,
        &|| {},
        &|state, automation| {
            effects.fetch_add(1, Ordering::SeqCst);
            completed(state, automation)
        }
    )
    .is_err());
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let conn = state.db.lock().unwrap();
    let receipt: (String, Option<i64>) = conn.query_row(
        "SELECT status,finished_at FROM automation_runs WHERE id=(SELECT schedule_claim_token FROM automations WHERE id='script')",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(receipt, ("running".into(), None));
    drop(conn);

    for changed_owner in [false, true] {
        let state = setup();
        state.db.lock().unwrap().execute_batch(
            "INSERT INTO automations(id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,
               executor_kind,script_args,timeout_seconds,next_run_at,created_at,updated_at)
             VALUES('reminder','Later reminder','','once','1970-01-01T00:00:11Z',1,'',
               'notification','[]',300,11,1,1);
             CREATE TRIGGER fail_terminal BEFORE UPDATE ON automation_runs WHEN NEW.status='completed'
               AND NEW.automation_id='script'
             BEGIN SELECT RAISE(ABORT,'fixture private finalization failure'); END;",
        ).unwrap();
        let effects = AtomicUsize::new(0);
        let dispatched_run = std::cell::RefCell::new(String::new());
        let batch =
            dispatch_due_automations(&state, 15, &|| {}, &|ticket| {
                *dispatched_run.borrow_mut() = ticket.run_id.clone();
                if changed_owner {
                    state.db.lock().unwrap().execute_batch(
                    "INSERT INTO automation_runs(id,automation_id,status,summary,started_at)
                     VALUES('new-owner','script','running','New owner',16);
                     UPDATE automations SET schedule_claim_token='new-owner' WHERE id='script';",
                ).unwrap();
                    Err("fixture private dispatch failure".into())
                } else {
                    execute_ticket(&state, ticket, &|| {}, &|state, automation, _, _| {
                        effects.fetch_add(1, Ordering::SeqCst);
                        completed(state, automation)
                    })
                    .map(|_| ())
                }
            })
            .unwrap();
        assert!(batch.failed);
        assert_eq!(batch.count, 1);
        assert_eq!(
            batch.notifications.len(),
            1,
            "unknown owned effects cannot block later reminders"
        );
        assert_eq!(effects.load(Ordering::SeqCst), usize::from(!changed_owner));
        let conn = state.db.lock().unwrap();
        let receipt: (String, String) = conn
            .query_row(
                "SELECT status,summary FROM automation_runs WHERE id=?1",
                [dispatched_run.borrow().as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let schedule: (bool, Option<i64>, String) = conn.query_row(
            "SELECT enabled,next_run_at,schedule_claim_token FROM automations WHERE id='script'",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        if changed_owner {
            assert_eq!(
                receipt.0, "running",
                "a stale dispatcher cannot seal another owner"
            );
            assert!(schedule.0);
            assert_eq!(schedule.2, "new-owner");
            let new_status: String = conn
                .query_row(
                    "SELECT status FROM automation_runs WHERE id='new-owner'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(new_status, "running");
        } else {
            assert_eq!(receipt.0, "attention_required");
            assert!(!receipt.1.contains("fixture private"));
            assert_eq!(schedule, (false, None, dispatched_run.borrow().clone()));
            drop(conn);
            assert_eq!(
                dispatch_due_automations(&state, 18, &|| {}, &|_| panic!(
                    "unknown effect replayed"
                ))
                .unwrap()
                .count,
                0
            );
        }
    }
}

#[test]
fn stale_receipt_pauses_instead_of_replaying_and_requires_explicit_manual_retry() {
    let state = setup();
    state.db.lock().unwrap().execute_batch(
        "INSERT INTO automation_runs(id,automation_id,status,summary,started_at) VALUES('old','script','running','unknown',15);
         UPDATE automations SET schedule_claim_token='old',schedule_claimed_at=15 WHERE id='script';",
    ).unwrap();
    let expired_at = 15 + AUTOMATION_CLAIM_TTL_SECONDS + 1;
    assert_eq!(run_due_automations_impl(&state, expired_at).unwrap(), 0);
    assert_eq!(next(&state), None);
    assert_eq!(owner(&state).as_deref(), Some("old"));
    assert!(set_automation_enabled_in_conn(
        &mut state.db.lock().unwrap(),
        "script",
        true,
        expired_at
    )
    .is_err());
    let status: String = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT status FROM automation_runs WHERE id='old'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "attention_required");
    execute(&state, Origin::Manual, expired_at + 1).unwrap();
    assert_eq!(next(&state), None);
    assert_eq!(owner(&state), None);
    let conn = state.db.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM automation_runs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
    let old: String = conn
        .query_row(
            "SELECT status FROM automation_runs WHERE id='old'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old, "attention_required");
}

#[test]
fn deleting_an_active_script_is_refused_without_losing_its_receipt() {
    let state = setup();
    let (worker, release) = start_blocked(&state);
    assert!(delete_automation_impl(&state, "script")
        .unwrap_err()
        .contains("不能删除"));
    assert!(owner(&state).is_some());
    release.send(()).unwrap();
    worker.join().unwrap().unwrap();
    assert_eq!(get_automation_runs_impl(&state, "script").unwrap().len(), 1);
}

#[test]
fn terminal_notification_is_claimed_once_without_changing_or_replaying_execution() {
    for mode in ["requested", "muted", "unavailable", "unconfirmed"] {
        let state = setup();
        let Admission::Started(ticket) =
            admit(&state, "script", Origin::Manual, 15, &|| {}).unwrap()
        else {
            panic!("no ticket");
        };
        let run_id = ticket.run_id.clone();
        let outcome = execute_ticket(&state, ticket, &|| {}, &|state, a, _, _| {
            let mut result = completed(state, a);
            result.output = "PRIVATE_RESULT_CANARY".into();
            result
        })
        .unwrap();
        let initial = get_automation_runs_impl(&state, "script").unwrap();
        assert!(initial[0].notification_status.is_none());
        let calls = AtomicUsize::new(0);
        if mode == "unconfirmed" {
            state
                .db
                .lock()
                .unwrap()
                .execute(
                    "UPDATE automation_runs SET notification_status='unconfirmed' WHERE id=?1",
                    [&run_id],
                )
                .unwrap();
        }
        notify_owned_run_outcome(&state, outcome, &|title, body| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(title, "AngelBot 自动化结果");
            assert!(!body.contains("PRIVATE_RESULT_CANARY"));
            let db = state.db.lock().unwrap();
            assert_eq!(
                db.query_row(
                    "SELECT notification_status FROM automation_runs WHERE id=?1",
                    [&run_id],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "unconfirmed"
            );
            match mode {
                "requested" => Ok(true),
                "muted" => Ok(false),
                _ => Err("PRIVATE_TRANSPORT_ERROR".into()),
            }
        })
        .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            usize::from(mode != "unconfirmed")
        );
        let runs = get_automation_runs_impl(&state, "script").unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "completed");
        assert_eq!(runs[0].output, "PRIVATE_RESULT_CANARY");
        assert_eq!(runs[0].notification_status.as_deref(), Some(mode));
        assert!(!runs[0].summary.contains("PRIVATE_TRANSPORT_ERROR"));
        let db = state.db.lock().unwrap();
        assert!(db
            .execute(
                "UPDATE automation_runs SET notification_status='delivered' WHERE id=?1",
                [&run_id]
            )
            .is_err());
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM automation_runs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn notification_migration_preserves_an_existing_finished_run_and_never_sends_it() {
    let state = setup();
    execute(&state, Origin::Manual, 15).unwrap();
    let before = get_automation_runs_impl(&state, "script").unwrap();
    let db = state.db.lock().unwrap();
    // Recreate precisely the pre-82 column/log state in this owned fixture.
    db.execute_batch(
        "ALTER TABLE automation_runs DROP COLUMN notification_status;
        DELETE FROM migration_log WHERE version=82;
        UPDATE settings SET value='81' WHERE key='schema_version';",
    )
    .unwrap();
    crate::db::migrate(&db).unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM automation_runs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(db
        .query_row("SELECT notification_status FROM automation_runs", [], |r| r
            .get::<_, Option<String>>(0))
        .unwrap()
        .is_none());
    assert!(db
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query([])
        .unwrap()
        .next()
        .unwrap()
        .is_none());
    drop(db);
    let after = get_automation_runs_impl(&state, "script").unwrap();
    assert_eq!(after[0].id, before[0].id);
    assert_eq!(after[0].status, before[0].status);
    assert_eq!(after[0].output, before[0].output);
    assert_eq!(after[0].summary, before[0].summary);
    let db = state.db.lock().unwrap();
    db.execute_batch(
        "UPDATE automation_runs SET notification_status='unconfirmed';
         DELETE FROM migration_log WHERE version=82;
         UPDATE settings SET value='81' WHERE key='schema_version';",
    )
    .unwrap();
    crate::db::migrate(&db).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT notification_status FROM automation_runs WHERE id=?1",
            [&before[0].id],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "unconfirmed"
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM migration_log WHERE version=82 AND status='applied'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1
    );
}
