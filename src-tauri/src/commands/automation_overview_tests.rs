use super::*;

struct Fixture {
    state: AppState,
    workspace_id: String,
}

impl Fixture {
    fn new() -> Self {
        let state = crate::tests::test_app_state();
        let workspace_id = {
            let conn = state.db.lock().unwrap();
            crate::db::migrate(&conn).unwrap();
            conn.execute(
                "INSERT INTO projects(id,name,path,created_at,kind,updated_at)
                VALUES('other-workspace','Other','',1,'project',1)",
                [],
            )
            .unwrap();
            conn.query_row("SELECT id FROM projects WHERE kind='personal'", [], |row| {
                row.get(0)
            })
            .unwrap()
        };
        Self {
            state,
            workspace_id,
        }
    }

    fn add(&self, id: &str, title: &str, kind: &str, workspace_id: Option<&str>) {
        self.state.db.lock().unwrap().execute(
            "INSERT INTO automations(id,title,prompt,trigger_kind,trigger_value,enabled,permission_summary,
             executor_kind,workspace_id,script_path,script_args,working_dir,timeout_seconds,next_run_at,created_at,updated_at)
             VALUES(?1,?2,'secret-prompt','schedule','每天 18:00',1,'secret-permission',?3,?4,
             'C:/private/secret.py','[\"secret-arg\"]','C:/private',300,10,1,1)",
            params![id, title, kind, workspace_id],
        ).unwrap();
    }

    fn receipt(&self, automation: &str, id: &str, status: &str, started: i64) {
        self.state.db.lock().unwrap().execute(
            "INSERT INTO automation_runs(id,automation_id,status,summary,output,exit_code,started_at,finished_at)
             VALUES(?1,?2,?3,'C:/private/secret.env secret-error','secret-output',?4,?5,NULL)",
            params![id, automation, status, (status == "completed").then_some(0), started],
        ).unwrap();
    }

    fn overview(&self) -> WorkspaceAutomationOverview {
        get_workspace_automation_overview_impl(&self.state, &self.workspace_id).unwrap()
    }
}

#[test]
fn overview_is_workspace_scoped_and_querying_never_changes_receipts_or_schedules() {
    let fixture = Fixture::new();
    fixture.add("visible", "MAA daily", "maa", Some(&fixture.workspace_id));
    fixture.add(
        "foreign",
        "Foreign secret",
        "script",
        Some("other-workspace"),
    );
    fixture.add("unowned", "Unowned secret", "script", None);
    fixture.receipt("visible", "old-running", "running", 1);
    let before = fixture.state.db.lock().unwrap().total_changes();
    let overview = fixture.overview();
    assert_eq!(overview.items.len(), 1);
    assert_eq!(overview.items[0].id, "visible");
    assert_eq!(
        overview.items[0].latest_run.as_ref().unwrap().status,
        "running"
    );
    assert_eq!(overview.items[0].next_run_at, Some(10));
    assert!(!overview.truncated);
    assert!(get_workspace_automation_overview_impl(&fixture.state, "").is_err());
    assert!(get_workspace_automation_overview_impl(&fixture.state, "missing-workspace").is_err());
    assert_eq!(
        fixture.state.db.lock().unwrap().total_changes(),
        before,
        "a read tool must not recover, retry, pause or finalize even an old running receipt"
    );
}

#[test]
fn notification_receipts_are_structured_scoped_and_not_delivery_or_business_proof() {
    let fixture = Fixture::new();
    fixture.add("owned", "Owned", "script", Some(&fixture.workspace_id));
    fixture.receipt("owned", "done", "completed", 20);
    for status in ["requested", "muted", "unavailable", "unconfirmed"] {
        fixture
            .state
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE automation_runs SET notification_status=?1 WHERE id='done'",
                [status],
            )
            .unwrap();
        let before = fixture.state.db.lock().unwrap().total_changes();
        let overview = fixture.overview();
        let result = overview.items[0].latest_run.as_ref().unwrap();
        assert_eq!(result.notification_status.as_deref(), Some(status));
        assert_eq!(result.status, "completed");
        assert!(result.summary.contains("不代表外部应用的业务目标完成"));
        if status == "requested" {
            assert!(result.summary.contains("不代表已显示或已读"));
        } else {
            assert!(result.summary.contains("不会自动补发"));
        }
        let json = serde_json::to_value(overview).unwrap();
        assert_eq!(json["items"][0]["latestRun"]["notificationStatus"], status);
        assert!(!json.to_string().contains("secret-output"));
        assert_eq!(fixture.state.db.lock().unwrap().total_changes(), before);
    }
}

#[test]
fn overview_generates_bounded_business_summaries_instead_of_exposing_raw_data() {
    let fixture = Fixture::new();
    fixture.add(
        "script",
        &"长".repeat(200),
        "script",
        Some(&fixture.workspace_id),
    );
    fixture.receipt("script", "done", "completed", 20);
    let overview = fixture.overview();
    assert_eq!(overview.items[0].title.chars().count(), 120);
    let receipt = overview.items[0].latest_run.as_ref().unwrap();
    assert_eq!(receipt.exit_code, Some(0));
    assert!(receipt.summary.contains("不代表外部应用的业务目标完成"));
    let value = serde_json::to_value(overview).unwrap();
    assert!(value["items"][0]["maaConfig"].is_null());
    let serialized = value.to_string();
    for secret in [
        "secret-prompt",
        "secret-permission",
        "secret-arg",
        "C:/private",
        "secret-error",
        "secret-output",
    ] {
        assert!(!serialized.contains(secret), "projection leaked {secret}");
    }
    for field in [
        "prompt",
        "permissionSummary",
        "scriptPath",
        "scriptArgs",
        "workingDir",
        "output",
        "supervisorInputId",
    ] {
        assert!(value["items"][0].get(field).is_none());
        assert!(value["items"][0]["latestRun"].get(field).is_none());
    }

    for kind in ["script", "maa"] {
        let id = format!("stopped-{kind}");
        fixture.add(
            &id,
            "Stopped owned fixture",
            kind,
            Some(&fixture.workspace_id),
        );
        fixture.receipt(&id, &format!("{id}-run"), "cancelled", 30);
        let overview = fixture.overview();
        let item = overview.items.iter().find(|item| item.id == id).unwrap();
        let summary = &item.latest_run.as_ref().unwrap().summary;
        assert!(summary.contains("本次已停止"));
        assert!(summary.contains("不会自动回滚"));
    }

    fixture.add("maa", "MAA daily", "maa", Some(&fixture.workspace_id));
    fixture.add("agent", "Agent daily", "agent", Some(&fixture.workspace_id));
    fixture.add(
        "notice",
        "Reminder",
        "notification",
        Some(&fixture.workspace_id),
    );
    {
        let conn = fixture.state.db.lock().unwrap();
        conn.execute(
            "INSERT INTO desktop_trusted_apps(id,display_name,executable_path,capabilities,enabled,created_at,updated_at)
             VALUES('saved-app',?1,'C:/private/MAA.exe','[\"launch\"]',1,1,7)",
            [&"名".repeat(200)],
        ).unwrap();
        conn.execute(
            "INSERT INTO maa_automation_options(automation_id,trusted_app_id,app_revision,launch_emulator)
             VALUES('maa','saved-app',7,1)", [],
        ).unwrap();
        conn.execute(
            "UPDATE automations SET timeout_seconds=1234 WHERE id='maa'",
            [],
        )
        .unwrap();
    }
    let before = fixture.state.db.lock().unwrap().total_changes();
    let overview = fixture.overview();
    assert!(overview
        .items
        .iter()
        .filter(|item| item.executor_kind != "maa")
        .all(|item| item.maa_config.is_none()));
    let config = overview
        .items
        .iter()
        .find(|item| item.id == "maa")
        .unwrap()
        .maa_config
        .as_ref()
        .unwrap();
    assert_eq!(config.app_id, "saved-app");
    assert_eq!(
        config.app_display_name.as_ref().unwrap().chars().count(),
        120
    );
    assert!(config.launch_emulator);
    assert_eq!(config.timeout_seconds, 1234);
    let value = serde_json::to_value(overview).unwrap();
    assert!(!value.to_string().contains("C:/private"));
    let maa = value["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "maa")
        .unwrap();
    assert_eq!(maa["maaConfig"].as_object().unwrap().len(), 4);
    assert_eq!(maa["maaConfig"]["appId"], "saved-app");
    assert_eq!(maa["maaConfig"]["launchEmulator"], true);
    assert_eq!(maa["maaConfig"]["timeoutSeconds"], 1234);
    assert_eq!(fixture.state.db.lock().unwrap().total_changes(), before);

    fixture.state.db.lock().unwrap().execute(
        "UPDATE desktop_trusted_apps SET display_name='Changed target',updated_at=8 WHERE id='saved-app'", [],
    ).unwrap();
    let overview = fixture.overview();
    let config = overview
        .items
        .iter()
        .find(|item| item.id == "maa")
        .unwrap()
        .maa_config
        .as_ref()
        .unwrap();
    assert_eq!(config.app_id, "saved-app");
    assert!(config.app_display_name.is_none());
    assert!(config.launch_emulator);
    assert_eq!(config.timeout_seconds, 1234);

    fixture.state.db.lock().unwrap().execute_batch(
        "DELETE FROM desktop_trusted_apps WHERE id='saved-app';
         INSERT INTO desktop_trusted_apps(id,display_name,executable_path,capabilities,enabled,created_at,updated_at)
         VALUES('saved-app','Recreated target','C:/private/new-MAA.exe','[\"launch\"]',1,9,9);",
    ).unwrap();
    let overview = fixture.overview();
    let item = overview.items.iter().find(|item| item.id == "maa").unwrap();
    assert!(
        item.maa_config.is_none(),
        "deletion removes the original consent binding"
    );
    assert!(!serde_json::to_string(&overview)
        .unwrap()
        .contains("Recreated target"));
}

#[test]
fn skipped_occurrences_do_not_hide_current_unknown_or_running_receipts() {
    let fixture = Fixture::new();
    fixture.add("maa", "MAA daily", "maa", Some(&fixture.workspace_id));
    fixture.receipt("maa", "owner-unknown", "attention_required", 10);
    fixture.receipt("maa", "live", "running", 20);
    fixture.receipt("maa", "newer-result", "completed", 30);
    for started in 40..70 {
        fixture.receipt("maa", &format!("skip-{started}"), "skipped", started);
    }
    fixture
        .state
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE automations SET schedule_claim_token='owner-unknown' WHERE id='maa'",
            [],
        )
        .unwrap();
    let item = fixture.overview().items.remove(0);
    assert_eq!(item.latest_run.unwrap().id, "owner-unknown");
    assert_eq!(item.last_skipped_at, Some(69));
    let dashboard = get_automation_dashboard_impl(&fixture.state).unwrap();
    assert_eq!(
        dashboard.items[0].latest_run.as_ref().unwrap().id,
        "owner-unknown"
    );
    assert_eq!(dashboard.items[0].last_skipped_at, Some(69));
    let history = get_automation_runs_impl(&fixture.state, "maa").unwrap();
    assert_eq!(history.len(), 20);
    assert!(history.iter().any(|run| run.id == "owner-unknown"));
    assert!(history.iter().any(|run| run.id == "live"));
    assert!(history
        .windows(2)
        .all(|pair| pair[0].started_at >= pair[1].started_at));
    fixture
        .state
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE automations SET schedule_claim_token=NULL WHERE id='maa'",
            [],
        )
        .unwrap();
    assert_eq!(
        fixture.overview().items[0].latest_run.as_ref().unwrap().id,
        "live"
    );
    assert_eq!(
        get_automation_dashboard_impl(&fixture.state).unwrap().items[0]
            .latest_run
            .as_ref()
            .unwrap()
            .id,
        "live"
    );
    fixture
        .state
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE automation_runs SET status='completed' WHERE id='live'",
            [],
        )
        .unwrap();
    assert_eq!(
        fixture.overview().items[0].latest_run.as_ref().unwrap().id,
        "newer-result"
    );
    assert_eq!(
        get_automation_dashboard_impl(&fixture.state).unwrap().items[0]
            .latest_run
            .as_ref()
            .unwrap()
            .id,
        "newer-result"
    );
}

#[test]
fn dashboard_contains_global_definitions_and_bounded_receipts_but_not_output_or_invented_stop() {
    let fixture = Fixture::new();
    fixture.add("owned", "Owned", "script", Some(&fixture.workspace_id));
    fixture.add("foreign", "Foreign", "agent", Some("other-workspace"));
    fixture.add("unowned", "Unowned", "script", None);
    fixture.add(
        "muted",
        "Muted",
        "notification",
        Some(&fixture.workspace_id),
    );
    fixture.receipt("owned", "live", "running", 1);
    fixture.receipt("foreign", "queued", "queued", 1);
    fixture.receipt("muted", "muted-run", "skipped", 2);
    let raw_summary = "长".repeat(500);
    {
        let conn = fixture.state.db.lock().unwrap();
        conn.execute(
            "UPDATE automation_runs SET summary=?1,notification_status='requested' WHERE id='live'",
            [&raw_summary],
        )
        .unwrap();
        conn.execute(
            "UPDATE automation_runs SET notification_status='muted' WHERE id='muted-run'",
            [],
        )
        .unwrap();
    }
    let before = fixture.state.db.lock().unwrap().total_changes();
    let value =
        serde_json::to_value(get_automation_dashboard_impl(&fixture.state).unwrap()).unwrap();
    let items = value["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    let owned = items.iter().find(|item| item["id"] == "owned").unwrap();
    assert_eq!(owned["scriptPath"], "C:/private/secret.py");
    assert_eq!(owned["latestRun"]["id"], "live");
    assert!(owned["latestRun"]["summary"]
        .as_str()
        .unwrap()
        .starts_with(&format!("{}…", "长".repeat(400))));
    assert!(owned["latestRun"]["summary"]
        .as_str()
        .unwrap()
        .contains("不代表已显示或已读"));
    assert!(owned["latestRun"].get("output").is_none());
    assert!(
        owned["latestRun"].get("stopControl").is_none(),
        "a saved running status is not live process ownership"
    );
    assert!(items.iter().find(|item| item["id"] == "unowned").unwrap()["latestRun"].is_null());
    assert_eq!(
        items.iter().find(|item| item["id"] == "muted").unwrap()["latestRun"]["status"],
        "skipped"
    );
    assert!(!value.to_string().contains("secret-output"));
    assert_eq!(fixture.state.db.lock().unwrap().total_changes(), before);
    assert!(get_workspace_automation_overview_impl(&fixture.state, "").is_err());
}

#[test]
fn dashboard_stop_control_tracks_the_exact_live_ticket_without_stopping_or_executing_it() {
    let fixture = Fixture::new();
    fixture.add("script", "Script", "script", Some(&fixture.workspace_id));
    let ticket = match script_admission::admit(
        &fixture.state,
        "script",
        script_admission::Origin::Manual,
        10,
        &|| {},
    )
    .unwrap()
    {
        script_admission::Admission::Started(ticket) => ticket,
        _ => panic!("expected an admitted ticket, without executing any process"),
    };
    let dashboard = get_automation_dashboard_impl(&fixture.state).unwrap();
    let head = dashboard.items[0].latest_run.as_ref().unwrap();
    assert_eq!(head.id, ticket.run_id);
    assert!(matches!(
        head.stop_control,
        Some(AutomationStopControl::Available)
    ));
    script_admission::request_stop(&fixture.state.db.lock().unwrap(), "script", &ticket.run_id)
        .unwrap();
    assert!(matches!(
        get_automation_dashboard_impl(&fixture.state).unwrap().items[0]
            .latest_run
            .as_ref()
            .unwrap()
            .stop_control,
        Some(AutomationStopControl::Requested)
    ));
    drop(ticket);
    assert!(
        get_automation_dashboard_impl(&fixture.state).unwrap().items[0]
            .latest_run
            .as_ref()
            .unwrap()
            .stop_control
            .is_none()
    );
}

#[test]
fn pending_agent_receipts_remain_visible_beyond_the_recent_history_page() {
    for pending in ["queued", "awaiting_confirmation", "running"] {
        let fixture = Fixture::new();
        fixture.add("agent", "Agent daily", "agent", Some(&fixture.workspace_id));
        fixture.receipt("agent", "pending", pending, 1);
        for started in 2..32 {
            fixture.receipt("agent", &format!("done-{started}"), "completed", started);
        }
        let before = fixture.state.db.lock().unwrap().total_changes();
        let overview = fixture.overview();
        assert_eq!(overview.items[0].latest_run.as_ref().unwrap().id, "pending");
        let history = get_automation_runs_impl(&fixture.state, "agent").unwrap();
        assert_eq!(history.len(), 20);
        assert!(history.iter().any(|run| run.id == "pending"));
        assert_eq!(history[0].id, "done-31");
        assert!(history
            .windows(2)
            .all(|pair| pair[0].started_at >= pair[1].started_at));
        assert_eq!(fixture.state.db.lock().unwrap().total_changes(), before);

        fixture
            .state
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE automation_runs SET status='needs_attention' WHERE id='pending'",
                [],
            )
            .unwrap();
        assert_eq!(
            fixture.overview().items[0].latest_run.as_ref().unwrap().id,
            "done-31",
            "historical stopped outcomes must not hide later results"
        );
        assert!(!get_automation_runs_impl(&fixture.state, "agent")
            .unwrap()
            .iter()
            .any(|run| run.id == "pending"));
    }
}

#[test]
fn same_second_receipts_preserve_owned_results_and_muted_notifications() {
    let fixture = Fixture::new();
    fixture.add("script", "Script", "script", Some(&fixture.workspace_id));
    fixture.receipt("script", "first", "completed", 20);
    fixture.receipt("script", "second", "failed", 20);
    fixture.receipt("script", "third", "skipped", 20);
    assert_eq!(
        get_automation_runs_impl(&fixture.state, "script")
            .unwrap()
            .iter()
            .map(|run| run.id.as_str())
            .collect::<Vec<_>>(),
        ["third", "second", "first"],
        "the bounded selection must retain same-second insertion order"
    );
    let item = fixture.overview().items.remove(0);
    assert_eq!(item.latest_run.unwrap().id, "second");
    assert_eq!(item.last_skipped_at, Some(20));
    fixture.receipt("script", "fourth", "completed", 20);
    let item = fixture.overview().items.remove(0);
    assert_eq!(item.latest_run.unwrap().id, "fourth");
    assert_eq!(item.last_skipped_at, None);
    fixture.add(
        "only-skip",
        "Only skipped",
        "maa",
        Some(&fixture.workspace_id),
    );
    fixture.receipt("only-skip", "skip", "skipped", 30);
    let item = fixture.overview().items.remove(0);
    assert_eq!(item.id, "only-skip");
    assert!(item.latest_run.is_none());
    assert_eq!(item.last_skipped_at, Some(30));

    fixture.add(
        "notice",
        "Muted reminder",
        "notification",
        Some(&fixture.workspace_id),
    );
    fixture.receipt("notice", "notice-accepted", "completed", 40);
    fixture.receipt("notice", "notice-muted", "skipped", 40);
    let item = fixture
        .overview()
        .items
        .into_iter()
        .find(|item| item.id == "notice")
        .unwrap();
    let run = item.latest_run.unwrap();
    assert_eq!(run.id, "notice-muted");
    assert_eq!(run.status, "skipped");
    assert_eq!(run.summary, "通知已关闭，本次提醒未发送；不会自动补发。");
    assert_eq!(
        item.last_skipped_at, None,
        "do not describe a muted notification as a job that never started"
    );
}

#[test]
fn overview_caps_items_and_title_query_is_literal_not_a_wildcard() {
    let fixture = Fixture::new();
    fixture.add(
        "old-attention",
        "Old attention",
        "maa",
        Some(&fixture.workspace_id),
    );
    fixture.receipt("old-attention", "uncertain", "attention_required", 1);
    fixture.add(
        "old-active",
        "Old active",
        "script",
        Some(&fixture.workspace_id),
    );
    fixture.receipt("old-active", "active", "running", 2);
    fixture.add(
        "old-pending",
        "Old pending",
        "agent",
        Some(&fixture.workspace_id),
    );
    fixture.receipt("old-pending", "waiting", "awaiting_confirmation", 1);
    fixture.receipt("old-pending", "newer-done", "completed", 2);
    fixture.state.db.lock().unwrap().execute(
        "UPDATE automations SET schedule_claim_token='uncertain',enabled=0,next_run_at=NULL WHERE id='old-attention'", [],
    ).unwrap();
    for index in 0..101 {
        fixture.add(
            &format!("item-{index}"),
            &format!("Routine {index}"),
            "agent",
            Some(&fixture.workspace_id),
        );
    }
    fixture.add(
        "literal",
        "Routine 100%_literal",
        "agent",
        Some(&fixture.workspace_id),
    );
    fixture
        .state
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE automations SET created_at=2 WHERE title LIKE 'Routine %'",
            [],
        )
        .unwrap();
    let overview = fixture.overview();
    assert_eq!(overview.items.len(), 100);
    assert!(overview.truncated);
    assert_eq!(
        overview.items[0].id, "old-attention",
        "old unresolved work must survive the limit"
    );
    assert_eq!(
        overview.items[1].id, "old-active",
        "old active work must survive the limit"
    );
    assert_eq!(
        overview.items[2].id, "old-pending",
        "pending work must survive the definition limit even with a newer result"
    );
    let filtered =
        workspace_automation_overview(fixture.state.db.clone(), &fixture.workspace_id, Some("%_"))
            .unwrap();
    assert_eq!(filtered.items.len(), 1);
    assert_eq!(filtered.items[0].id, "literal");
    assert!(!filtered.truncated);
    let ascii_case = workspace_automation_overview(
        fixture.state.db.clone(),
        &fixture.workspace_id,
        Some("routine 100%_LITERAL"),
    )
    .unwrap();
    assert_eq!(ascii_case.items.len(), 1);
    assert_eq!(ascii_case.items[0].id, "literal");
    assert!(workspace_automation_overview(
        fixture.state.db.clone(),
        &fixture.workspace_id,
        Some("absent")
    )
    .unwrap()
    .items
    .is_empty());
    assert!(workspace_automation_overview(
        fixture.state.db.clone(),
        &fixture.workspace_id,
        Some(&"x".repeat(121))
    )
    .is_err());
}

#[test]
fn workspace_pause_is_idempotent_and_never_stops_a_running_owned_receipt() {
    let fixture = Fixture::new();
    fixture.add("script", "Script", "script", Some(&fixture.workspace_id));
    fixture.add("foreign", "Other", "script", Some("other-workspace"));
    fixture.receipt("script", "running", "running", 20);
    fixture.state.db.lock().unwrap().execute(
        "UPDATE automations SET schedule_claim_token='running',schedule_claimed_at=20 WHERE id='script'", [],
    ).unwrap();
    assert!(
        pause_workspace_automation(fixture.state.db.clone(), &fixture.workspace_id, "foreign")
            .is_err()
    );
    assert!(pause_workspace_automation(fixture.state.db.clone(), "missing", "script").is_err());
    for _ in 0..2 {
        pause_workspace_automation(fixture.state.db.clone(), &fixture.workspace_id, "script")
            .unwrap();
        let item = fixture.overview().items.remove(0);
        assert!(!item.enabled);
        assert_eq!(item.next_run_at, None);
        assert_eq!(item.latest_run.unwrap().status, "running");
        let owner: String = fixture
            .state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT schedule_claim_token FROM automations WHERE id='script'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner, "running");
    }
    let foreign =
        workspace_automation_overview(fixture.state.db.clone(), "other-workspace", None).unwrap();
    assert!(foreign.items[0].enabled);
}
