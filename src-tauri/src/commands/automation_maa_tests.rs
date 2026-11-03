use super::*;
use std::{
    cell::{Cell, RefCell},
    fs,
};

struct Fixture {
    state: AppState,
    directory: tempfile::TempDir,
    executable: String,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("MAA.exe");
        fs::write(&executable, b"fixture only; never executable").unwrap();
        let state = crate::tests::test_app_state();
        {
            let conn = state.db.lock().unwrap();
            crate::db::migrate(&conn).unwrap();
            conn.execute(
                "INSERT INTO projects(id,name,path,created_at,kind,updated_at)
                 VALUES('maa-workspace','Routine fixture',?1,1,'project',1)",
                [directory.path().to_string_lossy().as_ref()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO desktop_trusted_apps(id,display_name,executable_path,capabilities,enabled,created_at,updated_at)
                 VALUES('maa-app','MAA fixture',?1,'[\"launch\"]',1,1,7)",
                [executable.to_string_lossy().as_ref()],
            ).unwrap();
        }
        Self {
            state,
            directory,
            executable: executable.to_string_lossy().into_owned(),
        }
    }

    fn create(&self, launch_emulator: bool) -> Automation {
        create_with_validation(
            &self.state.db,
            "MAA 18:00".into(),
            "每天 18:00".into(),
            "maa-workspace".into(),
            "maa-app".into(),
            launch_emulator,
            None,
            &|_| Ok(()),
        )
        .unwrap()
    }

    fn ticket(&self, id: &str, now: i64) -> script_admission::Ticket {
        match script_admission::admit(
            &self.state,
            id,
            script_admission::Origin::Manual,
            now,
            &|| {},
        )
        .unwrap()
        {
            script_admission::Admission::Started(ticket) => ticket,
            _ => panic!("manual admission must own a started receipt"),
        }
    }
}

fn result(status: &'static str) -> crate::maa_routine::MaaRunResult {
    crate::maa_routine::MaaRunResult {
        status,
        summary: "fixture queue result, not game completion".into(),
        output: "fixture evidence".into(),
        exit_code: Some(0),
    }
}

fn finish(fixture: &Fixture, ticket: script_admission::Ticket, status: &'static str) {
    script_admission::execute_ticket(
        &fixture.state,
        ticket,
        &|| {},
        &|state, automation, id, cancelled| {
            execute_with(
                state,
                automation,
                id,
                cancelled,
                &|_| Ok(()),
                &|_, _, _, _, _| result(status),
            )
        },
    )
    .unwrap();
}

#[test]
fn maa_creation_persists_explicit_binding_without_editing_the_install() {
    let fixture = Fixture::new();
    let config = fixture.directory.path().join("gui.json");
    fs::write(&config, b"original config").unwrap();
    let automation = fixture.create(true);
    assert_eq!(automation.executor_kind, "maa");
    assert_eq!(automation.timeout_seconds, 3600);
    assert!(automation.permission_summary.contains("当前 GUI 勾选队列"));
    assert!(automation.permission_summary.contains("已配置的模拟器"));
    assert!(automation
        .permission_summary
        .contains("不接管原先打开的 MAA 或关闭原先运行的模拟器进程"));
    assert!(automation
        .permission_summary
        .contains("清理本次启动的 MAA 和模拟器进程树"));
    assert_eq!(fs::read(config).unwrap(), b"original config");
    let saved = get_automations_impl(&fixture.state).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, automation.id);
    assert!(serde_json::to_value(&saved[0])
        .unwrap()
        .get("appId")
        .is_none());
    let options: (String, i64, bool) = fixture.state.db.lock().unwrap().query_row(
        "SELECT trusted_app_id,app_revision,launch_emulator FROM maa_automation_options WHERE automation_id=?1",
        [&automation.id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    assert_eq!(options, ("maa-app".into(), 7, true));
    delete_automation_impl(&fixture.state, &automation.id).unwrap();
    let count: i64 = fixture
        .state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM maa_automation_options", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn maa_creation_rejects_missing_consent_or_invalid_install_without_a_definition() {
    for change in [
        "UPDATE desktop_trusted_apps SET enabled=0",
        "UPDATE desktop_trusted_apps SET capabilities='[]'",
        "UPDATE desktop_trusted_apps SET executable_path='missing/MAA.exe'",
        "DELETE FROM projects WHERE id='maa-workspace'",
    ] {
        let fixture = Fixture::new();
        fixture
            .state
            .db
            .lock()
            .unwrap()
            .execute_batch(change)
            .unwrap();
        let created = create_with_validation(
            &fixture.state.db,
            "MAA".into(),
            "18:00".into(),
            "maa-workspace".into(),
            "maa-app".into(),
            false,
            None,
            &|_| Ok(()),
        );
        assert!(created.is_err(), "{change}");
        assert!(get_automations_impl(&fixture.state).unwrap().is_empty());
    }
    let fixture = Fixture::new();
    let invalid = create_with_validation(
        &fixture.state.db,
        "MAA".into(),
        "18:00".into(),
        "maa-workspace".into(),
        "maa-app".into(),
        false,
        None,
        &|_| Err("fixture RunDirectly conflict".into()),
    );
    assert!(invalid.unwrap_err().contains("RunDirectly"));
    for timeout in [9, 3601] {
        assert!(create_with_validation(
            &fixture.state.db,
            "MAA".into(),
            "18:00".into(),
            "maa-workspace".into(),
            "maa-app".into(),
            false,
            Some(timeout),
            &|_| Ok(())
        )
        .is_err());
    }
    assert!(get_automations_impl(&fixture.state).unwrap().is_empty());
}

#[test]
fn maa_options_write_failure_rolls_back_the_parent_definition() {
    let fixture = Fixture::new();
    fixture
        .state
        .db
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_maa_options BEFORE INSERT ON maa_automation_options
         BEGIN SELECT RAISE(ABORT,'fixture option failure'); END;",
        )
        .unwrap();
    let outcome = create_with_validation(
        &fixture.state.db,
        "MAA".into(),
        "18:00".into(),
        "maa-workspace".into(),
        "maa-app".into(),
        false,
        None,
        &|_| Ok(()),
    );
    assert!(outcome.unwrap_err().contains("fixture option failure"));
    assert!(get_automations_impl(&fixture.state).unwrap().is_empty());
}

#[test]
fn maa_manual_execution_updates_its_started_receipt_and_preserves_the_future_schedule() {
    for terminal in ["completed", "failed"] {
        let fixture = Fixture::new();
        let automation = fixture.create(true);
        let ticket = fixture.ticket(&automation.id, 15);
        let run_id = ticket.run_id.clone();
        let events = Cell::new(0);
        script_admission::execute_ticket(
            &fixture.state,
            ticket,
            &|| events.set(events.get() + 1),
            &|state, automation, id, cancelled| {
                execute_with(
                    state,
                    automation,
                    id,
                    cancelled,
                    &|_| Ok(()),
                    &|path, launch, id, timeout, revoked| {
                        assert_eq!(
                            Path::new(path),
                            fs::canonicalize(&fixture.executable).unwrap()
                        );
                        assert!(launch);
                        assert_eq!(id, run_id);
                        assert_eq!(timeout, Duration::from_secs(3600));
                        assert!(!revoked());
                        let runs = get_automation_runs_impl(state, &automation.id).unwrap();
                        assert_eq!(runs.len(), 1);
                        assert_eq!(
                            (runs[0].id.as_str(), runs[0].status.as_str()),
                            (id, "running")
                        );
                        result(terminal)
                    },
                )
            },
        )
        .unwrap();
        let runs = get_automation_runs_impl(&fixture.state, &automation.id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, run_id);
        assert_eq!(runs[0].status, terminal);
        assert_eq!(runs[0].output, "fixture evidence");
        assert_eq!(runs[0].started_at, 15);
        let saved = get_automations_impl(&fixture.state).unwrap().remove(0);
        assert_eq!(saved.next_run_at, automation.next_run_at);
        assert!(saved.enabled);
        assert!(saved.last_run_at.unwrap() > 15);
        assert_eq!(events.get(), 1);
    }
}

#[test]
fn maa_revoked_or_changed_authorization_never_launches_and_requires_explicit_attention() {
    for change in [
        "UPDATE desktop_trusted_apps SET updated_at=8",
        "UPDATE desktop_trusted_apps SET enabled=0",
        "UPDATE desktop_trusted_apps SET capabilities='[]'",
        "DELETE FROM maa_automation_options",
        "DELETE FROM desktop_trusted_apps WHERE id='maa-app';
         INSERT INTO desktop_trusted_apps(id,display_name,executable_path,capabilities,enabled,created_at,updated_at)
         VALUES('maa-app','Recreated MAA','missing/MAA.exe','[\"launch\"]',1,1,7);",
    ] {
        let fixture = Fixture::new();
        let automation = fixture.create(false);
        fixture.state.db.lock().unwrap().execute_batch(change).unwrap();
        if change.starts_with("DELETE FROM desktop_trusted_apps") {
            fixture.state.db.lock().unwrap().execute(
                "UPDATE desktop_trusted_apps SET executable_path=?1 WHERE id='maa-app'", [&fixture.executable],
            ).unwrap();
        }
        let ticket = fixture.ticket(&automation.id, 15);
        script_admission::execute_ticket(&fixture.state, ticket, &|| {}, &|state, automation, id, cancelled| {
            execute_with(state, automation, id, cancelled, &|_| Ok(()), &|_, _, _, _, _| panic!("revoked grant launched MAA"))
        }).unwrap();
        let runs = get_automation_runs_impl(&fixture.state, &automation.id).unwrap();
        assert_eq!(runs[0].status, "attention_required");
        let saved = get_automations_impl(&fixture.state).unwrap().remove(0);
        assert!(!saved.enabled);
        assert_eq!(saved.next_run_at, None);
        assert!(set_automation_enabled_impl(&fixture.state, &automation.id, true).is_err());
        assert_eq!(run_due_automations_impl(&fixture.state, 20).unwrap(), 0);
    }
}

#[test]
fn maa_live_revocation_requests_cleanup_but_pausing_only_changes_future_scheduling() {
    let fixture = Fixture::new();
    let automation = fixture.create(true);
    let ticket = fixture.ticket(&automation.id, 15);
    script_admission::execute_ticket(&fixture.state, ticket, &|| {}, &|state, automation, id, cancelled| {
        execute_with(state, automation, id, cancelled, &|_| Ok(()), &|_, _, _, _, revoked| {
            set_automation_enabled_impl(state, &automation.id, false).unwrap();
            assert!(!revoked(), "pause must not become cancellation");
            assert!(delete_automation_impl(state, &automation.id).unwrap_err().contains("不能删除"));
            state.db.lock().unwrap().execute(
                "UPDATE desktop_trusted_apps SET capabilities='[]',updated_at=8 WHERE id='maa-app'", [],
            ).unwrap();
            assert!(revoked(), "grant revocation must reach the owned runner");
            result("attention_required")
        })
    }).unwrap();
    assert_eq!(
        get_automation_runs_impl(&fixture.state, &automation.id).unwrap()[0].status,
        "attention_required"
    );
    assert!(!get_automations_impl(&fixture.state).unwrap()[0].enabled);

    // Only the runtime can attest clean pre-dispatch cleanup. An outer stop
    // request must not overwrite a delivered/unknown result or a revoked grant.
    for (runtime_status, revoke, expected) in [
        ("cancelled", false, "cancelled"),
        ("attention_required", false, "attention_required"),
        ("cancelled", true, "attention_required"),
        ("completed", false, "completed"),
    ] {
        let fixture = Fixture::new();
        let automation = fixture.create(false);
        let ticket = fixture.ticket(&automation.id, 15);
        let run_id = ticket.run_id.clone();
        script_admission::execute_ticket(
            &fixture.state,
            ticket,
            &|| {},
            &|state, automation, id, cancelled| {
                execute_with(
                    state,
                    automation,
                    id,
                    cancelled,
                    &|_| Ok(()),
                    &|_, _, _, _, interrupted| {
                        assert!(!interrupted());
                        super::stop_automation_run_impl(state, &automation.id, id).unwrap();
                        if revoke {
                            state
                                .db
                                .lock()
                                .unwrap()
                                .execute("UPDATE desktop_trusted_apps SET enabled=0", [])
                                .unwrap();
                        }
                        assert!(interrupted(), "stop must reach the owned runner");
                        result(runtime_status)
                    },
                )
            },
        )
        .unwrap();
        let runs = get_automation_runs_impl(&fixture.state, &automation.id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, run_id);
        assert_eq!(runs[0].status, expected);
        assert!(runs[0].stop_control.is_none());
        let saved = get_automations_impl(&fixture.state).unwrap().remove(0);
        if expected == "attention_required" {
            assert!(!saved.enabled);
            assert!(saved.next_run_at.is_none());
            assert!(set_automation_enabled_impl(&fixture.state, &automation.id, true).is_err());
        } else {
            assert!(saved.enabled);
            assert_eq!(saved.next_run_at, automation.next_run_at);
        }
    }
}

#[test]
fn due_maa_dispatch_does_not_wait_for_queue_completion_or_delay_reminders() {
    let fixture = Fixture::new();
    let automation = fixture.create(false);
    fixture.state.db.lock().unwrap().execute_batch(&format!(
        "UPDATE automations SET next_run_at=10 WHERE id='{}';
         INSERT INTO automations(id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,
           executor_kind,script_args,timeout_seconds,next_run_at,created_at,updated_at)
         VALUES('zz-reminder','Reminder','still on time','schedule','18:00',1,'','notification','[]',300,10,1,1);",
        automation.id,
    )).unwrap();
    let tickets = RefCell::new(Vec::new());
    let updates = Cell::new(0);
    let dispatched = dispatch_due_automations(
        &fixture.state,
        15,
        &|| updates.set(updates.get() + 1),
        &|ticket| {
            tickets.borrow_mut().push(ticket);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        dispatched.count, 2,
        "count is durable occurrences, not completed queues"
    );
    assert_eq!(dispatched.notifications.len(), 1);
    assert_eq!(dispatched.notifications[0].body, "still on time");
    assert_eq!(tickets.borrow().len(), 1);
    assert_eq!(
        get_automation_runs_impl(&fixture.state, &automation.id).unwrap()[0].status,
        "running"
    );
    assert!(
        updates.get() >= 2,
        "running must be published before the worker executes"
    );
    assert!(script_admission::admit(
        &fixture.state,
        &automation.id,
        script_admission::Origin::Manual,
        16,
        &|| {}
    )
    .is_err());
    assert_eq!(
        dispatch_due_automations(&fixture.state, 16, &|| {}, &|_| panic!(
            "duplicate due launch"
        ))
        .unwrap()
        .count,
        0
    );
    // A queued worker's live guard survives even a forward UTC jump.
    assert_eq!(
        dispatch_due_automations(
            &fixture.state,
            15 + AUTOMATION_CLAIM_TTL_SECONDS + 1,
            &|| {},
            &|_| panic!("live ticket was reclaimed")
        )
        .unwrap()
        .count,
        0
    );
    let ticket = tickets.borrow_mut().pop().unwrap();
    finish(&fixture, ticket, "completed");
    assert_eq!(
        get_automation_runs_impl(&fixture.state, &automation.id).unwrap()[0].status,
        "completed"
    );
}

#[test]
fn expired_disabled_maa_receipt_is_not_replayed_and_explicit_retry_keeps_its_evidence() {
    let fixture = Fixture::new();
    let automation = fixture.create(false);
    fixture.state.db.lock().unwrap().execute_batch(&format!(
        "UPDATE automations SET enabled=0,next_run_at=NULL,schedule_claim_token='lost-run',schedule_claimed_at=10 WHERE id='{}';
         INSERT INTO automation_runs(id,automation_id,status,summary,started_at,finished_at)
         VALUES('lost-run','{}','running','unknown',10,NULL);", automation.id, automation.id,
    )).unwrap();
    assert_eq!(
        run_due_automations_impl(&fixture.state, 11 + AUTOMATION_CLAIM_TTL_SECONDS).unwrap(),
        0
    );
    let runs = get_automation_runs_impl(&fixture.state, &automation.id).unwrap();
    assert_eq!(
        (runs[0].id.as_str(), runs[0].status.as_str()),
        ("lost-run", "attention_required")
    );
    assert!(set_automation_enabled_impl(&fixture.state, &automation.id, true).is_err());
    let retry = fixture.ticket(&automation.id, 12 + AUTOMATION_CLAIM_TTL_SECONDS);
    finish(&fixture, retry, "completed");
    let runs = get_automation_runs_impl(&fixture.state, &automation.id).unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].status, "completed");
    assert_eq!(
        (runs[1].id.as_str(), runs[1].status.as_str()),
        ("lost-run", "attention_required")
    );
    assert!(
        !get_automations_impl(&fixture.state).unwrap()[0].enabled,
        "manual retry must not enable scheduling"
    );
    set_automation_enabled_impl(&fixture.state, &automation.id, true).unwrap();
}
